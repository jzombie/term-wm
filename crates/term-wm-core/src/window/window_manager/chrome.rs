use super::WindowManager;
use crate::actions::TermWmAction;
use crate::components::{Component, Overlay, WmComponent};
use crate::window::WindowKey;
use crate::window::entry::{ClosePolicy, WindowState};

impl<C: Component<TermWmAction>, L: WmComponent, O: Overlay<TermWmAction>> WindowManager<C, L, O> {
    pub fn minimize_window(&mut self, key: WindowKey) {
        self.transition_window(key, WindowState::Iconic);
    }

    pub fn restore_minimized(&mut self, key: WindowKey) {
        self.transition_window(key, WindowState::Mapped);
    }

    pub fn toggle_maximize(&mut self, key: WindowKey) {
        use crate::window::FloatRectSpec;
        let full = FloatRectSpec::Absolute(crate::window::FloatRect {
            x: self.managed_area.x,
            y: self.managed_area.y,
            width: self.managed_area.width,
            height: self.managed_area.height,
        });
        if let Some(current) = self.floating_rect(key) {
            if current == full {
                // Unmaximize: restore previous geometry
                if let Some(prev) = self.take_prev_floating_rect(key) {
                    self.set_floating_rect(key, Some(prev));
                }
                if let Some(w) = self.windows.get_mut(key) {
                    w.is_maximized = false;
                    w.borders_enabled = true;
                }
            } else {
                // Maximize: save current and expand
                self.set_prev_floating_rect(key, Some(current));
                self.set_floating_rect(key, Some(full));
                if let Some(w) = self.windows.get_mut(key) {
                    w.is_maximized = true;
                    w.borders_enabled = false;
                }
            }
            self.bring_floating_to_front_key(key);
            return;
        }
        // Tiled window → detach and maximize
        let prev_rect = if let Some(rect) = self.regions.get(key) {
            FloatRectSpec::Absolute(crate::window::FloatRect {
                x: rect.x,
                y: rect.y,
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

        // Purge from tiling tree before expanding to floating full-screen
        self.detach_from_tiling_layout(key);

        self.set_prev_floating_rect(key, Some(prev_rect));
        self.set_floating_rect(key, Some(full));
        if let Some(w) = self.windows.get_mut(key) {
            w.is_maximized = true;
            w.borders_enabled = false;
        }
        self.bring_floating_to_front_key(key);
    }

    pub fn shade_window(&mut self, key: WindowKey) {
        self.transition_window(key, WindowState::Shaded);
    }

    pub fn unshade_window(&mut self, key: WindowKey) {
        self.transition_window(key, WindowState::Mapped);
    }

    /// Close a window according to its [`ClosePolicy`].
    ///
    /// - `Destroy`: transition to `Unmapped`, destroy the component, and
    ///   remove the key from the SlotMap.
    /// - `Unmap`: transition to `Unmapped` only.  The component and key
    ///   stay alive so the window can be re-shown via `transition_window`.
    pub fn close_window(&mut self, key: WindowKey) {
        tracing::debug!(window_key = ?key, "closing window");
        let policy = self.window(key).map(|w| w.close_policy).unwrap_or_default();
        self.transition_window(key, WindowState::Unmapped);

        if policy == ClosePolicy::Destroy {
            if let Some(w) = self.windows.get_mut(key) {
                if let Some(c) = self.components.get_mut(w.component_key) {
                    c.destroy();
                }
                self.components.remove(w.component_key);
            }
            self.windows.remove(key);
        }
    }
}
