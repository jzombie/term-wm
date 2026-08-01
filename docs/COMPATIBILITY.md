# Compatibility

`term-wm` runs natively on Linux, macOS, and Windows. This document details the minimum requirements and the behavior of the application in degraded terminal environments.

## Color Support

`term-wm` is designed and themed against **truecolor (24-bit)** output.

- When the `COLORTERM` environment variable contains `truecolor` or `24bit`, the UI renders full RGB colors (`Color::Rgb`).
- Otherwise, `term-wm` automatically degrades to the nearest **xterm-256** indexed color (`Color::Indexed`).

UI themes and drop shadows are designed against 24-bit depth, so low-color environments will look noticeably flatter.

### Minimum color support

- **Recommended:** Truecolor (24-bit) terminal.
- **Usable:** 256-color terminals (e.g., `TERM=xterm-256color`).
- **Degraded but functional:** 16/8-color terminals (e.g., raw Linux VTs), where the kernel console translates or ignores high-color escape sequences.

## Unicode & Fonts

- A **UTF-8** locale is required (`LANG` set to a UTF-8 locale, e.g. `en_US.UTF-8`).
- The terminal font must render standard **Unicode box-drawing characters** (`─│┌┐└┘├┤┬┴┼` and friends), which construct window borders, layout splits, and system chrome.

## Linux Virtual Terminals (TTY)

`term-wm` is fully usable in raw Linux VTs (e.g., accessed via `Ctrl+Alt+F1`).

- All window-management and multiplexing logic remains fully functional.
- Visual presentation differs significantly: the kernel framebuffer console has strict font and color limits, so borders, themes, and shadows render with reduced fidelity. The layout remains usable.

## Non-Standard OS Installs

Minimal or headless installations must meet the following for correct operation:

- A valid **`terminfo`** database must be present and include an entry for the terminal in use (`TERM` must resolve to an installed terminfo record).
- **`LANG`** (and `LC_ALL`) must be set to a UTF-8 locale to prevent layout corruption from non-UTF-8 output.
- When connecting remotely, `TERM` should match the client's terminal type so size and color reporting are accurate.

## Windows Session Hosting

`term-session` runs on Windows, but it does **not** currently automatically daemonize the session server. The auto-spawned server uses `CREATE_NO_WINDOW` rather than a full process-session detachment, so on Windows the server process remains tied to the launching console's lifetime instead of surviving independently as it does on macOS and Linux (`setsid`).
