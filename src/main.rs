use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;

use clap::Parser;
use line_ending::LineEnding;
use ratatui::prelude::Rect;

use crossbeam_channel::Sender;

use term_wm::app_context::AppContext;
use term_wm::components::MenuOverlay;
use term_wm::components::{Component, ComponentContext};
use term_wm::config::WmBuilder;
use term_wm::io::{
    RenderTarget,
    console::ConsoleRenderTarget,
    unified_event_source::{UnifiedEvent, UnifiedEventSource},
};
use term_wm::runner::{WindowManagerHost, WindowProvider, run_window_app};
use term_wm::ui::UiFrame;
use term_wm::window::{OverlayId, WindowDrawContext, WindowKey, WindowManager};
use term_wm::{ScrollViewComponent, TerminalComponent, default_shell_command};
use term_wm_sys_ui_components::wm_debug_log::{
    WmDebugLogComponent, install_panic_hook, set_global_debug_log,
};

/// Simple CLI for launching `term-wm` with optional commands / window count.
#[derive(Parser, Debug)]
#[command(
    name = env!("CARGO_PKG_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    about = env!("CARGO_PKG_DESCRIPTION"),
    long_about = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"), ": ", env!("CARGO_PKG_DESCRIPTION")),
)]
struct Cli {
    #[arg(short = 'n', long = "count")]
    count: Option<usize>,
    #[arg(value_name = "CMD", num_args = 0..)]
    cmds: Vec<String>,
    #[arg(long)]
    embedded: bool,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    let total = if cli.cmds.is_empty() {
        cli.count.unwrap_or(2).max(1)
    } else {
        cli.count.map(|c| c.max(1)).unwrap_or_else(|| {
            cli.cmds.len().max(1)
        })
    };
    let mut event_source = UnifiedEventSource::new()?;
    let pty_wakeup_tx = event_source.pty_wakeup_tx();
    let mut app = App::new_with(cli.cmds, total, cli.embedded, pty_wakeup_tx)?;
    let mut output = ConsoleRenderTarget::new()?;
    output.enter()?;

    let result = run_window_app(&mut output, &mut event_source, &mut app);

    output.exit()?;

    result
}

struct App {
    windows: WindowManager,
    terminals: BTreeMap<WindowKey, ScrollViewComponent<TerminalComponent>>,
    pty_wakeup_tx: Sender<UnifiedEvent>,
    debug_key: Option<WindowKey>,
    debug_visible: bool,
}

impl App {
    fn new_with(commands: Vec<String>, num_windows: usize, embedded: bool, pty_wakeup_tx: Sender<UnifiedEvent>) -> io::Result<Self> {
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
            dyn term_wm_core::top_panel_trait::TopPanel<WindowKey>,
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
                Some(top_panel),
                Some(bottom_panel),
                Some(menu_overlay),
            ),
            terminals: BTreeMap::new(),
            pty_wakeup_tx,
            debug_key: None,
            debug_visible: false,
        };

        // Initialize debug log system window
        {
            let (mut component, handle) = WmDebugLogComponent::new_default();
            component.set_selection_enabled(app.windows.clipboard_enabled());
            set_global_debug_log(handle);
            let debug_key = app.windows.set_system_window(Box::new(component));
            app.debug_key = Some(debug_key);
            app.windows.set_window_title(debug_key, "Debug Log");
            install_panic_hook();
            term_wm::tracing_sub::init_default();
        }

        let mut error_occurred = false;

        if !commands.is_empty() {
            let mut it = commands.into_iter();
            for _ in 0..num_windows {
                if let Some(cmd) = it.next() {
                    let cb = default_shell_command();
                    if let Err(e) = app.spawn_terminal_with_command(cb) {
                        tracing::error!("Window spawn error: {}", e);
                        error_occurred = true;
                    }
                    if !error_occurred {
                        if let Some(key) = app.terminals.keys().last().copied() {
                            if let Some(ref mut pane) = app.terminals.get_mut(&key) {
                                let mut line = cmd;
                                line.push_str(LineEnding::from_current_platform().as_str());
                                let _ = pane.content.write_bytes(line.as_bytes());
                            }
                        }
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
            // Debug window already created — include it in enumerate_windows
            // by ensuring debug_key is set.
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
        let key = self.windows.create_window();
        let tx = self.pty_wakeup_tx.clone();
        sv.content.set_wakeup(Some(std::sync::Arc::new(move || {
            let _ = tx.send(UnifiedEvent::PtyWakeup(key));
        })));
        self.terminals.insert(key, sv);
        self.windows.set_focus(key);
        self.windows.tile_window(key);
        self.windows
            .set_window_title(key, format!("Shell {}", self.terminals.len()));
        Ok(())
    }
}

impl WindowManagerHost for App {
    fn windows(&mut self) -> &mut WindowManager {
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

    fn on_panic(&mut self) {
        self.debug_visible = true;
    }

    fn toggle_debug_window(&mut self) {
        self.debug_visible = !self.debug_visible;
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
        let key = self.windows.create_window();
        let tx = self.pty_wakeup_tx.clone();
        sv.content.set_wakeup(Some(std::sync::Arc::new(move || {
            let _ = tx.send(UnifiedEvent::PtyWakeup(key));
        })));
        self.terminals.insert(key, sv);
        self.windows.set_focus(key);
        self.windows.tile_window(key);
        self.windows
            .set_window_title(key, format!("Shell {}", self.terminals.len()));
        Ok(())
    }

    fn wm_close_window(&mut self, key: WindowKey) -> io::Result<()> {
        if self.debug_key == Some(key) {
            self.debug_visible = false;
            return Ok(());
        }
        if let Some(mut sv) = self.terminals.remove(&key) {
            sv.content.terminate();
            if let Some((child, reader_handle)) = sv.content.take_parts() {
                self.windows.reaper().reap(
                    term_wm::reaper::ZombieChild::new(child, reader_handle),
                );
            }
        }
        Ok(())
    }

    fn set_clipboard_enabled(&mut self, _enabled: bool) {}

    fn set_window_selection_enabled(&mut self, enabled: bool) {
        for sv in self.terminals.values_mut() {
            sv.content.set_selection_enabled(enabled);
        }
    }
}

impl WindowProvider for App {
    fn enumerate_windows(&mut self) -> Vec<WindowKey> {
        let mut keys: Vec<WindowKey> = self.terminals
            .iter_mut()
            .filter_map(|(key, sv)| {
                if sv.content.has_exited() {
                    None
                } else {
                    Some(*key)
                }
            })
            .collect();
        if self.debug_visible {
            if let Some(debug_key) = self.debug_key {
                keys.push(debug_key);
            }
        }
        keys
    }

    fn render_window(
        &mut self,
        frame: &mut UiFrame<'_>,
        window: WindowDrawContext,
        ctx: &ComponentContext,
    ) {
        if Some(window.id) == self.debug_key {
            return; // rendered by WindowManager via component_for_key
        }
        render_pane(frame, self, window.id, window.surface.inner, ctx.clone());
    }

    fn empty_window_message(&self) -> &str {
        "all shells exited"
    }

    fn window_component(&mut self, key: WindowKey) -> Option<&mut dyn Component> {
        if let Some(sv) = self.terminals.get_mut(&key) {
            return Some(sv as &mut dyn Component);
        }
        self.windows.component_for_key(key)
    }

    fn window_pane_title(&mut self, key: WindowKey) -> Option<String> {
        self.terminals
            .get_mut(&key)
            .and_then(|sv| sv.content.take_pending_title())
    }
}

fn render_pane(
    frame: &mut UiFrame<'_>,
    app: &mut App,
    key: WindowKey,
    area: Rect,
    ctx: ComponentContext,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if let Some(sv) = app.terminals.get_mut(&key) {
        sv.resize(area, &ctx);
        sv.render(frame, area, &ctx);
    }
}
