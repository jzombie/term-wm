//! Probe binary for the nested-mouse regression test
//! (`term-wm-crossterm-adapter/tests/nested_mouse.rs`).
//!
//! Run by that test *inside* a real `portable_pty` pair, so its `stdin` is an
//! actual console/ConPTY input handle (unlike the test process, whose stdin is
//! a redirected pipe in CI). It enables mouse capture through the adapter and
//! reports:
//!
//! - `MOUSE_READY` — after mouse capture is enabled.
//! - `CONSOLE_MODE:<hex>` — Windows only: the `GetConsoleMode` value of its own
//!   stdin, so the test can assert `ENABLE_MOUSE_INPUT` is actually set.
//! - `MOUSE_EVENT:<debug>` — each crossterm mouse event received within the
//!   poll window.
//! - `MOUSE_TIMEOUT` — no mouse event arrived before the deadline.
//!
//! Output markers go to stdout (the PTY slave output → test reads the master);
//! mouse input is read from stdin (the PTY slave input ← test writes the
//! master). Every report is explicitly flushed because stdout here is a pipe,
//! which is block-buffered.

use std::io::Write;
use std::time::{Duration, Instant};

fn report(line: &str) {
    println!("{line}");
    let _ = std::io::stdout().flush();
}

fn main() {
    // Raw mode so the PTY line discipline doesn't mangle the SGR bytes on Unix.
    let _ = crossterm::terminal::enable_raw_mode();
    if let Err(e) = term_wm_crossterm_adapter::set_mouse_capture(true) {
        report(&format!("MOUSE_ENABLE_ERROR:{e}"));
        return;
    }
    report("MOUSE_READY");

    #[cfg(windows)]
    report_console_mode();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(true) = crossterm::event::poll(Duration::from_millis(100))
            && let Ok(evt) = crossterm::event::read()
            && let crossterm::event::Event::Mouse(mouse) = evt
        {
            report(&format!("MOUSE_EVENT:{mouse:?}"));
        }
        if Instant::now() >= deadline {
            report("MOUSE_TIMEOUT");
            break;
        }
    }
}

#[cfg(windows)]
fn report_console_mode() {
    use std::os::windows::io::AsRawHandle as _;

    unsafe extern "system" {
        fn GetConsoleMode(h: *mut std::ffi::c_void, mode: *mut u32) -> i32;
    }

    let mut mode = 0u32;
    // SAFETY: stdin inside a ConPTY is a console input handle; GetConsoleMode
    // validates it and reports failure via the return value.
    let ok = unsafe { GetConsoleMode(std::io::stdin().as_raw_handle(), &mut mode) };
    if ok != 0 {
        report(&format!("CONSOLE_MODE:{mode:#010x}"));
    } else {
        report("CONSOLE_MODE:ERROR");
    }
}
