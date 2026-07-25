# Plan: Update help.md with keybinding mode info and copy/paste docs

## Summary
Update `crates/term-wm-sys-ui-components/assets/help.md` to:
1. Clarify which keybindings work in normal mode vs direct mode
2. Document copy/paste/selection/clipboard functionality

## Changes to `crates/term-wm-sys-ui-components/assets/help.md`

### 1. Keybinding section — clarify mode scoping (lines 10-21)
Current text lists all bindings without distinguishing modes. Change to:
- Global (always works): `%SUPER%` — opens Command Palette
- Palette-only (while palette is open): `%FOCUS_NEXT%` / `%FOCUS_PREV%`, `%MENU_NAV%`, `%MENU_SELECT%`, `%HELP_MENU%`
- **Direct mode:** `%SUPER%` is the only WM keybinding that works — all others pass through to the running application

Fix the `%HELP_MENU%` description: currently says "Press %SUPER% and search for 'Help'". The `%HELP_MENU%` token resolves to a key combo but may be unbound. Change to:
```
* **%HELP_MENU%**: Open this help overlay. (Alternatively: press **%SUPER%** to open the Command Palette and search for 'Help').
```

This correctly separates the feature description ("Help menu") from the keybinding token (`%HELP_MENUM` which resolves to e.g. `<F1>`).

### 2. Add "Selection & Clipboard" section (~12 lines)
Insert after the "Mouse Capture" section (before the horizontal rule):

```
## Selection & Clipboard

`%PACKAGE%` provides text selection and clipboard integration for terminal windows.

**Selecting text:** Left-click drag within a terminal window to select text.  
On release, the selection is automatically copied to the system clipboard and a brief notification is shown.

**Pasting:** Right-click to paste the system clipboard contents at the cursor position.  
Paste also works via your terminal emulator's standard shortcut (e.g., Ctrl+Shift+V), which
sends the text as a bracketed-paste event.

**OSC 52 copy:** Terminal applications (e.g., `vim`, `tmux`) can write to the system clipboard
using the OSC 52 escape sequence. `%PACKAGE%` intercepts these sequences automatically.

Notes:

* Selection and right-click paste are available only while **not** in direct mode.
  In direct mode, all mouse events pass through to the running application unfiltered.
* To disable clipboard integration, open the Command Palette (`%SUPER%`) and toggle
  "Clipboard Mode".
* To enable/disable window text selection via the Command Palette, toggle
  "Window Selection: On/Off".
```

### 3. Direct mode mention — no change needed
Line 30 already correctly states: "disables all WM key interception, except for **%SUPER%**".

## Files to modify
- `crates/term-wm-sys-ui-components/assets/help.md` — only file

## Test audit (`wm_help_overlay.rs:249-394`)
Audited all 5 tests:
- `help_constructs` — no content assertions
- `placeholders_are_replaced_in_markdown` — only checks package name/version appear **somewhere** in rendered output; not sensitive to document length
- `show_and_close_toggle_visibility` — checks visibility flag only
- `handle_help_event_closes_on_close_key` — checks Esc closes overlay
- `clicking_outside_auto_closes_when_enabled` — checks outside-click close

None use snapshots, line counts, or scroll boundary assertions. No test changes needed.

The `markdown_viewer.rs` tests use a hardcoded `SAMPLE_HELP_MD` string, not the actual `help.md`. Unaffected.

## Verification
1. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
2. `cargo test`
3. Manual: run app, open help overlay via `Ctrl+G` → search "Help", verify rendering
