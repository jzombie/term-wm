use super::WindowManager;
use crate::actions::TermWmAction;
use crate::components::{Component, Overlay, WmComponent};
use crate::window::WindowKey;
use crate::window::entry::WindowState;

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

    /// Close a window: transition to Unmapped, destroy the component
    /// (kills child PTY processes), and remove from the SlotMap.
    ///
    /// All windows follow the same teardown path.  If the host application
    /// needs a toggleable window (debug log, help overlay), it must manage
    /// the show/hide lifecycle via `transition_window(key, Unmapped/Mapped)`
    /// and handle re-creation itself on reactivation.
    pub fn close_window(&mut self, key: WindowKey) {
        tracing::debug!(window_key = ?key, "closing window");
        self.transition_window(key, WindowState::Unmapped);

        // Destroy the component (kills child PTY processes) then
        // remove from SlotMap.
        if let Some(w) = self.windows.get_mut(key) {
            if let Some(c) = self.components.get_mut(w.component_key) {
                c.destroy();
            }
            self.components.remove(w.component_key);
        }
        self.windows.remove(key);
    }
}
