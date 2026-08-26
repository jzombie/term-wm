# term-test-support

Internal test tooling for the [term-wm](https://crates.io/crates/term-wm) workspace. Not intended for direct use.

A dev-dependency only: nothing in this crate may run in production.

## What this is

The workspace's testing policy is **"no blind sleeps"**: every wait must observe real state rather than hope a fixed delay was long enough. Each suite used to hand-roll (and drift on) the same few patterns; this crate consolidates them so timing flakiness, leaked resources from panicking tests, and cross-run interference are fixed once here instead of per suite.

## Utilities

| Utility | Purpose |
|---|---|
| [`wait_for`](src/poll_sync.rs) | Polls a sync probe until it returns `Some(value)`; panics with a descriptive message once the deadline elapses. The standard replacement for fixed sleeps. |
| [`wait_for_async`](src/poll_async.rs) | Async counterpart driven by tokio timers, for async test harnesses. Behind the `tokio` feature. |
| [`KillOnDrop`](src/guard.rs) | RAII guard that runs a cleanup closure exactly once when dropped, including during panic unwinding, so a failing test cannot leak spawned processes, PTYs, or temp state. Call `defuse()` when the happy path already performed a graceful teardown. |
| [`ManualClock`](src/clock.rs) | Thread-safe, cheaply cloneable virtual clock (`base + advanced offset`). Advancing moves only the timestamp provider; tests must still drain their scheduler explicitly after each advance before asserting. |
| [`unique_gateway_name`](src/naming.rs) | IPC channel names embedding tag, pid, and a per-process counter, so concurrent runs (or leftovers from crashed runs) can never claim each other's endpoints. |
| [`apply_test_logging`](src/log_capture.rs) | Points a spawned process at a unique `TERM_WM_LOG_FILE` under a stable per-run root (`TERM_WM_TEST_LOG_DIR`, default `<temp>/term-wm-test-logs/`) with `RUST_LOG=debug`, so CI can archive daemon diagnostics from failed runs. Companions [`test_log_dir`](src/log_capture.rs) / [`test_log_file`](src/log_capture.rs) expose the paths. |

## Usage

```toml
[dev-dependencies]
# Everything except wait_for_async is std-only:
term-test-support = { workspace = true }
# Or, when async polling is needed:
term-test-support = { workspace = true, features = ["tokio"] }
```

```rust
use std::time::Duration;

use term_test_support::{KillOnDrop, unique_gateway_name, wait_for};

let _daemon = KillOnDrop::new(|| daemon.kill());
let gateway = unique_gateway_name("probe");
let conn = wait_for(Duration::from_secs(10), "gateway reachable", || {
    try_connect(&gateway).ok()
});
```

All waits take an explicit deadline and a human-readable description; the panic message includes both, so a regression reads as evidence instead of a misleading downstream assert.

## Features

- `tokio`: enables [`wait_for_async`](src/poll_async.rs). Default features are empty.
