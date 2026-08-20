# UI Style Guide — User-Facing Strings

Scope: user-facing text in term-wm — action names, Command Palette labels, menu
labels, notifications, help text, and terminology. Code conventions (naming,
files, layout) live in [AGENTS.md](../AGENTS.md); this file covers the strings
*users see*.

## Title-Casing Rule

All user-facing action and command names use **Chicago title-case**:

- Capitalize the first and last word and every major word (nouns, verbs,
  adjectives, adverbs).
- Keep minor words lowercase unless they are the first or last word:
  `a`, `an`, `and`, `at`, `by`, `for`, `in`, `of`, `on`, `or`, `the`, `to`.

Examples:

- `New Workspace`
- `Switch to Workspace: <name>`
- `Detach Viewer`
- `Scroll to Top`
- `Menu Up`
- `Toggle Mouse Capture`
- `Begin Tap-to-Swap`

## Exceptions

- **Key names and acronyms stay uppercase:** `SUPER` (the Super Key), `UI`,
  `OSC 52`, `SGR`, `SSH`, `IPC`, `FPS`.
- **Established feature names are proper nouns** and keep their canonical
  spelling: `Command Palette`, `Debug Log`, `System Panel`, `Direct Input Mode`,
  `Monocle`, `Terminal`, `Workspace`, `Viewer`, `Gateway`.
- **Notification bodies and description text use sentence case** (only the
  first word capitalized). Title-case applies to *names and labels*, not prose.

## Canonical Action Names

`TermWmAction::Display` (`crates/term-wm-core/src/actions.rs`) is the canonical
machine-rendered form of every action name. It MUST match the Command Palette
labels (`crates/term-wm-core/src/window/window_manager/command_palette.rs`); the
palette is the source of truth for user-visible naming. If a label changes,
update the palette entry and the `Display` string together.

## Stable Terminology

- **workspace** — a named channel namespace; maps to the daemon channel
  `<workspace>/main`, each with its own PTY session and window-manager instance.
- **channel** — a daemon-hosted session endpoint (`<workspace>/main`).
- **gateway / gateway daemon** — the background session daemon; endpoint
  `term-wm/<env>/<user>/gateway`.
- **viewer** — a connected client of a channel (the local TUI or a `term-session`
  client).
- **detach** — disconnect a viewer from a session without terminating its
  processes.

## Environment Variables & Flags

- Environment variables are always uppercase `TERM_WM_*` (`TERM_WM_ENV`,
  `TERM_WM_GATEWAY`, `TERM_SESSION_CHANNEL`, `TERM_WM_NO_SESSION_PERSISTENCE`,
  `TERM_WM_TRACE_ESC`).
  - `TERM_WM_ENV` is the single environment override (`dev`/`prod`/`test`) read by
    `term_wm_config::env::active_environment()` for both IPC gateway scoping and task
    gating.
- CLI flags are lowercase kebab-case (`--no-session-persistence`,
  `--list-channels`, `-w/--workspace`).