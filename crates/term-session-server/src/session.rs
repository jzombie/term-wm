use portable_pty::{CommandBuilder, PtySize};
use term_session_muxio_service_definitions::ChannelName;
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
/// Prefers the caller's launch directory (non-empty); falls back to this
/// process's cwd (the daemon's) for legacy clients that send `None` or an
/// empty string.
fn resolve_cwd(cwd: Option<&str>) -> Option<std::path::PathBuf> {
    match cwd {
        Some(c) if !c.is_empty() => Some(std::path::PathBuf::from(c)),
        _ => std::env::current_dir().ok(),
    }
}

impl Session {
    pub fn spawn(
        id: u64,
        cmd: Option<Vec<String>>,
        cols: u16,
        rows: u16,
        channel: Option<&ChannelName>,
        cwd: Option<&str>,
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
            builder.env("TERM_WM_CHANNEL", ch.to_string());
        }
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

#[cfg(test)]
mod tests {
    use super::{resolve_cwd, Session};
    use std::time::{Duration, Instant};

    const TEST_COLS: u16 = 80;
    const TEST_ROWS: u16 = 24;
    const REPORT_TIMEOUT_SECS: u64 = 10;

    #[test]
    fn resolve_cwd_uses_provided_dir() {
        let dir = std::env::temp_dir().join("resolve-cwd-probe");
        let probe = dir.to_string_lossy().into_owned();
        assert_eq!(resolve_cwd(Some(&probe)), Some(dir));
    }

    #[test]
    fn resolve_cwd_falls_back_to_process_dir_when_none() {
        assert_eq!(resolve_cwd(None), std::env::current_dir().ok());
    }

    #[test]
    fn resolve_cwd_falls_back_to_process_dir_when_empty() {
        assert_eq!(resolve_cwd(Some("")), std::env::current_dir().ok());
    }

    /// Poll `report` until the mock `pwd` child has written its cwd, with a
    /// generous timeout so a broken spawn fails the assertion instead of
    /// hanging the test.
    fn read_report(report: &std::path::Path) -> String {
        let deadline = Instant::now() + Duration::from_secs(REPORT_TIMEOUT_SECS);
        loop {
            if let Ok(content) = std::fs::read_to_string(report) {
                return content;
            }
            assert!(
                Instant::now() < deadline,
                "mock pwd never wrote the report at {report:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Spawn a session running `mock pwd <report>` with the given cwd and
    /// return the cwd the child reports.
    fn spawn_pwd_report(cwd: Option<&str>) -> String {
        let dir = tempfile::tempdir().expect("report tempdir");
        let report = dir.path().join("pwd.txt");
        let mock = term_session_mock::get_mock_bin();
        let cmd = vec![
            mock.to_string_lossy().into_owned(),
            "pwd".to_string(),
            report.to_string_lossy().into_owned(),
        ];
        let _session =
            Session::spawn(1, Some(cmd), TEST_COLS, TEST_ROWS, None, cwd).expect("spawn session");
        read_report(&report)
    }

    fn canonical_process_cwd() -> String {
        std::fs::canonicalize(std::env::current_dir().expect("process cwd"))
            .expect("canonicalize process cwd")
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn spawn_starts_in_specified_cwd() {
        let client_dir = tempfile::tempdir().expect("client tempdir");
        let expected = std::fs::canonicalize(client_dir.path())
            .expect("canonicalize client dir")
            .to_string_lossy()
            .into_owned();
        let reported = spawn_pwd_report(Some(client_dir.path().to_str().expect("utf-8 cwd")));
        assert_eq!(reported, expected);
    }

    #[test]
    fn spawn_falls_back_to_process_cwd_when_cwd_none() {
        let reported = spawn_pwd_report(None);
        assert_eq!(reported, canonical_process_cwd());
    }

    #[test]
    fn spawn_falls_back_to_process_cwd_when_cwd_empty() {
        let reported = spawn_pwd_report(Some(""));
        assert_eq!(reported, canonical_process_cwd());
    }
}
