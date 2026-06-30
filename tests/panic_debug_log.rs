use std::io;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{Event, KeyEvent, MouseEvent};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use term_wm::app_context::AppContext;
use term_wm::bottom_panel_trait::BottomPanel;
use term_wm::components::MenuOverlay;
use term_wm::io::{EventSource, RenderTarget};
use term_wm::runner::{WindowManagerHost, WindowProvider, run_app};
use term_wm::top_panel_trait::TopPanel;
use term_wm::ui::UiFrame;
use term_wm::window::SystemWindowId;
use term_wm::window::{WindowDrawContext, WindowId, WindowManager, WmMenuAction};
use term_wm::wm_config::WmConfig;

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
    type Backend = TestBackend;

    fn enter(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn exit(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn draw<F>(&mut self, f: F) -> io::Result<()>
    where
        F: FnOnce(UiFrame<'_>),
    {
        self.terminal
            .draw(move |frame| {
                let wrapper = UiFrame::new(frame);
                f(wrapper);
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
    wm: WindowManager<usize>,
    draws: usize,
}

impl WindowManagerHost<usize> for SparseApp {
    fn windows(&mut self) -> &mut WindowManager<usize> {
        &mut self.wm
    }
}

impl WindowProvider<usize> for SparseApp {
    fn enumerate_windows(&mut self) -> Vec<usize> {
        // Return a window on the first two ticks so the app doesn't
        // hit the quit condition before the draw callback can panic.
        // After the panic + debug-window handling, return empty to
        // let the idle-path quit condition fire.
        if self.draws < 3 { vec![1] } else { vec![] }
    }

    fn render_window(&mut self, _frame: &mut UiFrame<'_>, _window: WindowDrawContext<usize>) {}
}

#[test]
fn render_panic_shows_in_debug_log() {
    let (_comp, handle) = term_wm_sys_ui_components::WmDebugLogComponent::new(2000);
    assert!(
        term_wm_sys_ui_components::set_global_debug_log(handle.clone()),
        "set_global_debug_log should succeed on first call"
    );
    term_wm_sys_ui_components::install_panic_hook();

    let menu: Box<dyn MenuOverlay<WmMenuAction>> =
        Box::new(term_wm_sys_ui_components::WmMenuOverlay::new());
    let wm = WindowManager::<usize>::with_config(
        0,
        WmConfig::standalone(),
        Arc::new(AppContext::new("test", "0.0.0")),
        None::<Box<dyn TopPanel<term_wm::window::WindowId<usize>>>>,
        None::<Box<dyn BottomPanel>>,
        menu,
    );

    let mut app = SparseApp { wm, draws: 0 };
    let mut output = TestOutput::new();
    let mut driver = ImmediateDriver;
    let focus_regions: Vec<usize> = vec![];

    let panic_msg = "intentional-panic-from-draw";

    let result = run_app(
        &mut output,
        &mut driver,
        &mut app,
        &focus_regions,
        |id| id,
        {
            move |_frame, app| {
                app.draws += 1;
                if app.draws == 1 {
                    panic!("{}", panic_msg);
                } else {
                    app.windows()
                        .close_window(WindowId::System(SystemWindowId::DebugLog));
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
