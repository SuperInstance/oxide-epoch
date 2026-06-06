# oxide-epoch

*Epoch-based memory reclamation for GPU data structures. Ternary epoch states: Active (+1) → Grace (0) → Reclaimable (-1). When the pipeline is asynchronous, you need to know when memory is safe to free.*

## Why This Exists

Lock-free data structures can't free memory immediately after a logical deletion — other threads (or GPU kernels) might still be reading it. The standard solution is epoch-based reclamation: divide time into epochs, track which epoch each thread is in, and only reclaim memory when no thread holds a reference to it.

On the GPU, this problem is harder. Kernels run asynchronously across hundreds of streaming multiprocessors. There's no garbage collector, no reference counting (without atomic overhead), and no way to interrupt a running kernel. The ternary epoch state machine solves this with three clear phases:

- **Active (+1):** The epoch has live guards. Memory is in use. No reclamation.
- **Grace Period (0):** No live guards, but too recent for safety. Deferred items wait.
- **Reclaimable (-1):** Safe to free. All prior epochs are guaranteed idle.

## Architecture

```
Timeline: ─── Epoch N-1 ─── Epoch N ─── Epoch N+1 ───
              Reclaimable    Grace         Active
                  (-1)         (0)          (+1)
                   ↓
               free(pending)
```

### Key Types

- **`Epoch`** — Global epoch counter. Advances when all active guards are from the current epoch.
- **`Guard`** — RAII guard that pins the current epoch. Created on kernel entry, dropped on exit. While a guard exists, its epoch won't advance past Grace.
- **`EpochState`** — Active / Grace / Reclaimable. Computed from the global epoch and guard set.
- **`Bag`** — Queue of deferred reclamation actions. Flush when epoch becomes Reclaimable.
- **`Collector`** — Manages epochs, guards, and bags. The top-level API.

## Usage

```rust
use oxide_epoch::*;

let collector = Collector::new();

// Kernel entry — pin epoch
let guard = collector.enter();
let epoch_state = collector.state(&guard);
assert_eq!(epoch_state, EpochState::Active);

// Defer reclamation of old data
let old_data = vec![1, 2, 3];
collector.defer(&guard, move || drop(old_data));

// Kernel exit — release guard
drop(guard);

// After all guards released, advance epoch and reclaim
collector.try_advance();
collector.collect(); // Flushes bags from Reclaimable epochs
```

## The Deeper Idea

The ternary state machine (Active→Grace→Reclaimable) is the same lifecycle that appears across the SuperInstance ecosystem:
- `ternary-consensus`: Commit/Pending/Abort
- `ternary-gc`: Live/Weak/Dead  
- `agent-phase-change`: Growth/Stasis/Decay

This isn't coincidence — it's the universal lifecycle of asynchronous resources. The three states capture the essential progression from "in use" through "transitioning" to "done." Binary (in-use/free) can't express the transitional grace period that prevents use-after-free.

## Related Crates

- `oxide-chunk` — Memory chunk allocator that uses epoch-based reclamation
- `oxide-tombstone` — Tombstone deletion (complementary reclamation strategy)
- `oxide-barrier` — Synchronization barriers that coordinate with epoch advancement
- `oxide-slotmap` — Slot-based allocation with generation counters (another safe reclamation approach)
