use std::io;
use std::sync::{Arc, OnceLock};

use clap::Parser;
use crossbeam_channel::Sender;

use term_wm::app_context::AppContext;
use term_wm::config::AppBuilder;
use term_wm::io::RenderTarget;
use term_wm::runner::WindowManagerHost;
use term_wm::term_wm_app::TermWmApp;
use term_wm::unified_event_source::{UnifiedEvent, UnifiedEventSource};
use term_wm::window::{OverlayId, WindowKey, wm_menu_items};
use term_wm::wm_config::WmConfig;
use term_wm::{
    PtyStatus, ScrollKeyMode, ScrollViewComponent, TerminalComponent, default_shell_command,
};
use term_wm_console::console_render_target::ConsoleRenderTarget;
use term_wm_sys_ui_components::WmSystemPanelComponent;
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

    // Determine total number of windows to open by default.
    //
    // Behavior:
    // - If no commands provided: open `--count` shells (default 2 if not given).
    // - If commands provided: if `--count` given use it, otherwise default to
    //   the number of commands.
    let total = if cli.cmds.is_empty() {
        cli.count.unwrap_or(2).max(1)
    } else {
        // default to number of commands when count not given
        cli.count
            .map(|c| c.max(1))
            .unwrap_or_else(|| cli.cmds.len().max(1))
    };

    let mut event_source = UnifiedEventSource::new()?;
    let pty_wakeup_tx = event_source.pty_wakeup_tx();
    let mut app = App::new_with(cli.cmds, total, cli.embedded, pty_wakeup_tx)?;

    let mut output = ConsoleRenderTarget::new()?;
    output.enter()?;
    let result = app.run_with(&mut output, &mut event_source);
    output.exit()?;
    result
}

/// Terminal-focused app that wraps [`TermWmApp`] and adds PTY session
/// management, debug window, and system overlays.
struct App {
    inner: TermWmApp,
    debug_key: Option<WindowKey>,
    debug_visible: bool,
    system_panel_key: Option<WindowKey>,
    system_panel_visible: bool,
    pty_wakeup_tx: Sender<UnifiedEvent>,
}

impl App {
    fn new_with(
        commands: Vec<String>,
        num_windows: usize,
        embedded: bool,
        pty_wakeup_tx: Sender<UnifiedEvent>,
    ) -> io::Result<Self> {
        let app_ctx = Arc::new(
            AppContext::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")).with_hostname(
                &hostname::get()
                    .ok()
                    .and_then(|s| s.into_string().ok())
                    .unwrap_or_else(|| "unknown-host".to_string()),
            ),
        );
        let hostname = app_ctx.hostname.as_deref();
        let app_name = app_ctx.app_name.clone();
        let app_version = app_ctx.app_version.clone();

        let wm = if embedded {
            AppBuilder::bare()
                .config(WmConfig::minimal())
                .app_ctx(Arc::clone(&app_ctx))
                .build()
                .expect("embedded build")
        } else {
            AppBuilder::bare()
                .app_ctx(Arc::clone(&app_ctx))
                .top_panel(Box::new(
                    term_wm_sys_ui_components::WmTopPanelComponent::new(&app_name),
                ))
                .bottom_panel(Box::new(
                    term_wm_sys_ui_components::WmBottomPanelComponent::new(
                        &app_name,
                        &app_version,
                        hostname,
                    ),
                ))
                .fab(Box::new(term_wm_sys_ui_components::WmFabComponent::new()))
                .build()
                .expect("standalone build")
        };

        let inner = TermWmApp::from_wm(wm);
        let mut app = Self {
            inner,
            debug_key: None,
            debug_visible: false,
            system_panel_key: None,
            system_panel_visible: false,
            pty_wakeup_tx,
        };

        // Initialize debug log system window
        {
            let (mut component, handle) = WmDebugLogComponent::new_default();
            component.set_selection_enabled(app.inner.wm().clipboard_enabled());
            set_global_debug_log(handle);
            let debug_key = app.inner.wm().set_system_window(Box::new(component));
            app.inner
                .wm()
                .transition_window(debug_key, term_wm::window::WindowState::Unmapped);
            app.debug_key = Some(debug_key);
            app.inner.wm().set_window_title(debug_key, "Debug Log");
            install_panic_hook();
            term_wm::tracing_sub::init_default();
        }

        // Initialize system panel system window
        {
            let component = WmSystemPanelComponent::new();
            let key = app.inner.wm().set_system_window(Box::new(component));
            app.inner
                .wm()
                .transition_window(key, term_wm::window::WindowState::Unmapped);
            app.system_panel_key = Some(key);
            app.inner.wm().set_window_title(key, "System Panel");
        }

        // If commands provided, open one per command; otherwise open `num_windows`
        // shells using the default shell.
        if !commands.is_empty() {
            let mut it = commands.into_iter();
            for _ in 0..num_windows {
                if let Some(cmd) = it.next() {
                    // Spawn an interactive shell and send the command as input so
                    // that when the command exits the shell remains.
                    let cb = default_shell_command();
                    if let Err(e) = app.spawn_terminal_with_command(cb, Some(cmd)) {
                        tracing::error!("Window spawn error: {}", e);
                    }
                } else if let Err(e) = app.wm_new_window() {
                    tracing::error!("Window spawn error: {}", e);
                }
            }
        } else {
            for _ in 0..num_windows {
                if let Err(e) = app.wm_new_window() {
                    tracing::error!("Window spawn error: {}", e);
                }
            }
        }

        app.open_help_overlay();
        Ok(app)
    }

    fn run_with<O, D>(&mut self, output: &mut O, driver: &mut D) -> io::Result<()>
    where
        O: term_wm::io::RenderTarget,
        D: term_wm::io::EventSource,
    {
        term_wm::runner::run_with_defaults(output, driver, self)
    }

    // TODO: Extract to a reusable place
    // TODO: Dedupe this and `wm_new_window`
    fn spawn_terminal_with_command(
        &mut self,
        cmd: portable_pty::CommandBuilder,
        command_to_send: Option<String>,
    ) -> io::Result<()> {
        // Configure the terminal BEFORE boxing (type erasure trap)
        let mut pane = TerminalComponent::spawn_default(cmd).map_err(io::Error::other)?;
        pane.set_link_handler_fn(|url| {
            let _ = webbrowser::open(url);
            true
        });

        let key_holder = Arc::new(OnceLock::new());
        let kh = key_holder.clone();
        let tx = self.pty_wakeup_tx.clone();
        pane.set_status_callback(Some(Box::new(move |status| match status {
            PtyStatus::Wakeup => {
                if let Some(&key) = kh.get() {
                    let _ = tx.send(UnifiedEvent::PtyWakeup(key));
                }
            }
            PtyStatus::Exited => {
                if let Some(&key) = kh.get() {
                    let _ = tx.send(UnifiedEvent::AppExited(key));
                }
            }
        })));
        let mut sv = ScrollViewComponent::new(pane);
        sv.set_keyboard_mode(ScrollKeyMode::PaginationOnly);
        let wm = self.inner.wm();
        let key = wm.create_window(Box::new(sv));
        wm.transition_window(key, term_wm::window::WindowState::Mapped);

        // The key is now known — store it so the callback can use it.
        let _ = key_holder.set(key);

        // Enable selection for the new terminal.
        let clipboard_enabled = wm.clipboard_enabled();
        if let Some(comp) = wm.component_for_key_mut(key) {
            comp.set_selection_enabled(clipboard_enabled);
        }

        // Inject boot-time command via the `paste` trait method.
        if let Some(line) = command_to_send {
            let mut line = line;
            line.push_str(line_ending::LineEnding::from_current_platform().as_str());
            if let Some(comp) = wm.component_for_key_mut(key) {
                let _ = comp.paste(&line);
            }
        }

        wm.set_focus(key);
        wm.tile_window(key);
        wm.set_window_title(key, format!("Shell {}", wm.window_count()));
        Ok(())
    }
}

impl WindowManagerHost for App {
    fn wm(&mut self) -> &mut term_wm::window::WindowManager {
        self.inner.wm()
    }

    fn open_help_overlay(&mut self) {
        use term_wm_sys_ui_components::wm_help_overlay::WmHelpOverlayComponent;
        let wm = self.inner.wm();
        let kb = wm.keybindings().clone();
        let mut h = WmHelpOverlayComponent::new(wm.app_ctx(), kb);
        h.show();
        h.set_selection_enabled(wm.clipboard_enabled());
        wm.open_overlay(OverlayId::Help, Some(Box::new(h)));
    }

    fn open_exit_confirm(&mut self) {
        use term_wm_ui_components::confirm_overlay::ConfirmOverlayComponent;
        let mut confirm = ConfirmOverlayComponent::new();
        confirm.open(
            "Exit App",
            "Exit the application?\nUnsaved changes will be lost.",
        );
        self.inner
            .wm()
            .open_overlay(OverlayId::ExitConfirm, Some(Box::new(confirm)));
    }

    fn open_command_palette(&mut self) {
        use term_wm_sys_ui_components::wm_command_palette::WmCommandPaletteComponent;
        let wm = self.inner.wm();
        let mut palette = WmCommandPaletteComponent::new();
        palette.show();
        // Set menu items based on current state
        let items = wm_menu_items(
            wm.mouse_capture_enabled(),
            wm.clipboard_enabled(),
            wm.window_selection_enabled(),
            wm.window_count() > 0,
        );
        let supported = wm.supported_menu_actions();
        let items: Vec<_> = items.into_iter().filter(|item| supported.contains(&item.action)).collect();
        palette.set_items(items);
        wm.open_overlay(OverlayId::CommandPalette, Some(Box::new(palette)));
    }

    fn on_panic(&mut self) {
        self.debug_visible = true;
        if let Some(key) = self.debug_key {
            self.inner
                .wm()
                .transition_window(key, term_wm::window::WindowState::Mapped);
        }
    }

    fn toggle_debug_window(&mut self) {
        self.debug_visible = !self.debug_visible;
        if let Some(key) = self.debug_key {
            let state = if self.debug_visible {
                term_wm::window::WindowState::Mapped
            } else {
                term_wm::window::WindowState::Unmapped
            };
            self.inner.wm().transition_window(key, state);
        }
    }

    fn toggle_system_panel(&mut self) {
        self.system_panel_visible = !self.system_panel_visible;
        if let Some(key) = self.system_panel_key {
            let state = if self.system_panel_visible {
                term_wm::window::WindowState::Mapped
            } else {
                term_wm::window::WindowState::Unmapped
            };
            self.inner.wm().transition_window(key, state);
        }
    }

    fn wm_new_window(&mut self) -> io::Result<()> {
        let mut pane =
            TerminalComponent::spawn_default(default_shell_command()).map_err(io::Error::other)?;
        pane.set_link_handler_fn(|url| {
            let _ = webbrowser::open(url);
            true
        });
        let key_holder = Arc::new(OnceLock::new());
        let kh = key_holder.clone();
        let tx = self.pty_wakeup_tx.clone();
        pane.set_status_callback(Some(Box::new(move |status| match status {
            PtyStatus::Wakeup => {
                if let Some(&key) = kh.get() {
                    let _ = tx.send(UnifiedEvent::PtyWakeup(key));
                }
            }
            PtyStatus::Exited => {
                if let Some(&key) = kh.get() {
                    let _ = tx.send(UnifiedEvent::AppExited(key));
                }
            }
        })));
        let mut sv = ScrollViewComponent::new(pane);
        sv.set_keyboard_mode(ScrollKeyMode::PaginationOnly);
        let wm = self.inner.wm();
        let key = wm.create_window(Box::new(sv));
        wm.transition_window(key, term_wm::window::WindowState::Mapped);

        // The key is now known — store it so the callback can use it.
        let _ = key_holder.set(key);

        // Enable selection for the new terminal.
        let clipboard_enabled = wm.clipboard_enabled();
        if let Some(comp) = wm.component_for_key_mut(key) {
            comp.set_selection_enabled(clipboard_enabled);
        }

        wm.set_focus(key);
        wm.tile_window(key);
        wm.set_window_title(key, format!("Shell {}", wm.window_count()));
        Ok(())
    }

    fn wm_close_window(&mut self, key: WindowKey) -> io::Result<()> {
        if self.debug_key == Some(key) {
            self.debug_visible = false;
            self.inner
                .wm()
                .transition_window(key, term_wm::window::WindowState::Unmapped);
            return Ok(());
        }
        if self.system_panel_key == Some(key) {
            self.system_panel_visible = false;
            self.inner
                .wm()
                .transition_window(key, term_wm::window::WindowState::Unmapped);
            return Ok(());
        }
        // Call destroy on the component (kills child process).
        // The component will be dropped after this, and the OS will
        // clean up the child process. See also: Reaper for async reaping.
        if let Some(comp) = self.inner.wm().component_for_key_mut(key) {
            comp.destroy();
        }
        Ok(())
    }

    fn set_clipboard_enabled(&mut self, _enabled: bool) {}

    fn set_window_selection_enabled(&mut self, enabled: bool) {
        for key in self.inner.wm().all_window_keys() {
            if let Some(comp) = self.inner.wm().component_for_key_mut(key) {
                comp.set_selection_enabled(enabled);
            }
        }
    }

    fn empty_window_message(&self) -> &str {
        "all shells exited"
    }

    fn render(&mut self, backend: &mut dyn term_wm_render::RenderBackend) {
        self.inner.render_app(backend);
    }
}
