use std::io;
use std::sync::Arc;

use clap::Parser;
use crossbeam_channel::Sender;

use term_wm::app_context::AppContext;
use term_wm::components::AppRootComponent;
use term_wm::components::NoopComponent;
use term_wm::config::AppBuilder;
use term_wm::default_shell_command;
use term_wm::io::RenderTarget;
use term_wm::runner::WindowManagerHost;
use term_wm::term_wm_app::TermWmApp;
use term_wm::unified_event_source::{UnifiedEvent, UnifiedEventSource};
use term_wm_console::console_render_target::ConsoleRenderTarget;
use term_wm_core::components::Component;
use term_wm_core::events::Event;
use term_wm_core::wm_config::WmConfig;
use term_wm_ui_facade::{LayerComponent, OverlayComponent};

/// Simple CLI for launching `term-wm` with optional commands / window count.
#[derive(Parser, Debug)]
#[command(
    name = env!("CARGO_PKG_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    about = env!("CARGO_PKG_DESCRIPTION"),
    long_about = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"), ": ", env!("CARGO_PKG_DESCRIPTION")),
)]
struct Cli {
    /// Total number of windows to open (default 2; min 1). Only takes affect on new sessions.
    #[arg(short = 'n', long = "count")]
    count: Option<usize>,

    /// Scrollback buffer size per terminal window (default 2000). Only takes affect on new sessions.
    #[arg(long = "scrollback", default_value_t = term_wm_core::constants::DEFAULT_SCROLLBACK_LEN)]
    scrollback: usize,

    /// Command to run in a window; repeatable, one window per `--run`. Only takes affect on new sessions.
    #[arg(short = 'r', long = "run", value_name = "CMD", action = clap::ArgAction::Append)]
    run_cmds: Vec<String>,

    /// One command for a window (the whole argv after `--`); it follows any
    /// `--run` windows. Remaining `--count` windows are default shells. Only takes affect on new sessions.
    #[arg(value_name = "CMD", num_args = 0.., trailing_var_arg = true, allow_hyphen_values = true)]
    cmds: Vec<String>,

    /// Workspace name (default: "default")
    #[arg(short = 'w', long = "workspace", default_value = "default")]
    workspace: String,

    /// Run without window manager (headless session client mode)
    #[arg(long = "no-wm")]
    no_wm: bool,

    /// Run as a standalone session daemon (gateway)
    #[arg(long = "daemon", hide = true)]
    daemon: bool,

    /// Hidden flag: running inside a daemon-managed persistent PTY channel
    #[arg(long = "internal-session", hide = true)]
    internal_session: bool,

    /// Stop the running background session daemon
    #[arg(long = "stop-daemon")]
    stop_daemon: bool,

    /// Force stop daemon or kill channels even if sessions/participants are active
    #[arg(long = "force", short = 'f')]
    force: bool,
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

    // 0. Stop daemon
    #[cfg(feature = "session-persistence")]
    if cli.stop_daemon {
        return term_session::stop_gateway(cli.force);
    }

    // 1. Standalone daemon mode
    #[cfg(feature = "session-persistence")]
    if cli.daemon {
        let gateway = term_session::auto_spawn::resolve_gateway();
        let rt = tokio::runtime::Runtime::new()?;
        return rt
            .block_on(term_session::server::run_gateway(gateway))
            .map(|_| ())
            .map_err(|e| io::Error::other(format!("{e}")));
    }

    // 2. Headless client mode (no WM chrome)
    #[cfg(feature = "session-persistence")]
    if cli.no_wm {
        let socket = term_session::auto_spawn::connect_or_spawn_server(None)?;
        let channel = format!("{}/main", cli.workspace);
        return term_session::client::run_session(&socket, &channel, &cli.cmds).map(|_| ());
    }

    // 3. Outer launcher with workspace rebind loop
    #[cfg(feature = "session-persistence")]
    if !cli.internal_session {
        let socket_path = term_session::auto_spawn::connect_or_spawn_server(None)?;
        let mut current_workspace = cli.workspace.clone();

        loop {
            let channel = format!("{}/main", current_workspace);

            let current_exe = std::env::current_exe()?.to_string_lossy().into_owned();

            let mut inner_cmd = vec![
                current_exe,
                "--internal-session".to_string(),
                "-w".to_string(),
                current_workspace.clone(),
            ];
            if let Some(count) = cli.count {
                inner_cmd.push("-n".to_string());
                inner_cmd.push(count.to_string());
            }
            if cli.scrollback != term_wm_core::constants::DEFAULT_SCROLLBACK_LEN {
                inner_cmd.push("--scrollback".to_string());
                inner_cmd.push(cli.scrollback.to_string());
            }
            for run_cmd in &cli.run_cmds {
                inner_cmd.push("--run".to_string());
                inner_cmd.push(run_cmd.clone());
            }
            if !cli.cmds.is_empty() {
                inner_cmd.push("--".to_string());
                inner_cmd.extend(cli.cmds.clone());
            }

            match term_session::client::run_session(&socket_path, &channel, &inner_cmd)? {
                Some(target_workspace) => {
                    current_workspace = target_workspace;
                    continue;
                }
                None => return Ok(()),
            }
        }
    }

    // 4. Inner session execution (inside daemon PTY or persistence disabled)
    let commands = build_commands(cli.run_cmds, cli.cmds);
    let total = total_windows(cli.count, &commands);

    #[cfg(feature = "session-persistence")]
    let rt = tokio::runtime::Runtime::new()?;
    #[cfg(feature = "session-persistence")]
    let _rt_guard = rt.enter();

    let config = WmConfig {
        scrollback_lines: cli.scrollback,
        ..Default::default()
    };

    let mut event_source = UnifiedEventSource::new()?;
    let pty_wakeup_tx = event_source.pty_wakeup_tx();
    let mut app = App::new_with(commands, total, config, pty_wakeup_tx, cli.workspace)?;

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
    /// Current workspace name for IPC source_channel identification.
    #[allow(dead_code, reason = "used only with session-persistence")]
    current_workspace: String,
}

/// Build the window manager the way the `term-wm` binary runs it: full system
/// chrome (top panel, bottom panel, FAB) and NO explicit menu-action allow-list,
/// so the full default action set is available.
fn build_wm(
    app_ctx: &Arc<AppContext>,
    config: WmConfig,
) -> term_wm::window::WindowManager<AppRootComponent, LayerComponent, OverlayComponent> {
    let hostname = app_ctx.hostname.as_deref();
    let app_name = app_ctx.app_name.clone();
    let app_version = app_ctx.app_version.clone();
    AppBuilder::<LayerComponent>::new()
        .config(config)
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
        config: WmConfig,
        pty_wakeup_tx: Sender<UnifiedEvent>,
        workspace: String,
    ) -> io::Result<Self> {
        let app_ctx = Arc::new(
            AppContext::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")).with_hostname(
                &hostname::get()
                    .ok()
                    .and_then(|s| s.into_string().ok())
                    .unwrap_or_else(|| "unknown-host".to_string()),
            ),
        );

        let wm = build_wm(&app_ctx, config);

        let mut inner = TermWmApp::from_wm(wm, pty_wakeup_tx.clone());
        #[cfg(feature = "session-persistence")]
        inner.set_current_workspace(workspace.clone());
        let mut app = Self {
            inner,
            pty_wakeup_tx,
            current_workspace: workspace,
        };

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
            if let Err(e) = app.wm_new_terminal() {
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
        self.inner
            .spawn_terminal_window(cmd, command_to_send, format!("Shell {}", count))?;
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

    fn handle_custom_action(&mut self, action: &term_wm_core::actions::TermWmAction) -> bool {
        use term_wm_core::actions::TermWmAction;
        match action {
            TermWmAction::SwitchWorkspace(_target) => {
                #[cfg(feature = "session-persistence")]
                {
                    let source_channel = format!("{}/main", self.current_workspace);
                    if let Err(e) = term_session::request_workspace_rebind(&source_channel, _target)
                    {
                        tracing::warn!("Failed to request workspace switch: {e}");
                    }
                }
                true
            }
            TermWmAction::NewWorkspace => {
                #[cfg(feature = "session-persistence")]
                {
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    // TODO: Don't hardcode
                    let target_ws = format!("ws-{}", ts);
                    let target_channel = format!("{}/main", target_ws);

                    if let Ok(exe) = std::env::current_exe() {
                        let inner_cmd = vec![
                            exe.to_string_lossy().into_owned(),
                            // TODO: Don't hardcode params; extremely hard to test this way
                            "--internal-session".to_string(),
                            "-w".to_string(),
                            target_ws.clone(),
                        ];

                        let res = term_session::with_gateway(|client| async move {
                            use term_session::protocol::{
                                Attach, AttachRequest, Spawn, SpawnRequest,
                            };
                            use term_session::rpc_client::RpcCallPrebuffered;
                            let _ = Attach::call(
                                &*client,
                                AttachRequest {
                                    channel: target_channel,
                                    // TODO: Don't hardcode
                                    hostname: "local".to_string(),
                                    pid: std::process::id() as u64,
                                    // TODO: Don't hardcode
                                    user: "term-wm".to_string(),
                                    version: env!("CARGO_PKG_VERSION").to_string(),
                                    ssh_ip: None,
                                },
                            )
                            .await
                            .map_err(std::io::Error::other)?;

                            Spawn::call(
                                &*client,
                                SpawnRequest {
                                    cmd: Some(inner_cmd),
                                    // TODO: Don't hardcode this (make it optional if it requires bogus initial params)
                                    cols: 80,
                                    rows: 24,
                                    cwd: None,
                                },
                            )
                            .await
                            .map_err(std::io::Error::other)
                        })
                        .and_then(|r| r);

                        if let Err(e) = res {
                            tracing::error!("Failed to spawn new workspace channel: {e}");
                        } else {
                            self.inner.refresh_workspace_cache();
                            self.inner.wm().push_notification(
                                format!("Created workspace: {target_ws}"),
                                std::time::Duration::from_secs(3),
                            );
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn open_help_overlay(&mut self) {
        self.inner.open_help_overlay();
    }

    fn open_exit_confirm(&mut self) {
        self.inner.open_exit_confirm();
    }

    fn open_command_palette(&mut self) {
        #[cfg(feature = "session-persistence")]
        self.inner.refresh_workspace_cache();
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

    fn wm_new_terminal(&mut self) -> io::Result<()> {
        <TermWmApp<NoopComponent> as term_wm::runner::WindowManagerHost<
            AppRootComponent<NoopComponent>,
            LayerComponent,
            OverlayComponent,
        >>::wm_new_terminal(&mut self.inner)
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
        let wm = build_wm(&app_ctx, WmConfig::default());
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
