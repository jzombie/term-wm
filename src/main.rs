//! This default application is a simple terminal app, which opens two sub-shells in side-by-side
//! windows, where more windows can be added, or windows can be removed.

// TODO: Add mode to auto-open debug window

use std::io;

use clap::Parser;
use ratatui::prelude::Rect;

use line_ending::LineEnding;
use term_wm::components::{
    Component, ScrollViewComponent, TerminalComponent, default_shell_command,
};
use term_wm::drivers::OutputDriver;
use term_wm::drivers::console::{ConsoleInputDriver, ConsoleOutputDriver};
use term_wm::runner::{HasWindowManager, WindowApp, run_window_app};
use term_wm::ui::UiFrame;
use term_wm::window::{AppWindowDraw, WindowManager};

type PaneId = usize;

/// Simple CLI for launching `term-wm` with optional commands / window count.
#[derive(Parser, Debug)]
#[command(
    name = env!("CARGO_PKG_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    about = env!("CARGO_PKG_DESCRIPTION"),
    long_about = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"), ": ", env!("CARGO_PKG_DESCRIPTION")),
)]
struct Cli {
    /// Number of terminal windows to open.
    ///
    /// When omitted and no commands are provided this defaults to 2. When commands are provided and `--count`
    /// is omitted, the number of windows will default to the number of
    /// commands
    #[arg(short = 'n', long = "count")]
    count: Option<usize>,

    /// Commands to run in created windows.
    ///
    /// If provided, the number of windows will equal the number of commands given and each command will be run
    /// in its respective window via the default shell (i.e. shell -c "CMD").
    #[arg(value_name = "CMD", num_args = 0..)]
    cmds: Vec<String>,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    // Determine total number of windows to open by default.
    //
    // Behavior:
    // - If no commands provided: open `--count` shells (default 2 if not given).
    // - If commands provided: if `--count` given use it, otherwise default to
    //   the number of commands.
    let total = if cli.cmds.is_empty() {
        cli.count.unwrap_or(2).max(1)
    } else {
        cli.count.map(|c| c.max(1)).unwrap_or_else(|| {
            // default to number of commands when count not given
            cli.cmds.len().max(1)
        })
    };
    let mut app = App::new_with(cli.cmds, total)?;
    // Use the initial window indices as focus regions; the WindowManager
    // will track additional windows as they are created.
    let focus_regions: Vec<PaneId> = (0..total).collect();
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
    );

    output.exit()?;

    result
}

struct App {
    windows: WindowManager<PaneId, PaneId>,
    terminals: Vec<ScrollViewComponent<TerminalComponent>>,
}

impl App {
    fn new_with(commands: Vec<String>, num_windows: usize) -> io::Result<Self> {
        let mut app = Self {
            windows: WindowManager::new_managed(0),
            terminals: Vec::new(),
        };

        let mut error_occurred = false;

        // If commands provided, open one per command; otherwise open `num_windows`
        // shells using the default shell.
        if !commands.is_empty() {
            let mut it = commands.into_iter();
            for _ in 0..num_windows {
                if let Some(cmd) = it.next() {
                    // Spawn an interactive shell and send the command as input so
                    // that when the command exits the shell remains.
                    let cb = default_shell_command();
                    if let Err(e) = app.spawn_terminal_with_command(cb) {
                        tracing::error!("Window spawn error: {}", e);
                        error_occurred = true;
                    }
                    // If spawn succeeded, write the command into the PTY.
                    if !error_occurred && let Some(pane) = app.terminals.last_mut() {
                        let mut line = cmd;
                        line.push_str(LineEnding::from_current_platform().as_str());
                        let _ = pane.content.write_bytes(line.as_bytes());
                    }
                } else if let Err(e) = app.wm_new_window() {
                    tracing::error!("Window spawn error: {}", e);
                    error_occurred = true;
                }
            }
        } else {
            for _ in 0..num_windows {
                if let Err(e) = app.wm_new_window() {
                    tracing::error!("Window spawn error: {}", e);
                    error_occurred = true;
                }
            }
        }

        if error_occurred {
            app.windows.open_debug_window();
        }

        app.windows.open_help_overlay();
        Ok(app)
    }

    fn spawn_terminal_with_command(&mut self, cmd: portable_pty::CommandBuilder) -> io::Result<()> {
        let mut pane = TerminalComponent::spawn_default(cmd).map_err(io::Error::other)?;
        pane.set_link_handler_fn(|url| {
            let _ = webbrowser::open(url);
            true
        });
        let mut sv = ScrollViewComponent::new(pane);
        sv.set_keyboard_enabled(false);
        sv.content
            .set_selection_enabled(self.windows.clipboard_enabled());
        let id = self.terminals.len();
        self.terminals.push(sv);
        self.windows.set_focus(id);
        self.windows.tile_window(id);
        self.windows
            .set_window_title(id, format!("Shell {}", id + 1));
        Ok(())
    }
}

impl HasWindowManager<PaneId, PaneId> for App {
    fn windows(&mut self) -> &mut WindowManager<PaneId, PaneId> {
        &mut self.windows
    }

    fn wm_new_window(&mut self) -> io::Result<()> {
        let mut pane =
            TerminalComponent::spawn_default(default_shell_command()).map_err(io::Error::other)?;
        pane.set_link_handler_fn(|url| {
            let _ = webbrowser::open(url);
            true
        });
        let mut sv = ScrollViewComponent::new(pane);
        sv.set_keyboard_enabled(false);
        sv.content
            .set_selection_enabled(self.windows.clipboard_enabled());
        let id = self.terminals.len();
        self.terminals.push(sv);
        self.windows.set_focus(id);
        self.windows.tile_window(id);
        // Set a user-visible title for the newly created pane.
        self.windows
            .set_window_title(id, format!("Shell {}", id + 1));
        Ok(())
    }

    fn wm_close_window(&mut self, id: PaneId) -> io::Result<()> {
        if let Some(sv) = self.terminals.get_mut(id) {
            // TODO: Show confirmation before abrupt termination
            sv.content.terminate();
        }
        Ok(())
    }

    fn set_clipboard_enabled(&mut self, enabled: bool) {
        for sv in &mut self.terminals {
            sv.content.set_selection_enabled(enabled);
        }
    }
}

impl WindowApp<PaneId, PaneId> for App {
    fn enumerate_windows(&mut self) -> Vec<PaneId> {
        self.terminals
            .iter_mut()
            .enumerate()
            .filter_map(|(id, sv)| (!sv.content.has_exited()).then_some(id))
            .collect()
    }

    fn render_window(&mut self, frame: &mut UiFrame<'_>, window: AppWindowDraw<PaneId>) {
        render_pane(frame, self, window.id, window.surface.inner, window.focused);
    }

    fn empty_window_message(&self) -> &str {
        "all shells exited"
    }

    fn window_component(&mut self, id: PaneId) -> Option<&mut dyn Component> {
        self.terminals
            .get_mut(id)
            .map(|sv| sv as &mut dyn Component)
    }
}

fn render_pane(frame: &mut UiFrame<'_>, app: &mut App, id: PaneId, area: Rect, focused: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if let Some(sv) = app.terminals.get_mut(id) {
        // We pass resize to the wrapper Viewport which passes it to content with updated context
        sv.resize(area, &term_wm::components::ComponentContext::new(focused));
        sv.render(
            frame,
            area,
            &term_wm::components::ComponentContext::new(focused),
        );
    }
}
