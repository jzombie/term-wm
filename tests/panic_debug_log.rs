use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect as RatatuiRect;
use term_wm::events::{Event, KeyEvent, MouseEvent};

use term_wm::actions::SystemTask;
use term_wm::app_context::AppContext;
use term_wm::config::AppBuilder;
use term_wm::io::{EventSource, RenderTarget};
use term_wm::runner::{WindowManagerHost, run_event_loop};
use term_wm::task_scheduler::TaskScheduler;
use term_wm::window::{WindowKey, WindowManager};
use term_wm_core::components::{NoopComponent, NoopOverlay, NoopWmComponent};
use term_wm_core::power_profile::PowerProfile;

#[derive(Debug)]
struct TestOutput {
    terminal: Terminal<TestBackend>,
}

impl TestOutput {
    fn new() -> Self {
        let backend = TestBackend::new(80, 24);
        let terminal = Terminal::new(backend).expect("TestBackend creation");
        Self { terminal }
    }
}

impl RenderTarget for TestOutput {
    fn enter(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn exit(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn draw<F>(&mut self, f: F) -> io::Result<()>
    where
        F: FnOnce(&mut dyn term_wm_render::RenderBackend),
    {
        self.terminal
            .draw(move |frame| {
                let area = frame.area();
                let buffer = Buffer::empty(area);
                let mut backend = term_wm_console::RatatuiBackend::new_simple(buffer, area);
                f(&mut backend);
                // Copy rendered buffer back to the terminal frame
                for y in 0..area.height {
                    for x in 0..area.width {
                        if let Some(cell) = backend.buffer.cell(RatatuiRect {
                            x,
                            y,
                            width: 1,
                            height: 1,
                        }) {
                            frame
                                .buffer_mut()
                                .set_string(x, y, cell.symbol(), cell.style());
                        }
                    }
                }
            })
            .map(|_| ())
            .map_err(|e| io::Error::other(e.to_string()))
    }
}

#[derive(Debug)]
struct ImmediateDriver;

impl EventSource for ImmediateDriver {
    fn poll(&mut self, _timeout: Duration) -> io::Result<bool> {
        Ok(false)
    }

    fn read(&mut self) -> io::Result<Event> {
        Err(io::Error::other("poll never returns true"))
    }

    fn next_key(&mut self) -> io::Result<KeyEvent> {
        Err(io::Error::other("not used"))
    }

    fn next_mouse(&mut self) -> io::Result<MouseEvent> {
        Err(io::Error::other("not used"))
    }

    fn current_profile(&self) -> PowerProfile {
        // Override the default PowerSaver so the FramePacer arms on idle
        // ticks.  This mock driver returns poll=false (no input events) and
        // never sets the global dirty bit, so all three pacer-arm checks
        // (Some(evt), take_redraw_request, current_profile != PowerSaver)
        // would otherwise return false/None.  The pacer stays disarmed,
        // try_expire() always returns false, the draw closure never runs,
        // and the test's intentional panic! payload is never reached.
        PowerProfile::Streaming
    }
}

struct SparseApp {
    wm: WindowManager<NoopComponent, NoopWmComponent, NoopOverlay>,
    draws: usize,
    window_key: Option<WindowKey>,
    should_quit: bool,
}

impl WindowManagerHost<NoopComponent, NoopWmComponent, NoopOverlay> for SparseApp {
    fn wm(&mut self) -> &mut WindowManager<NoopComponent, NoopWmComponent, NoopOverlay> {
        &mut self.wm
    }
    fn quit_requested(&self) -> bool {
        self.should_quit
    }
}

#[test]
fn render_panic_shows_in_debug_log() {
    let (_comp, handle) = term_wm_sys_ui_components::WmDebugLogComponent::new(2000);
    assert!(
        term_wm_sys_ui_components::set_global_debug_log(handle.clone()),
        "set_global_debug_log should succeed on first call"
    );
    term_wm_sys_ui_components::install_panic_hook();

    let mut wm = AppBuilder::<NoopWmComponent>::bare()
        .app_ctx(Arc::new(AppContext::new("test", "0.0.0")))
        .build::<NoopComponent, NoopOverlay>()
        .expect("test build");
    let key = wm.create_window(NoopComponent);
    wm.set_window_title(key, "test");

    let mut app = SparseApp {
        wm,
        draws: 0,
        window_key: Some(key),
        should_quit: false,
    };
    let mut output = TestOutput::new();
    let mut driver = ImmediateDriver;

    let panic_msg = "intentional-panic-from-draw";

    let result = run_event_loop(
        &mut output,
        &mut driver,
        &mut app,
        TaskScheduler::<SystemTask>::new(),
        |k| k,
        {
            move |_backend, app| {
                app.draws += 1;
                if app.draws == 1 {
                    panic!("{}", panic_msg);
                } else if let Some(k) = app.window_key.take() {
                    app.wm.close_window(k);
                    app.should_quit = true;
                }
            }
        },
    );

    assert!(
        result.is_ok(),
        "run_event_loop should return Ok after panic"
    );

    let lines = handle.lines();
    let joined = lines.join("\n");
    assert!(
        joined.contains(panic_msg),
        "panic message should appear in debug log\n=== log ===\n{joined}\n=========="
    );
    assert!(
        lines.iter().any(|l| l.len() > 10 && l.contains(':')),
        "backtrace frames should appear in debug log\n=== log ===\n{joined}\n=========="
    );
}

// ---------------------------------------------------------------------------
// Regression tests for FramePacer lifecycle invariants
// ---------------------------------------------------------------------------

/// Driver that yields a one-shot `take_redraw_request` then goes idle.
struct WakeupDriver {
    armed: bool,
}

impl EventSource for WakeupDriver {
    fn poll(&mut self, _: Duration) -> io::Result<bool> {
        Ok(false)
    }
    fn read(&mut self) -> io::Result<Event> {
        Err(io::Error::other(""))
    }
    fn next_key(&mut self) -> io::Result<KeyEvent> {
        Err(io::Error::other(""))
    }
    fn next_mouse(&mut self) -> io::Result<MouseEvent> {
        Err(io::Error::other(""))
    }
    fn current_profile(&self) -> PowerProfile {
        PowerProfile::Streaming
    }
    fn take_redraw_request(&mut self) -> bool {
        std::mem::replace(&mut self.armed, false)
    }
}

/// Helper: build a bare WindowManager with one window.
fn build_bare_wm() -> WindowManager<NoopComponent, NoopWmComponent, NoopOverlay> {
    let mut wm = AppBuilder::<NoopWmComponent>::bare()
        .app_ctx(Arc::new(term_wm::app_context::AppContext::new(
            "test", "0.0.0",
        )))
        .build::<NoopComponent, NoopOverlay>()
        .expect("test build");
    wm.create_window(NoopComponent);
    wm
}

/// Test 1: Background wakeup renders without input ("Wiggle to Update").
///
/// Regression guard: if `driver.take_redraw_request()` is removed from the
/// None branch's pacer-arm check, this test deadlocks because the FramePacer
/// never arms and the draw closure never fires.
#[test]
fn background_dirty_bit_triggers_render_without_input() {
    let mut app = SparseApp {
        wm: build_bare_wm(),
        draws: 0,
        window_key: None,
        should_quit: false,
    };
    let mut output = TestOutput::new();
    let mut driver = WakeupDriver { armed: true };

    let result = run_event_loop(
        &mut output,
        &mut driver,
        &mut app,
        TaskScheduler::<SystemTask>::new(),
        |k| k,
        |_, app| {
            app.draws += 1;
            app.should_quit = true;
        },
    );

    assert!(result.is_ok(), "run_event_loop should return Ok");
    assert_eq!(app.draws, 1, "draw must fire even without input events");
}

/// Test 2: FramePacer method injection — deterministic time control.
///
/// Validates that `try_expire(now)` and `time_until_deadline(now)` work
/// correctly with a mock clock instead of real `Instant::now()`.
#[test]
fn frame_pacer_deterministic_timing() {
    let mut pacer = term_wm_core::io::FramePacer::new();
    let epoch = Instant::now();

    // No deadline set — no time until deadline
    assert!(pacer.time_until_deadline(epoch).is_none());

    // Tick 1: arm and expire (epoch + 17ms past the 16ms interval)
    pacer.notify_pending(epoch);
    assert!(
        pacer.try_expire(epoch + Duration::from_millis(17)),
        "frame at t+17ms should be allowed (16ms interval)"
    );

    // Tick 2: re-arm and check 1ms later — still inside next interval
    pacer.notify_pending(epoch + Duration::from_millis(17));
    assert!(
        !pacer.try_expire(epoch + Duration::from_millis(18)),
        "frame at t+18ms should be rejected (1ms since arm)"
    );

    // Tick 3: 33ms after re-arm — interval elapsed, allowed
    assert!(
        pacer.try_expire(epoch + Duration::from_millis(51)),
        "frame at t+51ms should be allowed"
    );

    // Tick 4: re-arm with drag interval (33ms)
    pacer.set_interval(Duration::from_millis(33));
    pacer.notify_pending(epoch + Duration::from_millis(51));
    assert!(
        !pacer.try_expire(epoch + Duration::from_millis(60)),
        "drag frame at t+60ms should be rejected (9ms since arm)"
    );
    assert!(
        pacer.try_expire(epoch + Duration::from_millis(85)),
        "drag frame at t+85ms should be allowed (34ms since arm)"
    );

    // Deadline should be cleared after expiry
    assert!(pacer.time_until_deadline(Instant::now()).is_none());
}

/// Test 3: Skipped frames do not consume dirty state.
///
/// Regression guard: if the `did_render && !did_panic` gate is removed from
/// `flush_state_changes`, dirty windows are consumed on skipped frames and
/// the driver drops back to PowerSaver (3600s poll interval).
#[test]
fn skipped_frame_preserves_dirty_state() {
    let mut app = SparseApp {
        wm: build_bare_wm(),
        draws: 0,
        window_key: None,
        should_quit: false,
    };
    let mut output = TestOutput::new();
    // WakeupDriver with armed=false: no explicit redraw request, but
    // Streaming profile keeps driver_has_work true so the pacer arms.
    let mut driver = WakeupDriver { armed: false };

    let result = run_event_loop(
        &mut output,
        &mut driver,
        &mut app,
        TaskScheduler::<SystemTask>::new(),
        |k| k,
        |_, app| {
            app.draws += 1;
            app.should_quit = true;
        },
    );

    assert!(result.is_ok(), "run_event_loop should return Ok");
    assert_eq!(app.draws, 1, "draw must fire via Streaming profile");
}
