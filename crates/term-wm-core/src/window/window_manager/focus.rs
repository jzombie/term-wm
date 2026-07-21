use crate::components::{Component, Overlay, WmComponent};
use crate::events::{Event, MouseEventKind};

use super::WindowManager;
use crate::actions::{EventResult, TermWmAction};
use crate::window::WindowKey;

impl<C: Component<TermWmAction>, L: WmComponent, O: Overlay<TermWmAction>> WindowManager<C, L, O> {
    pub fn set_focus_order(&mut self, order: Vec<WindowKey>) {
        self.focus.set_order(order);
    }

    pub fn set_focus(&mut self, key: WindowKey) {
        self.focus.set_current(key);
    }

    pub fn focused_window(&self) -> WindowKey {
        *self.focus.current()
    }

    pub fn focused_window_event(&self, event: &Event) -> Option<(WindowKey, Event)> {
        let window_key = self.focused_window();
        let localized = self
            .localize_event_content(window_key, event)
            .unwrap_or_else(|| event.clone());
        Some((window_key, localized))
    }

    /// Route an event to the focused window's component, returning EventResult.
    /// Returns None if no focused window could be determined.
    ///
    /// NOTE: Mouse events are dispatched entirely through `dispatch_mouse()`.
    /// This function handles keyboard events for WM-stored components.
    pub fn dispatch_focused_event(
        &mut self,
        event: &Event,
    ) -> Option<(WindowKey, EventResult<TermWmAction>)> {
        let (window_key, localized) = self.focused_window_event(event)?;
        let adjusted = self.adjust_event_for_window(window_key, &localized);
        let ctx = self.component_context_for(true, window_key);
        self.component_for_key_mut(window_key)
            .map(|c| (window_key, c.handle_events(&adjusted, &ctx)))
    }

    pub fn focus_app_window(&mut self, key: WindowKey) {
        self.focus.set_current(key);
        self.bring_to_front_key(key);
        self.mark_layout_dirty();
    }

    pub fn focus_window_key(&mut self, key: WindowKey) {
        // Focus shifts must not mutate geometry — maximized floating windows
        // retain their size regardless of Z-order changes.
        self.focus.set_current(key);
        self.bring_to_front_key(key);
        self.mark_layout_dirty();

        // If the command palette is open, rebuild its items with the new
        // focus state so "Switch to" and window management buttons reflect
        // the currently focused window.
        if !self.command_menu_visible() {
            return;
        }
        let Some(palette_key) = self.command_palette_key else {
            return;
        };

        // Build fresh items BEFORE accessing the overlay (borrow checker).
        let items = self.wm_menu_items();
        let supported = &self.supported_menu_actions;
        let filtered: Vec<_> = items
            .into_iter()
            .filter(|item| {
                supported.contains(&item.action)
                    || matches!(item.action, crate::actions::TermWmAction::FocusWindow(_))
            })
            .collect();

        if let Some(overlay) = self.overlays.get_mut(palette_key) {
            overlay.set_menu_items(filtered);
        }
    }

    #[expect(dead_code)]
    pub(super) fn unmaximize_window(&mut self, key: WindowKey) {
        use crate::window::FloatRectSpec;
        let full = FloatRectSpec::Absolute(crate::window::FloatRect {
            x: self.managed_area.x,
            y: self.managed_area.y,
            width: self.managed_area.width,
            height: self.managed_area.height,
        });
        if let Some(current) = self.floating_rect(key)
            && current == full
        {
            if let Some(prev) = self.take_prev_floating_rect(key) {
                self.set_floating_rect(key, Some(prev));
            } else {
                self.clear_floating_rect(key);
            }
            if let Some(w) = self.windows.get_mut(key) {
                w.is_maximized = false;
                w.borders_enabled = true;
            }
        }
    }

    pub(super) fn rebuild_focus_ring(&mut self, active_keys: &[WindowKey]) {
        use std::collections::BTreeSet;
        if active_keys.is_empty() {
            self.focus.set_order(Vec::new());
            return;
        }
        let active: BTreeSet<_> = active_keys.iter().copied().collect();
        let mut next_order: Vec<WindowKey> = Vec::with_capacity(active.len());
        let mut seen: BTreeSet<WindowKey> = BTreeSet::new();

        for &key in self.focus.order() {
            if active.contains(&key) && seen.insert(key) {
                next_order.push(key);
            }
        }
        for &key in active_keys {
            if seen.insert(key) {
                next_order.push(key);
            }
        }
        self.focus.set_order(next_order);
    }

    pub(crate) fn advance_focus(&mut self, forward: bool) {
        if self.focus.order().is_empty() {
            return;
        }
        self.focus.advance(forward);
        let focused = *self.focus.current();
        self.focus_window_key(focused);
    }

    pub(super) fn select_fallback_focus(&mut self) {
        if let Some(fallback) = self.focus.order().first().copied() {
            self.focus.set_current(fallback);
        }
    }

    pub fn handle_focus_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(key) => {
                if !self.keyboard_focus_enabled() {
                    return false;
                }
                let kb = self.keybindings();
                if kb.matches(TermWmAction::FocusNext, key) {
                    self.advance_focus(true);
                    true
                } else if kb.matches(TermWmAction::FocusPrev, key) {
                    self.advance_focus(false);
                    true
                } else {
                    false
                }
            }
            Event::Mouse(mouse) => {
                self.hover = Some((mouse.column, mouse.row));
                match mouse.kind {
                    MouseEventKind::Press(_) => {
                        if self.config.wm_command_menu_enabled
                            && !self.managed_draw_order.is_empty()
                        {
                            let hit = self.hit_test_region_topmost(
                                mouse.column,
                                mouse.row,
                                &self.managed_draw_order,
                            );
                            if let Some(key) = hit {
                                self.focus.set_current(key);
                                self.bring_floating_to_front_key(key);
                                return true;
                            }
                            return false;
                        }
                        let hit =
                            self.hit_test_region(mouse.column, mouse.row, &self.managed_draw_order);
                        if let Some(hit) = hit {
                            self.focus.set_current(hit);
                            if self.config.wm_command_menu_enabled {
                                self.bring_floating_to_front_key(hit);
                            }
                            true
                        } else {
                            false
                        }
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    #[expect(dead_code)]
    pub(super) fn focus_for_region(&self, key: WindowKey) -> Option<WindowKey> {
        if *self.focus.current() == key {
            Some(key)
        } else {
            self.focus.order().iter().copied().find(|&k| k == key)
        }
    }
}
