# Environment Variables

`term-wm` respects the following environment variables. They are listed here for quick reference; see the linked sections for detailed behavior.

| Variable | Purpose | Default |
| :--- | :--- | :--- |
| `TERM_WM_ENV` | Runtime environment (`dev`/`prod`/`test`, case-insensitive); scopes project-task visibility only. Gateway endpoints do not depend on it. | `dev` in debug builds, `prod` in release |
| `TERM_WM_NAMESPACE` | Namespace-root override of the gateway endpoint, preserving the `<user>` segment (`<ns>/<user>/gateway`). Set for cargo-driven executions by the committed `.cargo/config.toml`. | unset (`term-wm`) |
| `TERM_SESSION_CHANNEL` | Session channel override (read by `term-session`). | `default/main` |
| `TERM_WM_NO_SESSION_PERSISTENCE` | Disables session-persistence behavior at runtime (same as `--no-session-persistence`). | unset (persistence enabled) |
| `TERM_WM_TRACE_ESC` | Dumps raw PTY->emulator bytes to a file (debugging aid). | off |
| `TERM_WM_LOG_FILE` | Durable log destination: tracing events append to this file and rotate when exceeding 10 MB, keeping 4 rotated files plus the active file (5 files, 50 MB total, `0o600` files in `0o700` directory on POSIX). In `term-wm`, events mirror the in-app Debug Log stream; in detached daemons this is the only way to keep diagnostics. Filtered by `RUST_LOG` (default `info,muxio=warn`). Read once when the daemon process starts: daemons already running without it are unaffected until restarted. | unset (Debug Log window / stdout) |
| `TERM_WM_TEST_LOG_DIR` | Test-only capture root: harnesses using `term-test-support::apply_test_logging` write spawned daemons'/clients' diagnostics here; CI archives the directory on failure. | unset (`<temp>/term-wm-test-logs/<pid>-<nanos>/`) |
