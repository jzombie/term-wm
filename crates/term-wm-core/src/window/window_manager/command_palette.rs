use std::any::TypeId;
use std::time::{Duration, Instant};

use term_wm_layout_engine::LayoutRect;

use super::{OverlayKey, WindowManager};
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

    // TODO: Workspaces & current_workspace should be derived from context, I think
    pub fn wm_menu_items(
        &self,
        workspaces: &[String],
        current_workspace: &str,
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

        fn mi(
            label: &'static str,
            icon: Option<&'static str>,
            action: crate::actions::TermWmAction,
        ) -> MenuDisplayItem<crate::actions::TermWmAction> {
            MenuDisplayItem::Item(MenuItem {
                label: label.into(),
                icon,
                action,
                disabled: false,
            })
        }

        let focused = self.focused_window();
        let has_active = self.windows.contains_key(focused);

        let mut items: Vec<MenuDisplayItem<crate::actions::TermWmAction>> = vec![
            // Top group
            mi("Resume", Some("▶"), crate::actions::TermWmAction::CloseMenu),
            mi(
                "New Terminal",
                Some("+"),
                crate::actions::TermWmAction::NewTerminal,
            ),
            MenuDisplayItem::Separator,
        ];

        // Workspace group — always show "New workspace"
        items.push(mi(
            "New Workspace",
            Some("+"),
            crate::actions::TermWmAction::NewWorkspace,
        ));

        if !workspaces.is_empty() {
            for ws in workspaces {
                items.push(MenuDisplayItem::Item(MenuItem {
                    label: format!("Switch to Workspace: {ws}").into(),
                    icon: Some("→"),
                    action: crate::actions::TermWmAction::SwitchWorkspace(ws.clone()),
                    disabled: ws == current_workspace,
                }));
            }
        }
        items.push(MenuDisplayItem::Item(MenuItem {
            label: "Detach Viewer".into(),
            icon: Some("-"),
            action: crate::actions::TermWmAction::DetachCurrentClient,
            disabled: false,
        }));
        items.push(MenuDisplayItem::Separator);

        // Window management group (directly below top group)
        {
            if has_active {
                let raw_title = self.window_title(focused);
                let title = crate::utils::truncate_with_ellipsis(&raw_title, 25);
                let super_key = self
                    .keybindings()
                    .combos_for(crate::actions::TermWmAction::OpenCommandPalette)
                    .first()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "Super".to_string());

                // Send SUPER key to window
                items.push(MenuDisplayItem::Item(MenuItem {
                    label: format!("Send {} to {}", super_key, title).into(),
                    icon: Some("A"),
                    action: crate::actions::TermWmAction::SendSuperKeyToWindow(focused),
                    disabled: false,
                }));

                // Close window (disabled for non-closable windows)
                items.push(MenuDisplayItem::Item(MenuItem {
                    label: format!("Close {}", title).into(),
                    icon: Some("X"),
                    action: crate::actions::TermWmAction::CloseWindow(focused),
                    disabled: !self.window(focused).is_some_and(|w| w.closable()),
                }));

                // Maximize / Restore
                let is_maxed = self.window(focused).is_some_and(|w| w.is_maximized());
                if !self.is_monocle() {
                    items.push(MenuDisplayItem::Item(MenuItem {
                        label: (if is_maxed {
                            format!("Restore {}", title)
                        } else {
                            format!("Maximize {}", title)
                        })
                        .into(),
                        icon: Some(if is_maxed { "─" } else { "▢" }),
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

                // Switch to windows
                let switch_titles = self.window_titles();
                if !switch_titles.is_empty() {
                    items.push(MenuDisplayItem::Separator);
                    for (key, switch_title) in switch_titles {
                        items.push(MenuDisplayItem::Item(MenuItem {
                            label: format!("Switch to: {}", switch_title).into(),
                            icon: Some("→"),
                            action: crate::actions::TermWmAction::FocusWindow(key),
                            disabled: key == focused,
                        }));
                    }
                }
            }
        }

        // View group
        {
            items.push(MenuDisplayItem::Separator);
            items.push(mi(
                self.monocle_mode.action_label(),
                Some("▢"),
                crate::actions::TermWmAction::ToggleMonocle,
            ));
            {
                let label = if self.managed_layout.is_some() {
                    "View: Float Windows"
                } else {
                    "View: Tile Windows"
                };
                let mut item = mi(label, Some("⊞"), crate::actions::TermWmAction::ToggleTiling);
                if self.is_monocle()
                    && let MenuDisplayItem::Item(ref mut mi) = item
                {
                    mi.disabled = true;
                }
                items.push(item);
            }
        }

        // Settings groups
        {
            {
                items.push(MenuDisplayItem::Separator);
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
            }

            items.push(MenuDisplayItem::Separator);

            {
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
            }
        }

        // Help/Exit as last group
        {
            items.push(MenuDisplayItem::Separator);
            items.push(mi("Help", Some("?"), crate::actions::TermWmAction::Help));
            items.push(mi(
                "Exit UI",
                Some("⏻"),
                crate::actions::TermWmAction::ExitUi,
            ));
        }

        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_context::AppContext;
    use crate::components::NoopWmComponent;
    use crate::window::test_component::TestComponent;
    use crate::window::window_manager::TestOverlay;
    use crate::wm_config::WmConfig;
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
    fn wm_menu_items_separates_controls_and_switcher() {
        use crate::components::{MenuDisplayItem, MenuItem};
        use crate::window::WindowState;
        let mut wm = make_wm::<TestOverlay>();
        let key = wm.create_window(TestComponent::Noop(crate::components::NoopComponent));
        wm.transition_window(key, WindowState::Mapped);
        wm.focus_window_key(key);
        wm.set_window_title(key, "alpha");

        let items = wm.wm_menu_items(&[], "");
        let switcher_idx = items.iter().position(|entry| {
            matches!(
                entry,
                MenuDisplayItem::Item(MenuItem { label, .. }) if label.starts_with("Switch to: ")
            )
        });
        let idx = switcher_idx.expect("Switch to entry present");
        assert!(
            matches!(&items[idx - 1], MenuDisplayItem::Separator),
            "separator must precede the Switch to list"
        );
        assert!(
            matches!(&items[idx - 2], MenuDisplayItem::Item(_)),
            "window controls must precede the separator"
        );
    }

    #[test]
    fn wm_menu_items_skips_separator_when_no_switch_targets() {
        use crate::components::{MenuDisplayItem, MenuItem};
        let mut wm = make_wm::<TestOverlay>();
        // Create a window that is focused but not part of the display order
        // (never registered/layout-managed), so `window_titles()` is empty
        // while the WM still has an active focus target. The Switch to list
        // must be skipped entirely (no dangling separator).
        let key = wm.create_window(TestComponent::Noop(crate::components::NoopComponent));
        wm.focus_window_key(key);

        let items = wm.wm_menu_items(&[], "");
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
}
