use portable_pty::{CommandBuilder, PtySize};
use term_session_muxio_service_definitions::ChannelName;
use term_session_muxio_service_definitions::PathWire;
use term_wm_pty_engine::{Pty, PtyResult, PtyStatus};

pub struct Session {
    pub id: u64,
    pub pty: Pty,
    pub title: Option<String>,
    pub exited: bool,
    pub exit_code: Option<i32>,
    pub cols: u16,
    pub rows: u16,
}

fn default_shell_command() -> CommandBuilder {
    #[cfg(not(windows))]
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string());
    #[cfg(windows)]
    let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
    CommandBuilder::new(shell)
}

/// Resolve the working directory a newly spawned session should start in.
/// Prefers the caller's launch directory (losslessly decoded wire bytes);
/// falls back to this process's cwd (the daemon's) for legacy clients that
/// send `None` or an empty payload.
fn resolve_cwd(cwd: Option<&PathWire>) -> Option<std::path::PathBuf> {
    match cwd {
        Some(c) if !c.is_empty() => Some(c.decode()),
        _ => std::env::current_dir().ok(),
    }
}

impl Session {
    /// Spawn a session PTY.
    ///
    /// `bound_gateway` is the socket name THIS daemon actually bound; it is
    /// stamped verbatim as the inception marker (`TERM_SESSION_GATEWAY`)
    /// so children can never inherit a drifted or stale endpoint value.
    pub fn spawn(
        id: u64,
        cmd: Option<Vec<String>>,
        cols: u16,
        rows: u16,
        channel: Option<&ChannelName>,
        cwd: Option<&PathWire>,
        bound_gateway: &str,
    ) -> PtyResult<Self> {
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        // Prefer the caller's launch directory; fall back to this process's
        // cwd (the daemon's) for legacy clients that send no cwd.
        let resolved_cwd = resolve_cwd(cwd);
        let mut builder = if let Some(cmd_parts) = &cmd {
            let mut b = CommandBuilder::new(&cmd_parts[0]);
            for arg in &cmd_parts[1..] {
                b.arg(arg);
            }
            b
        } else {
            default_shell_command()
        };
        if let Some(ch) = channel {
            builder.env(term_wm_config::env::CHANNEL_ENV_VAR, ch.to_string());
        } else {
            // Unset so the child never inherits a stale value from the parent process.
            builder.env_remove(term_wm_config::env::CHANNEL_ENV_VAR);
        }
        // Stamp the inception marker with the daemon's actually-bound
        // socket name so a nested `term-session`/`term-wm` attach can
        // detect same-gateway inception and refuse it (see `run_session`'s
        // nesting guard).
        builder.env(term_wm_config::env::SESSION_GATEWAY_ENV_VAR, bound_gateway);
        // Give session shells a CLEAN gateway-resolution context: strip the
        // toolchain namespace policy so descendants resolve endpoints from
        // their own launch context, never an ancestor's.
        builder.env_remove(term_wm_config::NAMESPACE_ENV_VAR);
        if let Some(c) = resolved_cwd {
            builder.cwd(c);
        }
        let pty = Pty::spawn(builder, size)?;
        Ok(Self {
            id,
            pty,
            title: None,
            exited: false,
            exit_code: None,
            cols,
            rows,
        })
    }

    pub fn read_output(&mut self) -> Vec<u8> {
        // Clear dirty flag and wake the PTY reader thread from I/O burst budget parking
        self.pty.screen();
        // Sync title from the background engine (replaces manual OSC extraction)
        if let Some(title) = self.pty.take_pending_title() {
            self.title = Some(title);
        }
        self.pty.drain_pending()
    }

    /// Drain the session's final output after the child has exited, waiting
    /// (bounded by `grace`) for the PTY reader thread to finish EOF processing
    /// so trailing bytes are not truncated. Used when retaining output for a
    /// late subscriber across session teardown.
    pub fn read_final_output(&mut self, grace: std::time::Duration) -> Vec<u8> {
        let out = self.pty.drain_final_output(grace);
        if let Some(title) = self.pty.take_pending_title() {
            self.title = Some(title);
        }
        out
    }

    /// Sync screen state without draining pending output.
    /// Clears the dirty flag (waking the reader thread from I/O burst budget parking)
    /// and syncs the title, but leaves accumulated bytes in the pending buffer so
    /// they can be sent to a future subscriber.
    pub fn sync_screen(&mut self) {
        self.pty.screen();
        if let Some(title) = self.pty.take_pending_title() {
            self.title = Some(title);
        }
    }

    pub fn check_exited(&mut self) -> bool {
        if !self.exited && self.pty.has_exited() {
            self.exited = true;
            self.exit_code = self.pty.exit_status().map(|s| s.exit_code() as i32);
            true
        } else {
            false
        }
    }

    pub fn take_exit_code(&mut self) -> Option<i32> {
        self.exit_code.take()
    }

    pub fn generate_snapshot(&mut self) -> Vec<u8> {
        self.pty.generate_snapshot()
    }

    pub fn set_status_callback(&mut self, cb: Option<Box<dyn Fn(PtyStatus) + Send + Sync>>) {
        self.pty.set_status_callback(cb);
    }
}

/// Safety net, not a lifecycle path: normal teardown (ShutdownGateway,
/// CloseSession escalation, exited-session cleanup) kills the child
/// explicitly BEFORE the session is dropped, so this best-effort kill is a
/// no-op there. It exists for the abnormal paths: a panicking daemon or a
/// dropped-without-teardown session would otherwise leak the child holding
/// its PTY. This mirrors the Windows Job Object `KILL_ON_JOB_CLOSE` net that
/// already guarantees tree-death when the `Pty` handle dies.
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.pty.kill_child();
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::{Session, resolve_cwd};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use term_session_muxio_service_definitions::path_wire;
    use term_session_muxio_service_definitions::path_wire::PathWire;

    const TEST_COLS: u16 = 80;
    const TEST_ROWS: u16 = 24;
    const REPORT_TIMEOUT_SECS: u64 = 10;

    #[test]
    fn resolve_cwd_uses_provided_dir() {
        let dir = std::env::temp_dir().join("resolve-cwd-probe");
        let probe = path_wire::encode_path(&dir);
        assert_eq!(resolve_cwd(Some(&probe)), Some(dir));
    }

    #[test]
    fn resolve_cwd_falls_back_to_process_dir_when_none() {
        assert_eq!(resolve_cwd(None), std::env::current_dir().ok());
    }

    #[test]
    fn resolve_cwd_falls_back_to_process_dir_when_empty() {
        assert_eq!(
            resolve_cwd(Some(&PathWire::default())),
            std::env::current_dir().ok()
        );
    }

    /// Try to create a directory whose name contains bytes that are not valid
    /// UTF-8. Only possible on Unix, and some filesystems refuse it: macOS
    /// requires valid UTF-8 filenames, so this returns `None` there and the
    /// tests skip (losslessness is still covered by the pure `path_wire`
    /// round-trip test, which needs no filesystem). Returns `Some` on Linux
    /// etc., proving the cwd round-trip is byte-for-byte, not merely
    /// UTF-8-equivalent.
    #[cfg(unix)]
    fn try_non_utf8_dir(base: &Path) -> Option<PathBuf> {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::OsStr::from_bytes(b"cwd-\xff\xfe-non-utf8");
        let dir = base.join(name);
        std::fs::create_dir_all(&dir).ok().map(|()| dir)
    }

    #[cfg(unix)]
    #[test]
    fn resolve_cwd_round_trips_non_utf8_dir() {
        let base = tempfile::tempdir().expect("tempdir");
        let Some(dir) = try_non_utf8_dir(base.path()) else {
            eprintln!("skipping: filesystem rejects non-UTF-8 directory names");
            return;
        };
        let probe = path_wire::encode_path(&dir);
        assert_eq!(resolve_cwd(Some(&probe)), Some(dir));
    }

    /// Poll `report` until the mock `pwd` child has written its cwd, with a
    /// generous timeout so a broken spawn fails the assertion instead of
    /// hanging the test. Meanwhile `read_output()` pumps the PTY so the
    /// child's DSR startup handshake completes (a Windows console child
    /// stalls until the host answers `\x1b[6n`) — otherwise the mock never
    /// runs and no report is written. Returns the raw report bytes so
    /// losslessness is asserted byte-for-byte.
    fn read_report(session: &mut Session, report: &Path) -> Vec<u8> {
        let mut content: Option<Vec<u8>> = None;
        term_test_support::wait_for(
            Duration::from_secs(REPORT_TIMEOUT_SECS),
            &format!("mock pwd wrote the report at {report:?}"),
            || {
                if let Ok(bytes) = std::fs::read(report) {
                    content = Some(bytes);
                    return Some(());
                }
                // Pump the PTY so the child's DSR handshake completes.
                session.read_output();
                None
            },
        );
        content.expect("report bytes captured")
    }

    /// Spawn a session running `mock pwd <report>` with the given wire-encoded
    /// cwd and return the wire bytes the child reports.
    ///
    /// The mock is a console app spawned through a PTY, which on Windows
    /// stalls at startup until the host answers its DSR cursor-position query
    /// (`\x1b[6n` → `\x1b[row;colR`). The wait loop therefore pumps the PTY via
    /// `read_output()` → `screen()`, mirroring the real daemon's poll/sync loop.
    fn spawn_pwd_report(cwd: Option<&PathWire>) -> PathWire {
        let dir = tempfile::tempdir().expect("report tempdir");
        let report = dir.path().join("pwd.txt");
        let mock = term_session_mock::get_mock_bin();
        let cmd = vec![
            mock.to_string_lossy().into_owned(),
            "pwd".to_string(),
            report.to_string_lossy().into_owned(),
        ];
        let mut session = Session::spawn(
            1,
            Some(cmd),
            TEST_COLS,
            TEST_ROWS,
            None,
            cwd,
            "test/bound/gateway",
        )
        .expect("spawn session");
        let bytes = read_report(&mut session, &report);
        session.pty.kill_child().ok();
        PathWire::from(bytes)
    }

    fn canonical_process_cwd() -> PathBuf {
        std::fs::canonicalize(std::env::current_dir().expect("process cwd"))
            .expect("canonicalize process cwd")
    }

    #[test]
    fn spawn_starts_in_specified_cwd() {
        let client_dir = tempfile::tempdir().expect("client tempdir");
        let expected = std::fs::canonicalize(client_dir.path()).expect("canonicalize client dir");
        let reported = spawn_pwd_report(Some(&path_wire::encode_path(client_dir.path())));
        let reported = std::fs::canonicalize(reported.decode()).expect("canonicalize reported");
        assert_eq!(reported, expected);
    }

    #[test]
    fn spawn_falls_back_to_process_cwd_when_cwd_none() {
        let reported = spawn_pwd_report(None);
        let reported = std::fs::canonicalize(reported.decode()).expect("canonicalize reported");
        assert_eq!(reported, canonical_process_cwd());
    }

    #[test]
    fn spawn_falls_back_to_process_cwd_when_cwd_empty() {
        let reported = spawn_pwd_report(Some(&PathWire::default()));
        let reported = std::fs::canonicalize(reported.decode()).expect("canonicalize reported");
        assert_eq!(reported, canonical_process_cwd());
    }

    /// End-to-end losslessness proof: a non-UTF-8 cwd survives the full
    /// `Session::spawn` → child cwd → report pipeline byte-for-byte (skipped on
    /// filesystems that reject non-UTF-8 names, e.g. macOS).
    #[cfg(unix)]
    #[test]
    fn spawn_round_trips_non_utf8_cwd() {
        let base = tempfile::tempdir().expect("tempdir");
        let Some(dir) = try_non_utf8_dir(base.path()) else {
            eprintln!("skipping: filesystem rejects non-UTF-8 directory names");
            return;
        };
        let reported = spawn_pwd_report(Some(&path_wire::encode_path(&dir)));
        let expected = std::fs::canonicalize(&dir).expect("canonicalize non-utf8 dir");
        assert_eq!(path_wire::decode_path(&reported), expected);
    }

    /// Spawn a session running `mock envvar <NAME> <report>` and return the
    /// report bytes (`<NAME>=<value>`), mirroring `spawn_pwd_report`.
    fn spawn_envvar_report(name: &str) -> Vec<u8> {
        let dir = tempfile::tempdir().expect("report tempdir");
        let report = dir.path().join("envvar.txt");
        let mock = term_session_mock::get_mock_bin();
        let cmd = vec![
            mock.to_string_lossy().into_owned(),
            "envvar".to_string(),
            name.to_string(),
            report.to_string_lossy().into_owned(),
        ];
        let mut session = Session::spawn(
            1,
            Some(cmd),
            TEST_COLS,
            TEST_ROWS,
            None,
            None,
            "test/bound/gateway",
        )
        .expect("spawn session");
        let bytes = read_report(&mut session, &report);
        session.pty.kill_child().ok();
        bytes
    }

    #[test]
    fn spawn_injects_gateway_socket_path() {
        let var = term_wm_config::env::SESSION_GATEWAY_ENV_VAR;
        let report = spawn_envvar_report(var);
        let report_str = std::str::from_utf8(&report).expect("utf8 report");
        // The report must be `TERM_SESSION_GATEWAY=<non-empty-value>`
        assert!(
            report_str.starts_with(&format!("{var}=")),
            "expected {var}=<value>, got: {report_str}"
        );
        let value = &report_str[var.len() + 1..];
        assert!(!value.is_empty(), "gateway env var must be non-empty");
    }

    /// With `channel = None` the daemon sets no channel env var; this both
    /// confirms the mock reports a genuinely-unset var as `NAME=` (no
    /// fabricated `1`) and that the `TERM_SESSION_CHANNEL` injection stays
    /// conditional on a channel being present.
    #[test]
    fn spawn_leaves_channel_var_unset_when_no_channel() {
        let report = spawn_envvar_report(term_wm_config::env::CHANNEL_ENV_VAR);
        assert_eq!(
            std::str::from_utf8(&report).expect("utf8 report"),
            format!("{}=", term_wm_config::env::CHANNEL_ENV_VAR)
        );
    }
}
