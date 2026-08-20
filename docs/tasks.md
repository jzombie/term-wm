# Project Tasks — Tasks File Specification

Scope: the `.term-wm/tasks.json` file format, discovery, environment gating, and
runtime behavior. Code conventions live in [AGENTS.md](../AGENTS.md); UI string
style lives in [UI-STYLE.md](./UI-STYLE.md).

## File Location & Discovery

- **Canonical path:** `.term-wm/tasks.json`
- **Discovery:** walk from the WM launch directory (`std::env::current_dir()`
  captured at app init) upward through ancestors. The first directory containing
  `.term-wm/tasks.json` wins.
- **Malformed file:** if the file exists but fails to parse, log a warning and
  return `None` (no tasks). Do NOT keep walking upward.
- **No file anywhere:** silent `None` (no tasks group in the Command Palette).

## Schema

Top-level JSON **array** — no envelope, no version field. Each element is a task
object:

| Field          | Type       | Default | Notes |
|----------------|------------|---------|-------|
| `label`        | `string`   | —       | **Required.** Shown in the Command Palette under "Project Tasks". |
| `command`      | `string`   | `null`  | Shell-words tokenized into argv. Whitespace-only counts as absent. |
| `args`         | `string[]` | `null`  | Appended after `command` tokens. If `command` is absent, used wholesale as argv. |
| `cwd`          | `string`   | `null`  | Absolute, or relative to the task root (project root or launch cwd). |
| `env`          | `object`   | `{}`    | Extra environment variables (`{ "KEY": "VALUE" }`) passed to the child. |
| `environments` | `string[]` | `[]`    | Gating: `"dev"` / `"prod"` / `"test"` (see below). Empty = visible everywhere. |

### argv Rules

1. If `command` is present and non-empty: tokenize via `shell_words::split` (no
   subcommand truncation). On split error or empty token list → task is invalid
   (not shown in palette, not runnable).
2. Append `args` entries after the command tokens.
3. If `command` is omitted or whitespace-only and `args` is present: argv = `args`
   directly.
4. If argv is empty → task is invalid.

Callers must always guard with `let Some(argv) = task.argv() else { ... }`. Never
index `argv[0]` without the non-empty check.

## Environment Gating

A task may declare an `environments` list to restrict which runtime environments
display it. The list uses the same environment identity the IPC/gateway channel
naming uses (`term_wm_config::env`):

| Value   | Meaning |
|---------|---------|
| `"dev"` | Cargo-hosted / debug builds (`CARGO_MANIFEST_DIR` present, or `cfg!(debug_assertions)`) |
| `"prod"` | Installed / release builds |
| `"test"` | Test harness |

The active environment is resolved once at startup via `active_environment()`:
`TERM_WM_ENV` override when set and valid, else the compile-time default (`dev`
when `CARGO_MANIFEST_DIR` is present or `cfg!(debug_assertions)`, `prod`
otherwise).

**Filtering rules:**
- Empty list → visible in all environments (default).
- Contains an environment string → visible only when `active_environment()`
  matches. Case-insensitive and whitespace-trimmed; unknown strings never match.
- Filtering happens at load time (`load_tasks_for_cwd`); the cached task list
  in `WindowManager` is already filtered.

## Run Semantics

- Task argv is the PTY's direct child — no shell paste, no shell wrapping. Commands
  needing shell operators (`&&`, pipes, redirects) should set `"command": "sh",
  "args": ["-c", "..."]` on Unix.
- The task runs in a **new window** titled with the task label.
- On exit: the window stays open; a toast fires: `Task '<label>' finished` or
  `Task '<label>' finished (exit N)` on non-zero exit.

## Canonical Example

```json
[
    { "label": "dev: Run", "command": "cargo", "args": ["run"], "environments": ["dev"] },
    { "label": "dev: Help", "command": "cargo", "args": ["run", "--", "--help"], "environments": ["dev"] },
    { "label": "ci: Lint", "command": "cargo", "args": ["clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"] },
    { "label": "dev: udeps", "command": "cargo", "args": ["udeps", "--workspace", "--all-targets", "--all-features"], "environments": ["dev"] }
]
```

## Out of Scope

- Zed `tasks.json` compatibility (v1 flat array, v2 `{ "tasks": [...] }` object,
  `.zed/tasks.json` discovery). We own the schema independently.
- `$ZED_*` / `${VAR:default}` variable interpolation.
- Per-window cwd tracking or OSC 7.
- `spawn_mode`, `env_required`, `env_gate` fields.
