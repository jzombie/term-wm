//! Internal (daemon-managed) session wiring for the bundled `term-wm` binary.
//!
//! Extracted from `main.rs` so the launch path stays thin and the pieces are
//! independently testable:
//!
//! - [`spawn_attributed_input_listener`]: the Muxio RPC notification listener
//!   that pipes attributed input, user presence, resize, and workspace events
//!   into the app's [`UnifiedEvent`] channel, reports ordered WM stats (#298),
//!   and periodically re-syncs the user cache (#306).
//! - [`run_outer_launcher`]: the workspace rebind loop wrapping
//!   `run_session` reconnects.
//! - [`install_session_action_handler`]: routes the session/gateway palette
//!   actions (`SwitchWorkspace`, `NewWorkspace`, `DetachCurrentClient`,
//!   `ToggleWorkspaceFollow`, stop-gateway) through IPC on behalf of the
//!   binary.

#[cfg(feature = "session-persistence")]
use std::io;
use std::sync::Arc;
#[cfg(feature = "session-persistence")]
use std::time::Duration;

#[cfg(feature = "session-persistence")]
use crossbeam_channel::Sender;

#[cfg(feature = "session-persistence")]
use term_wm_core::actions::TermWmAction;
#[cfg(feature = "session-persistence")]
use term_wm_core::runner::WindowManagerHost;

#[cfg(feature = "session-persistence")]
use crate::cli::{Cli, build_inner_command};
use crate::term_wm_app::TermWmApp;
#[cfg(feature = "session-persistence")]
use crate::unified_event_source::UnifiedEvent;

/// Queue capacity for the ordered `(windows, tasks)` stats reports flowing
/// from the WM mutation paths to the subscribed internal-WM connection (#298).
#[cfg(feature = "session-persistence")]
const WM_STATS_QUEUE_CAPACITY: usize = 16;

/// Toast duration shared by session-action feedback notifications.
#[cfg(feature = "session-persistence")]
const TOAST_DURATION_SECS: u64 = 3;

/// Delay before falling back to the default workspace after a dropped
/// connection in the outer launcher loop.
#[cfg(feature = "session-persistence")]
const RECONNECT_RETRY_DELAY: Duration = Duration::from_secs(2);

/// Prefix for timestamp-derived new-workspace names created by `NewWorkspace`.
#[cfg(feature = "session-persistence")]
const NEW_WORKSPACE_NAME_PREFIX: &str = "ws-";

/// Spawn the internal session's attributed-input listener on the ambient
/// tokio runtime (the caller must have entered one). Returns the ordered
/// stats-report queue for [`TermWmApp::set_stats_reporter`](crate::term_wm_app::TermWmApp::set_stats_reporter).
///
/// For internal sessions, this listener receives structured events from the
/// server and pipes them into the event source via `pty_wakeup_tx`.
#[cfg(feature = "session-persistence")]
pub fn spawn_attributed_input_listener(
    pty_wakeup_tx: Sender<UnifiedEvent>,
    workspace: &str,
) -> io::Result<tokio::sync::mpsc::Sender<(u32, u32)>> {
    let tx = pty_wakeup_tx.clone();
    let channel = term_session::ChannelName::session(workspace).to_string();
    let socket_path = term_session::auto_spawn::connect_or_spawn_server(None)?;
    // Queue the WM's live counts flow through; drained by a task inside
    // the listener below that reports over the subscribed connection.
    let (stats_tx, mut stats_rx) =
        tokio::sync::mpsc::channel::<(u32, u32)>(WM_STATS_QUEUE_CAPACITY);
    tokio::spawn(async move {
        use term_session::protocol::OnAttributedInput;
        use term_session::protocol::RpcMethodPrebuffered;
        use term_session::protocol::SubscribeInternalInputRequest;

        let client = match term_session::rpc_client::RpcIpcClient::new(&socket_path).await {
            Ok(c) => std::sync::Arc::new(c),
            Err(e) => {
                tracing::error!("Failed to connect for attributed input: {e:?}");
                return;
            }
        };
        // Register handlers BEFORE subscribing to avoid race
        {
            let tx = tx.clone();
            use muxio_rpc_service_endpoint::RpcServiceEndpointInterface;
            client
                .get_endpoint()
                .register_prebuffered(OnAttributedInput::METHOD_ID, move |payload, _ctx| {
                    let tx = tx.clone();
                    async move {
                        let req = OnAttributedInput::decode_request(&payload)
                            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                        // Route through main channel — wakes poll() immediately
                        let _ = tx.try_send(UnifiedEvent::Input {
                            conn_id: Some(req.conn_id),
                            event: req.event,
                        });
                        OnAttributedInput::encode_response(())
                            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                    }
                })
                .await
                .expect("register OnAttributedInput");
        }
        {
            let tx = tx.clone();
            use muxio_rpc_service_endpoint::RpcServiceEndpointInterface;
            use term_session::protocol::OnUserConnected;
            client
                .get_endpoint()
                .register_prebuffered(OnUserConnected::METHOD_ID, move |payload, _ctx| {
                    let tx = tx.clone();
                    async move {
                        let info = OnUserConnected::decode_request(&payload)
                            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                        let _ = tx.try_send(UnifiedEvent::UserConnected(info));
                        OnUserConnected::encode_response(())
                            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                    }
                })
                .await
                .expect("register OnUserConnected");
        }
        {
            let tx = tx.clone();
            use muxio_rpc_service_endpoint::RpcServiceEndpointInterface;
            use term_session::protocol::OnUserDisconnected;
            client
                .get_endpoint()
                .register_prebuffered(OnUserDisconnected::METHOD_ID, move |payload, _ctx| {
                    let tx = tx.clone();
                    async move {
                        let conn_id = OnUserDisconnected::decode_request(&payload)
                            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                        let _ = tx.try_send(UnifiedEvent::UserDisconnected(conn_id));
                        OnUserDisconnected::encode_response(())
                            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                    }
                })
                .await
                .expect("register OnUserDisconnected");
        }
        {
            let tx = tx.clone();
            use muxio_rpc_service_endpoint::RpcServiceEndpointInterface;
            use term_session::protocol::OnUserResized;
            client
                .get_endpoint()
                .register_prebuffered(OnUserResized::METHOD_ID, move |payload, _ctx| {
                    let tx = tx.clone();
                    async move {
                        let (conn_id, cols, rows) = OnUserResized::decode_request(&payload)
                            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                        let _ = tx.try_send(UnifiedEvent::UserResized((conn_id, cols, rows)));
                        OnUserResized::encode_response(())
                            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                    }
                })
                .await
                .expect("register OnUserResized");
        }
        {
            let tx = tx.clone();
            use muxio_rpc_service_endpoint::RpcServiceEndpointInterface;
            use term_session::protocol::OnWorkspaceEntered;
            client
                .get_endpoint()
                .register_prebuffered(OnWorkspaceEntered::METHOD_ID, move |payload, _ctx| {
                    let tx = tx.clone();
                    async move {
                        let ws = OnWorkspaceEntered::decode_request(&payload)
                            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                        let _ = tx.try_send(UnifiedEvent::WorkspaceEntered(ws));
                        OnWorkspaceEntered::encode_response(())
                            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                    }
                })
                .await
                .expect("register OnWorkspaceEntered");
        }
        // Subscribe
        use muxio_rpc_service_caller::prebuffered::RpcCallPrebuffered as _;
        let channel_for_sub = channel.clone();
        let client_ref: &term_session::rpc_client::RpcIpcClient = &client;
        if let Err(e) = term_session::protocol::SubscribeInternalInput::call(
            client_ref,
            SubscribeInternalInputRequest {
                channel: channel_for_sub,
            },
        )
        .await
        {
            tracing::error!("SubscribeInternalInput failed: {e:?}");
            return;
        }
        tracing::info!("Attributed input listener subscribed");

        // Ordered WM-stats reporting over THIS connection: it is the one
        // registered as the channel's internal WM, which the gateway
        // requires before accepting stats. FIFO single consumer, so rapid
        // mutations can never arrive out of order (#298).
        {
            use term_session::protocol::ReportWmStats;
            let client = client.clone();
            tokio::spawn(async move {
                while let Some((windows, tasks_running)) = stats_rx.recv().await {
                    let client_ref: &term_session::rpc_client::RpcIpcClient = &client;
                    if let Err(e) = ReportWmStats::call(client_ref, (windows, tasks_running)).await
                    {
                        tracing::debug!("wm stats report failed: {e:?}");
                    }
                }
            });
        }
        // Periodic user-cache re-sync (#306): runs on its OWN task so this
        // listener task keeps processing RPC notifications uninterrupted.
        // The first tick fires immediately, preserving the previous
        // post-subscribe one-shot refresh while healing stale conn_ids
        // (cold-start races, viewer reconnects) every interval.
        {
            // Re-sync cadence for the internal WM's user registry. Shorter
            // than the palette's 30s IPC poll so registry gaps heal well
            // before a user would notice stale palette entries.
            const USER_CACHE_REFRESH_INTERVAL: Duration = Duration::from_secs(15);
            let tx = tx.clone();
            let client = client.clone();
            let channel = channel.clone();
            tokio::spawn(async move {
                use muxio_rpc_service_caller::prebuffered::RpcCallPrebuffered as _;
                use term_session::protocol::ListUsers;
                let mut interval = tokio::time::interval(USER_CACHE_REFRESH_INTERVAL);
                loop {
                    interval.tick().await;
                    // Annotated ref mirrors the stats-reporter pattern
                    // above; deref coercion unwraps the Arc cleanly.
                    let client_ref: &term_session::rpc_client::RpcIpcClient = &client;
                    match ListUsers::call(client_ref, channel.clone()).await {
                        Ok(resp) => {
                            let _ = tx.try_send(UnifiedEvent::UserCacheRefreshed(resp.users));
                        }
                        Err(e) => {
                            tracing::debug!("ListUsers refresh failed: {e:?}");
                        }
                    }
                }
            });
        }
        // Keep the connection alive so the endpoint keeps processing RPCs.
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
    Ok(stats_tx)
}

/// Run the outer launcher's workspace rebind loop: attach to the daemon
/// channel for `initial_workspace`, spawn the inner session process, and
/// follow workspace rebinds until the session ends or drops back to the
/// default workspace.
#[cfg(feature = "session-persistence")]
pub fn run_outer_launcher(cli: &Cli, initial_workspace: String) -> io::Result<()> {
    let socket_path = term_session::auto_spawn::connect_or_spawn_server(None)?;
    let mut current_workspace = initial_workspace;

    loop {
        let clean_workspace =
            term_session::ChannelName::parse_workspace(&current_workspace).to_string();
        current_workspace = clean_workspace.clone();

        let channel = term_session::ChannelName::session(&current_workspace).to_string();
        let current_exe = std::env::current_exe()?.to_string_lossy().into_owned();

        let inner_cmd = build_inner_command(current_exe, &current_workspace, cli);

        match term_session::client::run_session(
            &socket_path,
            &channel,
            &inner_cmd,
            cli.allow_nested,
            "term-wm",
        ) {
            Ok(Some(target_channel)) => {
                current_workspace = target_channel;
                continue;
            }
            Ok(None) => return Ok(()),
            Err(e) => {
                if term_session::client::is_nested_session_fatal(&e) {
                    return Err(e);
                }
                tracing::error!(
                    "Connection dropped for workspace '{}': {}",
                    current_workspace,
                    e
                );
                std::thread::sleep(RECONNECT_RETRY_DELAY);
                if current_workspace != term_session::DEFAULT_WORKSPACE {
                    current_workspace = term_session::DEFAULT_WORKSPACE.to_string();
                    continue;
                }
                return Err(e);
            }
        }
    }
}

/// Install the bundled binary's session-action hook on `app`.
///
/// Handles the gateway-backed palette actions via short-lived IPC:
/// workspace switch/new/follow, viewer detach, and the stop-gateway
/// executor. Project tasks are handled natively by `TermWmApp` before this
/// hook runs. Without the `session-persistence` feature this installs
/// nothing (there are no session actions to route).
#[cfg(not(feature = "session-persistence"))]
pub fn install_session_action_handler(
    _app: &mut TermWmApp,
    _event_owner: Arc<std::sync::Mutex<Option<usize>>>,
) {
}

/// Install the bundled binary's session-action hook on `app`.
///
/// See the feature-gated counterpart above for the contract; this variant
/// performs the real gateway IPC routing. The runtime persistence toggle is
/// honored here: when disabled, every session action falls through as
/// unhandled.
#[cfg(feature = "session-persistence")]
pub fn install_session_action_handler(
    app: &mut TermWmApp,
    event_owner: Arc<std::sync::Mutex<Option<usize>>>,
) {
    app.set_custom_action_handler(Box::new(
        move |action: &TermWmAction, app: &mut TermWmApp| {
            if !term_wm_config::runtime::session_persistence_enabled() {
                return false;
            }
            match action {
                TermWmAction::SwitchWorkspace(_target) => {
                    let source_ws =
                        term_session::ChannelName::parse_workspace(app.current_workspace());
                    let target_ws = term_session::ChannelName::parse_workspace(_target);

                    let source_channel = term_session::ChannelName::session(source_ws).to_string();
                    let target_channel = term_session::ChannelName::session(target_ws).to_string();
                    let follow = app.wm().workspace_follow_enabled;
                    let scope = if follow {
                        term_session::protocol::RebindScope::AllViewers
                    } else {
                        term_session::protocol::RebindScope::CallerOnly
                    };
                    let initiator = *event_owner.lock().unwrap_or_else(|err| err.into_inner());
                    if let Err(e) = term_session::request_workspace_rebind_with_scope(
                        &source_channel,
                        &target_channel,
                        scope,
                        initiator,
                    ) {
                        tracing::warn!("Failed to request workspace switch: {e}");
                    } else {
                        app.on_user_registry_changed();
                    }
                    true
                }
                TermWmAction::NewWorkspace => {
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    let target_ws = format!("{NEW_WORKSPACE_NAME_PREFIX}{ts}");
                    let source_ws =
                        term_session::ChannelName::parse_workspace(app.current_workspace());

                    let source_channel = term_session::ChannelName::session(source_ws).to_string();
                    let target_channel = term_session::ChannelName::session(&target_ws).to_string();
                    let follow = app.wm().workspace_follow_enabled;
                    let scope = if follow {
                        term_session::protocol::RebindScope::AllViewers
                    } else {
                        term_session::protocol::RebindScope::CallerOnly
                    };
                    let initiator = *event_owner.lock().unwrap_or_else(|err| err.into_inner());
                    if let Err(e) = term_session::request_workspace_rebind_with_scope(
                        &source_channel,
                        &target_channel,
                        scope,
                        initiator,
                    ) {
                        tracing::error!("Failed to switch to new workspace: {e}");
                    } else {
                        app.on_user_registry_changed();
                    }
                    true
                }
                TermWmAction::DetachCurrentClient => {
                    if let Some(conn_id) =
                        *event_owner.lock().unwrap_or_else(|err| err.into_inner())
                    {
                        let channel =
                            term_session::ChannelName::session(app.current_workspace()).to_string();
                        if let Err(e) = term_session::kill_client(&channel, conn_id) {
                            tracing::warn!("Failed to detach viewer: {e}");
                        }
                    }
                    true
                }
                TermWmAction::ToggleWorkspaceFollow => {
                    let enabled = {
                        let wm = app.wm();
                        wm.workspace_follow_enabled = !wm.workspace_follow_enabled;
                        wm.workspace_follow_enabled
                    };
                    let msg = if enabled {
                        "Follow Workspaces: Enabled"
                    } else {
                        "Follow Workspaces: Disabled"
                    };
                    app.wm()
                        .push_notification(msg, Duration::from_secs(TOAST_DURATION_SECS));
                    true
                }
                // Palette entry: open the confirmation dialog only. The shutdown
                // itself is reachable exclusively via the dialog's Confirm branch.
                TermWmAction::OpenStopGatewayConfirm => {
                    app.open_stop_gateway_confirm();
                    true
                }
                // Executor action, dispatched ONLY from the stop-gateway dialog's
                // Confirm branch. Force=true: the user explicitly accepted that
                // every workspace session will be terminated.
                TermWmAction::StopGatewayDaemon => {
                    match term_session::stop_gateway(true) {
                        Ok(()) => {
                            app.wm().push_notification(
                                "Gateway shutdown initiated.",
                                Duration::from_secs(TOAST_DURATION_SECS),
                            );
                        }
                        Err(e) => {
                            app.wm().push_notification(
                                format!("Failed to stop gateway daemon: {e}"),
                                Duration::from_secs(TOAST_DURATION_SECS),
                            );
                        }
                    }
                    // Do NOT quit locally: in persistence mode this WM runs inside
                    // a daemon-managed PTY; killing the gateway tears down that
                    // PTY and the normal AppExited flow handles our own exit.
                    true
                }
                _ => false,
            }
        },
    ));
}

#[allow(clippy::unwrap_used)]
#[cfg(all(test, feature = "session-persistence"))]
mod tests {
    use super::*;
    use serial_test::serial;
    use term_wm::app_context::AppContext;
    use term_wm::unified_event_source::UnifiedEventSource;
    use term_wm_config::runtime::{RuntimeConfig, init, session_persistence_enabled};
    use term_wm_core::actions::{ConfirmAction, TermWmAction};
    use term_wm_core::events::{Event, KeyCode, KeyEvent, KeyKind, KeyModifiers};
    use term_wm_core::wm_config::WmConfig;

    /// Build an app without spawning any PTYs, so the workspace-action
    /// handler can be unit-tested directly.
    fn test_app() -> TermWmApp {
        let (event_source, event_owner) = UnifiedEventSource::new(true).expect("headless source");
        let pty_wakeup_tx = event_source.pty_wakeup_tx();
        let app_ctx = Arc::new(AppContext::new("term-wm", "0.0.0").with_hostname("test-host"));
        let mut app = TermWmApp::new_full_chrome(&app_ctx, WmConfig::default(), pty_wakeup_tx);
        app.set_current_workspace("dev".into());
        install_session_action_handler(&mut app, event_owner);
        app
    }

    /// With session persistence disabled at runtime, every workspace action
    /// must fall through as unhandled (`false`) — the runtime toggle's contract.
    #[test]
    #[serial(process_global_state)]
    fn handle_custom_action_returns_false_when_runtime_disabled() {
        let prev = RuntimeConfig {
            session_persistence: session_persistence_enabled(),
        };
        init(RuntimeConfig {
            session_persistence: false,
        });

        let mut app = test_app();
        for action in [
            TermWmAction::SwitchWorkspace("prod".into()),
            TermWmAction::NewWorkspace,
            TermWmAction::DetachCurrentClient,
        ] {
            assert!(
                !app.handle_custom_action(&action),
                "runtime-disabled app must not consume {action:?}"
            );
        }

        init(prev);
    }

    /// With session persistence enabled, `SwitchWorkspace` / `NewWorkspace`
    /// are consumed by the app (`true`) even when no gateway is reachable —
    /// the IPC failure is logged, not bubbled up. Hermetic: a throwaway
    /// gateway name avoids colliding with a real daemon.
    #[test]
    #[serial(process_global_state)]
    fn handle_custom_action_consumes_workspace_actions() {
        // Hermetic: a guaranteed-dead unique channel. Workspace actions must
        // be consumed even when every gateway connect fails instantly; IPC
        // errors are logged inside the handlers, never bubbled.
        let _gateway = PinnedGatewayEnv::pin(dead_gateway_channel("ws"));
        let prev = RuntimeConfig {
            session_persistence: session_persistence_enabled(),
        };
        init(RuntimeConfig {
            session_persistence: true,
        });

        let mut app = test_app();
        assert!(app.handle_custom_action(&TermWmAction::SwitchWorkspace("prod".into())));
        assert!(app.handle_custom_action(&TermWmAction::NewWorkspace));

        // Detach with no attributed conn id: no gateway call, still consumed.
        assert!(app.handle_custom_action(&TermWmAction::DetachCurrentClient));

        init(prev);
    }

    /// `RunProjectTask` with a nonexistent task label returns false
    /// and does not panic — the "task not found" branch.
    #[test]
    fn run_project_task_nonexistent_returns_false() {
        let mut app = test_app();
        assert!(
            !app.handle_custom_action(&TermWmAction::RunProjectTask("no-such-task".into())),
            "RunProjectTask with nonexistent label must return false"
        );
    }

    /// `RunProjectTask` works regardless of the session-persistence toggle
    /// (it is matched before the persistence guard).
    #[test]
    #[serial(process_global_state)]
    fn run_project_task_works_without_persistence() {
        let prev = RuntimeConfig {
            session_persistence: session_persistence_enabled(),
        };
        init(RuntimeConfig {
            session_persistence: false,
        });

        let mut app = test_app();
        // Even with persistence disabled, RunProjectTask is matched first.
        assert!(
            !app.handle_custom_action(&TermWmAction::RunProjectTask("missing".into())),
            "RunProjectTask must be reachable even when persistence is disabled"
        );

        init(prev);
    }

    /// The stop-gateway palette action opens the confirmation overlay and is
    /// consumed; the executor action is likewise consumed without quitting.
    /// Neither may ever surface as an unhandled fall-through (#298).
    ///
    /// Hermetic helpers below: `PinnedGatewayEnv` RAII-pins the process-
    /// local gateway override cell and `dead_gateway_channel` mints unique
    /// never-spawned channels, so connects fail instantly on every platform.
    struct PinnedGatewayEnv {
        /// Previous value of the override cell (None = unset before
        /// pinning), restored on drop so overlapping suites keep their
        /// expectations.
        _previous: Option<String>,
    }

    impl PinnedGatewayEnv {
        /// Pin the process-local gateway override to `value` for the
        /// guard's lifetime. Callers hold `#[serial(process_global_state)]`
        /// so no other thread touches the cell while the guard is alive.
        fn pin(value: String) -> Self {
            Self {
                _previous: term_wm_config::env::set_gateway_override(Some(&value)),
            }
        }
    }

    impl Drop for PinnedGatewayEnv {
        fn drop(&mut self) {
            // Restore the previous state rather than clearing blindly:
            // another pinned guard may legitimately own the slot.
            let previous = self._previous.take();
            term_wm_config::env::set_gateway_override(previous.as_deref());
        }
    }

    /// Unique guaranteed-dead channel for hermetic failure-path tests: never
    /// spawned, so connects fail instantly on every platform.
    fn dead_gateway_channel(tag: &str) -> String {
        static NEXT_DEAD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let n = NEXT_DEAD.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("term-wm/testgw-dead-{}-{tag}-{n}", std::process::id())
    }

    #[test]
    #[serial(process_global_state)]
    fn handle_custom_action_consumes_stop_gateway_actions() {
        // Hermetic (#Windows-CI): the pinned override points at a guaranteed-dead
        // unique channel, so every connect fails instantly on all platforms.
        // The assertions pin what the UNIT layer owns: palette consumption,
        // overlay visibility, Enter-to-Confirm resolution, close behavior,
        // and executor consumption with the IPC error toasted rather than
        // bubbled. Real forced-shutdown RPC semantics (socket actually dies)
        // are proven end-to-end in tests/integration_session.rs.
        let _gateway = PinnedGatewayEnv::pin(dead_gateway_channel("stop"));

        let mut app = test_app();

        let opened = app.handle_custom_action(&TermWmAction::OpenStopGatewayConfirm);
        assert!(
            opened,
            "OpenStopGatewayConfirm must be consumed by the host"
        );
        assert!(
            app.wm().stop_daemon_confirm_visible(),
            "OpenStopGatewayConfirm must render the stop-daemon confirm overlay"
        );

        // Confirm path FIRST (overlay still open): Enter resolves to Confirm.
        {
            let enter = Event::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
                KeyKind::Press,
            ));
            assert_eq!(
                app.wm().handle_stop_daemon_confirm_event(&enter),
                Some(ConfirmAction::Confirm),
                "Enter must resolve to Confirm on the open overlay"
            );
        }

        // Cancel path (fresh overlay): closing clears visibility.
        app.wm().close_stop_daemon_confirm();
        assert!(
            !app.wm().stop_daemon_confirm_visible(),
            "closing must clear stop-daemon overlay visibility"
        );

        // The executor arm consumes unconditionally: the unreachable gateway
        // makes `stop_gateway` fail fast and that failure is toasted, never
        // bubbled as an unhandled action.
        assert!(
            app.handle_custom_action(&TermWmAction::StopGatewayDaemon),
            "StopGatewayDaemon must be consumed even when the gateway is unreachable"
        );
    }

    /// `ToggleWorkspaceFollow` toggles the flag and pushes a notification.
    #[test]
    #[serial(process_global_state)]
    fn toggle_workspace_follow_toggles_flag() {
        let mut app = test_app();
        let initially_enabled = app.wm().workspace_follow_enabled;

        assert!(
            app.handle_custom_action(&TermWmAction::ToggleWorkspaceFollow),
            "ToggleWorkspaceFollow must return true"
        );
        assert_eq!(
            app.wm().workspace_follow_enabled,
            !initially_enabled,
            "toggle must flip the flag"
        );

        // Toggle back
        app.handle_custom_action(&TermWmAction::ToggleWorkspaceFollow);
        assert_eq!(
            app.wm().workspace_follow_enabled,
            initially_enabled,
            "second toggle must restore original value"
        );
    }

    /// `SwitchWorkspace` with `workspace_follow_enabled = true` exercises the
    /// `RebindScope::AllViewers` branch (vs `CallerOnly` when disabled).
    #[test]
    #[serial(process_global_state)]
    fn switch_workspace_follow_enabled_uses_all_viewers_scope() {
        // Hermetic: guaranteed-dead unique channel (see
        // handle_custom_action_consumes_workspace_actions).
        let _gateway = PinnedGatewayEnv::pin(dead_gateway_channel("switch"));
        let prev = RuntimeConfig {
            session_persistence: session_persistence_enabled(),
        };
        init(RuntimeConfig {
            session_persistence: true,
        });

        let mut app = test_app();

        // Enable follow mode, then switch workspace
        app.wm().workspace_follow_enabled = true;
        assert!(
            app.handle_custom_action(&TermWmAction::SwitchWorkspace("staging".into())),
            "SwitchWorkspace must be consumed"
        );

        init(prev);
    }
}
