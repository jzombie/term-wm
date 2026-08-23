use std::any::TypeId;
use std::time::{Duration, Instant};

use term_wm_layout_engine::LayoutRect;

use super::{OverlayKey, WindowManager};

/// Per-workspace live WM totals (windows, still-running tasks), keyed by
/// workspace name. Reported by each instance to the gateway and surfaced in
/// the palette's workspace list (#298). A missing entry means unknown (no
/// reporting connection), not zero.
pub type WorkspaceTotals = std::collections::BTreeMap<String, (u32, u32)>;

// TODO: Dedupe in codebase (term-session has a similar version)
/// Format a `connected_at_unix` timestamp into a compact uptime string.
#[cfg(feature = "session-persistence")]
fn format_uptime(connected_at_unix: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(connected_at_unix);
    let diff = now.saturating_sub(connected_at_unix);
    if diff < 60 {
        format!("{diff}s")
    } else if diff < 3_600 {
        format!("{}m", diff / 60)
    } else if diff < 86_400 {
        format!("{}h", diff / 3_600)
    } else {
        format!("{}d {}h", diff / 86_400, (diff % 86_400) / 3_600)
    }
}
use crate::actions::{EventResult, TermWmAction, WmInputMode};
use crate::components::{Component, Overlay, WmComponent};
use crate::events::Event;
use crate::window::window_manager::system_tags;

impl<C: Component<TermWmAction>, L: WmComponent, O: Overlay<TermWmAction>> WindowManager<C, L, O> {
    pub fn open_command_palette_overlay(&mut self, overlay: O) {
        let key = self.overlays.insert(overlay);
        self.register_overlay::<system_tags::CommandPalette>(key);
        self.input_mode = WmInputMode::CommandPalette;
    }

    /// Enter tab outline mode — palette becomes dim overlay, panels hide in monocle.
    pub fn set_tab_outline_mode(&mut self, duration: Duration) {
        let expires = Instant::now() + duration;
        self.tab_outline_until = Some(expires);
        if let Some(key) = self.get_overlay::<system_tags::CommandPalette>()
            && let Some(overlay) = self.overlays.get_mut(key)
        {
            overlay.set_tab_outline(Some(expires));
        }
        if let Some(handle) = &self.system_task_handle {
            let _ = handle.schedule_once(duration, crate::actions::SystemTask::ClearTabOutline);
        }
    }

    /// Clear tab outline mode — restore palette/panels to normal.
    pub fn clear_tab_outline(&mut self) {
        self.tab_outline_until = None;
        if let Some(key) = self.get_overlay::<system_tags::CommandPalette>()
            && let Some(overlay) = self.overlays.get_mut(key)
        {
            overlay.set_tab_outline(None);
        }
    }

    pub fn command_palette_key(&self) -> Option<OverlayKey> {
        self.get_overlay::<system_tags::CommandPalette>()
    }

    pub fn command_palette_visible(&self) -> bool {
        self.system_overlays
            .contains_key(&TypeId::of::<system_tags::CommandPalette>())
    }

    pub fn command_palette_bounds(&self) -> Option<LayoutRect> {
        self.get_overlay::<system_tags::CommandPalette>()
            .and_then(|key| self.overlays.get(key))
            .and_then(|o| o.render_area())
    }

    pub fn close_command_palette(&mut self) {
        if let Some(key) = self
            .system_overlays
            .remove(&TypeId::of::<system_tags::CommandPalette>())
        {
            self.overlays.remove(key);
        }
        self.input_mode = WmInputMode::Passthrough;
        self.pending_palette_anchor = None;
    }

    /// Consume the anchor rect captured by the current mouse dispatch, if any.
    /// Returns `None` when the palette is opened via keyboard (no mouse hitbox),
    /// so the app renders it centered.
    pub fn take_pending_palette_anchor(&mut self) -> Option<LayoutRect> {
        self.pending_palette_anchor.take()
    }

    pub fn handle_command_palette_event(&mut self, event: &Event) -> Option<TermWmAction> {
        if let Event::Mouse(mouse) = event {
            self.hover = Some((mouse.column, mouse.row));
        }
        let ctx = self
            .component_context(false)
            .with_overlay(true)
            .with_screen_area(self.managed_area())
            .with_hover_pos(self.hover);

        let key = self.get_overlay::<system_tags::CommandPalette>()?;
        let palette = self.overlays.get_mut(key)?;
        match palette.handle_events(event, &ctx) {
            EventResult::Action(action) => {
                self.close_command_palette();
                Some(action)
            }
            _ => None,
        }
    }

    pub fn command_menu_visible(&self) -> bool {
        self.command_palette_visible()
    }

    pub fn close_command_menu(&mut self) {
        self.close_command_palette();
    }

    /// Rebuild the command palette's action list from current WM state and
    /// apply it to the overlay if visible. Used to refresh a stale palette
    /// when users connect/disconnect, workspaces change, or focus shifts.
    pub fn refresh_palette_items(&mut self) {
        if !self.command_menu_visible() {
            return;
        }
        let Some(palette_key) = self.get_overlay::<system_tags::CommandPalette>() else {
            return;
        };

        use crate::components::MenuDisplayItem;
        #[cfg(feature = "session-persistence")]
        let totals = &self.cached_workspace_totals;
        #[cfg(not(feature = "session-persistence"))]
        let totals = &WorkspaceTotals::new();
        let items = self.wm_menu_items(
            &self.cached_workspaces,
            &self.current_workspace,
            &self.project_tasks,
            &self.all_users_by_ws,
            totals,
        );
        let supported = &self.supported_menu_actions;
        let filtered: Vec<MenuDisplayItem<TermWmAction>> = items
            .into_iter()
            .filter(|entry| match entry {
                MenuDisplayItem::Item(item) => {
                    let always_pass = matches!(
                        item.action,
                        TermWmAction::FocusWindow(_)
                            | TermWmAction::MaximizeWindow(_)
                            | TermWmAction::MinimizeWindow(_)
                            | TermWmAction::CloseWindow(_)
                            | TermWmAction::SendSuperKeyToWindow(_)
                            | TermWmAction::SendSuperKeyToFocusedWindow
                            | TermWmAction::RunProjectTask(_)
                    );
                    #[cfg(feature = "session-persistence")]
                    let always_pass = always_pass
                        || (term_wm_config::runtime::session_persistence_enabled()
                            && matches!(
                                item.action,
                                TermWmAction::SwitchWorkspace(_)
                                    | TermWmAction::NewWorkspace
                                    | TermWmAction::ToggleWorkspaceFollow
                            ));
                    item.disabled || supported.contains(&item.action) || always_pass
                }
                MenuDisplayItem::Separator => true,
            })
            .collect();

        if let Some(overlay) = self.overlays.get_mut(palette_key) {
            overlay.set_menu_items(filtered);
        }
    }

    // TODO: Workspaces & current_workspace should be derived from context, I think
    pub fn wm_menu_items(
        &self,
        workspaces: &[String],
        current_workspace: &str,
        project_tasks: &[crate::project_tasks::ProjectTaskConfig],
        all_users_by_ws: &std::collections::BTreeMap<String, Vec<crate::user_registry::UserEntry>>,
        workspace_totals: &WorkspaceTotals,
    ) -> Vec<crate::components::MenuDisplayItem<crate::actions::TermWmAction>> {
        use crate::components::{MenuDisplayItem, MenuItem};
        use crate::window::WindowState;
        use crate::window::window_manager::system_tags;

        let debug_log_visible = self
            .get_system_window::<system_tags::DebugLog>()
            .is_some_and(|k| self.window_state(k) == Some(WindowState::Mapped));
        let system_panel_visible = self
            .get_system_window::<system_tags::SystemPanel>()
            .is_some_and(|k| self.window_state(k) == Some(WindowState::Mapped));

        let _mouse_label = if self.mouse_capture_enabled {
            "Mouse: Disable Capture"
        } else {
            "Mouse: Enable Capture"
        };
        let _clipboard_label = if self.clipboard_enabled {
            "Clipboard: Disable"
        } else {
            "Clipboard: Enable"
        };
        let _debug_label = if debug_log_visible {
            "System: Disable Debug Log"
        } else {
            "System: Enable Debug Log"
        };
        let _panel_label = if system_panel_visible {
            "System: Disable Panel"
        } else {
            "System: Enable Panel"
        };

        let mi = |label: &'static str,
                  icon: Option<&'static str>,
                  action: crate::actions::TermWmAction| {
            MenuDisplayItem::Item(MenuItem {
                label: label.into(),
                icon,
                action,
                disabled: false,
            })
        };

        let header = |title: &'static str| {
            MenuDisplayItem::Item(MenuItem {
                label: format!("── {} ──", title).into(),
                icon: None,
                action: crate::actions::TermWmAction::CloseMenu,
                disabled: true,
            })
        };

        #[cfg(feature = "session-persistence")]
        let info_item = |label: String| {
            MenuDisplayItem::Item(MenuItem {
                label: label.into(),
                icon: None,
                action: crate::actions::TermWmAction::CloseMenu,
                disabled: true,
            })
        };

        let focused = self.focused_window();
        let has_active = self.windows.contains_key(focused);

        let mut items: Vec<MenuDisplayItem<crate::actions::TermWmAction>> = Vec::new();

        // ─────────────────────────────────────────────────────────
        // 1. QUICK ACTIONS & TASKS
        // ─────────────────────────────────────────────────────────
        items.push(header("QUICK ACTIONS"));
        items.push(mi(
            "Resume",
            Some("▶"),
            crate::actions::TermWmAction::CloseMenu,
        ));
        items.push(mi(
            "New Terminal",
            Some("+"),
            crate::actions::TermWmAction::NewTerminal,
        ));

        #[cfg(feature = "project-tasks")]
        {
            for task in project_tasks.iter().filter(|t| t.argv().is_some()) {
                items.push(MenuDisplayItem::Item(MenuItem {
                    label: task.label.clone().into(),
                    icon: Some("▶"),
                    action: crate::actions::TermWmAction::RunProjectTask(task.label.clone()),
                    disabled: false,
                }));
            }
        }
        #[cfg(not(feature = "project-tasks"))]
        {
            let _ = project_tasks;
        }
        items.push(MenuDisplayItem::Separator);

        // ─────────────────────────────────────────────────────────
        // 2. WORKSPACES & COLLABORATION (Consolidated)
        // ─────────────────────────────────────────────────────────
        #[cfg(feature = "session-persistence")]
        if term_wm_config::runtime::session_persistence_enabled() {
            items.push(header("WORKSPACES & COLLABORATION"));
            items.push(mi(
                "New Workspace",
                Some("+"),
                crate::actions::TermWmAction::NewWorkspace,
            ));

            let follow_label = if self.workspace_follow_enabled {
                "Follow Workspaces: Disable"
            } else {
                "Follow Workspaces: Enable"
            };
            let follow_icon = if self.workspace_follow_enabled {
                Some("◎")
            } else {
                Some("○")
            };
            items.push(MenuDisplayItem::Item(MenuItem {
                label: follow_label.into(),
                icon: follow_icon,
                action: crate::actions::TermWmAction::ToggleWorkspaceFollow,
                disabled: false,
            }));

            items.push(MenuDisplayItem::Item(MenuItem {
                label: "Detach Viewer".into(),
                icon: Some("-"),
                action: crate::actions::TermWmAction::DetachCurrentClient,
                disabled: false,
            }));

            if !workspaces.is_empty() {
                for ws in workspaces {
                    let is_current = ws == current_workspace;
                    let label = if is_current {
                        format!("Switch to Workspace: {ws} (current)")
                    } else {
                        format!("Switch to Workspace: {ws}")
                    };

                    items.push(MenuDisplayItem::Item(MenuItem {
                        label: label.into(),
                        icon: Some("→"),
                        action: crate::actions::TermWmAction::SwitchWorkspace(ws.clone()),
                        disabled: is_current,
                    }));

                    // Live windows/tasks totals for this workspace (#298).
                    // Absent entry = unknown (no reporting connection); a
                    // present zero-zero entry renders as an empty workspace.
                    if let Some(&(windows, tasks)) = workspace_totals.get(ws) {
                        let window_word = if windows == 1 { "window" } else { "windows" };
                        let task_word = if tasks == 1 { "task" } else { "tasks" };
                        items.push(info_item(format!(
                            "    └ {windows} {window_word} · {tasks} running {task_word}"
                        )));
                    }

                    let render_primary = |u: &crate::user_registry::UserEntry| {
                        let mut label = format!("    └ {}@{}", u.user, u.hostname);
                        if let Some(ip) = &u.ssh_ip {
                            if let Some(port) = u.ssh_port {
                                label.push_str(&format!(" ({}:{})", ip, port));
                            } else {
                                label.push_str(&format!(" ({})", ip));
                            }
                        }
                        label
                    };
                    let render_detail = |u: &crate::user_registry::UserEntry| -> Option<String> {
                        let mut parts = Vec::new();
                        if u.cols > 0 && u.rows > 0 {
                            parts.push(format!("{}×{}", u.cols, u.rows));
                        }
                        if u.connected_at_unix > 0 {
                            parts.push(format_uptime(u.connected_at_unix));
                        }
                        // conn_id is the strongest discriminator for same user+IP
                        parts.push(format!("#{}", u.conn_id));
                        if u.pid != 0 {
                            parts.push(format!("pid {}", u.pid));
                        }
                        if parts.is_empty() {
                            None
                        } else {
                            Some(format!("      {}", parts.join(" · ")))
                        }
                    };
                    let push_user = |u: &crate::user_registry::UserEntry,
                                     items: &mut Vec<
                        crate::components::MenuDisplayItem<crate::actions::TermWmAction>,
                    >| {
                        items.push(info_item(render_primary(u)));
                        if let Some(detail) = render_detail(u) {
                            items.push(info_item(detail));
                        }
                    };
                    let users_exist = all_users_by_ws.get(ws).is_some_and(|u| !u.is_empty());
                    if users_exist {
                        for u in &all_users_by_ws[ws] {
                            push_user(u, &mut items);
                        }
                    } else if ws == current_workspace && !self.user_registry.is_empty() {
                        for (_key, u) in self.user_registry.iter() {
                            push_user(u, &mut items);
                        }
                    }
                }
            }
            items.push(MenuDisplayItem::Separator);
        }

        let _ = (
            workspaces,
            current_workspace,
            all_users_by_ws,
            workspace_totals,
        );

        // ─────────────────────────────────────────────────────────
        // 3. WINDOW MANAGEMENT
        // ─────────────────────────────────────────────────────────
        if has_active {
            items.push(header("WINDOW MANAGEMENT"));
            let raw_title = self.window_title(focused);
            let title = crate::utils::truncate_with_ellipsis(&raw_title, 25);
            let super_key = self
                .keybindings()
                .combos_for(crate::actions::TermWmAction::OpenCommandPalette)
                .first()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "Super".to_string());

            items.push(MenuDisplayItem::Item(MenuItem {
                label: format!("Send {} to {}", super_key, title).into(),
                icon: Some("A"),
                action: crate::actions::TermWmAction::SendSuperKeyToWindow(focused),
                disabled: false,
            }));

            items.push(MenuDisplayItem::Item(MenuItem {
                label: format!("Close {}", title).into(),
                icon: Some("X"),
                action: crate::actions::TermWmAction::CloseWindow(focused),
                disabled: !self.window(focused).is_some_and(|w| w.closable()),
            }));

            let is_maxed = self.window(focused).is_some_and(|w| w.is_maximized());
            if !self.is_monocle() {
                let max_label = if is_maxed {
                    format!("Restore {}", title)
                } else {
                    format!("Maximize {}", title)
                };
                let max_icon = if is_maxed { "─" } else { "▢" };
                items.push(MenuDisplayItem::Item(MenuItem {
                    label: max_label.into(),
                    icon: Some(max_icon),
                    action: crate::actions::TermWmAction::MaximizeWindow(focused),
                    disabled: false,
                }));
                items.push(MenuDisplayItem::Item(MenuItem {
                    label: format!("Minimize {}", title).into(),
                    icon: Some("_"),
                    action: crate::actions::TermWmAction::MinimizeWindow(focused),
                    disabled: false,
                }));
            }

            let switch_titles = self.window_titles();
            if !switch_titles.is_empty() {
                for (key, switch_title) in switch_titles {
                    items.push(MenuDisplayItem::Item(MenuItem {
                        label: format!("Switch to: {}", switch_title).into(),
                        icon: Some("→"),
                        action: crate::actions::TermWmAction::FocusWindow(key),
                        disabled: key == focused,
                    }));
                }
            }
            items.push(MenuDisplayItem::Separator);
        }

        // ─────────────────────────────────────────────────────────
        // 4. VIEW & LAYOUT
        // ─────────────────────────────────────────────────────────
        items.push(header("VIEW & LAYOUT"));
        items.push(mi(
            self.monocle_mode.action_label(),
            Some("▢"),
            crate::actions::TermWmAction::ToggleMonocle,
        ));

        let layout_label = if self.managed_layout.is_some() {
            "View: Float Windows"
        } else {
            "View: Tile Windows"
        };
        let mut tile_item = mi(
            layout_label,
            Some("⊞"),
            crate::actions::TermWmAction::ToggleTiling,
        );
        if self.is_monocle()
            && let MenuDisplayItem::Item(ref mut item) = tile_item
        {
            item.disabled = true;
        }
        items.push(tile_item);
        items.push(MenuDisplayItem::Separator);

        // ─────────────────────────────────────────────────────────
        // 5. SETTINGS & SYSTEM
        // ─────────────────────────────────────────────────────────
        items.push(header("SETTINGS & SYSTEM"));

        let mouse_label = if self.mouse_capture_enabled {
            "Mouse: Disable Capture"
        } else {
            "Mouse: Enable Capture"
        };
        let clipboard_label = if self.clipboard_enabled {
            "Clipboard: Disable"
        } else {
            "Clipboard: Enable"
        };

        items.push(mi(
            mouse_label,
            Some("◆"),
            crate::actions::TermWmAction::ToggleMouseCapture,
        ));
        items.push(mi(
            clipboard_label,
            Some("■"),
            crate::actions::TermWmAction::ToggleClipboardMode,
        ));
        items.push(mi(
            "Paste",
            Some("■"),
            crate::actions::TermWmAction::PasteClipboard,
        ));

        let debug_log_visible = self
            .get_system_window::<system_tags::DebugLog>()
            .is_some_and(|k| self.window_state(k) == Some(WindowState::Mapped));
        let system_panel_visible = self
            .get_system_window::<system_tags::SystemPanel>()
            .is_some_and(|k| self.window_state(k) == Some(WindowState::Mapped));

        let debug_label = if debug_log_visible {
            "System: Disable Debug Log"
        } else {
            "System: Enable Debug Log"
        };
        let panel_label = if system_panel_visible {
            "System: Disable Panel"
        } else {
            "System: Enable Panel"
        };

        items.push(mi(
            debug_label,
            Some("≣"),
            crate::actions::TermWmAction::ToggleDebugWindow,
        ));
        items.push(mi(
            panel_label,
            Some("*"),
            crate::actions::TermWmAction::ToggleSystemPanel,
        ));

        items.push(mi("Help", Some("?"), crate::actions::TermWmAction::Help));
        items.push(mi(
            "Exit UI",
            Some("⏻"),
            crate::actions::TermWmAction::ExitUi,
        ));
        // Stop Gateway Daemon: opens a confirmation dialog (never stops
        // directly). Gated like the workspace group — requires the compiled-in
        // feature AND the runtime toggle.
        #[cfg(feature = "session-persistence")]
        if term_wm_config::runtime::session_persistence_enabled() {
            items.push(mi(
                "Stop Gateway Daemon",
                Some("⏻"),
                crate::actions::TermWmAction::OpenStopGatewayConfirm,
            ));
        }

        items
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_context::AppContext;
    use crate::components::NoopWmComponent;
    use crate::window::test_component::TestComponent;
    use crate::window::window_manager::TestOverlay;
    use crate::wm_config::WmConfig;
    use serial_test::serial;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_wm<O: Overlay<TermWmAction>>() -> WindowManager<TestComponent, NoopWmComponent, O> {
        WindowManager::with_config(
            WmConfig::default(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            HashMap::new(),
        )
    }

    fn key_esc() -> Event {
        Event::Key(crate::events::KeyEvent {
            code: crate::events::KeyCode::Esc,
            modifiers: crate::events::KeyModifiers::NONE,
            kind: crate::events::KeyKind::Press,
        })
    }

    /// Records `set_tab_outline` calls for tab-outline propagation assertions.
    struct TabTrackingOverlay {
        tab_outline: Option<Option<Instant>>,
    }

    impl Component<TermWmAction> for TabTrackingOverlay {
        fn render(
            &mut self,
            _backend: &mut dyn term_wm_render::RenderBackend,
            _area: LayoutRect,
            _ctx: &crate::component_context::ComponentContext,
            _registry: &mut crate::hitbox_registry::HitboxRegistry,
        ) {
        }
        fn update(
            &mut self,
            _action: TermWmAction,
            _ctx: &crate::component_context::ComponentContext,
            _actions: &mut std::collections::VecDeque<(crate::window::WindowKey, TermWmAction)>,
        ) {
        }
    }

    impl Overlay<TermWmAction> for TabTrackingOverlay {
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
        fn set_tab_outline(&mut self, expires_at: Option<Instant>) {
            self.tab_outline = Some(expires_at);
        }
    }

    /// Emits an action from `handle_events` to exercise the dismiss path.
    struct MenuActionOverlay;

    impl Component<TermWmAction> for MenuActionOverlay {
        fn render(
            &mut self,
            _backend: &mut dyn term_wm_render::RenderBackend,
            _area: LayoutRect,
            _ctx: &crate::component_context::ComponentContext,
            _registry: &mut crate::hitbox_registry::HitboxRegistry,
        ) {
        }
        fn update(
            &mut self,
            _action: TermWmAction,
            _ctx: &crate::component_context::ComponentContext,
            _actions: &mut std::collections::VecDeque<(crate::window::WindowKey, TermWmAction)>,
        ) {
        }
        fn handle_events(
            &mut self,
            _event: &Event,
            _ctx: &crate::component_context::ComponentContext,
        ) -> EventResult<TermWmAction> {
            EventResult::Action(TermWmAction::ExitUi)
        }
    }

    impl Overlay<TermWmAction> for MenuActionOverlay {
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    #[test]
    fn command_palette_empty_map_returns_no_action() {
        let mut wm: WindowManager<TestComponent> = make_wm();
        assert!(!wm.command_menu_visible());
        assert!(wm.handle_command_palette_event(&key_esc()).is_none());
    }

    #[test]
    fn command_menu_visible_derived_from_overlay_map() {
        let mut wm: WindowManager<TestComponent> = make_wm();
        assert!(!wm.command_menu_visible());
        wm.close_command_menu();
        assert!(!wm.command_menu_visible());
    }

    #[test]
    fn command_palette_key_returns_none_initially() {
        let wm: WindowManager<TestComponent> = make_wm();
        assert_eq!(wm.command_palette_key(), None);
    }

    #[test]
    fn command_palette_bounds_returns_none_when_no_key() {
        let wm = make_wm::<TestOverlay>();
        assert_eq!(wm.command_palette_bounds(), None);
    }

    #[test]
    fn command_palette_bounds_delegates_to_overlay_render_area() {
        let mut wm = make_wm::<TestOverlay>();
        let expected = LayoutRect {
            x: 10,
            y: 5,
            width: 40,
            height: 10,
        };
        wm.open_command_palette_overlay(TestOverlay {
            bounds: Some(expected),
        });
        assert_eq!(wm.command_palette_bounds(), Some(expected));
    }

    #[test]
    fn command_palette_bounds_returns_none_for_zero_size() {
        let mut wm = make_wm::<TestOverlay>();
        wm.open_command_palette_overlay(TestOverlay {
            bounds: Some(LayoutRect::default()),
        });
        // zero-size is filtered by render_area() → None
        assert_eq!(wm.command_palette_bounds(), None);
    }

    #[test]
    fn command_palette_bounds_returns_none_for_missing_overlay() {
        let mut wm = make_wm::<TestOverlay>();
        // Insert a different overlay but don't set command_palette_key
        wm.overlays.insert(TestOverlay {
            bounds: Some(LayoutRect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            }),
        });
        assert_eq!(wm.command_palette_bounds(), None);
    }

    #[test]
    fn open_overlay_sets_command_palette_input_mode() {
        let mut wm = make_wm::<TestOverlay>();
        wm.open_command_palette_overlay(TestOverlay { bounds: None });
        assert_eq!(wm.input_mode(), WmInputMode::CommandPalette);
        assert!(wm.command_palette_visible());
    }

    #[test]
    fn close_resets_input_mode_and_visibility() {
        let mut wm = make_wm::<TestOverlay>();
        wm.open_command_palette_overlay(TestOverlay { bounds: None });
        assert!(wm.command_palette_visible());
        wm.close_command_palette();
        assert_eq!(wm.input_mode(), WmInputMode::Passthrough);
        assert!(!wm.command_palette_visible());
    }

    #[test]
    fn set_tab_outline_sets_expiry_and_propagates() {
        let mut wm = make_wm::<TabTrackingOverlay>();
        let overlay = TabTrackingOverlay { tab_outline: None };
        wm.open_command_palette_overlay(overlay);
        wm.set_tab_outline_mode(Duration::from_millis(100));
        assert!(wm.tab_outline_until.is_some());
        let recorded = wm
            .overlays
            .get_mut(wm.command_palette_key().expect("palette key"))
            .expect("palette overlay")
            .as_any_mut()
            .downcast_mut::<TabTrackingOverlay>()
            .expect("TabTrackingOverlay")
            .tab_outline
            .expect("set_tab_outline called");
        assert!(recorded.is_some(), "expiry should be propagated to overlay");
    }

    #[test]
    fn clear_tab_outline_resets_state() {
        let mut wm = make_wm::<TabTrackingOverlay>();
        let overlay = TabTrackingOverlay { tab_outline: None };
        wm.open_command_palette_overlay(overlay);
        wm.set_tab_outline_mode(Duration::from_millis(100));
        assert!(wm.tab_outline_until.is_some());
        wm.clear_tab_outline();
        assert!(wm.tab_outline_until.is_none());
        let recorded = wm
            .overlays
            .get_mut(wm.command_palette_key().expect("palette key"))
            .expect("palette overlay")
            .as_any_mut()
            .downcast_mut::<TabTrackingOverlay>()
            .expect("TabTrackingOverlay")
            .tab_outline
            .expect("set_tab_outline called");
        assert!(recorded.is_none(), "clear should propagate None to overlay");
    }

    #[test]
    fn handle_event_action_dismisses_and_returns() {
        let mut wm = make_wm::<MenuActionOverlay>();
        wm.open_command_palette_overlay(MenuActionOverlay);
        let result = wm.handle_command_palette_event(&key_esc());
        assert_eq!(result, Some(TermWmAction::ExitUi));
        assert_eq!(wm.input_mode(), WmInputMode::Passthrough);
        assert!(!wm.command_palette_visible());
    }

    #[test]
    fn handle_event_ignored_keeps_palette() {
        let mut wm = make_wm::<TestOverlay>();
        wm.open_command_palette_overlay(TestOverlay { bounds: None });
        assert!(wm.handle_command_palette_event(&key_esc()).is_none());
        assert_eq!(wm.input_mode(), WmInputMode::CommandPalette);
        assert!(wm.command_palette_visible());
    }

    #[test]
    #[serial(wm_menu_items)]
    fn wm_menu_items_separates_controls_and_switcher() {
        use crate::components::{MenuDisplayItem, MenuItem};
        use crate::window::WindowState;
        let mut wm = make_wm::<TestOverlay>();
        let key = wm.create_window(TestComponent::Noop(crate::components::NoopComponent));
        wm.transition_window(key, WindowState::Mapped);
        wm.focus_window_key(key);
        wm.set_window_title(key, "alpha");

        let items = wm.wm_menu_items(
            &[],
            "",
            &[],
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );
        let switcher_idx = items.iter().position(|entry| {
            matches!(
                entry,
                MenuDisplayItem::Item(MenuItem { label, .. }) if label.starts_with("Switch to: ")
            )
        });
        let idx = switcher_idx.expect("Switch to entry present");
        assert!(idx > 0, "Switch to should not be first item");
        // 5-section layout: Switch to is after MinimizeWindow, not necessarily after Separator
        assert!(
            items[..idx]
                .iter()
                .any(|e| matches!(e, MenuDisplayItem::Separator)),
            "at least one separator must precede Switch to list"
        );
    }

    #[test]
    #[serial(wm_menu_items)]
    fn wm_menu_items_skips_separator_when_no_switch_targets() {
        use crate::components::{MenuDisplayItem, MenuItem};
        let mut wm = make_wm::<TestOverlay>();
        // Create a window that is focused but not part of the display order
        // (never registered/layout-managed), so `window_titles()` is empty
        // while the WM still has an active focus target. The Switch to list
        // must be skipped entirely (no dangling separator).
        let key = wm.create_window(TestComponent::Noop(crate::components::NoopComponent));
        wm.focus_window_key(key);

        let items = wm.wm_menu_items(
            &[],
            "",
            &[],
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );
        let has_switch = items.iter().any(|entry| {
            matches!(
                entry,
                MenuDisplayItem::Item(MenuItem { label, .. }) if label.starts_with("Switch to: ")
            )
        });
        assert!(
            !has_switch,
            "no Switch to entries expected when display order is empty"
        );
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    #[serial(wm_menu_items)]
    fn wm_menu_items_omits_workspace_group_when_runtime_disabled() {
        use crate::components::{MenuDisplayItem, MenuItem};
        let wm = make_wm::<TestOverlay>();

        // Disable the runtime toggle so workspace actions are suppressed.
        term_wm_config::runtime::init(term_wm_config::runtime::RuntimeConfig {
            session_persistence: false,
        });

        let items = wm.wm_menu_items(
            &["dev".into(), "prod".into()],
            "default",
            &[],
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );

        // Restore the default so parallel tests see the expected state.
        term_wm_config::runtime::init(term_wm_config::runtime::RuntimeConfig::default());

        let has_workspace_action = items.iter().any(|entry| {
            matches!(
                entry,
                MenuDisplayItem::Item(MenuItem { action, .. })
                    if matches!(
                        action,
                        TermWmAction::NewWorkspace
                            | TermWmAction::SwitchWorkspace(_)
                            | TermWmAction::DetachCurrentClient
                    )
            )
        });
        assert!(
            !has_workspace_action,
            "workspace actions must not appear when runtime toggle is disabled"
        );
        // The stop-gateway entry follows the same runtime gating.
        assert!(
            !items.iter().any(|entry| matches!(
                entry,
                MenuDisplayItem::Item(MenuItem {
                    action: TermWmAction::OpenStopGatewayConfirm,
                    ..
                })
            )),
            "stop-gateway entry must not appear when runtime toggle is disabled"
        );
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    #[serial(wm_menu_items)]
    fn wm_menu_items_stop_gateway_entry_opens_confirm_dialog() {
        use crate::components::{MenuDisplayItem, MenuItem};
        let wm = make_wm::<TestOverlay>();

        // Runtime enabled by default: the SETTINGS & SYSTEM section must offer
        // "Stop Gateway Daemon" bound to the OPENER action — never the
        // executor (`StopGatewayDaemon`), which is reachable only from the
        // confirmation dialog's Confirm branch (#298).
        let items = wm.wm_menu_items(
            &[],
            "",
            &[],
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );
        let mut stop_entries = items.iter().filter(|entry| {
            matches!(
                entry,
                MenuDisplayItem::Item(MenuItem { label, .. }) if label == "Stop Gateway Daemon"
            )
        });
        let Some(MenuDisplayItem::Item(MenuItem {
            action: TermWmAction::OpenStopGatewayConfirm,
            disabled,
            ..
        })) = stop_entries.next()
        else {
            panic!("palette must list 'Stop Gateway Daemon' with OpenStopGatewayConfirm");
        };
        assert!(!disabled, "stop-gateway entry must be enabled");
        assert!(
            stop_entries.next().is_none(),
            "'Stop Gateway Daemon' must appear exactly once"
        );
        assert!(
            !items.iter().any(|entry| matches!(
                entry,
                MenuDisplayItem::Item(MenuItem {
                    action: TermWmAction::StopGatewayDaemon,
                    ..
                })
            )),
            "the executor action must never be a palette entry"
        );
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    #[serial(wm_menu_items)]
    fn wm_menu_items_shows_workspace_group_when_runtime_enabled() {
        use crate::components::{MenuDisplayItem, MenuItem};
        let wm = make_wm::<TestOverlay>();

        // Runtime enabled by default: the workspace group must offer
        // New Workspace, Switch to Workspace entries (current one disabled),
        // and Detach Viewer.
        let items = wm.wm_menu_items(
            &["dev".into(), "prod".into()],
            "dev",
            &[],
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );

        let mut workspace: Vec<(String, bool)> = items
            .iter()
            .filter_map(|entry| match entry {
                MenuDisplayItem::Item(MenuItem {
                    label,
                    action:
                        TermWmAction::NewWorkspace
                        | TermWmAction::SwitchWorkspace(_)
                        | TermWmAction::DetachCurrentClient,
                    disabled,
                    ..
                }) => Some((label.to_string(), *disabled)),
                _ => None,
            })
            .collect();

        workspace.sort();
        assert_eq!(
            workspace,
            vec![
                ("Detach Viewer".to_string(), false),
                ("New Workspace".to_string(), false),
                ("Switch to Workspace: dev (current)".to_string(), true),
                ("Switch to Workspace: prod".to_string(), false),
            ],
            "workspace group must list all actions, disabling the current workspace"
        );
    }

    // ── Per-workspace WM totals (#298) ──────────────────────────────────

    fn workspace_totals_tests_common(
        totals: &WorkspaceTotals,
    ) -> Vec<crate::components::MenuDisplayItem<TermWmAction>> {
        let wm = make_wm::<TestOverlay>();
        wm.wm_menu_items(
            &["dev".into()],
            "dev",
            &[],
            &std::collections::BTreeMap::new(),
            totals,
        )
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    #[serial(wm_menu_items)]
    fn wm_menu_items_renders_totals_line_under_workspace() {
        use crate::components::{MenuDisplayItem, MenuItem};
        let mut totals = WorkspaceTotals::new();
        totals.insert("dev".to_string(), (3, 1));
        let items = workspace_totals_tests_common(&totals);

        let ws_idx = items
            .iter()
            .position(|entry| {
                matches!(
                    entry,
                    MenuDisplayItem::Item(MenuItem { label, .. })
                        if label == "Switch to Workspace: dev (current)"
                )
            })
            .expect("workspace entry found");
        let next = &items[ws_idx + 1];
        assert!(
            matches!(next, MenuDisplayItem::Item(MenuItem { label, disabled: true, .. })
                if label == "    └ 3 windows · 1 running task"),
            "totals line must sit directly under the workspace entry: {next:?}"
        );
    }

    #[test]
    #[serial(wm_menu_items)]
    fn wm_menu_items_omits_totals_line_when_unknown() {
        use crate::components::{MenuDisplayItem, MenuItem};
        let items = workspace_totals_tests_common(&WorkspaceTotals::new());
        assert!(
            !items.iter().any(|entry| matches!(
                entry,
                MenuDisplayItem::Item(MenuItem { label, .. }) if label.contains("running task")
            )),
            "no totals line may render without a stats entry (unknown != zero)"
        );
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    #[serial(wm_menu_items)]
    fn wm_menu_items_totals_zero_and_singular_forms() {
        use crate::components::{MenuDisplayItem, MenuItem};

        let mut zero = WorkspaceTotals::new();
        zero.insert("dev".to_string(), (0, 0));
        let items = workspace_totals_tests_common(&zero);
        assert!(items.iter().any(|entry| matches!(
            entry,
            MenuDisplayItem::Item(MenuItem { label, .. })
                if label == "    └ 0 windows · 0 running tasks"
        )));

        let mut single = WorkspaceTotals::new();
        single.insert("dev".to_string(), (1, 1));
        let items = workspace_totals_tests_common(&single);
        assert!(items.iter().any(|entry| matches!(
            entry,
            MenuDisplayItem::Item(MenuItem { label, .. })
                if label == "    └ 1 window · 1 running task"
        )));
    }

    #[test]
    #[serial(wm_menu_items)]
    fn wm_menu_items_renders_5_titled_section_headers() {
        use crate::components::{MenuDisplayItem, MenuItem};
        use crate::window::WindowState;
        let mut wm = make_wm::<TestOverlay>();
        let key = wm.create_window(TestComponent::Noop(crate::components::NoopComponent));
        wm.transition_window(key, WindowState::Mapped);
        wm.focus_window_key(key);

        let items = wm.wm_menu_items(
            &[],
            "",
            &[],
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );

        let headers: Vec<String> = items
            .iter()
            .filter_map(|entry| match entry {
                MenuDisplayItem::Item(MenuItem {
                    label,
                    disabled: true,
                    ..
                }) if label.starts_with("── ") => Some(label.to_string()),
                _ => None,
            })
            .collect();

        assert!(headers.contains(&"── QUICK ACTIONS ──".to_string()));
        #[cfg(feature = "session-persistence")]
        assert!(headers.contains(&"── WORKSPACES & COLLABORATION ──".to_string()));
        assert!(headers.contains(&"── WINDOW MANAGEMENT ──".to_string()));
        assert!(headers.contains(&"── VIEW & LAYOUT ──".to_string()));
        assert!(headers.contains(&"── SETTINGS & SYSTEM ──".to_string()));
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    #[serial(wm_menu_items)]
    fn wm_menu_items_nests_users_under_workspaces() {
        use crate::components::{MenuDisplayItem, MenuItem};
        let wm = make_wm::<TestOverlay>();

        let mut users_by_ws = std::collections::BTreeMap::new();
        users_by_ws.insert(
            "dev".to_string(),
            vec![crate::user_registry::UserEntry {
                conn_id: 1,
                user: "alice".to_string(),
                hostname: "host-a".to_string(),
                ssh_ip: Some("192.168.1.50".to_string()),
                ssh_port: Some(54321),
                cols: 0,
                rows: 0,
                connected_at_unix: 0,
                pid: 4242,
            }],
        );

        let items = wm.wm_menu_items(
            &["dev".to_string()],
            "dev",
            &[],
            &users_by_ws,
            &std::collections::BTreeMap::new(),
        );

        let ws_idx = items.iter().position(|entry| matches!(
            entry,
            MenuDisplayItem::Item(MenuItem { label, .. }) if label == "Switch to Workspace: dev (current)"
        )).expect("workspace entry found");

        let primary = &items[ws_idx + 1];
        assert!(
            matches!(primary, MenuDisplayItem::Item(MenuItem { label, disabled: true, .. }) if label == "    └ alice@host-a (192.168.1.50:54321)"),
            "primary user line must be nested directly beneath the workspace entry"
        );
        let detail = &items[ws_idx + 2];
        assert!(
            matches!(detail, MenuDisplayItem::Item(MenuItem { label, disabled: true, .. }) if label.contains("#1") && label.contains("pid 4242")),
            "detail line must contain discriminator (#conn and pid): {detail:?}"
        );
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    #[serial(wm_menu_items)]
    fn wm_menu_items_user_registry_fallback_nested() {
        use crate::components::{MenuDisplayItem, MenuItem};
        let mut wm = make_wm::<TestOverlay>();
        wm.user_registry.upsert(
            1,
            "bob".to_string(),
            "host-b".to_string(),
            Some("10.0.0.1".to_string()),
            None,
            0,
            0,
            0,
            0,
        );

        // Empty all_users_by_ws map forces fallback to local user_registry for current_workspace
        let items = wm.wm_menu_items(
            &["dev".to_string()],
            "dev",
            &[],
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );

        let ws_idx = items.iter().position(|entry| matches!(
            entry,
            MenuDisplayItem::Item(MenuItem { label, .. }) if label == "Switch to Workspace: dev (current)"
        )).expect("workspace entry found");

        let user_item = &items[ws_idx + 1];
        assert!(
            matches!(user_item, MenuDisplayItem::Item(MenuItem { label, disabled: true, .. }) if label == "    └ bob@host-b (10.0.0.1)"),
            "local user registry fallback must render nested under current workspace"
        );
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    #[serial(wm_menu_items)]
    fn wm_menu_items_workspace_follow_toggle_state() {
        use crate::components::{MenuDisplayItem, MenuItem};
        let mut wm = make_wm::<TestOverlay>();

        wm.workspace_follow_enabled = false;
        let items_off = wm.wm_menu_items(
            &[],
            "",
            &[],
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );
        assert!(items_off.iter().any(|entry| matches!(
            entry,
            MenuDisplayItem::Item(MenuItem { label, icon: Some("○"), action: TermWmAction::ToggleWorkspaceFollow, .. })
                if label == "Follow Workspaces: Enable"
        )));

        wm.workspace_follow_enabled = true;
        let items_on = wm.wm_menu_items(
            &[],
            "",
            &[],
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );
        assert!(items_on.iter().any(|entry| matches!(
            entry,
            MenuDisplayItem::Item(MenuItem { label, icon: Some("◎"), action: TermWmAction::ToggleWorkspaceFollow, .. })
                if label == "Follow Workspaces: Disable"
        )));
    }

    #[test]
    #[cfg(not(feature = "project-tasks"))]
    #[serial(wm_menu_items)]
    fn wm_menu_items_hides_project_tasks_when_feature_disabled() {
        use crate::components::{MenuDisplayItem, MenuItem};
        let wm = make_wm::<TestOverlay>();
        let tasks = vec![crate::project_tasks::ProjectTaskConfig {
            label: "should-be-hidden".into(),
            command: Some("echo hello".into()),
            args: None,
            cwd: None,
            env: std::collections::HashMap::new(),
            environments: Vec::new(),
            platforms: None,
        }];
        let items = wm.wm_menu_items(
            &[],
            "",
            &tasks,
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );
        assert!(
            !items.iter().any(|entry| matches!(
                entry,
                MenuDisplayItem::Item(MenuItem {
                    action: TermWmAction::RunProjectTask(_),
                    ..
                })
            )),
            "RunProjectTask must be hidden when project-tasks feature is disabled"
        );
        // No separator leak after Quick Actions when tasks are hidden
        let quick_actions_sep_idx = items
            .iter()
            .position(|e| matches!(e, MenuDisplayItem::Separator))
            .expect("at least one separator");
        assert!(
            quick_actions_sep_idx < 5,
            "first separator should be after Quick Actions, not leaked from tasks"
        );
    }

    #[test]
    #[cfg(not(feature = "session-persistence"))]
    #[serial(wm_menu_items)]
    fn wm_menu_items_hides_workspaces_when_feature_disabled_at_compile_time() {
        use crate::components::{MenuDisplayItem, MenuItem};
        let wm = make_wm::<TestOverlay>();
        let mut users_by_ws = std::collections::BTreeMap::new();
        users_by_ws.insert(
            "dev".to_string(),
            vec![crate::user_registry::UserEntry {
                conn_id: 1,
                user: "alice".to_string(),
                hostname: "host".to_string(),
                ssh_ip: None,
                ssh_port: None,
                cols: 0,
                rows: 0,
                connected_at_unix: 0,
                pid: 0,
            }],
        );
        let items = wm.wm_menu_items(
            &["dev".to_string()],
            "dev",
            &[],
            &users_by_ws,
            &std::collections::BTreeMap::new(),
        );
        assert!(
            !items.iter().any(|e| matches!(e, MenuDisplayItem::Item(MenuItem { label, .. }) if label.contains("Workspace") || label.contains("Follow Workspaces"))),
            "workspace UI must be hidden when session-persistence feature is disabled at compile time"
        );
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    #[serial(wm_menu_items)]
    fn wm_menu_items_user_renders_size_and_uptime() {
        use crate::components::{MenuDisplayItem, MenuItem};
        let wm = make_wm::<TestOverlay>();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        // Connected 90 seconds ago -> should show "1m" (or close)
        let connected_at = now.saturating_sub(90);
        let mut users_by_ws = std::collections::BTreeMap::new();
        users_by_ws.insert(
            "dev".to_string(),
            vec![crate::user_registry::UserEntry {
                conn_id: 1,
                user: "alice".to_string(),
                hostname: "host-a".to_string(),
                ssh_ip: None,
                ssh_port: None,
                cols: 80,
                rows: 24,
                connected_at_unix: connected_at,
                pid: 1234,
            }],
        );
        let items = wm.wm_menu_items(
            &["dev".to_string()],
            "dev",
            &[],
            &users_by_ws,
            &std::collections::BTreeMap::new(),
        );
        let ws_idx = items
            .iter()
            .position(|entry| {
                matches!(entry, MenuDisplayItem::Item(MenuItem { label, .. }) if label == "Switch to Workspace: dev (current)")
            })
            .expect("workspace entry found");
        // Primary line
        let primary = &items[ws_idx + 1];
        match primary {
            MenuDisplayItem::Item(MenuItem {
                label,
                disabled: true,
                ..
            }) => {
                assert!(
                    label.contains("alice@host-a"),
                    "primary must contain user@host: {label}"
                );
                // size/uptime moved to detail line, primary must not be overly long
                assert!(label.len() < 40, "primary line must stay compact: {label}");
            }
            other => panic!("unexpected primary item: {other:?}"),
        }
        // Detail line contains size, uptime, discriminator
        let detail = &items[ws_idx + 2];
        match detail {
            MenuDisplayItem::Item(MenuItem {
                label,
                disabled: true,
                ..
            }) => {
                assert!(
                    label.contains("80×24"),
                    "detail must contain terminal size: {label}"
                );
                assert!(
                    label.contains('m') || label.contains('s'),
                    "detail must contain uptime: {label}"
                );
                assert!(label.contains("#1"), "detail must contain conn id: {label}");
                assert!(
                    label.contains("pid 1234"),
                    "detail must contain pid: {label}"
                );
            }
            other => panic!("unexpected detail item: {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    fn format_uptime_produces_expected_strings() {
        // Zero diff
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(format_uptime(now), "0s");
        // 45s ago
        assert_eq!(format_uptime(now.saturating_sub(45)), "45s");
        // 5 minutes ago
        let five_min = format_uptime(now.saturating_sub(300));
        assert_eq!(five_min, "5m");
        // 2 hours ago
        let two_hours = format_uptime(now.saturating_sub(7200));
        assert_eq!(two_hours, "2h");
        // 1 day 2 hours ago
        let day = format_uptime(now.saturating_sub(86400 + 7200));
        assert_eq!(day, "1d 2h");
    }
}
