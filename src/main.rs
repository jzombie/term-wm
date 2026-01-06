use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, terminal};
use ratatui::backend::CrosstermBackend;
use ratatui::prelude::Rect;
use ratatui::{Frame, Terminal};

use portable_pty::PtySize;
use term_wm::components::{Component, TerminalComponent, default_shell_command};
use term_wm::drivers::console::ConsoleDriver;
use term_wm::runner::{HasWindowManager, WindowApp, run_window_app};
use term_wm::window::{AppWindowDraw, WindowManager};

type PaneId = usize;

const MAX_WINDOWS: usize = 8;

fn main() -> io::Result<()> {
    let mut app = App::new()?;
    let focus_regions: Vec<PaneId> = (0..MAX_WINDOWS).collect();
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    terminal::enable_raw_mode()?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut driver = ConsoleDriver::new();

    let result = run_window_app(
        &mut terminal,
        &mut driver,
        &mut app,
        &focus_regions,
        |id| id,
        Some,
        Duration::from_millis(16),
        |event, app| {
            if matches!(event, Event::Mouse(_)) && app.windows.handle_managed_event(event) {
                return true;
            }
            if let Some(pane) = app.terminals.get_mut(app.windows.focus()) {
                return pane.handle_event(event);
            }
            false
        },
        |event, app| {
            if app.terminals.iter_mut().all(|pane| pane.has_exited()) {
                return true;
            }
            matches!(
                event,
                Some(Event::Key(key))
                    if key.code == KeyCode::Char('q')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
            )
        },
    );

    terminal::disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        event::DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    result
}

struct App {
    windows: WindowManager<PaneId, PaneId>,
    terminals: Vec<TerminalComponent>,
}

impl App {
    fn new() -> io::Result<Self> {
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let left =
            TerminalComponent::spawn(default_shell_command(), size).map_err(io::Error::other)?;
        let right =
            TerminalComponent::spawn(default_shell_command(), size).map_err(io::Error::other)?;
        let mut windows = WindowManager::new_managed(0);
        windows.set_focus_order(vec![0, 1]);
        Ok(Self {
            windows,
            terminals: vec![left, right],
        })
    }
}

impl HasWindowManager<PaneId, PaneId> for App {
    fn windows(&mut self) -> &mut WindowManager<PaneId, PaneId> {
        &mut self.windows
    }

    fn wm_new_window(&mut self) {
        if self.terminals.len() >= MAX_WINDOWS {
            return;
        }
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pane =
            TerminalComponent::spawn(default_shell_command(), size).map_err(io::Error::other);
        if let Ok(pane) = pane {
            let id = self.terminals.len();
            self.terminals.push(pane);
            self.windows.set_focus(id);
            self.windows.tile_window(id);
        }
    }

    fn wm_close_window(&mut self, id: PaneId) {
        if let Some(pane) = self.terminals.get_mut(id) {
            // TODO: Show confirmation before abrupt termination
            pane.terminate();
        }
    }
}

impl WindowApp<PaneId, PaneId> for App {
    fn enumerate_windows(&mut self) -> Vec<PaneId> {
        self.terminals
            .iter_mut()
            .enumerate()
            .filter_map(|(id, pane)| (!pane.has_exited()).then_some(id))
            .collect()
    }

    fn render_window(&mut self, frame: &mut Frame, window: AppWindowDraw<PaneId>) {
        render_pane(frame, self, window.id, window.surface.inner, window.focused);
    }

    fn empty_window_message(&self) -> &str {
        "all shells exited"
    }
}

fn render_pane(frame: &mut Frame, app: &mut App, id: PaneId, area: Rect, focused: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if let Some(pane) = app.terminals.get_mut(id) {
        pane.resize(area);
        pane.render(frame, area, focused);
    }
}
