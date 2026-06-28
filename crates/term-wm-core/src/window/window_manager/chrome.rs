use std::sync::Arc;

use crossterm::event::{Event, KeyEvent, KeyEventKind, KeyEventState, MouseEventKind};

use super::{WindowId, WindowManager};

impl<Id: Copy + Eq + Ord + std::fmt::Debug + 'static> WindowManager<Id> {
    pub fn decorator(&self) -> Arc<dyn super::WindowDecorator> {
        Arc::clone(&self.decorator)
    }

    pub fn set_decorator(&mut self, decorator: Arc<dyn super::WindowDecorator>) {
        self.decorator = decorator;
    }

    pub fn handle_managed_event(&mut self, event: &Event) -> bool {
        if let Event::Mouse(mouse) = event {
            self.hover = Some((mouse.column, mouse.row));
        }
        if self.config.wm_overlay_enabled {
            if let Event::Mouse(mouse) = event
                && self.panel_active()
                && self.top_panel.as_ref().is_some_and(|p| {
                    crate::layout::rect_contains(p.area(), mouse.column, mouse.row)
                })
            {
                if matches!(mouse.kind, MouseEventKind::Down(_))
                    && self
                        .top_panel
                        .as_ref()
                        .is_some_and(|p| p.menu_icon_contains_point(mouse.column, mouse.row))
                {
                    if self.wm_overlay_visible() {
                        self.close_wm_overlay();
                    } else {
                        self.open_wm_overlay();
                    }
                } else if self
                    .top_panel
                    .as_ref()
                    .is_some_and(|p| p.hit_test_mouse_capture(event))
                {
                    self.toggle_mouse_capture();
                } else if self
                    .top_panel
                    .as_ref()
                    .is_some_and(|p| p.hit_test_selection(event))
                {
                    self.toggle_window_selection();
                } else if self
                    .top_panel
                    .as_ref()
                    .is_some_and(|p| p.hit_test_clipboard(event))
                {
                    self.toggle_clipboard_enabled();
                } else if self
                    .top_panel
                    .as_ref()
                    .is_some_and(|p| p.hit_test_copy(event))
                {
                    self.copy_selection_to_clipboard();
                } else if let Some(id) = self
                    .top_panel
                    .as_ref()
                    .and_then(|p| p.hit_test_window(event))
                {
                    if self.is_minimized(id) {
                        self.restore_minimized(id);
                    }
                    self.focus_window_id(id);
                }
                return true;
            }
            if let Event::Mouse(mouse) = event
                && matches!(mouse.kind, MouseEventKind::Down(_))
            {
                self.focus_window_at(mouse.column, mouse.row);
            }
            if self.handle_resize_event(event) {
                return true;
            }
            if self.handle_header_drag_event(event) {
                return true;
            }
            if self.handle_system_window_event(event) {
                return true;
            }
        }
        // Hint click in bottom bar — works in both standalone and embedded modes
        if let Event::Mouse(mouse) = event
            && matches!(mouse.kind, MouseEventKind::Down(_))
            && self
                .bottom_panel
                .as_ref()
                .is_some_and(|p| crate::layout::rect_contains(p.area(), mouse.column, mouse.row))
            && let Some(action) = self
                .bottom_panel
                .as_ref()
                .and_then(|p| p.hit_test_hint(event))
        {
            if let Some(combo) = self.keybindings.first_combo(action) {
                self.synthetic_event = Some(Event::Key(KeyEvent {
                    code: combo.code,
                    modifiers: combo.mods,
                    kind: KeyEventKind::Press,
                    state: KeyEventState::NONE,
                }));
            }
            return true;
        }
        if let Some(layout) = self.managed_layout.as_mut() {
            return layout.handle_event(event, self.managed_area);
        }
        false
    }

    pub fn minimize_window(&mut self, id: WindowId<Id>) {
        if self.is_minimized(id) {
            return;
        }
        self.clear_floating_rect(id);
        self.z_order.retain(|x| *x != id);
        self.managed_draw_order.retain(|x| *x != id);
        self.set_minimized(id, true);
        if self.wm_focus.current() == id {
            self.select_fallback_focus();
        }
    }

    pub fn restore_minimized(&mut self, id: WindowId<Id>) {
        if !self.is_minimized(id) {
            return;
        }
        self.set_minimized(id, false);
        if !self.z_order.contains(&id) {
            self.z_order.push(id);
        }
        if !self.managed_draw_order.contains(&id) {
            self.managed_draw_order.push(id);
        }
    }

    pub fn toggle_maximize(&mut self, id: WindowId<Id>) {
        use crate::window::FloatRectSpec;
        let full = FloatRectSpec::Absolute(crate::window::FloatRect {
            x: self.managed_area.x as i32,
            y: self.managed_area.y as i32,
            width: self.managed_area.width,
            height: self.managed_area.height,
        });
        if let Some(current) = self.floating_rect(id) {
            if current == full {
                if let Some(prev) = self.take_prev_floating_rect(id) {
                    self.set_floating_rect(id, Some(prev));
                }
            } else {
                self.set_prev_floating_rect(id, Some(current));
                self.set_floating_rect(id, Some(full));
            }
            self.bring_floating_to_front_id(id);
            return;
        }
        let prev_rect = if let Some(rect) = self.regions.get(id) {
            FloatRectSpec::Absolute(crate::window::FloatRect {
                x: rect.x as i32,
                y: rect.y as i32,
                width: rect.width,
                height: rect.height,
            })
        } else {
            FloatRectSpec::Percent {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            }
        };
        self.set_prev_floating_rect(id, Some(prev_rect));
        self.set_floating_rect(id, Some(full));
        self.bring_floating_to_front_id(id);
    }

    pub fn close_window(&mut self, id: WindowId<Id>) {
        tracing::debug!(window_id = ?id, "closing window");
        if let WindowId::System(system_id) = id {
            self.hide_system_window(system_id);
            return;
        }

        self.clear_floating_rect(id);
        self.z_order.retain(|x| *x != id);
        self.managed_draw_order.retain(|x| *x != id);
        self.set_minimized(id, false);
        self.regions.remove(id);
        if self.wm_focus.current() == id {
            self.select_fallback_focus();
        }
        if let Some(app_id) = id.as_app() {
            self.closed_app_windows.push(app_id);
        }
    }
}
