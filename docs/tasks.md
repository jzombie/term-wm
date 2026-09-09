# Project Tasks: Tasks File Specification

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

## JSONC Comments

The file is parsed as **JSONC**: standard JSON plus `//` line comments and
`/* … */` block comments (stripped before parsing, respecting string
literals). Use comments to document tasks in-file: for example a header
explaining the `command` shell-words form at the top of `tasks.json`.
Comment-like sequences inside `"strings"` (e.g. `"https://example.com"` or
`"echo // not a comment"`) are preserved verbatim.

## Schema

Top-level JSON **array** (JSONC): no envelope, no version field. Each element
is a task object:

| Field          | Type       | Default | Notes |
|----------------|------------|---------|-------|
| `label`        | `string`   | n/a     | **Required.** Shown in the Command Palette under "Project Tasks". |
| `command`      | `string`   | `null`  | Shell-words tokenized into argv (after substitution). Whitespace-only counts as absent. |
| `args`         | `string[]` | `null`  | Appended after `command` tokens; each element is substitution-aware too. If `command` is absent, used wholesale as argv. |
| `cwd`          | `string`   | `null`  | Absolute, or relative to the task root (project root or launch cwd). |
| `env`          | `object`   | `{}`    | Extra environment variables (`{ "KEY": "VALUE" }`) passed to the child. |
| `environments` | `string[]` | `[]`    | Gating: `"dev"` / `"prod"` / `"test"` (see below). Empty = visible everywhere. |
| `platforms`    | `string[]` | none    | Gating: OS names (see Platform Gating below). Absent or empty = visible on every platform. |

### Variable Substitution

Placeholders in task strings are replaced before execution. Registered
placeholders:

| Placeholder  | Resolves to |
|--------------|-------------|
| `{wm.pid}`   | The OS PID of the term-wm process that spawns the task: the window-manager process for palette-launched tasks, the CLI process for `--task` runs. |
| `{wm.exe}`   | The full path of that same process's executable (`std::env::current_exe()`), so tasks can invoke the term-wm binary itself (e.g. piping into `{wm.exe} --util copy`). |

Rules:

1. Substitution applies to `command`, every `args` element, `cwd`, and each
   `env` **value**.
2. It runs **before** shell-words tokenization of `command`, so a substituted
   value can never be split into stray tokens, even inside quoted segments.
3. Unknown `{...}` sequences are preserved verbatim (forward-compatible;
   literal braces survive).
4. This is term-wm's own placeholder syntax, not Zed-style `${VAR}` /
   `${VAR:default}` interpolation (see Out of Scope).
5. Quoting guidance for `{wm.exe}`: inside shell scripts, prefer passing it
   through an `env` entry and referencing `$VAR` (`$env:VAR` in PowerShell).
   Inline use inside quoted shell text breaks when the resolved path contains
   quote characters; env-var expansion is immune to spaces and quotes alike.

Example (attach a profiler to the window manager itself):

```jsonc
[
    { "label": "profile: Time Profiler",
      "command": "xcrun xctrace record --template 'Time Profiler' --time-limit 10s --output /tmp/term-wm.trace --attach {wm.pid}",
      "platforms": ["macos"] }
]
```

Example (pipe `git diff` into the built-in copy utility via `{wm.exe}`,
platform-gated because tasks run as direct argv children with no shell):

```jsonc
[
    { "label": "dev: Copy Git Diff",
      "command": "sh",
      "args": ["-c", "git diff | \"$TERM_WM_EXE\" --util copy"],
      "env": { "TERM_WM_EXE": "{wm.exe}" },
      "platforms": ["linux", "macos"] },
    { "label": "dev: Copy Git Diff",
      "command": "powershell",
      "args": ["-NoProfile", "-Command", "git diff | & $env:TERM_WM_EXE --util copy"],
      "env": { "TERM_WM_EXE": "{wm.exe}" },
      "platforms": ["windows"] }
]
```

### argv Rules

1. If `command` is present and non-empty: tokenize the **entire** string via
   `shell_words::split` (POSIX shell word splitting: quotes and escapes
   respected, no subcommand truncation). On split error or empty token list →
   task is invalid (not shown in palette, not runnable).
2. Append `args` entries after the command tokens.
3. If `command` is omitted or whitespace-only and `args` is present: argv = `args`
   directly.
4. If argv is empty → task is invalid.

`command` alone can carry the full invocation: `"command": "cargo run -- --help"`
is equivalent to `"command": "cargo", "args": ["run", "--", "--help"]` and is
the preferred short form. Keep `args` for programmatic composition or when you
prefer split arrays.

Callers must always guard with `let Some(argv) = task.argv() else { ... }`. Never
index `argv[0]` without the non-empty check.

## Environment Gating

A task may declare an `environments` list to restrict which runtime environments
display it.

Environment identity is resolved strictly via `term_wm_config::env::active_environment()`
the **single source of truth** shared with IPC gateway channel scoping. Task visibility and
gateway channel names can never disagree because both resolve through the exact same function.

| Value   | Meaning |
|---------|---------|
| `"dev"` | Cargo-hosted execution (`CARGO_MANIFEST_DIR` set, including `cargo run` and `cargo run --release`) or debug builds (`cfg!(debug_assertions)`) |
| `"prod"` | Installed / standalone release binaries running outside Cargo |
| `"test"` | Test harness execution |

### Resolution & Precedence Chain

When resolving environment identity:

1. **`TERM_WM_ENV` override:** if set to `dev`, `prod`, or `test` (case-insensitive,
   trimmed), overrides the default environment used for **project-task gating**.
2. **`default_environment()` fallback:**
   - Resolves to `"dev"` if `CARGO_MANIFEST_DIR` is set (any `cargo run` execution,
     including `cargo run --release`) or `cfg!(debug_assertions)` is true.
   - Resolves to `"prod"` for standalone installed release binaries without
     `CARGO_MANIFEST_DIR`.

> **Note on `cargo run --release`:** Cargo sets `CARGO_MANIFEST_DIR` when driving
> a binary. Consequently, `cargo run --release` resolves to `"dev"` by default. If
> you expect `"prod"` task behavior while launching through Cargo, set
> `TERM_WM_ENV=prod`.

The environment scopes project tasks ONLY. Gateway endpoints never consult it.

### Gateway / Environment Decoupling

The persistence gateway endpoint is `{namespace}/<user>/gateway` and is
independent of the runtime environment. Resolution order:

1. **`--gateway <NAME>`:** wholesale override of the full endpoint path
   (multi-segment values round-trip byte-exact; the caller owns the whole
   path).
2. **`TERM_WM_NAMESPACE`:** namespace-root override preserving the OS-level
   `<user>` segment. Values are validated against the strict segment charset;
   invalid or empty values fall back to the default.
3. **Static default:** namespace `term-wm`.

**Local development isolation** is enforced at the toolchain boundary: the
committed `.cargo/config.toml` injects `TERM_WM_NAMESPACE=term-wm-dev`, so
every cargo-driven execution (`cargo run`, `cargo test`) uses
`term-wm-dev/<user>/gateway` while binaries executed directly bind the shared
`term-wm/<user>/gateway`. The `<user>` segment is always resolved at runtime,
so two developers on a shared machine can never collide on a dev socket.
Auto-spawned daemons are pinned to the launcher's resolved endpoint via a
hidden `--gateway <name>` argument, so a freshly spawned daemon always binds
exactly the socket its launcher probed.

### Forcing Environments

- Force production task gating under Cargo: `TERM_WM_ENV=prod cargo run --release`
- Force development task gating in an installed binary: `TERM_WM_ENV=dev term-wm`

**Filtering rules:**
- Empty list → visible in all environments (default).
- Contains an environment string → visible only when `active_environment()` matches.
  Case-insensitive and whitespace-trimmed; unknown strings never match.
- Filtering happens at load time (`load_tasks_for_cwd`); the cached task list
  in `WindowManager` is already filtered.
- If you observe gating that disagrees with your expectations (e.g. dev tasks
  visible under `cargo run --release`), check `active_environment()`: both IPC
  and tasks resolve through it, making the two views always consistent by
  construction.

**Filtering rules:**
- Empty list → visible in all environments (default).
- Contains an environment string → visible only when `active_environment()` matches.
  Case-insensitive and whitespace-trimmed; unknown strings never match.
- Filtering happens at load time (`load_tasks_for_cwd`); the cached task list
  in `WindowManager` is already filtered.
- If you observe gating that disagrees with your expectations (e.g. dev tasks
  visible under `cargo run --release`), check `active_environment()`: both IPC
  and tasks resolve through it, making the two views always consistent by
  construction.

## Platform Gating

A task may declare a `platforms` list to restrict which operating systems
display it. Entries are matched case-insensitively against the current OS
(`std::env::consts::OS`: `macos`, `linux`, or `windows`), with `darwin`
accepted as an alias for `macos`.

**Filtering rules:**
- Absent list (field omitted) or empty list → visible on every platform (default).
- Contains an OS name (or alias) → visible only when it matches the current OS.
- Unknown strings simply never match.
- Like environment gating, filtering happens at load time in
  `load_tasks_for_cwd`.

```jsonc
[
    { "label": "macos: Profile WM",  "command": "xcrun xctrace record --template 'Time Profiler' --attach {wm.pid}", "platforms": ["macos"] },
    { "label": "linux: perf stat",   "command": "perf stat -d",   "platforms": ["linux"] },
    { "label": "windows: List",      "command": "cmd", "args": ["/c", "dir"], "platforms": ["windows"] }
]
```

## Run Semantics

- Task argv is the PTY's direct child: no shell paste, no shell wrapping. Commands
  needing shell operators (`&&`, pipes, redirects) should set `"command": "sh",
  "args": ["-c", "..."]` on Unix.
- The task runs in a **new window** titled with the task label.
- On exit: the window stays open; a toast fires: `Task '<label>' finished` or
  `Task '<label>' finished (exit N)` on non-zero exit.

## Canonical Example

Preferred short form: full invocation in `command` (shell-words tokenized, no
separate `args` needed):

```jsonc
// `command` is shell-words tokenized: args can live inline:
[
    { "label": "dev: Run", "command": "cargo run", "environments": ["dev"] },
    { "label": "dev: Help", "command": "cargo run -- --help", "environments": ["dev"] },
    { "label": "ci: Lint", "command": "cargo clippy --workspace --all-targets --all-features -- -D warnings" },
    { "label": "dev: udeps", "command": "cargo udeps --workspace --all-targets --all-features", "environments": ["dev"] },
    { "label": "macos: Profile WM", "command": "xcrun xctrace record --template 'Time Profiler' --attach {wm.pid}", "platforms": ["macos"] }
]
```

Split form is also supported (`args` appended after `command`):

```json
[
    { "label": "dev: Run", "command": "cargo", "args": ["run"], "environments": ["dev"] },
    { "label": "dev: Help", "command": "cargo", "args": ["run", "--", "--help"], "environments": ["dev"] }
]
```

## Out of Scope

- Zed `tasks.json` compatibility (v1 flat array, v2 `{ "tasks": [...] }` object,
  `.zed/tasks.json` discovery). We own the schema independently.
- `$ZED_*` / `${VAR:default}` variable interpolation.
- Per-window cwd tracking or OSC 7.
- `spawn_mode`, `env_required`, `env_gate` fields.
