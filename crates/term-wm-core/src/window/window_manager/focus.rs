use crossterm::event::{Event, MouseEventKind};

use super::WindowManager;
use crate::layout::rect_contains;
use crate::window::WindowKey;

impl WindowManager {
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

    pub fn dispatch_focused_event<F>(&mut self, event: &Event, mut on_app: F) -> bool
    where
        F: FnMut(WindowKey, &Event) -> bool,
    {
        // Block mouse events in focused windows if direct mode is enabled
        if let Event::Mouse(mouse) = event {
            let focused = self.focused_window();
            let in_content = self.config.chrome_enabled
                && rect_contains(self.region_for_key(focused), mouse.column, mouse.row);
            if !(in_content && self.direct_mode(focused)) && self.handle_managed_event(event) {
                return true;
            }
        }

        // Hover-to-scroll: route scroll events to the window under the cursor
        // without changing keyboard focus. This lets you scroll any visible
        // window while keeping keyboard input directed at the active window.
        if let Event::Mouse(mouse) = event
            && matches!(
                mouse.kind,
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
            )
            && let Some(hovered) =
                self.hit_test_region_topmost(mouse.column, mouse.row, &self.managed_draw_order)
            && hovered != self.focused_window()
            && let Some(localized) = self.localize_event_content(hovered, event)
        {
            let adjusted = self.adjust_event_for_window(hovered, &localized);
            return on_app(hovered, &adjusted);
        }

        let Some((window_key, localized)) = self.focused_window_event(event) else {
            return false;
        };
        let adjusted = self.adjust_event_for_window(window_key, &localized);
        on_app(window_key, &adjusted)
    }

    pub fn focus_app_window(&mut self, key: WindowKey) {
        let prev = *self.focus.current();
        if prev != key {
            self.unmaximize_window(prev);
        }
        self.focus.set_current(key);
        self.bring_to_front_key(key);
        self.managed_draw_order = self.z_order.clone();
    }

    pub fn focus_window_key(&mut self, key: WindowKey) {
        // If another window was maximized (full-screen floating), restore it
        // so the newly-focused window isn't hidden behind it.
        let prev = *self.focus.current();
        if prev != key {
            self.unmaximize_window(prev);
        }
        self.focus.set_current(key);
        self.bring_to_front_key(key);
        self.managed_draw_order = self.z_order.clone();
    }

    pub(super) fn unmaximize_window(&mut self, key: WindowKey) {
        use crate::window::FloatRectSpec;
        let full = FloatRectSpec::Absolute(crate::window::FloatRect {
            x: self.managed_area.x as i32,
            y: self.managed_area.y as i32,
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

    pub fn handle_focus_event(&mut self, event: &Event, hit_targets: &[WindowKey]) -> bool {
        match event {
            Event::Key(key) => {
                if !self.keyboard_focus_enabled() {
                    return false;
                }
                let kb = self.keybindings();
                if kb.matches(crate::keybindings::Action::FocusNext, key) {
                    self.advance_focus(true);
                    true
                } else if kb.matches(crate::keybindings::Action::FocusPrev, key) {
                    self.advance_focus(false);
                    true
                } else {
                    false
                }
            }
            Event::Mouse(mouse) => {
                self.hover = Some((mouse.column, mouse.row));
                match mouse.kind {
                    MouseEventKind::Down(_) => {
                        if self.config.wm_overlay_enabled && !self.managed_draw_order.is_empty() {
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
                        let hit = self.hit_test_region(mouse.column, mouse.row, hit_targets);
                        if let Some(hit) = hit {
                            self.focus.set_current(hit);
                            if self.config.wm_overlay_enabled {
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
