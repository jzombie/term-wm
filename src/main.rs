//! This default application is a simple terminal app, which opens two sub-shells in side-by-side
//! windows, where more windows can be added, or windows can be removed.

use std::io;
use std::time::Duration;

use crossterm::event::Event;
use term_wm::keybindings::{KeyBindings, Action};
use ratatui::prelude::Rect;

use portable_pty::PtySize;
use term_wm::components::{Component, TerminalComponent, default_shell_command};
use term_wm::drivers::OutputDriver;
use term_wm::drivers::console::{ConsoleInputDriver, ConsoleOutputDriver};
use term_wm::runner::{HasWindowManager, WindowApp, run_window_app};
use term_wm::ui::UiFrame;
use term_wm::window::{AppWindowDraw, WindowManager};

type PaneId = usize;

const MAX_WINDOWS: usize = 8;

fn main() -> io::Result<()> {
    let mut app = App::new()?;
    let focus_regions: Vec<PaneId> = (0..MAX_WINDOWS).collect();
    let mut output = ConsoleOutputDriver::new()?;
    output.enter()?;
    let mut input = ConsoleInputDriver::new();

    let result = run_window_app(
        &mut output,
        &mut input,
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
            if let Some(Event::Key(key)) = event {
                let kb = KeyBindings::default();
                return kb.matches(Action::Quit, key);
            }
            false
        },
    );

    output.exit()?;

    result
}

struct App {
    windows: WindowManager<PaneId, PaneId>,
    terminals: Vec<TerminalComponent>,
}

impl App {
    fn new() -> io::Result<Self> {
        let mut app = Self {
            windows: WindowManager::new_managed(0),
            terminals: Vec::new(),
        };
        app.wm_new_window()?;
        app.wm_new_window()?;
        app.windows.open_help_overlay();
        Ok(app)
    }
}

impl HasWindowManager<PaneId, PaneId> for App {
    fn windows(&mut self) -> &mut WindowManager<PaneId, PaneId> {
        &mut self.windows
    }

    fn wm_new_window(&mut self) -> io::Result<()> {
        if self.terminals.len() >= MAX_WINDOWS {
            return Ok(());
        }
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pane =
            TerminalComponent::spawn(default_shell_command(), size).map_err(io::Error::other)?;
        pane.set_link_handler_fn(|url| {
            let _ = webbrowser::open(url);
            true
        });
        let id = self.terminals.len();
        self.terminals.push(pane);
        self.windows.set_focus(id);
        self.windows.tile_window(id);
        // Set a user-visible title for the newly created pane.
        self.windows
            .set_window_title(id, format!("Shell {}", id + 1));
        Ok(())
    }

    fn wm_close_window(&mut self, id: PaneId) -> io::Result<()> {
        if let Some(pane) = self.terminals.get_mut(id) {
            // TODO: Show confirmation before abrupt termination
            pane.terminate();
        }
        Ok(())
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

    fn render_window(&mut self, frame: &mut UiFrame<'_>, window: AppWindowDraw<PaneId>) {
        render_pane(frame, self, window.id, window.surface.inner, window.focused);
    }

    fn empty_window_message(&self) -> &str {
        "all shells exited"
    }
}

fn render_pane(frame: &mut UiFrame<'_>, app: &mut App, id: PaneId, area: Rect, focused: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if let Some(pane) = app.terminals.get_mut(id) {
        pane.resize(area);
        pane.render(frame, area, focused);
    }
}
