use std::io;
use std::sync::Arc;

use clap::Parser;
use crossbeam_channel::Sender;

use term_wm::app_context::AppContext;
use term_wm::components::AppRootComponent;
use term_wm::config::AppBuilder;
use term_wm::default_shell_command;
use term_wm::io::RenderTarget;
use term_wm::runner::WindowManagerHost;
use term_wm::term_wm_app::TermWmApp;
use term_wm::unified_event_source::{UnifiedEvent, UnifiedEventSource};
use term_wm_console::console_render_target::ConsoleRenderTarget;
use term_wm_core::components::Component;
use term_wm_core::events::Event;
use term_wm_ui_facade::{LayerComponent, OverlayComponent};

// TODO: Make this user-configurable
const PTY_SCROLLBACK_LEN: usize = 2000;

/// Simple CLI for launching `term-wm` with optional commands / window count.
#[derive(Parser, Debug)]
#[command(
    name = env!("CARGO_PKG_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    about = env!("CARGO_PKG_DESCRIPTION"),
    long_about = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"), ": ", env!("CARGO_PKG_DESCRIPTION")),
)]
struct Cli {
    /// Total number of windows to open (default 2; min 1).
    #[arg(short = 'n', long = "count")]
    count: Option<usize>,

    /// Command to run in a window; repeatable, one window per `--run`.
    #[arg(short = 'r', long = "run", value_name = "CMD", action = clap::ArgAction::Append)]
    run_cmds: Vec<String>,

    /// One command for a window (the whole argv after `--`); it follows any
    /// `--run` windows. Remaining `--count` windows are default shells.
    #[arg(value_name = "CMD", num_args = 0.., trailing_var_arg = true, allow_hyphen_values = true)]
    cmds: Vec<String>,
}

/// Combine repeatable `--run` commands with the single trailing `--` command
/// (joined into one command line). `--run` windows come first.
fn build_commands(run_cmds: Vec<String>, positional: Vec<String>) -> Vec<String> {
    let mut commands = run_cmds;
    if !positional.is_empty() {
        commands.push(positional.join(" "));
    }
    commands
}

/// Total number of windows: explicit commands take precedence over a smaller
/// `-n`; without commands, default to 2 (min 1).
fn total_windows(count: Option<usize>, commands: &[String]) -> usize {
    if commands.is_empty() {
        count.unwrap_or(2).max(1)
    } else {
        commands.len().max(count.unwrap_or(0))
    }
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    let commands = build_commands(cli.run_cmds, cli.cmds);
    let total = total_windows(cli.count, &commands);

    let mut event_source = UnifiedEventSource::new()?;
    let pty_wakeup_tx = event_source.pty_wakeup_tx();
    let mut app = App::new_with(commands, total, pty_wakeup_tx)?;

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
    #[expect(dead_code)]
    pty_wakeup_tx: Sender<UnifiedEvent>,
}

/// Build the window manager the way the `term-wm` binary runs it: full system
/// chrome (top panel, bottom panel, FAB) and NO explicit menu-action allow-list,
/// so the full default action set is available.
fn build_wm(
    app_ctx: &Arc<AppContext>,
) -> term_wm::window::WindowManager<AppRootComponent, LayerComponent, OverlayComponent> {
    let hostname = app_ctx.hostname.as_deref();
    let app_name = app_ctx.app_name.clone();
    let app_version = app_ctx.app_version.clone();
    AppBuilder::<LayerComponent>::new()
        .app_ctx(Arc::clone(app_ctx))
        .top_panel(LayerComponent::TopPanel(
            term_wm_sys_ui_components::WmTopPanelComponent::new(&app_name),
        ))
        .bottom_panel(LayerComponent::BottomPanel(
            term_wm_sys_ui_components::WmBottomPanelComponent::new(
                &app_name,
                &app_version,
                hostname,
            ),
        ))
        .fab(LayerComponent::Fab(
            term_wm_sys_ui_components::WmFabComponent::new(),
        ))
        .build()
        .expect("standalone build")
}

impl App {
    fn new_with(
        commands: Vec<String>,
        num_windows: usize,
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

        let wm = build_wm(&app_ctx);

        let inner = TermWmApp::from_wm(wm, pty_wakeup_tx.clone());
        let mut app = Self {
            inner,
            pty_wakeup_tx,
        };

        // Initialize debug log and system panel windows.
        app.inner.init_system_windows();

        // One window per command (shell + the command as input), then default
        // shells to fill `num_windows`. `commands` is owned and consumed here.
        let mut used = 0;
        for cmd in commands {
            let cb = default_shell_command();
            if let Err(e) = app.spawn_terminal_with_command(cb, Some(cmd)) {
                tracing::error!("Window spawn error: {}", e);
            }
            used += 1;
        }
        for _ in used..num_windows {
            if let Err(e) = app.wm_new_window() {
                tracing::error!("Window spawn error: {}", e);
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

    fn spawn_terminal_with_command(
        &mut self,
        cmd: portable_pty::CommandBuilder,
        command_to_send: Option<String>,
    ) -> io::Result<()> {
        let count = self.inner.wm().window_count() + 1;
        self.inner.spawn_terminal_window(
            cmd,
            PTY_SCROLLBACK_LEN,
            command_to_send,
            format!("Shell {}", count),
        )?;
        Ok(())
    }
}

impl WindowManagerHost<AppRootComponent, LayerComponent, OverlayComponent> for App {
    fn wm(
        &mut self,
    ) -> &mut term_wm::window::WindowManager<AppRootComponent, LayerComponent, OverlayComponent>
    {
        self.inner.wm()
    }

    fn handle_app_event(&mut self, event: &Event) -> bool {
        self.inner.handle_app_event(event)
    }

    fn open_help_overlay(&mut self) {
        self.inner.open_help_overlay();
    }

    fn open_exit_confirm(&mut self) {
        self.inner.open_exit_confirm();
    }

    fn open_command_palette(&mut self) {
        self.inner.open_command_palette();
    }

    fn on_panic(&mut self) {
        self.inner.on_panic();
    }

    fn toggle_debug_window(&mut self) {
        self.inner.toggle_debug_window();
    }

    fn toggle_system_panel(&mut self) {
        self.inner.toggle_system_panel();
    }

    fn wm_new_window(&mut self) -> io::Result<()> {
        let count = self.inner.wm().window_count() + 1;
        self.inner.spawn_terminal_window(
            default_shell_command(),
            PTY_SCROLLBACK_LEN,
            None,
            format!("Shell {}", count),
        )?;
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

    fn render(&mut self, backend: &mut dyn term_wm_render::RenderBackend) {
        self.inner.render_app(backend);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_build_wm_gets_full_default_menu_actions() {
        let app_ctx = Arc::new(AppContext::new("term-wm", "0.0.0").with_hostname("test-host"));
        let wm = build_wm(&app_ctx);
        assert_eq!(
            wm.supported_menu_actions(),
            term_wm::constants::DEFAULT_SUPPORTED_MENU_ACTIONS,
            "the term-wm binary must not restrict the command-palette actions to a subset"
        );
    }

    #[test]
    fn build_commands_appends_joined_positional_after_run() {
        let commands = build_commands(
            vec!["vim -l".into(), "htop".into()],
            vec!["git".into(), "log".into(), "--oneline".into()],
        );
        assert_eq!(commands, vec!["vim -l", "htop", "git log --oneline"]);
    }

    #[test]
    fn build_commands_positional_only_is_single_command() {
        let commands = build_commands(vec![], vec!["ls".into(), "-la".into()]);
        assert_eq!(commands, vec!["ls -la"]);
    }

    #[test]
    fn build_commands_run_only() {
        let commands = build_commands(vec!["top".into()], vec![]);
        assert_eq!(commands, vec!["top"]);
    }

    #[test]
    fn build_commands_none() {
        assert!(build_commands(vec![], vec![]).is_empty());
    }

    #[test]
    fn total_windows_defaults_to_two_without_commands() {
        assert_eq!(total_windows(None, &[]), 2);
    }

    #[test]
    fn total_windows_count_without_commands() {
        assert_eq!(total_windows(Some(4), &[]), 4);
    }

    #[test]
    fn total_windows_zero_count_without_commands_clamps_to_one() {
        assert_eq!(total_windows(Some(0), &[]), 1);
    }

    #[test]
    fn total_windows_commands_take_precedence_over_smaller_count() {
        let cmds = vec!["a".into(), "b".into()];
        assert_eq!(total_windows(Some(1), &cmds), 2);
        // `-n 0` with commands still opens one window per command.
        assert_eq!(total_windows(Some(0), &cmds), 2);
    }

    #[test]
    fn total_windows_count_expands_beyond_commands() {
        let cmds = vec!["a".into()];
        assert_eq!(total_windows(Some(4), &cmds), 4);
    }
}
