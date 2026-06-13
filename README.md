# Oxide Epoch

**Oxide Epoch** provides epoch-based memory reclamation (EBR) for GPU data structures with ternary epoch states — `+1` (active), `0` (grace period), `-1` (reclaimable) — enabling lock-free safe deallocation of concurrently accessed data.

## Why It Matters

Lock-free data structures (concurrent queues, hash maps, skip lists) have a fundamental problem: when thread A removes a node, can it safely free the memory? Thread B might still be reading it. Epoch-based reclamation solves this by tracking which epoch each thread is in. Nodes removed in epoch N can only be freed once all threads have moved past epoch N+1. This is **O(1)** overhead per operation — far cheaper than hazard pointers (**O(k)** per access with k pointers) or reference counting (**O(1)** but with atomic CAS contention).

## How It Works

### Epoch Tracking

The global epoch counter increments periodically. Each thread pins itself to a snapshot:

```
Global: epoch = 5

Thread A: pin() → local_epoch = 5
Thread B: pin() → local_epoch = 5
Thread A: unpin() → no longer active
Thread B: unpin() → no longer active

advance():
  epoch 4: Active? No. Grace period passed? Yes → Reclaimable (-1)
  epoch 5: Active? No → Grace Period (0)
  epoch 6: Active (has live guards) → Active (+1)
```

### Ternary Epoch Classification

For any epoch E relative to current epoch C:

```
E has live guards            → Active (+1)
E has no guards, C - E ≤ 1   → Grace Period (0)   [not yet safe]
E has no guards, C - E > 1   → Reclaimable (-1)   [safe to free]
```

The grace window of one epoch ensures that any thread that read the epoch value before advancing has had time to finish its critical section.

### Guard Lifecycle

```rust
let guard = manager.pin();   // Enter critical section
// ... access lock-free data structures ...
drop(guard);                  // Exit critical section
// Deferred items from our epoch will be reclaimed later
```

Guard creation: **O(1)** (atomic increment of local epoch counter). Guard destruction: **O(1)**.

### Deferred Reclamation

When a node is removed, it's added to the current epoch's deferred list:

```
manager.defer(node_ptr);  // O(1) — push to Vec
```

On `advance_and_reclaim()`:
1. Increment global epoch: **O(1)** atomic
2. Scan thread-local epochs: **O(T)** where T = thread count
3. For each reclaimable epoch, free all deferred items: **O(D)** total deferred items

Total reclaim cost amortized: **O(D/T)** per call.

## Quick Start

```rust
use oxide_epoch::EpochManager;

let manager = EpochManager::new();
let guard = manager.pin();
// ... lock-free operations ...
drop(guard);
manager.defer(node_to_free);
manager.advance_and_reclaim(); // Safely frees deferred items
```

## API

| Type | Description |
|------|-------------|
| `EpochManager` | Global epoch counter with per-thread tracking |
| `EpochGuard` | RAII guard — active while pinned |
| `EpochState` | `Active (+1)`, `GracePeriod (0)`, `Reclaimable (-1)` |

Key methods: `pin()`, `defer(ptr)`, `advance_and_reclaim()`, `active_epoch_count()`.

## Architecture Notes

Oxide Epoch provides safe memory reclamation for GPU data structures in the oxide-* stack. In γ + η = C, epoch advancement is γ (growth — making progress on reclamation) while the grace period is η (avoidance — waiting until deallocation is provably safe). Integrates with `oxide-ring` (ring buffer reclamation) and `oxide-tombstone` (lazy deletion garbage collection).

See [ARCHITECTURE.md](https://github.com/SuperInstance/SuperInstance/blob/main/ARCHITECTURE.md) for GPU memory management architecture.

## References

1. Fraser, K. (2004). *Practical Lock-Freedom*. PhD Thesis, University of Cambridge. (On epoch-based reclamation)
2. Hart, D. E. et al. (2007). "Performance of memory reclamation for lockless synchronization." *Journal of Parallel and Distributed Computing*, 67(12), 1270–1285.
3. Lea, D. (2013). "Reclamation for lock-free data structures." *java.util.concurrent documentation*.

## License

MIT
