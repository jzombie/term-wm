// Integration tests for drain-synchronized PTY resize: a resize request is
// applied by the reader thread at the pipe-drain boundary (emulator reflow +
// OS ioctl/SIGWINCH), not mid-shell-write. These live here (not in src/pty.rs)
// because they exercise only the public `Pty` API and spawn real child
// processes (`cat` / `cmd.exe`), which is what integration tests are for.

use portable_pty::{CommandBuilder, PtySize};
use term_wm_pty_engine::{Pty, PtyResult};

/// Wraps a `Pty` and guarantees the child is killed on drop, even when a test
/// assertion panics or times out. On Windows this terminates the Job Object
/// tree (`cmd.exe` and any descendants); on Unix it kills the child directly.
/// Without it, a leaked idle child would hold the ConPTY pipe / pty master open.
struct AutoKillPty(Option<Pty>);

impl AutoKillPty {
    fn spawn(cmd: CommandBuilder, size: PtySize) -> PtyResult<Self> {
        let pty = Pty::spawn_with_scrollback(cmd, size, 1000)?;
        Ok(Self(Some(pty)))
    }
}

impl std::ops::Deref for AutoKillPty {
    type Target = Pty;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for AutoKillPty {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().unwrap()
    }
}

impl Drop for AutoKillPty {
    fn drop(&mut self) {
        if let Some(mut pty) = self.0.take() {
            let _ = pty.kill_child();
        }
    }
}

/// Returns a platform-appropriate dummy executable for PTY plumbing tests.
/// On Unix, `cat` blocks on stdin and echoes output. On Windows, `cmd.exe`
/// blocks on stdin and keeps the ConPTY alive.
fn get_test_executable() -> &'static str {
    if cfg!(windows) { "cmd.exe" } else { "cat" }
}

const TEST_SIZE: PtySize = PtySize {
    rows: 24,
    cols: 80,
    pixel_width: 0,
    pixel_height: 0,
};

/// A resize request is applied by the reader thread once the pipe drains
/// (not mid-shell-write), updating the shared emulator size.
#[test]
fn drain_sync_applies_resize_after_pipe_drain() {
    let mut pty =
        AutoKillPty::spawn(CommandBuilder::new(get_test_executable()), TEST_SIZE).expect("spawn");
    let target = PtySize {
        rows: 30,
        cols: 100,
        pixel_width: 0,
        pixel_height: 0,
    };
    pty.resize(target).expect("request resize");
    // Prompt the reader to drain (the `cat`/`cmd` child echoes "hi" back).
    let _ = pty.write_bytes(b"hi\n");
    // Poll (up to 5s) for the reader to apply the resize at the drain.
    for _ in 0..250 {
        if pty.size().rows == 30 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(
        pty.size().rows,
        30,
        "emulator resized after the pipe drained"
    );
    assert_eq!(pty.size().cols, 100);
}

/// Rapid drag resizes coalesce to the final size only (frame-dropping).
#[test]
fn drain_sync_coalesces_rapid_resizes_to_final() {
    let mut pty =
        AutoKillPty::spawn(CommandBuilder::new(get_test_executable()), TEST_SIZE).expect("spawn");
    pty.resize(PtySize {
        rows: 30,
        cols: 100,
        pixel_width: 0,
        pixel_height: 0,
    })
    .unwrap();
    pty.resize(PtySize {
        rows: 40,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    })
    .unwrap();
    let final_size = PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    };
    pty.resize(final_size).unwrap();
    let _ = pty.write_bytes(b"x\n");
    for _ in 0..250 {
        if pty.size() == final_size {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(pty.size(), final_size, "only the final size is applied");
}

/// For an idle shell (empty pipe), the resize-wake makes the reader drain
/// immediately and apply — no data is needed.
#[test]
fn drain_sync_applies_resize_when_pipe_idle() {
    let mut pty =
        AutoKillPty::spawn(CommandBuilder::new(get_test_executable()), TEST_SIZE).expect("spawn");
    let target = PtySize {
        rows: 33,
        cols: 90,
        pixel_width: 0,
        pixel_height: 0,
    };
    pty.resize(target).unwrap();
    for _ in 0..250 {
        if pty.size() == target {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(pty.size(), target, "resize applied at the empty-pipe drain");
}
