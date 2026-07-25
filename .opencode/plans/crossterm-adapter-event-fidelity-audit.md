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

4. **🟢 `KeyboardNormalizer` drops `KeyKind::Release` on all platforms, `KeyKind::Repeat` on Windows** — This happens downstream of the adapter in `unified_event_source.rs`, `console_event_source.rs`. Intentional behavior.

5. **🟢 `term-session-client` filters to only Press/Repeat key events** — Downstream consumer filtering, intentional (only sends to PTY).

## Proposed Changes

### 1. Add media key support to `translate_key_code` (`crates/term-wm-crossterm-adapter/src/lib.rs`)

Add explicit match arms for crossterm Media keys before the wildcard, using the correct type `MediaKeyCode` and consolidating discrete `Play`/`Pause` into the core `MediaPlayPause` variant:

```rust
crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::PlayPause | crossterm::event::MediaKeyCode::Play | crossterm::event::MediaKeyCode::Pause) => Some(KeyCode::MediaPlayPause),
crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::Stop) => Some(KeyCode::MediaStop),
crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::TrackNext) => Some(KeyCode::MediaTrackNext),
crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::TrackPrevious) => Some(KeyCode::MediaTrackPrevious),
```

### 2. Add tests for media key translation

Add tests covering:
- Each media key variant maps correctly
- `try_translate_key_event` returns `Some` for media keys
- `try_translate_event` with media key events returns the correct `Event::Key`

### 3. Run verification

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p term-wm-crossterm-adapter
```

## Files to Modify
- `/Volumes/2TB Storage Vault/term-wm/crates/term-wm-crossterm-adapter/src/lib.rs`

## Verification
- `cargo test -p term-wm-crossterm-adapter` — existing tests must pass
- `cargo test` — full suite to confirm no regressions
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
