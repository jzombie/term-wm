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

    pub fn wm_menu_items(
        &self,
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
        let selection_label = if self.window_selection_enabled {
            "Clipboard: Disable Selection"
        } else {
            "Clipboard: Enable Selection"
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
                "New Window",
                Some("+"),
                crate::actions::TermWmAction::NewWindow,
            ),
            MenuDisplayItem::Separator,
        ];

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

                // Close window
                items.push(MenuDisplayItem::Item(MenuItem {
                    label: format!("Close {}", title).into(),
                    icon: Some("X"),
                    action: crate::actions::TermWmAction::CloseWindow(focused),
                    disabled: false,
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
                for (key, switch_title) in self.window_titles() {
                    items.push(MenuDisplayItem::Item(MenuItem {
                        label: format!("Switch to: {}", switch_title).into(),
                        icon: Some("→"),
                        action: crate::actions::TermWmAction::FocusWindow(key),
                        disabled: key == focused,
                    }));
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

        // Settings group
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
                selection_label,
                Some("●"),
                crate::actions::TermWmAction::ToggleWindowSelection,
            ));
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

            // Help/Exit as last group
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
