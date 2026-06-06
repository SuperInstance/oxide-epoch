//! # oxide-epoch
//!
//! Epoch-based memory reclamation for GPU data structures with ternary epoch states.
//!
//! ## Ternary Epoch States
//! - **Active (+1):** The epoch has live guards; no reclamation allowed.
//! - **Grace Period (0):** The epoch has no live guards but is too recent; deferred items
//!   are not yet safe to reclaim.
//! - **Reclaimable (-1):** All guards have moved on; deferred items can be reclaimed.
//!
//! ## Usage
//! ```rust
//! use oxide_epoch::EpochManager;
//!
//! let manager = EpochManager::new();
//! let guard = manager.pin();
//! // ... do work with the pinned epoch ...
//! drop(guard);
//! manager.advance_and_reclaim();
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

// ---------------------------------------------------------------------------
// Ternary epoch classification
// ---------------------------------------------------------------------------

/// Classification of an epoch relative to the global epoch counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpochState {
    /// Epoch has at least one active guard → not safe to touch.
    Active,
    /// Epoch has no active guards but is within the grace window → not yet reclaimable.
    GracePeriod,
    /// Epoch has no active guards and is outside the grace window → safe to reclaim.
    Reclaimable,
}

/// Ternary numeric representation used for quick classification:
/// `+1 = Active`, `0 = GracePeriod`, `-1 = Reclaimable`.
impl From<EpochState> for i8 {
    fn from(s: EpochState) -> Self {
        match s {
            EpochState::Active => 1,
            EpochState::GracePeriod => 0,
            EpochState::Reclaimable => -1,
        }
    }
}

impl std::fmt::Display for EpochState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let v: i8 = (*self).into();
        match self {
            EpochState::Active => write!(f, "Active(+{v})"),
            EpochState::GracePeriod => write!(f, "GracePeriod({v})"),
            EpochState::Reclaimable => write!(f, "Reclaimable({v})"),
        }
    }
}

// ---------------------------------------------------------------------------
// Deferred free item
// ---------------------------------------------------------------------------

/// A value that has been logically freed and is waiting for safe reclamation.
pub struct DeferredFree<T> {
    /// The epoch in which the value was retired.
    pub epoch: u64,
    /// The value to be dropped once reclamation is safe.
    pub value: T,
}

// ---------------------------------------------------------------------------
// EpochGuard – RAII pin
// ---------------------------------------------------------------------------

/// Inner state shared between the guard and the manager.
struct GuardInner {
    epoch: u64,
    manager: Weak<EpochManagerInner>,
}

/// RAII guard that pins the current epoch, preventing reclamation of that epoch.
///
/// When dropped, the guard unpins its epoch via the manager.
pub struct EpochGuard {
    inner: Option<GuardInner>,
}

impl EpochGuard {
    /// Returns the epoch number this guard pins.
    pub fn epoch(&self) -> u64 {
        self.inner.as_ref().unwrap().epoch
    }
}

impl Drop for EpochGuard {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            if let Some(mgr) = inner.manager.upgrade() {
                let mut guards = mgr.active_guards.lock().unwrap();
                let count = guards.entry(inner.epoch).or_insert(0);
                *count = count.saturating_sub(1);
                if *count == 0 {
                    guards.remove(&inner.epoch);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// EpochManager inner (shared state)
// ---------------------------------------------------------------------------

struct EpochManagerInner {
    current_epoch: AtomicU64,
    active_guards: Mutex<HashMap<u64, usize>>,
    grace_lag: u64,
}

// ---------------------------------------------------------------------------
// EpochManager
// ---------------------------------------------------------------------------

/// Manages epoch advancement, guard tracking, deferred frees, and bulk reclamation.
pub struct EpochManager {
    inner: Arc<EpochManagerInner>,
    deferred: Mutex<Vec<DeferredFree<Box<dyn std::any::Any + Send>>>>,
}

impl EpochManager {
    /// Create a new `EpochManager` starting at epoch 0 with the default grace lag of 2.
    pub fn new() -> Self {
        Self::with_grace_lag(2)
    }

    /// Create with a custom grace-lag value.
    ///
    /// An epoch becomes reclaimable when `current_epoch - epoch > grace_lag` and
    /// there are no active guards on that epoch.
    pub fn with_grace_lag(grace_lag: u64) -> Self {
        Self {
            inner: Arc::new(EpochManagerInner {
                current_epoch: AtomicU64::new(0),
                active_guards: Mutex::new(HashMap::new()),
                grace_lag,
            }),
            deferred: Mutex::new(Vec::new()),
        }
    }

    // -- epoch operations ---------------------------------------------------

    /// Returns the current global epoch.
    pub fn current_epoch(&self) -> u64 {
        self.inner.current_epoch.load(Ordering::Acquire)
    }

    /// Advances the global epoch by one and returns the new epoch.
    pub fn advance(&self) -> u64 {
        let prev = self.inner.current_epoch.fetch_add(1, Ordering::AcqRel);
        prev + 1
    }

    /// Pin the current epoch, returning an `EpochGuard` that prevents reclamation.
    pub fn pin(&self) -> EpochGuard {
        let epoch = self.current_epoch();
        {
            let mut guards = self.inner.active_guards.lock().unwrap();
            *guards.entry(epoch).or_insert(0) += 1;
        }
        EpochGuard {
            inner: Some(GuardInner {
                epoch,
                manager: Arc::downgrade(&self.inner),
            }),
        }
    }

    /// Returns the number of active guards for a given epoch.
    pub fn active_guard_count(&self, epoch: u64) -> usize {
        self.inner
            .active_guards
            .lock()
            .unwrap()
            .get(&epoch)
            .copied()
            .unwrap_or(0)
    }

    // -- ternary classification --------------------------------------------

    /// Classify an epoch as Active, GracePeriod, or Reclaimable.
    pub fn classify(&self, epoch: u64) -> EpochState {
        let current = self.current_epoch();
        let has_guards = self.active_guard_count(epoch) > 0;
        if has_guards {
            return EpochState::Active;
        }
        if current >= epoch && current - epoch > self.inner.grace_lag {
            EpochState::Reclaimable
        } else {
            EpochState::GracePeriod
        }
    }

    /// Convenience: returns the ternary numeric value (`+1`, `0`, `-1`).
    pub fn classify_value(&self, epoch: u64) -> i8 {
        self.classify(epoch).into()
    }

    // -- deferred free ------------------------------------------------------

    /// Mark a value for deferred reclamation at the current epoch.
    pub fn defer_free<T: 'static + Send>(&self, value: T) {
        let epoch = self.current_epoch();
        let mut deferred = self.deferred.lock().unwrap();
        deferred.push(DeferredFree {
            epoch,
            value: Box::new(value),
        });
    }

    /// Mark a value for deferred reclamation at a specific epoch.
    pub fn defer_free_at<T: 'static + Send>(&self, epoch: u64, value: T) {
        let mut deferred = self.deferred.lock().unwrap();
        deferred.push(DeferredFree {
            epoch,
            value: Box::new(value),
        });
    }

    /// Number of pending deferred items across all epochs.
    pub fn deferred_count(&self) -> usize {
        self.deferred.lock().unwrap().len()
    }

    // -- reclamation --------------------------------------------------------

    /// Reclaim all deferred items whose epochs are classified as `Reclaimable`.
    ///
    /// Returns the number of items reclaimed.
    pub fn reclaim(&self) -> usize {
        let mut deferred = self.deferred.lock().unwrap();
        let mut keep = Vec::new();
        let mut reclaimed = 0;

        for item in deferred.drain(..) {
            if self.classify(item.epoch) == EpochState::Reclaimable {
                // Drop the value automatically by letting it go out of scope.
                reclaimed += 1;
            } else {
                keep.push(item);
            }
        }

        *deferred = keep;
        reclaimed
    }

    /// Advance the epoch and then reclaim.
    ///
    /// This is the common "tick" operation: move the world forward, then
    /// clean up whatever became safe.
    ///
    /// Returns `(new_epoch, items_reclaimed)`.
    pub fn advance_and_reclaim(&self) -> (u64, usize) {
        let new = self.advance();
        let count = self.reclaim();
        (new, count)
    }
}

impl Default for EpochManager {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_manager_starts_at_epoch_zero() {
        let mgr = EpochManager::new();
        assert_eq!(mgr.current_epoch(), 0);
    }

    #[test]
    fn test_advance_increments_monotonically() {
        let mgr = EpochManager::new();
        assert_eq!(mgr.advance(), 1);
        assert_eq!(mgr.advance(), 2);
        assert_eq!(mgr.advance(), 3);
        assert_eq!( mgr.current_epoch(), 3);
    }

    #[test]
    fn test_guard_pins_epoch_and_releases_on_drop() {
        let mgr = EpochManager::new();
        mgr.advance(); // epoch 1
        assert_eq!(mgr.active_guard_count(1), 0);

        let guard = mgr.pin();
        assert_eq!(guard.epoch(), 1);
        assert_eq!(mgr.active_guard_count(1), 1);

        drop(guard);
        assert_eq!(mgr.active_guard_count(1), 0);
    }

    #[test]
    fn test_multiple_guards_same_epoch() {
        let mgr = EpochManager::new();
        mgr.advance(); // epoch 1
        let g1 = mgr.pin();
        let g2 = mgr.pin();
        let g3 = mgr.pin();
        assert_eq!(mgr.active_guard_count(1), 3);
        drop(g2);
        assert_eq!(mgr.active_guard_count(1), 2);
        drop(g1);
        drop(g3);
        assert_eq!(mgr.active_guard_count(1), 0);
    }

    #[test]
    fn test_ternary_classification() {
        let mgr = EpochManager::with_grace_lag(1);
        // epoch 0 is current, grace_lag = 1
        assert_eq!(mgr.classify(0), EpochState::GracePeriod); // 0-0=0, not > 1

        mgr.advance(); // epoch 1
        // 1-0=1 > grace_lag(1) is false → GracePeriod
        assert_eq!(mgr.classify(0), EpochState::GracePeriod);

        mgr.advance(); // epoch 2
        assert_eq!(mgr.classify(0), EpochState::Reclaimable); // 2-0=2 > 1 → Reclaimable
        assert_eq!(mgr.classify(1), EpochState::GracePeriod); // 2-1=1 > 1 false

        // Active: pin epoch 2
        let _guard = mgr.pin(); // pins epoch 2
        assert_eq!(mgr.classify(2), EpochState::Active);
    }

    #[test]
    fn test_classify_value_ternary_numbers() {
        let mgr = EpochManager::with_grace_lag(0);
        // grace_lag=0, so current-epoch > 0 → reclaimable when epoch < current
        let _g = mgr.pin(); // pins epoch 0
        assert_eq!(mgr.classify_value(0), 1); // Active (+1)

        mgr.advance(); // epoch 1
        // epoch 0 still has active guard → Active
        assert_eq!(mgr.classify_value(0), 1); // Active

        drop(_g);
        // now epoch 0 has no guard, 1-0=1 > 0 → Reclaimable
        assert_eq!(mgr.classify_value(0), -1); // Reclaimable
        assert_eq!(mgr.classify_value(1), 0); // GracePeriod: 1-1=0 > 0 → false
    }

    #[test]
    fn test_deferred_free_and_reclaim() {
        let mgr = EpochManager::with_grace_lag(0);
        mgr.defer_free(42i32);
        mgr.defer_free(String::from("hello"));
        assert_eq!(mgr.deferred_count(), 2);

        // epoch 0 → not yet reclaimable (current==epoch)
        let r = mgr.reclaim();
        assert_eq!(r, 0);
        assert_eq!(mgr.deferred_count(), 2);

        // advance → epoch 1, now 1-0 > 0 → reclaimable
        mgr.advance();
        let r = mgr.reclaim();
        assert_eq!(r, 2);
        assert_eq!(mgr.deferred_count(), 0);
    }

    #[test]
    fn test_deferred_free_blocked_by_active_guard() {
        let mgr = EpochManager::with_grace_lag(0);

        let guard = mgr.pin(); // pin epoch 0
        mgr.defer_free(100u64);

        mgr.advance(); // epoch 1
        // epoch 0 still has a guard → Active, cannot reclaim
        let r = mgr.reclaim();
        assert_eq!(r, 0);

        drop(guard);
        // now epoch 0 has no guards and is in the past
        let r = mgr.reclaim();
        assert_eq!(r, 1);
        assert_eq!(mgr.deferred_count(), 0);
    }

    #[test]
    fn test_advance_and_reclaim_bulk() {
        let mgr = EpochManager::with_grace_lag(1);
        // Defer items at epoch 0
        for i in 0..5 {
            mgr.defer_free(format!("item-{i}"));
        }

        // advance to 1 → grace_lag=1, 1-0=1 not > 1
        let (ep, r) = mgr.advance_and_reclaim();
        assert_eq!(ep, 1);
        assert_eq!(r, 0);

        // advance to 2 → 2-0=2 > 1 → reclaimable
        let (ep, r) = mgr.advance_and_reclaim();
        assert_eq!(ep, 2);
        assert_eq!(r, 5);
        assert_eq!(mgr.deferred_count(), 0);
    }

    #[test]
    fn test_epoch_state_display_and_from() {
        assert_eq!(i8::from(EpochState::Active), 1);
        assert_eq!(i8::from(EpochState::GracePeriod), 0);
        assert_eq!(i8::from(EpochState::Reclaimable), -1);
        assert_eq!(format!("{}", EpochState::Active), "Active(+1)");
        assert_eq!(format!("{}", EpochState::GracePeriod), "GracePeriod(0)");
        assert_eq!(format!("{}", EpochState::Reclaimable), "Reclaimable(-1)");
    }
}
