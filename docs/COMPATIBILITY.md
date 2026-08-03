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

`term-session` auto-daemonizes on Windows like it does on macOS and Linux. The gateway is spawned with `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS`, which gives the child no console — so the parent console's `CTRL_CLOSE_EVENT` never reaches it — and standard handles are disinherited (`Stdio::null()` + `inherit_handles(false)`), so wrappers, CI, and SSH runners never wait on inherited pipes. PTY children are contained in a Win32 Job Object (spawned `CREATE_SUSPENDED`, assigned to the job, then resumed) so killing a session terminates the whole process tree rather than just the shell.
