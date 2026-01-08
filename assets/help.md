# %PACKAGE% -- Quick Help

**Package:** `%PACKAGE%` ([https://crates.io/crates/%PACKAGE%](https://crates.io/crates/%PACKAGE%))  
**Version:** `%VERSION%` (%PLATFORM%)

Submit bug reports: %REPOSITORY%/issues/new

Welcome to `%PACKAGE%`! This page is a quick reference for navigating the UI.

# Keybindings

_See also: [No-Keybinding-Conflict Philosophy](#no-keybinding-conflict-philosophy)_

* **%SUPER%**: Exit the active window context and open the menu (the "WM layer").

While the menu is open:

* **%FOCUS_NEXT% / %FOCUS_PREV%**: Cycle focus between windows.
* **%NEW_WINDOW%**: Create a new window.
* **%MENU_NAV%** or **%MENU_ALT%**: Move up/down in lists and menus.
* **%MENU_SELECT%**: Activate the selected menu item.
* **%HELP_MENU%**: Open the full help overlay (Panel menu: Top-left -> Help).

## No-Keybinding-Conflict Philosophy

A core goal of `%PACKAGE%` is conflict-free keybindings so you can run terminal
apps (e.g., `screen`, `tmux`, editors, etc.) without the window manager (WM) stealing their keys.

By default, the WM only watches **%SUPER%**. When you press it, you enter the
WM layer and can use WM commands (like **%FOCUS_NEXT%** / **%FOCUS_PREV%**).

To send **%SUPER%** to the currently focused application, press **%SUPER%**
twice quickly. (The second press is forwarded to the active window.)

## Mouse Capture

Mouse capture is enabled by default when supported. To disable it, open the
menu and toggle `Mouse Capture`.

Mouse capture lets `%PACKAGE%` receive mouse input for WM actions like:

* selecting/focusing windows
* moving windows
* resizing windows
* interacting with the panel UI

Most of these actions are also purely keyboard driven, by initially pressing the **%SUPER%** key.

Notes:

* Mouse interactions work only while `Mouse Capture` is enabled.
* Use the panel menu (top-left) for common WM actions.
