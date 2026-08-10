# term-wm-pty-engine

Internal PTY (pseudo-terminal) engine for [term-wm](https://crates.io/crates/term-wm). Not intended for direct use.

See the main [term-wm](https://crates.io/crates/term-wm) crate for documentation.

## Debugging: Tracing PTY output

Set `TERM_WM_TRACE_ESC=<path>` to dump every chunk of raw bytes the PTY reader feeds into the
terminal emulator, as lowercase hex, one line per read chunk (up to 64 KB each):

```sh
TERM_WM_TRACE_ESC=/tmp/pico.trace cargo run -p term-wm -- --run pico
```

If the value is unset the tracer defaults to `term_wm_esc_trace.log`. When the variable is not
set at all the tracer is off (checked once per process via `OnceLock`), so there is no per-chunk
cost in normal operation.

This is primarily a debugging aid for seeing exactly what a child application writes to the
terminal (e.g. which escape sequences an editor like `pico` emits). Each line is one `read()`
chunk; concatenate and hex-decode the lines to recover the full byte stream. See
`esc_trace_chunk` in `src/pty.rs`.
