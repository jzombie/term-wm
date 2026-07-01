//! This default application is a simple terminal app, which opens two sub-shells in side-by-side
//! windows, where more windows can be added, or windows can be removed.

// TODO: Add mode to auto-open debug window

use std::io;
use std::sync::Arc;

use clap::Parser;
use line_ending::LineEnding;
use ratatui::prelude::Rect;

use term_wm::app_context::AppContext;
use term_wm::components::MenuOverlay;
use term_wm::components::{Component, ComponentContext};
use term_wm::config::WmBuilder;
use term_wm::io::{
    RenderTarget,
    console::{ConsoleEventSource, ConsoleRenderTarget},
};
use term_wm::runner::{WindowManagerHost, WindowProvider, run_window_app};
use term_wm::ui::UiFrame;
use term_wm::window::{OverlayId, SystemWindowId, WindowDrawContext, WindowManager};
use term_wm::{ScrollViewComponent, TerminalComponent, default_shell_command};
use term_wm_sys_ui_components::wm_debug_log::{
    WmDebugLogComponent, install_panic_hook, set_global_debug_log,
};

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

    /// Run in embedded mode (no chrome, no floating windows, no WM overlay).
    #[arg(long)]
    embedded: bool,
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
    let mut app = App::new_with(cli.cmds, total, cli.embedded)?;
    let mut output = ConsoleRenderTarget::new()?;
    output.enter()?;
    let mut input = ConsoleEventSource::new();

    let result = run_window_app(&mut output, &mut input, &mut app);

    output.exit()?;

    result
}

struct App {
    windows: WindowManager<PaneId>,
    terminals: Vec<ScrollViewComponent<TerminalComponent>>,
}

impl App {
    fn new_with(commands: Vec<String>, num_windows: usize, embedded: bool) -> io::Result<Self> {
        let app_ctx = Arc::new(
            AppContext::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")).with_hostname(
                &hostname::get()
                    .ok()
                    .and_then(|s| s.into_string().ok())
                    .unwrap_or_else(|| "unknown-host".to_string()),
            ),
        );
        let hostname = app_ctx.hostname.as_deref();
        let top_panel: Box<
            dyn term_wm_core::top_panel_trait::TopPanel<term_wm_core::window::WindowId<usize>>,
        > = Box::new(term_wm_sys_ui_components::WmTopPanelComponent::new(
            &app_ctx.app_name,
        ));
        let bottom_panel: Box<dyn term_wm_core::bottom_panel_trait::BottomPanel> =
            Box::new(term_wm_sys_ui_components::WmBottomPanelComponent::new(
                &app_ctx.app_name,
                &app_ctx.app_version,
                hostname,
            ));
        let builder = if embedded {
            WmBuilder::embedded()
        } else {
            WmBuilder::standalone()
        };
        let config = builder.config().clone();
        let mut raw_menu = term_wm_sys_ui_components::WmMenuOverlay::new();
        raw_menu.set_timeout(config.menu_outline_timeout);
        let menu_overlay: Box<dyn MenuOverlay<term_wm_core::window::WmMenuAction>> = if embedded {
            Box::new(term_wm_sys_ui_components::WmMenuOverlay::new())
        } else {
            Box::new(raw_menu)
        };
        let mut app = Self {
            windows: builder.app_ctx(Arc::clone(&app_ctx)).build(
                0,
                Some(top_panel),
                Some(bottom_panel),
                Some(menu_overlay),
            ),
            terminals: Vec::new(),
        };

        // Initialize debug log system window
        {
            let (mut component, handle) = WmDebugLogComponent::new_default();
            component.set_selection_enabled(app.windows.clipboard_enabled());
            set_global_debug_log(handle);
            app.windows
                .set_system_window(SystemWindowId::DebugLog, Box::new(component));
            install_panic_hook();
            term_wm::tracing_sub::init_default();
        }

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
            app.windows().open_debug_window();
        }

        app.open_help_overlay();
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

impl WindowManagerHost<PaneId> for App {
    fn windows(&mut self) -> &mut WindowManager<PaneId> {
        &mut self.windows
    }

    fn open_help_overlay(&mut self) {
        use term_wm_sys_ui_components::wm_help_overlay::WmHelpOverlayComponent;
        let kb = self.windows.keybindings().clone();
        let mut h = WmHelpOverlayComponent::new(self.windows.app_ctx(), kb);
        h.show();
        h.set_selection_enabled(self.windows.clipboard_enabled());
        self.windows
            .open_overlay(OverlayId::Help, Some(Box::new(h)));
    }

    fn open_keybindings_overlay(&mut self) {
        use term_wm_sys_ui_components::wm_keybinding_overlay::WmKeybindingOverlayComponent;
        let kb = self.windows.keybindings().clone();
        let mut o = WmKeybindingOverlayComponent::new(self.windows.app_ctx(), kb);
        o.show();
        self.windows
            .open_overlay(OverlayId::Keybindings, Some(Box::new(o)));
    }

    fn open_exit_confirm(&mut self) {
        use term_wm_ui_components::confirm_overlay::ConfirmOverlayComponent;
        let mut confirm = ConfirmOverlayComponent::new();
        confirm.open(
            "Exit App",
            "Exit the application?\nUnsaved changes will be lost.",
        );
        self.windows
            .open_overlay(OverlayId::ExitConfirm, Some(Box::new(confirm)));
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
            sv.content.terminate();
        }
        Ok(())
    }

    fn set_clipboard_enabled(&mut self, _enabled: bool) {}

    fn set_window_selection_enabled(&mut self, enabled: bool) {
        for sv in &mut self.terminals {
            sv.content.set_selection_enabled(enabled);
        }
    }
}

impl WindowProvider<PaneId> for App {
    fn enumerate_windows(&mut self) -> Vec<PaneId> {
        self.terminals
            .iter_mut()
            .enumerate()
            .filter_map(|(id, sv)| (!sv.content.has_exited()).then_some(id))
            .collect()
    }

    fn render_window(
        &mut self,
        frame: &mut UiFrame<'_>,
        window: WindowDrawContext<PaneId>,
        ctx: &ComponentContext,
    ) {
        render_pane(frame, self, window.id, window.surface.inner, ctx.clone());
    }

    fn empty_window_message(&self) -> &str {
        "all shells exited"
    }

    fn window_component(&mut self, id: PaneId) -> Option<&mut dyn Component> {
        self.terminals
            .get_mut(id)
            .map(|sv| sv as &mut dyn Component)
    }

    fn window_pane_title(&mut self, id: PaneId) -> Option<String> {
        self.terminals
            .get_mut(id)
            .and_then(|sv| sv.content.take_pending_title())
    }
}

fn render_pane(
    frame: &mut UiFrame<'_>,
    app: &mut App,
    id: PaneId,
    area: Rect,
    ctx: ComponentContext,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if let Some(sv) = app.terminals.get_mut(id) {
        sv.resize(area, &ctx);
        sv.render(frame, area, &ctx);
    }
}
