//! Workspace-wide operational log defaults.
//!
//! These are application policy constants shared by `term-session` (daemon) and
//! the `term-wm` GUI binary, as well as test harnesses. They describe *what* to
//! log and *how much* to retain, not *how* files are secured on disk.
//!
//! OS-level permission invariants (`0o600` file mode, `0o700` directory mode,
//! Windows `FILE_SHARE_*` flags) remain private to `term-session::logging`
//! where the actual `OpenOptions` are constructed, preventing low-level
//! platform details from leaking into the config crate.

/// Default `RUST_LOG` filter when the environment variable is unset.
/// `info` is the global default; `muxio` is forced to `warn` to silence
/// high-frequency transport traces (≈95% volume reduction).
pub const DEFAULT_DAEMON_LOG_FILTER: &str = "info,muxio=warn";

/// Maximum bytes per log file before rotation (10 MiB).
pub const DEFAULT_LOG_MAX_BYTES: usize = 10 * 1024 * 1024;

/// Maximum number of rotated files retained.
/// `file-rotate` with `AppendCount(N)` keeps N rotated files plus the active
/// file → N+1 total. For a 10 MiB per-file cap and a 50 MiB total budget,
/// use N=4 → active 10 MiB + 4×10 MiB = 50 MiB max.
pub const DEFAULT_LOG_MAX_ROTATED_FILES: usize = 4;
