# Plan: Multi-Heuristic Power Profiling

## Goal
Replace the single `profile_from_activity(last_event_at)` with a `select_profile()` that weighs multiple signals:
1. **Input recency** (existing, now multi-threshold)
2. **PTY output streaming** — new data arrived since last render
3. **SSH session** — cap fps over slow links

## Changes

### 1. `crates/term-wm-core/src/power_profile.rs`

**Add `MediumPerformance` variant** (30fps, 33ms):
```rust
pub enum PowerProfile {
    HighPerformance,     // 16ms, ~60fps
    MediumPerformance,   // 33ms, ~30fps
    #[default]
    PowerSaver,          // 500ms, ~2fps
}
```
- `poll_interval()` → 33ms for Medium
- `name()` → "MediumPerformance"
- `indicator_color()` → `theme::profile_mid_bg()` (amber, already exists as dead code)

**Replace `profile_from_activity` with `select_profile`:**
```rust
pub struct ProfileSignals {
    pub last_input_at: Option<Instant>,
    pub pty_bytes_received_since_render: bool,
    pub is_ssh_session: bool,
}

pub fn select_profile(signals: &ProfileSignals) -> PowerProfile { ... }
```
Logic:
- `HighPerformance` if input within 500ms AND NOT SSH
- `MediumPerformance` if input within 500ms AND SSH, OR input within 2s, OR PTY streaming
- `PowerSaver` otherwise

**Update `PowerProfileTracker`** — no changes needed (generic over the enum).

**Update tests:** profile selection tests for all signal combinations, new tier poll interval test.

### 2. `crates/term-wm-core/src/io/console.rs`

`ConsoleEventSource::current_profile()` now calls `select_profile()` instead of `profile_from_activity()`. Pass `last_event_at` as the input signal. Other signals (PTY, SSH) are set to their defaults (`false`) since the event source doesn't know about them — they enter via the runner.

### 3. `crates/term-wm-sys-ui-components/src/wm_bottom_panel.rs` — **no changes needed**
The profile indicator already renders `indicator_color()` generically. `MediumPerformance` → amber via `profile_mid_bg()`.

### 4. `crates/term-wm-core/src/theme.rs` — **no changes needed**
`ProfileMid`/`profile_mid_bg()` (amber `Rgb(255, 193, 7)`) already exists as dead code, now activated.

### 5. `crates/term-wm-core/src/runner.rs`

The runner is where PTY state and SSH info are available. After calling `driver.current_profile()`, enrich the signals:
- `pty_bytes_received_since_render` → requires plumbing from `TerminalComponent` (need to add a tracker on the `Pane` trait or `WindowProvider`)
- `is_ssh_session` → check `std::env::var("SSH_CONNECTION").is_ok()` once at startup

This is the trickiest part. Approach: add an optional method on `WindowProvider`:
```rust
trait WindowProvider {
    fn has_pending_output(&self) -> bool { false }
}
```
`App` in `main.rs` overrides this by checking if any terminal has received new bytes since last render.

### 6. `crates/term-wm-core/src/event_loop.rs`

Update `profiles_affect_event_loop_poll_interval` to cycle through all three tiers (500ms → 33ms → 16ms).

## Heuristic details

| Signal | Source | Effect |
|--------|--------|--------|
| Input recency (< 500ms) | `ConsoleEventSource.last_event_at` | HP (local) or MP (SSH) |
| Input recency (500ms–2s) | same | MP |
| PTY bytes since last render | `Pane::bytes_received()` diff | MP (streaming output needs ~30fps) |
| SSH session | `SSH_CONNECTION` env var | Caps to MP (network bottleneck) |

## Files modified
| File | Action |
|------|--------|
| `power_profile.rs` | Add MediumPerformance, ProfileSignals, select_profile, update tests |
| `console.rs` | Update `current_profile()` to use `select_profile()` |
| `runner.rs` | Enrich signals from app state, add `has_pending_output` to `WindowProvider` |
| `event_loop.rs` | Extend test to all three tiers |

## Verification
```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test
```
