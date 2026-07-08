use std::io;
use std::sync::Arc;
use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect as RatatuiRect;
use term_wm::events::{Event, KeyEvent, MouseEvent};

use term_wm::actions::SystemTask;
use term_wm::app_context::AppContext;
use term_wm::config::AppBuilder;
use term_wm::io::{EventSource, RenderTarget};
use term_wm::runner::{WindowManagerHost, run_app};
use term_wm::task_scheduler::TaskScheduler;
use term_wm::window::{WindowKey, WindowManager};

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
                let mut backend = term_wm_console::RatatuiBackend::new(buffer, area);
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
}

struct SparseApp {
    wm: WindowManager,
    draws: usize,
    window_key: Option<WindowKey>,
    should_quit: bool,
}

impl WindowManagerHost for SparseApp {
    fn wm(&mut self) -> &mut WindowManager {
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

    let menu: Box<dyn term_wm_core::components::WmComponent> =
        Box::new(term_wm_sys_ui_components::WmMenuOverlay::new());
    let mut wm = AppBuilder::bare()
        .app_ctx(Arc::new(AppContext::new("test", "0.0.0")))
        .command_menu(menu)
        .build()
        .expect("test build");
    let key = wm.create_window(Box::new(term_wm::components::NoopComponent));
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

    let result = run_app(
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

    assert!(result.is_ok(), "run_app should return Ok after panic");

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
