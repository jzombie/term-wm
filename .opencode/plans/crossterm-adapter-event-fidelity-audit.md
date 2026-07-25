# Plan: Audit Crossterm Adapter Event Fidelity

## Goals
Verify that `crates/term-wm-crossterm-adapter/` passes all keyboard and mouse events through without dropping or modifying them adversely.

## Findings

### Adapter (`crates/term-wm-crossterm-adapter/src/lib.rs`)

| Aspect | Status | Details |
|---|---|---|
| All crossterm `Event` variants routed | ✅ | Key, Mouse, Resize, FocusGained, FocusLost, Paste — all handled |
| All mouse event kinds mapped | ✅ | Down→Press, Up→Release, Drag, Moved, ScrollUp/Down/Left/Right |
| All mouse buttons mapped | ✅ | Left, Right, Middle |
| `KeyEventKind` faithfully forwarded | ✅ | Press→Press, Repeat→Repeat, Release→Release |
| `KeyModifiers` faithfully forwarded | ✅ | shift/control/alt flags mirrored |
| BackTab normalization | ⚠️ Intentional | BackTab→Tab+shift (crossterm `BackTab` becomes core `Tab` + `shift: true`) |

### Issues Found

1. **🔴 Media keys dropped despite core support** — `translate_key_code` wildcard `_ => None` catches `crossterm::event::KeyCode::Media(MediaKey::{PlayPause, Stop, TrackNext, TrackPrevious})`. The core `KeyCode` has matching variants (`MediaPlayPause`, `MediaStop`, `MediaTrackNext`, `MediaTrackPrevious`). These events are silently dropped (or mapped to `Esc` via the infallible path). **Fix: add explicit matches for crossterm Media keys.**

2. **🟡 Unknown keys → `Esc` in infallible path** — `translate_key_event` maps unrecognized keys (CapsLock, NumLock, etc.) to `KeyCode::Esc` with preserved modifiers. This means pressing CapsLock registers as an Esc press with those modifiers. `try_translate_event` correctly returns `None` for these. The infallible path is used by `unified_event_source.rs` and `console_event_source.rs`; `term-session-client` uses the fallible path. **Potential issue but low impact — escape handling in the WM is well-defined.**

3. **🟡 `KeyEventState` dropped** — Crossterm 0.29 `KeyEvent` has a `state: KeyEventState` field (CapsLock/NumLock flags). The core `KeyEvent` has no equivalent. **Design limitation, not a bug in the adapter.**

4. **🔴 `KeyboardNormalizer` drops `KeyKind::Repeat` on Windows** — A localized workaround for Windows ConPTY Esc bouncing was improperly generalized during a refactor. The old `esc_down` state tracked Esc repeats specifically; when that was removed (Esc dedup moved to WM's `super_passthrough_window`), the blanket `KeyKind::Repeat => return None` was left in place. This drops **all** repeat keys on Windows, breaking scrolling, cursor movement, and continuous typing within child PTYs. **Fix: remove the Windows-specific Repeat suppression and consolidate OS branches.**

5. **🟢 `term-session-client` filters to only Press/Repeat key events** — Downstream consumer filtering, intentional (only sends to PTY).

## Proposed Changes

### 1. Fix Windows Repeat suppression (`crates/term-wm-core/src/utils/keyboard_normalizer.rs`)

Remove the `KeyKind::Repeat => return None` branch and consolidate the `cfg!(windows)` conditional into a single check that only drops `KeyKind::Release` on all platforms:

```rust
pub fn normalize(&mut self, evt: Event) -> Option<Event> {
    match evt {
        Event::Key(key) => {
            // Shift+Tab passes through — FocusPrev keybinding matches Tab+Shift.
            // BackTab normalization is handled in term-wm-crossterm-adapter.
            if key.kind == KeyKind::Release {
                return None;
            }
            Some(Event::Key(key))
        }
        other => Some(other),
    }
}
```

### 2. Update tests (`crates/term-wm-core/src/utils/keyboard_normalizer.rs`)

Rename `repeat_key_passes_through_on_unix` to `repeat_key_passes_through`. Remove the `#[cfg(target_os = "windows")]` conditional assertions — enforce that `KeyKind::Repeat` passes through unconditionally on all platforms.

### 3. Add media key support to `translate_key_code` (`crates/term-wm-crossterm-adapter/src/lib.rs`)

Add explicit match arms for crossterm Media keys before the wildcard, using the correct type `MediaKeyCode` and consolidating discrete `Play`/`Pause` into the core `MediaPlayPause` variant:

```rust
crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::PlayPause | crossterm::event::MediaKeyCode::Play | crossterm::event::MediaKeyCode::Pause) => Some(KeyCode::MediaPlayPause),
crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::Stop) => Some(KeyCode::MediaStop),
crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::TrackNext) => Some(KeyCode::MediaTrackNext),
crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::TrackPrevious) => Some(KeyCode::MediaTrackPrevious),
```

### 4. Add tests for media key translation

Add tests covering:
- Each media key variant maps correctly
- `try_translate_key_event` returns `Some` for media keys
- `try_translate_event` with media key events returns the correct `Event::Key`

### 5. Run verification

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test
```

## Files to Modify
- `crates/term-wm-core/src/utils/keyboard_normalizer.rs`
- `crates/term-wm-crossterm-adapter/src/lib.rs`

## Verification
- `cargo test` — full suite to confirm no regressions
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
