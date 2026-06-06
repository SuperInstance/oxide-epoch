# oxide-epoch

GPU infrastructure crate from the SuperInstance ecosystem.

## Overview

# oxide-epoch

Epoch-based memory reclamation for GPU data structures with ternary epoch states.

## Architecture

This crate sits within the **five-layer Oxide Stack**:

| Layer | Crate | Role |
|-------|-------|------|
| 1 | open-parallel | Async runtime (tokio fork) |
| 2 | pincher | "Vector DB as runtime, LLM as compiler" |
| 3 | flux-core | Bytecode VM + A2A agent protocol |
| 4 | cuda-oxide | Flux→MIR→Pliron→NVVM→PTX compiler |
| 5 | cudaclaw | Persistent GPU kernels, warp consensus, SmartCRDT |

The key insight: **ternary values {-1, 0, +1} map directly to GPU compute**. They pack 16× denser than FP32, enable XNOR+popcount matmul, and conservation laws become compile-time checks.

## Stats

| Metric | Value |
|--------|-------|
| Tests | 10 |
| Lines of Code | 439 |
| Public API Surface | 18 items |
| License | Apache-2.0 |

## Installation

```toml
[dependencies]
oxide-epoch = "0.1.0"
```

## Usage

```rust
use oxide_epoch::*;
// See src/lib.rs tests for complete working examples
```

### Key Types

```
- pub enum EpochState {
- pub struct DeferredFree<T> {
- pub struct EpochGuard {
    pub fn epoch(&self) -> u64 {
- pub struct EpochManager {
    pub fn new() -> Self {
    pub fn with_grace_lag(grace_lag: u64) -> Self {
    pub fn current_epoch(&self) -> u64 {
    pub fn advance(&self) -> u64 {
    pub fn pin(&self) -> EpochGuard {
```

## Design Philosophy

This crate uses **ternary algebra** (Z₃) where every value is {-1, 0, +1}:

- **+1** → positive signal (healthy, allocated, converged, ready)
- **0** → neutral (pending, balanced, monitoring, degraded)
- **-1** → negative signal (failed, free, diverged, overloaded)

This isn't arbitrary — ternary is the natural encoding for:
1. **BitNet b1.58** (Microsoft) — ternary neural networks at 60% less power
2. **GPU warp voting** — hardware ballot instructions return ternary consensus
3. **Conservation laws** — {-1, 0, +1} preserves quantity (what goes in must come out)

## Testing

```bash
git clone https://github.com/SuperInstance/oxide-epoch.git
cd oxide-epoch
cargo test
```

## License

Apache-2.0
