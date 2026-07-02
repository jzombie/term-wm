use std::io;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{Event, KeyEvent, MouseEvent};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use term_wm::app_context::AppContext;
use term_wm::components::{ComponentContext, MenuOverlay};
use term_wm::io::{EventSource, RenderTarget};
use term_wm::runner::{WindowManagerHost, WindowProvider, run_app};
use term_wm::ui::UiFrame;
use term_wm::window::{WindowDrawContext, WindowKey, WindowManager, WmMenuAction};
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
    wm: WindowManager,
    draws: usize,
    window_key: Option<WindowKey>,
    should_quit: bool,
}

impl WindowManagerHost for SparseApp {
    fn windows(&mut self) -> &mut WindowManager {
        &mut self.wm
    }
    fn quit_requested(&self) -> bool {
        self.should_quit
    }
}

impl WindowProvider for SparseApp {
    fn enumerate_windows(&mut self) -> Vec<WindowKey> {
        if self.should_quit {
            vec![]
        } else {
            self.window_key.map(|k| vec![k]).unwrap_or_default()
        }
    }

    fn render_window(
        &mut self,
        _frame: &mut UiFrame<'_>,
        _window: WindowDrawContext,
        _ctx: &ComponentContext,
    ) {
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

    let menu: Box<dyn MenuOverlay<WmMenuAction>> =
        Box::new(term_wm_sys_ui_components::WmMenuOverlay::new());
    let mut wm = WindowManager::with_config(
        WmConfig::standalone(),
        Arc::new(AppContext::new("test", "0.0.0")),
        None::<Box<dyn term_wm::top_panel_trait::TopPanel<WindowKey>>>,
        None::<Box<dyn term_wm::bottom_panel_trait::BottomPanel>>,
        Some(menu),
    );
    let key = wm.create_window();
    wm.set_window_title(key, "test");

    let mut app = SparseApp {
        wm,
        draws: 0,
        window_key: Some(key),
        should_quit: false,
    };
    let mut output = TestOutput::new();
    let mut driver = ImmediateDriver;
    let focus_regions: Vec<WindowKey> = vec![];

    let panic_msg = "intentional-panic-from-draw";

    let result = run_app(&mut output, &mut driver, &mut app, &focus_regions, |k| k, {
        move |_frame, app| {
            app.draws += 1;
            if app.draws == 1 {
                panic!("{}", panic_msg);
            } else if let Some(k) = app.window_key.take() {
                app.wm.close_window(k);
                app.should_quit = true;
            }
        }
    });

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
