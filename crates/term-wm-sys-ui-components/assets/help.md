# %PACKAGE% -- Quick Help

**Package:** `%PACKAGE%` ([https://crates.io/crates/%PACKAGE%](https://crates.io/crates/%PACKAGE%))  
**Version:** `%VERSION%` (%PLATFORM%)

Submit bug reports to %REPOSITORY%/issues/new

Welcome to `%PACKAGE%`! This page is a quick reference for navigating the UI.

# Keybindings

_See also: [No-Keybinding-Conflict Philosophy](#no-keybinding-conflict-philosophy)_

The following keybindings are active in the WM layer. The Command Palette binding works in any mode, including direct mode:

* **%SUPER%**: Open (or close) the `Command Palette`.

While the Command Palette is open:

* **%FOCUS_NEXT% / %FOCUS_PREV%**: Cycle focus between windows.
* **%MENU_NAV%**: Move up/down in lists and menus.
* **%MENU_SELECT%**: Activate the selected menu item.
* **%HELP_MENU%**: Open this help overlay. (Alternatively: press **%SUPER%** to open the Command Palette and search for 'Help').

## No-Keybinding-Conflict Philosophy

A core goal of `%PACKAGE%` is conflict-free keybindings so you can run terminal apps (e.g., `screen`, `tmux`, editors, etc.) without the window manager (WM) stealing their keys.

By default, the WM only watches **%SUPER%**. When you press it, you enter the
WM layer and can use WM commands (like **%FOCUS_NEXT%** / **%FOCUS_PREV%**).

Per-window `direct mode` (toggled via the `[D]` header button) disables all WM key interception, except for **%SUPER%**, so keyboard-driven apps receive every keystroke unfiltered.

## Mouse Capture

Mouse capture is enabled by default when supported. To disable it, open the
Command Palette and toggle `Mouse Capture`.

Mouse capture lets `%PACKAGE%` receive mouse input for WM actions like:

* selecting/focusing windows
* moving windows
* resizing windows
* interacting with the panel UI

Most of these actions are also purely keyboard driven, by initially pressing the **%SUPER%** key.

Notes:

* Mouse interactions work only while `Mouse Capture` is enabled.
* Open the Command Palette (%SUPER%) for common WM actions.

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

* Selection and right-click paste are available only while **not** in direct mode.
  In direct mode, all mouse events pass through to the running application unfiltered.
* To disable clipboard integration, open the Command Palette (`%SUPER%`) and toggle
  "Clipboard Mode".
* To enable or disable window text selection via the Command Palette, toggle
  "Window Selection: On/Off".

----

_For additional information, see the [README](%REPOSITORY%) on the project's repo: %REPOSITORY%._
