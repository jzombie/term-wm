use std::sync::Arc;

use super::WindowManager;
use crate::window::WindowKey;
use crate::window::entry::WindowState;

impl WindowManager {
    pub fn decorator(&self) -> Arc<dyn super::WindowDecorator> {
        self.config.decorator()
    }

    pub fn set_decorator(&mut self, decorator: Arc<dyn super::WindowDecorator>) {
        self.config.decorator = Some(decorator);
    }

    pub fn minimize_window(&mut self, key: WindowKey) {
        self.transition_window(key, WindowState::Iconic);
    }

    pub fn restore_minimized(&mut self, key: WindowKey) {
        self.transition_window(key, WindowState::Mapped);
    }

    pub fn toggle_maximize(&mut self, key: WindowKey) {
        use crate::window::FloatRectSpec;
        let full = FloatRectSpec::Absolute(crate::window::FloatRect {
            x: self.managed_area.x as i32,
            y: self.managed_area.y as i32,
            width: self.managed_area.width,
            height: self.managed_area.height,
        });
        if let Some(current) = self.floating_rect(key) {
            if current == full {
                if let Some(prev) = self.take_prev_floating_rect(key) {
                    self.set_floating_rect(key, Some(prev));
                }
            } else {
                self.set_prev_floating_rect(key, Some(current));
                self.set_floating_rect(key, Some(full));
            }
            self.bring_floating_to_front_key(key);
            return;
        }
        let prev_rect = if let Some(rect) = self.regions.get(key) {
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
        self.set_prev_floating_rect(key, Some(prev_rect));
        self.set_floating_rect(key, Some(full));
        self.bring_floating_to_front_key(key);
    }

    pub fn shade_window(&mut self, key: WindowKey) {
        self.transition_window(key, WindowState::Shaded);
    }

    pub fn unshade_window(&mut self, key: WindowKey) {
        self.transition_window(key, WindowState::Mapped);
    }

    pub fn close_window(&mut self, key: WindowKey) {
        tracing::debug!(window_key = ?key, "closing window");
        self.transition_window(key, WindowState::Unmapped);
        self.closed_windows.push(key);

        // Remove from SlotMap unless it's a system window (debug log, etc.)
        // that the WindowManager owns and can show again later.
        let is_system = self.windows.get(key).is_some_and(|w| w.is_system_window);
        if !is_system {
            self.windows.remove(key);
        }
    }
}
