use portable_pty::{CommandBuilder, PtySize};
use term_wm_pty_engine::{Pty, PtyResult};

pub struct Session {
    pub id: u64,
    pub pty: Pty,
    pub parser: vt100::Parser,
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
    let mut cmd = CommandBuilder::new(shell);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }
    cmd
}

impl Session {
    pub fn spawn(id: u64, cmd: Option<Vec<String>>, cols: u16, rows: u16) -> PtyResult<Self> {
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pty = if let Some(cmd_parts) = &cmd {
            let mut builder = CommandBuilder::new(&cmd_parts[0]);
            for arg in &cmd_parts[1..] {
                builder.arg(arg);
            }
            if let Ok(cwd) = std::env::current_dir() {
                builder.cwd(cwd);
            }
            Pty::spawn(builder, size)?
        } else {
            Pty::spawn(default_shell_command(), size)?
        };
        let parser = vt100::Parser::new(rows, cols, 0);
        Ok(Self {
            id,
            pty,
            parser,
            title: None,
            exited: false,
            exit_code: None,
            cols,
            rows,
        })
    }

    pub fn read_output(&mut self) -> Vec<u8> {
        let bytes = self.pty.drain_pending();
        if !bytes.is_empty() {
            self.parser.process(&bytes);
            if let Some(title) = term_wm_pty_engine::title::extract_osc_title(&bytes) {
                self.title = Some(title);
            }
            if let Some(text) = term_wm_pty_engine::clipboard::extract_osc52_text(&bytes) {
                let mut cb = term_wm_pty_engine::clipboard::Clipboard::new();
                let _ = cb.set(&text);
            }
        }
        bytes
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
        self.parser.screen().state_formatted()
    }
}
