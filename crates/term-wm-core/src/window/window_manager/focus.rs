use crossterm::event::{Event, MouseEventKind};

use super::WindowId;
use super::WindowManager;
use crate::layout::rect_contains;

impl<Id: Copy + Eq + Ord + std::fmt::Debug + 'static> WindowManager<Id> {
    pub fn focus(&self) -> Id {
        *self.app_focus.current()
    }

    pub fn focused_window(&self) -> WindowId<Id> {
        *self.wm_focus.current()
    }

    pub fn focused_window_event(&self, event: &Event) -> Option<(WindowId<Id>, Event)> {
        let window_id = self.focused_window();
        let localized = self
            .localize_event_content(window_id, event)
            .unwrap_or_else(|| event.clone());
        Some((window_id, localized))
    }

    pub fn dispatch_focused_event<F>(&mut self, event: &Event, mut on_app: F) -> bool
    where
        F: FnMut(Id, &Event) -> bool,
    {
        if let Event::Mouse(mouse) = event {
            let focused = self.focused_window();
            let in_content = self.config.chrome_enabled
                && rect_contains(self.region_for_id(focused), mouse.column, mouse.row);
            if !(in_content && self.direct_mode(focused))
                && self.handle_managed_event(event)
            {
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
            return match hovered {
                WindowId::App(id) => on_app(id, &adjusted),
                WindowId::System(system_id) => {
                    self.dispatch_system_window_event_localized(system_id, &adjusted)
                }
            };
        }

        let Some((window_id, localized)) = self.focused_window_event(event) else {
            return false;
        };
        let adjusted = self.adjust_event_for_window(window_id, &localized);
        match window_id {
            WindowId::App(id) => on_app(id, &adjusted),
            WindowId::System(system_id) => {
                self.dispatch_system_window_event_localized(system_id, &adjusted)
            }
        }
    }

    pub fn set_focus(&mut self, focus: Id) {
        self.app_focus.set_current(focus);
    }

    pub fn focus_app_window(&mut self, id: Id) {
        let prev = *self.wm_focus.current();
        if prev != WindowId::app(id) {
            self.unmaximize_window(prev);
        }

        self.app_focus.set_current(id);
        self.set_wm_focus(WindowId::app(id));
        self.bring_to_front_id(WindowId::app(id));
        self.managed_draw_order = self.z_order.clone();
    }

    pub fn set_focus_order(&mut self, order: Vec<Id>) {
        self.app_focus.set_order(order);
    }

    pub fn advance_focus(&mut self, forward: bool) {
        self.app_focus.advance(forward);
    }

    pub fn wm_focus(&self) -> WindowId<Id> {
        *self.wm_focus.current()
    }

    pub fn wm_focus_app(&self) -> Option<Id> {
        (*self.wm_focus.current()).as_app()
    }

    pub(super) fn set_wm_focus(&mut self, focus: WindowId<Id>) {
        self.wm_focus.set_current(focus);
        if let Some(app_id) = focus.as_app()
            && let Some(app_focus) = self.focus_for_region(app_id)
        {
            self.app_focus.set_current(app_focus);
        }
    }

    pub fn focus_window_id(&mut self, id: WindowId<Id>) {
        // If another window was maximized (full-screen floating), restore it
        // so the newly-focused window isn't hidden behind it.
        let prev = *self.wm_focus.current();
        if prev != id {
            self.unmaximize_window(prev);
        }

        self.set_wm_focus(id);
        self.bring_to_front_id(id);
        self.managed_draw_order = self.z_order.clone();
        if let Some(app_id) = id.as_app()
            && let Some(app_focus) = self.focus_for_region(app_id)
        {
            self.app_focus.set_current(app_focus);
        }
    }

    pub(super) fn unmaximize_window(&mut self, id: WindowId<Id>) {
        use crate::window::FloatRectSpec;
        let full = FloatRectSpec::Absolute(crate::window::FloatRect {
            x: self.managed_area.x as i32,
            y: self.managed_area.y as i32,
            width: self.managed_area.width,
            height: self.managed_area.height,
        });
        if let Some(current) = self.floating_rect(id)
            && current == full
        {
            if let Some(prev) = self.take_prev_floating_rect(id) {
                self.set_floating_rect(id, Some(prev));
            } else {
                self.clear_floating_rect(id);
            }
        }
    }

    pub(super) fn set_wm_focus_order(&mut self, order: Vec<WindowId<Id>>) {
        self.wm_focus.set_order(order);
    }

    pub(super) fn rebuild_wm_focus_ring(&mut self, active_ids: &[WindowId<Id>]) {
        use std::collections::BTreeSet;
        if active_ids.is_empty() {
            self.set_wm_focus_order(Vec::new());
            return;
        }
        let active: BTreeSet<_> = active_ids.iter().copied().collect();
        let mut next_order: Vec<WindowId<Id>> = Vec::with_capacity(active.len());
        let mut seen: BTreeSet<WindowId<Id>> = BTreeSet::new();

        for &id in self.wm_focus.order() {
            if active.contains(&id) && seen.insert(id) {
                next_order.push(id);
            }
        }
        for &id in active_ids {
            if seen.insert(id) {
                next_order.push(id);
            }
        }
        self.set_wm_focus_order(next_order);
    }

    pub(super) fn advance_wm_focus(&mut self, forward: bool) {
        if self.wm_focus.order().is_empty() {
            return;
        }
        self.wm_focus.advance(forward);
        let focused = *self.wm_focus.current();
        self.focus_window_id(focused);
    }

    pub(super) fn select_fallback_focus(&mut self) {
        if let Some(fallback) = self.wm_focus.order().first().copied() {
            self.set_wm_focus(fallback);
        }
    }

    pub fn handle_focus_event<F>(&mut self, event: &Event, hit_targets: &[Id], map: F) -> bool
    where
        F: Fn(Id) -> Id,
    {
        match event {
            Event::Key(key) => {
                if !self.keyboard_focus_enabled {
                    return false;
                }
                let kb = &self.keybindings;
                if kb.matches(crate::keybindings::Action::FocusNext, key) {
                    if self.config.wm_overlay_enabled {
                        self.advance_wm_focus(true);
                    } else {
                        self.app_focus.advance(true);
                        let focused_app = *self.app_focus.current();
                        let region = focused_app;
                        self.set_wm_focus(WindowId::app(region));
                        self.bring_to_front_id(WindowId::app(region));
                        self.managed_draw_order = self.z_order.clone();
                    }
                    true
                } else if kb.matches(crate::keybindings::Action::FocusPrev, key) {
                    if self.config.wm_overlay_enabled {
                        self.advance_wm_focus(false);
                    } else {
                        self.app_focus.advance(false);
                        let focused_app = *self.app_focus.current();
                        self.set_wm_focus(WindowId::app(focused_app));
                        self.bring_to_front_id(WindowId::app(focused_app));
                        self.managed_draw_order = self.z_order.clone();
                    }
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
                            if let Some(id) = hit {
                                self.set_wm_focus(id);
                                self.bring_floating_to_front_id(id);
                                return true;
                            }
                            return false;
                        }
                        let hit = self.hit_test_region(mouse.column, mouse.row, hit_targets);
                        if let Some(hit) = hit {
                            self.app_focus.set_current(map(hit));
                            if self.config.wm_overlay_enabled {
                                self.set_wm_focus(WindowId::app(hit));
                                self.bring_floating_to_front(hit);
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

    pub(super) fn focus_for_region(&self, id: Id) -> Option<Id> {
        if self.app_focus.order().is_empty() {
            if id == *self.app_focus.current() {
                Some(*self.app_focus.current())
            } else {
                None
            }
        } else {
            self.app_focus
                .order()
                .iter()
                .copied()
                .find(|focus| id == *focus)
        }
    }
}
