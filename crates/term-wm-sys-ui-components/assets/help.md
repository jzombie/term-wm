# %PACKAGE% -- Quick Help

**Package:** `%PACKAGE%` ([https://crates.io/crates/%PACKAGE%](https://crates.io/crates/%PACKAGE%))  
**Version:** `%VERSION%` (%PLATFORM%)

Submit bug reports to %REPOSITORY%/issues/new

Welcome to `%PACKAGE%`! This page is a quick reference for navigating the UI.

# Keybindings

_See also: [No-Keybinding-Conflict Philosophy](#no-keybinding-conflict-philosophy)_

The following keybindings are active in the WM layer. The Command Palette binding works in any mode, including Direct Mode:

* **%SUPER%**: Open (or close) the Command Palette.

While the Command Palette is open:

* **%FOCUS_NEXT% / %FOCUS_PREV%**: Cycle focus between windows.
* **%MENU_NAV%**: Move up/down in lists and menus.
* **%MENU_SELECT%**: Activate the selected menu item.
* **%HELP_MENU%**: Open this help overlay. (Alternatively: press **%SUPER%** to open the Command Palette and search for 'Help').

## No-Keybinding-Conflict Philosophy

A core goal of `%PACKAGE%` is **minimally invasive** keybindings so you can run terminal apps (e.g., `screen`, `tmux`, editors, etc.) without the window manager (WM) stealing their keys.

By default, the WM's keybindings are **minimally invasive**: it primarily listens for **%SUPER%**, plus a small set of navigation keys — **PageUp / PageDown / Home / End** — which its built-in scrollback consumes when a window is focused and not in Direct Mode. Everything else (including arrow keys) falls through to the running application. Press **%SUPER%** to open the Command Palette and use WM commands (like **%FOCUS_NEXT%** / **%FOCUS_PREV%**).

To send **%SUPER%** to the currently focused application, press **%SUPER%** while the Command Palette is open (the key is forwarded to the active window).

Direct Mode is fully automatic — see [Automatic Direct Mode](#automatic-direct-mode) below.

## Automatic Direct Mode

When a terminal application (e.g., `vim`, `tmux`, or `less`) requests the alternate screen buffer, mouse tracking, or custom scroll margins, `%PACKAGE%` automatically enters **Direct Mode**.

* **State Awareness:** A brief notification toast appears on screen to indicate when Direct Mode has been enabled or disabled.
* **Keyboard Routing:** All keystrokes pass directly to the running application unfiltered, ensuring native shortcuts work without WM interference. **%SUPER%** remains active to open the Command Palette.
* **Mouse & Selection:** WM-level text selection and right-click pasting are suspended — all mouse events over the terminal pass through to the running application. Window chrome (dragging and resizing via the header and borders) continues to work.

## Mouse Capture

Mouse capture is enabled by default when supported. To disable it, open the Command Palette and toggle `Mouse Capture`.

Mouse capture lets `%PACKAGE%` receive mouse input for WM actions like:

* selecting/focusing windows
* moving windows
* resizing windows
* interacting with the panel UI

Most of these actions are also purely keyboard driven, by initially pressing the **%SUPER%** key.

Notes:

* Mouse interactions work only while `Mouse Capture` is enabled.
* Open the Command Palette (%SUPER%) for common WM actions.

## Window Snapping & Drag Preview

While dragging a floating window by its title bar, hovering over a snap target shows a live **ghost preview** — a dashed outline with a shaded fill and a label describing the pending action.

* **Snap targets:** screen edges (`snap to edge`), screen corners (`snap to corner`), and the top edge (`maximize`).
* **Auto-snap countdown:** if the pointer leaves the screen area while a snap target is active, the window snaps automatically after a short countdown (default 2 seconds). Releasing the button over the target also snaps.
* **Micro-positioning:** to place a window at a precise position, float it first, move it where you want, then tile it.

## Selection & Clipboard

`%PACKAGE%` provides text selection and clipboard integration for terminal windows.

**Selecting text:** Left-click drag within a terminal window to select text.
On release, the selection is automatically copied to the system clipboard
and a brief notification is shown.

**Pasting:** Right-click to paste the system clipboard contents at the cursor position.
Paste also works via your terminal emulator's standard shortcut (e.g., Ctrl+Shift+V),
which sends the text as a bracketed-paste event.

**OSC 52 copy:** Terminal applications (e.g., `vim`, `tmux`) can write to the system
clipboard using the OSC 52 escape sequence. `%PACKAGE%` intercepts these sequences
automatically.

Notes:

* Selection and right-click paste are available only while **not** in Direct Mode.
  In Direct Mode, all mouse events pass through to the running application unfiltered.
* To disable clipboard integration, open the Command Palette (`%SUPER%`) and toggle
  "Clipboard Mode".
* To enable or disable window text selection via the Command Palette, toggle
  "Window Selection: On/Off".

## Environment & Compatibility

`%PACKAGE%` runs natively on Linux, macOS, and Windows. 

* **Colors & Unicode:** Truecolor (24-bit) and a UTF-8 environment with box-drawing support are highly recommended for the intended visual experience.
* **Linux VT:** The window manager is fully usable in raw Linux Virtual Terminals (e.g., `Ctrl+Alt+F1`). Due to TTY framebuffer limits, expect colors and borders to degrade, but core mechanics will continue to function properly.

See the [compatibility notes](%REPOSITORY%/blob/main/docs/COMPATIBILITY.md) for full details.

----

_For additional information, see the [README](%REPOSITORY%) on the project's repo: %REPOSITORY%._
