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
