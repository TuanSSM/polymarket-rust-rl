# CLAUDE.md

## Project Overview

Polymarket RL trading bot — a Rust-based reinforcement learning system for trading on Polymarket prediction markets. Uses WebSocket feeds, Bayesian inference, Kelly criterion sizing, and a lock-free execution engine.

## Build & Verify

```bash
cargo build              # compile
cargo test               # run all tests (42 currently)
cargo clippy             # lint — treat warnings as actionable
cargo fmt -- --check     # formatting check
```

Always run `cargo test` and `cargo clippy` before committing. All tests must pass and no new clippy warnings should be introduced.

## Conventional Commits

All commit messages **must** follow the [Conventional Commits](https://www.conventionalcommits.org/) specification:

```
<type>[optional scope]: <description>

[optional body]

[optional footer(s)]
```

### Types

| Type       | When to use                                          |
|------------|------------------------------------------------------|
| `feat`     | New feature or capability                            |
| `fix`      | Bug fix                                              |
| `refactor` | Code change that neither fixes a bug nor adds a feature |
| `perf`     | Performance improvement                              |
| `test`     | Adding or updating tests                             |
| `docs`     | Documentation only                                   |
| `ci`       | CI/CD configuration changes                          |
| `build`    | Build system or dependency changes                   |
| `chore`    | Maintenance tasks (tooling, config, no prod code)    |

### Scopes

Use the module name as scope when the change is localized to a single module:

`feat(engine):`, `fix(bayesian):`, `refactor(kelly):`, `test(spsc):`, `perf(seg_lock):`

Omit scope for cross-cutting changes.

### Rules

- **Subject line**: imperative mood, lowercase, no period, max 72 chars (e.g. `feat(policy): add epsilon-greedy exploration`)
- **Body** (optional): wrap at 80 chars, explain *why* not *what*
- **Breaking changes**: add `!` after type/scope and a `BREAKING CHANGE:` footer (e.g. `feat(config)!: require api_key in config`)
- **No** `WIP`, `misc`, `update`, or vague messages

### Examples

```
feat(engine): add order throttling per market

Prevents exceeding Polymarket's rate limits by tracking order
timestamps per condition_id and delaying when within the window.

fix(polymarket_ws): reconnect on abnormal close codes

refactor(kelly): extract bet fraction clamping into helper

test(signal): add edge case for zero-spread oracle delay

build: bump tokio to 1.38 for io_uring support

BREAKING CHANGE: minimum supported Rust version is now 1.75
```

## Pull Requests

### Branch Naming

```
<type>/<short-description>
```

Examples: `feat/order-throttling`, `fix/ws-reconnect`, `refactor/kelly-clamping`

### PR Title

Follow the same conventional commit format as the commit subject line:

```
feat(engine): add order throttling per market
```

### PR Description

Use this template:

```markdown
## Summary
- Bullet points describing what changed and why (1-3 bullets)

## Test plan
- [ ] How the change was verified
- [ ] Edge cases considered
```

### PR Rules

- One logical change per PR — don't mix unrelated fixes/features
- PR title must be a valid conventional commit subject line
- Squash-merge into `main`; the PR title becomes the merge commit message
- All CI checks (test, clippy, fmt) must pass before merge
- Keep PRs small and reviewable (< 400 lines diff preferred)

---

## Architecture

```
Main (tokio::main)
└── Controller::run()
    ├── 4 Feed tasks ── push TradeEvents/OracleTicks via SPSC ──┐
    ├── SignalEngine (50ms) ── broadcast SignalSnapshot ─────────┤
    ├── N CoreEngine tasks (one per market) ◄───────────────────┘
    │     ├── Seqlock reader (lock-free param load)
    │     ├── Broadcast receiver (signals)
    │     ├── Watch receiver (order book)
    │     └── SPSC sender (episode outcomes)
    ├── PolymarketWsClient ── routes books via per-market watch channels
    └── Main loop (100ms) ── collects outcomes, updates policy via seqlock
```

**Module map:**

| Module             | Purpose                                       | Hot path? |
|--------------------|-----------------------------------------------|-----------|
| `engine.rs`        | Per-market trading loop, episode execution     | **Yes**   |
| `policy.rs`        | Linear Q-function, TD(0) updates, action space | **Yes**   |
| `gate.rs`          | Branchless signal gating                       | **Yes**   |
| `bayesian.rs`      | Bayesian probability estimator (log-odds)      | **Yes**   |
| `kelly.rs`         | Kelly criterion position sizer                 | **Yes**   |
| `spsc.rs`          | Lock-free SPSC ring buffer                     | **Yes**   |
| `seg_lock.rs`      | Seqlock for parameter distribution             | **Yes**   |
| `signal.rs`        | CVD aggregation, oracle delay, premium calc    | Warm      |
| `exchange_ws.rs`   | Binance/Bybit trade feed clients               | Warm      |
| `polymarket_ws.rs` | Order book feed, per-market routing            | Warm      |
| `chainlink.rs`     | Oracle feed with HMAC auth                     | Warm      |
| `polymarket_rest.rs` | CLOB REST API (orders, market discovery)     | Cold      |
| `controller.rs`    | Orchestrator, spawns tasks, RL updates         | Cold      |
| `config.rs`        | TOML config loading and validation             | Cold      |
| `error.rs`         | Error type hierarchy                           | Cold      |
| `main.rs`          | Entry point                                    | Cold      |

---

## Rust Development Rules

### Idioms & Style

- **Edition 2021** — use all stable edition features; do not use nightly-only features
- **Max line width: 100** (per `rustfmt.toml`) — no exceptions
- Prefer `thiserror` for library-style error enums; reserve `anyhow` for top-level binaries only
- Use `tracing` for all logging — never `println!` or `eprintln!` in production code
- Prefer `impl Trait` in argument position over generics when only one call site exists
- Use exhaustive `match` over `if let` chains — let the compiler catch missing variants
- Prefer `?` propagation over `.unwrap()` and `.expect()` — unwrap is only acceptable in tests and provably-safe `const` contexts
- Use `Self` in impl blocks — avoid repeating the type name
- Destructure structs at use site rather than accessing fields through intermediate bindings
- Derive traits in canonical order: `Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord`
- Prefer `#[must_use]` on pure functions that return computed values

### Type System

- Newtypes for domain quantities — don't pass raw `f64` where `Bps`, `Usd`, or `Probability` is meant
- Use `#[non_exhaustive]` on public enums that may grow variants
- Prefer `enum` over `bool` parameters — `Mode::Live` reads better than `true`
- Lifetime elision is fine when unambiguous; annotate explicitly when multiple references are involved
- Keep `pub` surface minimal — default to `pub(crate)`, expose only what's needed

### Dependencies

- Minimize dependency count — each new crate is attack surface, compile time, and audit burden
- Pin major versions in `Cargo.toml` (e.g. `tokio = "1"`, not `tokio = "*"`)
- Prefer `cargo update --precise` for targeted security patches
- Run `cargo audit` periodically — no known advisories in production deps
- Prefer `no_std`-compatible crates when possible for core numeric types

### Compilation & Optimization

- **Dev builds**: default profile; use for rapid iteration
- **Release builds**: `cargo build --release` with LTO for production
- Profile with `cargo flamegraph` or `perf` before optimizing — measure, don't guess
- Recommended release profile additions for production:
  ```toml
  [profile.release]
  lto = "fat"
  codegen-units = 1
  panic = "abort"
  strip = true
  ```

---

## HFT & Low-Latency Rules

### Zero-Allocation Hot Path

The trading loop in `CoreEngine::run_episode()` is the critical path. These rules apply to **all code reachable from the engine loop**:

- **No heap allocations** — no `Vec::push`, `String::from`, `Box::new`, `format!`, or `to_string()` in the hot path
- **No syscalls** — no file I/O, no `println!`, no logging at `debug!` or `trace!` level on every tick (use `tracing`'s compile-time level filtering)
- **No locks** — use the existing SPSC and seqlock primitives; never introduce `Mutex`, `RwLock`, or `mpsc` channels on the hot path
- **Stack-resident data only** — all signal/state/action types must implement `Copy` and fit in cache lines
- **Pre-allocate everything** — size buffers and channels at startup; the engine loop must never grow a collection

### Memory Layout & Cache Discipline

- **Cache-line awareness**: hot-path structs should be ≤ 64 bytes or aligned to 64-byte boundaries with `#[repr(align(64))]` to prevent false sharing
- `StateVec` (56 bytes, 7×f64) and `SignalSnapshot` (72 bytes) are already cache-friendly — keep them that way
- `Params` (360 bytes) is shared via seqlock — if false sharing is observed under profiling, add `#[repr(align(64))]`
- Use `#[repr(u8)]` for small enums (e.g. `Action`) to minimize discriminant size
- Keep arrays contiguous — prefer `[f64; N]` over `Vec<f64>` for fixed-size numeric data
- When adding fields to hot structs, verify size with `std::mem::size_of::<T>()` in a test

### Branchless & Predictable Code

- Prefer branchless arithmetic over `if/else` in signal processing (see `BranchlessGate::apply()`)
- Use `.min()` / `.max()` / `.clamp()` instead of branching comparisons for saturation
- Avoid `match` on runtime values in tight loops — if the dispatch is static, use generics/monomorphization
- Sort infrequently-taken error paths out of the hot path with `#[cold]` annotation
- Use `std::hint::unreachable_unchecked()` only when the invariant is proven and profiling justifies it

### Inline Discipline

- `#[inline(always)]` — only for functions ≤ 10 lines called in the innermost loop (e.g. `gate.apply()`, `q_value()`)
- `#[inline]` — for functions called from one or two sites across crate boundaries
- **Never** `#[inline(always)]` on functions that allocate, log, or call async — it bloats instruction cache
- Let the compiler decide for everything else — premature inlining defeats optimizer heuristics

### Atomic Ordering

- **Acquire/Release** — for all cross-task data visibility (SPSC head/tail, seqlock sequence)
- **Relaxed** — only for local cached indices or counters not used for synchronization
- **SeqCst** — avoid; it's a full fence on x86 and rarely needed. If you think you need it, document why
- Every atomic operation must have a comment explaining the ordering choice if it's not Acquire/Release

### SPSC & Seqlock Usage

These are the project's core lock-free primitives — handle with care:

- **SPSC** (`spsc.rs`): one producer, one consumer — enforced by API. Never clone or share the sender/receiver
  - Use `push_overwrite()` for feed data (freshness > completeness)
  - Use `try_push()` when drops must be tracked
  - Use `drain_last()` to skip stale entries and get only the latest
- **Seqlock** (`seg_lock.rs`): single writer, multiple readers — writer must be unique
  - Readers spin on odd sequence numbers — keep write duration minimal
  - Write the entire `Params` struct atomically (single `write()` call), never partial updates
  - Readers should not hold the read value across await points

### Unsafe Code Discipline

This codebase has 12 justified `unsafe` blocks (all in `spsc.rs` and `seg_lock.rs`). Rules for any new unsafe:

1. **Exhaust safe alternatives first** — unsafe is a last resort, not a shortcut
2. **Document the safety invariant** with a `// SAFETY:` comment directly above the block
3. **Minimize scope** — the unsafe block should contain the bare minimum operations
4. **Pair with tests** — every unsafe block must have corresponding unit tests exercising the boundary conditions
5. **No new `unsafe impl Send/Sync`** without review — incorrect Send/Sync is undefined behavior
6. **Miri validation** — run `cargo +nightly miri test` on any module with new unsafe code
7. **Forbidden patterns**:
   - No `unsafe` to bypass borrow checker for convenience
   - No `transmute` without a proven layout guarantee
   - No raw pointer arithmetic without bounds checks (except in ring buffers with power-of-two masking)

### Concurrency Rules

- **Task topology is fixed at startup** — don't spawn tasks dynamically during trading
- **One SPSC channel per data flow** — never multiplex different message types on one channel
- **Broadcast for fan-out** (signals), **Watch for latest-value** (order books), **SPSC for point-to-point** (feeds, outcomes)
- **Backpressure strategy**: feeds overwrite stale data (`push_overwrite`); engines skip lagged signals; outcomes are drained in batch
- **Graceful shutdown**: use `tokio::signal` and `CancellationToken` — never `std::process::exit()` in library code
- Pin CPU-intensive tasks to dedicated threads with `tokio::task::spawn_blocking` if they risk starving the async executor

### WebSocket & Feed Resilience

- **Reconnect with exponential backoff** — 1s initial, 60s cap, 20 retries max (already implemented)
- Add **jitter** (±25%) to backoff to prevent thundering herd on mass reconnect
- Log reconnect attempts at `warn!` level, successful reconnects at `info!`
- **Never block the feed task** waiting for downstream — use `push_overwrite` to drop stale data
- Validate all incoming JSON defensively — malformed exchange messages must not panic the feed loop
- Treat WebSocket `Close` frames with codes 1000/1001 as normal; all others as errors requiring reconnect

### Risk & Safety Guards

- **All order quantities go through Kelly sizing** — never bypass `kelly.size()` for live orders
- **DryRun mode must be functionally identical** to Live except for the REST call — same signals, same sizing, same logging
- **Config validation at startup** — fail fast with clear error messages; never silently default a risk parameter
- **Position limits are hard caps** — `max_position_usd` must be checked both in sizing and before order submission
- **Secrets** (`private_key`, `api_secret`, `hmac_secret`) must never appear in logs, error messages, or debug output

---

## Testing Standards

### Test Categories

| Category      | Location         | Runs in CI | Purpose                                 |
|---------------|------------------|------------|-----------------------------------------|
| Unit tests    | `#[cfg(test)]` in each module | Yes | Verify module-level logic in isolation |
| Integration   | `tests/`         | Yes        | End-to-end with mocked feeds            |
| Property      | `proptest` / `quickcheck` | Yes | Fuzz numeric edge cases (NaN, Inf, subnormals) |
| Latency       | `benches/`       | No (manual) | Criterion benchmarks for hot path       |

### What to Test

- **Every `unsafe` block** — boundary conditions, concurrent access, drop behavior
- **Numeric edge cases** — NaN, infinity, negative zero, subnormal floats, division by zero
- **State machine transitions** — episode lifecycle, reconnect state, order state
- **Concurrency** — SPSC under contention, seqlock write-during-read, broadcast lag recovery
- **Config validation** — invalid values, missing fields, boundary values

### Test Rules

- Tests must be deterministic — use `SmallRng::seed_from_u64()` for reproducibility, never `thread_rng()`
- No `#[ignore]` without a tracking issue
- No `sleep()` in tests — use `tokio::time::pause()` for async timing tests
- Assert with descriptive messages: `assert!(edge > 0.0, "edge must be positive, got {edge}")` 
- Benchmark before and after any `perf`-type commit — include numbers in the commit body
