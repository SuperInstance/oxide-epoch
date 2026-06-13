# Oxide Epoch

**Epoch-based memory reclamation** for concurrent GPU data structures, using a **ternary epoch state** model. This crate implements the epoch reclamation pattern (similar to `crossbeam-epoch`) with a ternary classification — Active, GracePeriod, Reclaimable — that maps to the numeric values +1, 0, −1.

## Why It Matters

Lock-free and wait-free concurrent data structures (queues, stacks, hash maps, skip lists) face the **safe memory reclamation problem**: a thread may still be reading a node that another thread has "deleted." Solutions include:

| Technique | Overhead | Complexity | Platform |
|-----------|----------|------------|----------|
| Hazard pointers | O(1) per access | High (per-pointer tracking) | All |
| **Epoch-based reclamation (EBR)** | O(1) amortized | Medium | All |
| RCU (Read-Copy-Update) | Near-zero read | Low (kernel-level) | Linux kernel |
| Reference counting | O(1) per access | Low | All (but costly) |
| Garbage collection | Variable | Very high | JVM/Go/.NET |

EBR offers the best throughput for general-purpose concurrent programming — it avoids the per-pointer overhead of hazard pointers and works in any language with manual memory management.

## How It Works

### The Epoch Model

The global epoch counter advances monotonically. Each thread pins an epoch by holding a guard:

```
Thread A: pin() → epoch 5
  ... do work ...
drop(guard) → epoch 5 unpinned

Thread B: advance() → epoch 6

Thread C: advance() → epoch 7
```

An epoch becomes reclaimable when **all** of:
1. No active guards reference it.
2. The global epoch has advanced beyond the grace lag: `current - epoch > grace_lag`.

### Ternary Classification

| State | Value | Condition | Reclaim? |
|-------|-------|-----------|----------|
| **Active** | +1 | Has ≥1 active guard | Never |
| **GracePeriod** | 0 | No guards, but `current - epoch ≤ grace_lag` | Not yet |
| **Reclaimable** | −1 | No guards AND `current - epoch > grace_lag` | Yes |

The default grace lag is 2 — an epoch must be at least 2 generations old before reclamation.

### RAII Guard

```
EpochGuard {
    inner: GuardInner {
        epoch: u64,             // epoch pinned at creation
        manager: Weak<Inner>,   // back-reference to unpin on drop
    }
}

impl Drop for EpochGuard {
    fn drop(&mut self) {
        // Decrement guard count for this epoch
        // Remove epoch entry when count reaches 0
    }
}
```

### Deferred Free

Retired objects are stored in a `Vec<DeferredFree<Box<dyn Any + Send>>>` and reclaimed during `advance_and_reclaim()`:

```
advance_and_reclaim():
  1. current_epoch += 1
  2. For each deferred item with epoch.classify() == Reclaimable:
     → drop the value (free the memory)
  3. Remove reclaimed items from the deferred list
```

### Complexity

| Operation | Time | Notes |
|-----------|------|-------|
| `pin()` | O(1) — atomic load + mutex lock | Amortized very fast |
| `advance()` | O(1) — atomic fetch_add | |
| `classify(epoch)` | O(G) — G = guarded epochs | Mutex-protected map lookup |
| `advance_and_reclaim()` | O(D + G) | D = deferred items, G = guarded epochs |

## Quick Start

```rust
use oxide_epoch::EpochManager;

let manager = EpochManager::new();

// Pin the current epoch to safely access shared data
let guard = manager.pin();
println!("Pinned at epoch {}", guard.epoch());

// ... do lock-free reads ...

drop(guard); // unpin

// Advance and reclaim deferred frees
manager.advance();
// Old epochs with no guards are now reclaimable
```

## API

### `EpochManager`

| Method | Description |
|--------|-------------|
| `new()` | Create with default grace lag = 2 |
| `with_grace_lag(g)` | Create with custom grace lag |
| `current_epoch()` | Read the global epoch counter |
| `advance()` | Increment global epoch by 1 |
| `pin()` | Get RAII `EpochGuard` preventing reclamation |
| `classify(epoch)` | Classify as Active / GracePeriod / Reclaimable |
| `active_guard_count(epoch)` | Number of active guards on an epoch |

### `EpochGuard`

RAII guard. Dropping unpins the associated epoch.

### `EpochState`

Enum with `Active` (+1), `GracePeriod` (0), `Reclaimable` (−1). Implements `Display` and `From<EpochState> for i8`.

## Architecture Notes

The manager uses `Arc<EpochManagerInner>` for shared state, with a `Weak` reference held by each guard. This ensures the manager outlives all guards and prevents use-after-free if the manager is dropped first.

The active guard counts are stored in a `HashMap<u64, usize>` — mapping epoch number to number of active guards. This allows O(1) increment/decrement on pin/drop.

The **γ + η = C** ternary model is the central design principle: each epoch is **(γ) Active** (has live guards, not safe to reclaim), **(η) Reclaimable** (no guards and old enough, safe to free), or in the **GracePeriod** boundary state (η₀). The classification determines whether deferred frees can proceed.

## References

1. Fraser, K. (2004). *Practical Lock-Freedom*. PhD thesis, University of Cambridge. — Epoch-based reclamation in the context of lock-free algorithms.
2. Leis, V. et al. (2015). "The adaptive radix tree: ARTful indexing for main-memory databases." *ICDE 2014*. — Uses epoch reclamation for concurrent index structures.
3. Cruz, F. P. (2019). "Crossbeam Epoch: A safe and fast memory reclamation mechanism." crossbeam-rs documentation.
4. Hart, T. E. et al. (2007). "Performance of memory reclamation for lockless synchronization." *Journal of Parallel and Distributed Computing*, 67(11), 1270–1285.
5. McKenney, P. E. & Slingwine, J. D. (1998). "Read-copy update: Using execution history to solve concurrency problems." *Parallel and Distributed Computing and Systems*.

## License

MIT
