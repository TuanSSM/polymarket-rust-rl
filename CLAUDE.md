# polymarket-rust-rl

HFT-inspired per-core execution architecture for Polymarket reinforcement learning.

## Quick Reference

```bash
cargo test                          # 105 tests, must all pass
cargo bench                         # criterion: gate, seqlock, spsc, tick_cycle
cargo clippy -- -D warnings         # zero warnings policy
cargo fmt --check                   # rustfmt.toml enforced
```

## Architecture

```
Controller (1)          ──publish──▶  SeqLock<Parameters>
   │                                       ▲ read (~1ns)
   │ send via SPSC                         │
   ▼                                       │
CoreEngine (N)          ──────────────────────┘
   │  owns: Consumer<TradeEvent>, Position, momentum state
   │  hot path: pop events → gate eval → emit signals
   └──▶ Vec<Signal> ──drain──▶ Controller collects
```

**Zero shared mutable state on hot path.** Each CoreEngine owns its position
and reads parameters via SeqLock (read-only, ~1ns cache-hit, 99.6%+ hit rate).

## Module Map

| Module | Purpose | Key Types |
|--------|---------|-----------|
| `types.rs` | Shared types, `CachePadded<T>` (64-byte align) | `TradeEvent`, `Signal`, `Parameters`, `Position`, `Side`, `Direction` |
| `spsc.rs` | Lock-free SPSC ring buffer, power-of-2 capacity | `SpscRingBuffer<T>`, `Producer<T>`, `Consumer<T>` |
| `seqlock.rs` | SeqLock for parameter broadcast | `SeqLock<T>` — `.read()`, `.write()`, `.read_with_seq()` |
| `gate.rs` | Branchless risk checks via arithmetic composition | `position_limit_gate`, `signal_strength_gate`, `spread_gate`, `direction_gate`, `evaluate_gates`, `full_gate_eval` |
| `core_engine.rs` | Single-position-per-core engine | `CoreEngine` — `.process_tick()`, `.drain_signals()` |
| `controller.rs` | Multi-core orchestrator | `Controller` — `.send_event()`, `.route_event()`, `.broadcast_event()`, `.publish_parameters()`, `create_infrastructure()` |
| `analytics.rs` | VWAP, volatility, spread/mid (Decimal→f64 via ToPrimitive) | `VwapAccumulator`, `MarketAnalytics`, `realized_volatility()` |
| `cvd_momentum.rs` | CVD momentum indicator (Decimal→f64 via ToPrimitive) | `CvdMomentum`, `batch_cvd()` |

## Performance-Critical Rules

1. **No allocations on hot path.** SPSC ring buffer and signal vecs are pre-allocated. Never `Box`, `Vec::push` past capacity, or format strings in `CoreEngine::process_tick`.
2. **No branches in gate evaluation.** All gates use sign-bit extraction (`to_bits() >> 63`) for branchless pass/fail. Compose multiplicatively — never use `if`.
3. **No locks on hot path.** SeqLock readers spin on sequence mismatch only during writes (~0.4% of reads). No Mutex, RwLock, or channels.
4. **CachePadded for cross-thread atomics.** `head` and `tail` in SPSC must remain on separate cache lines (64-byte aligned).
5. **SPSC ring buffer capacity must be power of 2.** Index wrapping uses bitmask (`& mask`), not modulo.

## Decimal-to-f64 Conversion Rule

**Always use `ToPrimitive::to_f64()` from `num_traits`.** Never `decimal.to_string().parse::<f64>()`.

```rust
// CORRECT
use num_traits::ToPrimitive;
let val = decimal.to_f64().unwrap_or(0.0);

// WRONG — string round-trip loses precision and is 100x slower
let val: f64 = decimal.to_string().parse().unwrap();
```

This applies everywhere `rust_decimal::Decimal` is converted to `f64`. The `analytics.rs` and `cvd_momentum.rs` modules demonstrate the correct pattern.

## Code Conventions

- Edition 2021, `max_width = 100` (rustfmt.toml)
- Tests inline in `#[cfg(test)] mod tests` per module
- Benchmarks in `benches/` using Criterion 0.5
- `#[inline(always)]` on gate functions only — let compiler decide elsewhere
- `unsafe` blocks: only in SPSC (`UnsafeCell` access) and SeqLock (data read). Each has `// SAFETY` justification via Send/Sync impls
- Error handling: `Result<(), T>` for try_push, `Option<T>` for try_pop — no panics on hot path

## Testing Patterns

- **SPSC cross-thread tests:** use `Arc<SpscRingBuffer>` + raw pointer casting for Send. Verify FIFO ordering, concurrent consistency, and sum invariants.
- **SeqLock concurrent tests:** write correlated fields (e.g., `a` and `b = a*2`), readers verify consistency to detect torn reads.
- **Gate tests:** test each gate at boundary (under/at/over), test composition, verify branchless behavior (0.0 or 1.0 output only).
- **Integration test pattern:** `create_infrastructure()` → split buffers → create Controller + CoreEngines → send events → process_tick → drain signals.

## Benchmark Targets

| Benchmark | Expected | Notes |
|-----------|----------|-------|
| `position_limit_gate` | <1ns | Single branchless gate |
| `seqlock_read` | ~1ns | Cache-hit steady state |
| `spsc_push_pop` | <10ns | Single push+pop cycle |
| `full_tick_cycle_1core` | <50ns | Push → process → drain |

Run: `cargo bench -- <name>` for individual, `cargo bench` for all.

## Adding New Modules

1. Add `pub mod name;` to `lib.rs`
2. Include `#[cfg(test)] mod tests` in the module
3. If performance-critical: add bench in `benches/`, register in `Cargo.toml`
4. Follow existing patterns: `Copy` types for SPSC/SeqLock, `ToPrimitive` for Decimal
5. Run full `cargo test && cargo clippy -- -D warnings` before committing
