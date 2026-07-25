# Plan: Crossterm Adapter Event Fidelity & Interface Hardening

## Goals
Guarantee that `crates/term-wm-crossterm-adapter/` passes all keyboard and mouse events through without silent drops or adverse modification, using **compile-time enforcement** rather than ad-hoc tests.

## Corrective Actions Completed

| Change | Commit | Status |
|---|---|---|
| Media key mapping (PlayPause/Play/Pause → `MediaPlayPause`, Stop, TrackNext, TrackPrevious) | `0c256a7` | ✅ |
| Remove Windows-specific `KeyKind::Repeat` suppression in `KeyboardNormalizer` | `72e1bba` | ✅ |
| Rename tests, consolidate OS branches in normalizer | `72e1bba`, `4ddce3c` | ✅ |

## Remaining Work — Architectural Hardening

### 1. 🔴 Eliminate Wildcard in `translate_key_code` (`crates/term-wm-crossterm-adapter/src/lib.rs`)

Replace `_ => None` with an **exhaustive list** of explicitly dropped crossterm variants. This turns silent runtime drops into **compile-time errors** when crossterm adds new `KeyCode` variants.

**Before:**
```rust
_ => None,
```

**After:**
```rust
crossterm::event::KeyCode::ScrollLock
| crossterm::event::KeyCode::NumLock
| crossterm::event::KeyCode::PrintScreen
| crossterm::event::KeyCode::Pause
| crossterm::event::KeyCode::Menu
| crossterm::event::KeyCode::KeypadBegin
| crossterm::event::KeyCode::CapsLock
| crossterm::event::KeyCode::Null
| crossterm::event::KeyCode::Modifier(_) => None,
```

**Effect:** A crossterm bump adding `KeyCode::NewFeature` → compiler error at the match site → engineer must explicitly decide: map it or list it as intentionally dropped.

### 2. 🔴 Domain Reachability (Surjectivity) Test — Compile-Time Enforced (`crates/term-wm-crossterm-adapter/src/lib.rs`)

Do NOT inject `strum` or any foreign dep into the core domain. Instead, use a declarative macro within the adapter's test module that exploits Rust's native `match` exhaustiveness to mathematically bind the target array to the enum's variant list.

The macro generates an array of `KeyCode` instances while simultaneously emitting a dummy `match` statement. If a new variant is added to `KeyCode`, the `match` triggers `E0004` (pattern not covered), making it impossible to compile the test suite without updating the array.

```rust
/// Macro: compile-time exhaustive KeyCode array.
/// Adding a variant to `KeyCode` without updating this macro → compile error.
macro_rules! exhaustive_core_keys {
    ($($variant:pat => $instance:expr),+ $(,)?) => {{
        #[allow(dead_code)]
        fn enforce_exhaustiveness(k: KeyCode) {
            match k {
                $($variant => {}),+
            }
        }
        [$($instance),+]
    }};
}

#[test]
fn test_all_core_keycodes_reachable() {
    let core_targets = exhaustive_core_keys! {
        KeyCode::Char(_) => KeyCode::Char('a'),
        KeyCode::Enter => KeyCode::Enter,
        KeyCode::Tab => KeyCode::Tab,
        KeyCode::Backspace => KeyCode::Backspace,
        KeyCode::Esc => KeyCode::Esc,
        KeyCode::Left => KeyCode::Left,
        KeyCode::Right => KeyCode::Right,
        KeyCode::Up => KeyCode::Up,
        KeyCode::Down => KeyCode::Down,
        KeyCode::Home => KeyCode::Home,
        KeyCode::End => KeyCode::End,
        KeyCode::PageUp => KeyCode::PageUp,
        KeyCode::PageDown => KeyCode::PageDown,
        KeyCode::Delete => KeyCode::Delete,
        KeyCode::Insert => KeyCode::Insert,
        KeyCode::F(_) => KeyCode::F(1),
        KeyCode::MediaPlayPause => KeyCode::MediaPlayPause,
        KeyCode::MediaStop => KeyCode::MediaStop,
        KeyCode::MediaTrackNext => KeyCode::MediaTrackNext,
        KeyCode::MediaTrackPrevious => KeyCode::MediaTrackPrevious,
    };

    for core_key in core_targets.iter() {
        let reachable = ALL_CROSSTERM_KEYS.iter().any(|ck| {
            translate_key_code(*ck) == Some(*core_key)
        });
        assert!(reachable,
            "Core KeyCode::{:?} is unreachable from any crossterm input", core_key);
    }
}
```

**Why not strum:** `KeyCode::Char(char)` is a data-carrying variant — `EnumIter`/`strum` can't enumerate infinite values. The macro approach sidesteps this entirely: `Char(_)` in a pattern covers all values, while the instance `Char('a')` provides a concrete test input.

**Effect:** Adding `KeyCode::MediaRecord` to the domain → the `match` inside `exhaustive_core_keys!` fails to compile → developer must add it to the macro ➡ the array → reachability test runner then validates a crossterm input maps to it.

### 3. 🟡 Snapshot Testing for Behavioral Lockdown

Use `insta` to freeze the entire translation boundary, catching regressions in modifier propagation, event dropping, and coordinate translation.

**Steps:**
- Add `insta` to dev-dependencies of `crates/term-wm-crossterm-adapter/Cargo.toml`
- Build a static input matrix covering:
  - Each standard key code (Enter, Tab, Esc, F1..F12, arrows, etc.)
  - BackTab (verifies modifier injection)
  - Each media key variant
  - Each mouse button × event kind combination
  - Edge cases: null modifiers, combined modifiers (Ctrl+Alt+Shift), scroll events
- Assert `try_translate_event` output matches stored snapshot:
```rust
#[test]
fn snapshot_translation_boundary() {
    for (i, (label, crossterm_event)) in EVENT_MATRIX.iter().enumerate() {
        let result = try_translate_event(*crossterm_event);
        insta::assert_debug_snapshot!(format!("event_{:03}_{}", i, label), &result);
    }
}
```

## Files to Modify
- `crates/term-wm-crossterm-adapter/src/lib.rs`
- `crates/term-wm-crossterm-adapter/Cargo.toml`

## Verification
```bash
cargo test  # 79+ tests including new surjectivity + snapshot tests
cargo clippy --workspace --all-targets --all-features -- -D warnings
# Snapshot review: `cargo insta review` to accept new snapshots
```
