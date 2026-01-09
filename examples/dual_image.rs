use std::io;
use std::time::Duration;

use crossterm::event::Event;
use ratatui::prelude::Rect;
use ratatui::widgets::Clear;

use term_wm::components::{Component, ComponentContext, SvgImageComponent};
use term_wm::drivers::OutputDriver;
use term_wm::drivers::console::{ConsoleInputDriver, ConsoleOutputDriver};
use term_wm::runner::{HasWindowManager, WindowApp, run_window_app};
use term_wm::ui::UiFrame;
use term_wm::window::{AppWindowDraw, WindowManager};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PaneId {
    Left,
    Right,
}

fn main() -> io::Result<()> {
    let mut app = App::new(std::env::args().skip(1).collect())?;
    let mut output = ConsoleOutputDriver::new()?;
    output.enter()?;
    let mut input = ConsoleInputDriver::new();

    let result = run_window_app(
        &mut output,
        &mut input,
        &mut app,
        &[PaneId::Left, PaneId::Right],
        |id| id,
        Some,
        Duration::from_millis(16),
        |event, app| {
            if matches!(event, Event::Mouse(_)) && app.windows.handle_managed_event(event) {
                return true;
            }
            match app.windows.focus() {
                PaneId::Left => app.left.handle_event(event, &ComponentContext::new(true)),
                PaneId::Right => app.right.handle_event(event, &ComponentContext::new(true)),
            }
        },
        |event, _app| {
            if let Some(evt) = event {
                term_wm::keybindings::KeyBindings::default().action_for_event(evt)
                    == Some(term_wm::keybindings::Action::Quit)
            } else {
                false
            }
        },
    );

    output.exit()?;

    result
}

struct App {
    windows: WindowManager<PaneId, PaneId>,
    left: SvgImageComponent,
    right: SvgImageComponent,
    pending_paths: Vec<String>,
    loaded_count: usize,
}

impl App {
    fn new(mut paths: Vec<String>) -> io::Result<Self> {
        let mut left = SvgImageComponent::new();
        let mut right = SvgImageComponent::new();
        left.set_keep_aspect(true);
        right.set_keep_aspect(true);
        left.set_colorize(true);
        right.set_colorize(true);
        if paths.is_empty() {
            paths.push("assets/zenOSmosis-logo.svg".to_string());
        }
        if paths.len() == 1 {
            paths.push(paths[0].clone());
        }
        let mut windows = WindowManager::new_managed(PaneId::Left);
        windows.set_focus_order(vec![PaneId::Left, PaneId::Right]);
        let mut app = Self {
            windows,
            left,
            right,
            pending_paths: paths,
            loaded_count: 0,
        };
        // Initialize windows via the wm_new_window API so creation paths match runtime behavior.
        app.wm_new_window()?;
        app.wm_new_window()?;
        Ok(app)
    }
}

impl HasWindowManager<PaneId, PaneId> for App {
    fn windows(&mut self) -> &mut WindowManager<PaneId, PaneId> {
        &mut self.windows
    }

    fn wm_new_window(&mut self) -> io::Result<()> {
        // Load next pending path into the next available pane (Left then Right).
        if self.loaded_count >= self.pending_paths.len() {
            return Ok(());
        }
        let path = &self.pending_paths[self.loaded_count];
        match self.loaded_count {
            0 => load_into(&mut self.left, path)?,
            1 => load_into(&mut self.right, path)?,
            _ => {}
        }
        self.loaded_count += 1;
        Ok(())
    }
}

impl WindowApp<PaneId, PaneId> for App {
    fn enumerate_windows(&mut self) -> Vec<PaneId> {
        vec![PaneId::Left, PaneId::Right]
    }

    fn render_window(&mut self, frame: &mut UiFrame<'_>, window: AppWindowDraw<PaneId>) {
        match window.id {
            PaneId::Left => {
                render_pane(frame, &mut self.left, window.surface.inner, window.focused)
            }
            PaneId::Right => {
                render_pane(frame, &mut self.right, window.surface.inner, window.focused)
            }
        }
    }

    fn empty_window_message(&self) -> &str {
        "no images loaded"
    }
}

fn render_pane(frame: &mut UiFrame<'_>, image: &mut SvgImageComponent, area: Rect, focused: bool) {
    // Clear the area and render the image directly (no inner decorative frame).
    frame.render_widget(Clear, area);
    image.render(frame, area, &ComponentContext::new(focused));
}

fn load_into(component: &mut SvgImageComponent, path: &str) -> io::Result<()> {
    component.load_from_path(path)
}
