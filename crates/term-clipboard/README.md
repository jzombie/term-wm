# term-clipboard

Cross-platform terminal clipboard utility: system clipboard (arboard) + OSC 52, for [term-wm](https://crates.io/crates/term-wm).

`Clipboard` composes a pluggable backend registry over a public `ClipboardBackend` trait:

- `ArboardBackend` — the OS/system clipboard.
- `InMemoryBackend` — a process-global shared buffer (internal copy→paste fallback).
- `Osc52Backend` — write-only OSC 52 emission to the host terminal (only when stdout is an active terminal).

`set()` fans out to every backend in order (OSC 52 last, so the host terminal becomes the
final clipboard owner); `get()` reads the system clipboard when available and otherwise falls
back to the internal buffer — a single unified paste path. `set_from_reader` / `set_from_path`
ingest a file or stream with typed errors (`ClipboardError::InvalidUtf8` / `Io`) for
programmatic use (MCP servers, agents, embedded tools).

## CLI: `term-copy`

Copies UTF-8 text to the clipboard — flagless:

```sh
term-copy [FILE]        # or: cat file.txt | term-copy
```

Reads from `FILE` (or stdin when omitted) and writes to every backend, so it works locally,
over SSH, and inside terminals without OSC 52 support. With no file and an interactive stdin
it prints an error and exits instead of hanging.

The main `term-wm` binary exposes the same mechanics as a built-in utility:

```sh
term-wm --util copy [FILE]   # or: git diff | term-wm --util copy
```

Both frontends share one ingestion core (`copy::run_copy_util`), so their FILE/stdin
handling, messages, and exit codes are identical; only the program label in errors differs.
This is what makes `--util copy` usable inside `tasks.json` pipelines (see
`docs/tasks.md`).

See the main [term-wm](https://crates.io/crates/term-wm) crate for documentation.
