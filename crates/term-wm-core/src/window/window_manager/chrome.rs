use super::WindowManager;
use crate::actions::TermWmAction;
use crate::components::{Component, Overlay, WmComponent};
use crate::layout::LayoutNode;
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

        let is_maxed = self.window(key).is_some_and(|w| w.is_maximized());

        if is_maxed {
            // UNMAXIMIZE PATH
            let prev_float = self.take_prev_floating_rect(key);
            if let Some(prev_spec) = prev_float {
                // Was floating: restore pre-maximize floating rect
                self.set_floating_rect(key, Some(prev_spec));
            } else {
                // Was tiled: re-insert key into layout tree
                self.clear_floating_rect(key);
                if let Some(ref mut layout) = self.managed_layout {
                    if !layout.root().collect_leaves().is_empty() {
                        layout.split_root(key, term_wm_layout_engine::InsertPosition::Right);
                    } else {
                        *layout = crate::layout::TilingLayout::new(LayoutNode::leaf(key));
                    }
                }
            }

            if let Some(w) = self.windows.get_mut(key) {
                w.set_maximized(false);
            }
            self.bring_floating_to_front_key(key);
            return;
        }

        // MAXIMIZE PATH
        let full = FloatRectSpec::Absolute(crate::window::FloatRect {
            x: self.managed_area.x,
            y: self.managed_area.y,
            width: self.managed_area.width,
            height: self.managed_area.height,
        });

        if let Some(current) = self.floating_rect(key) {
            // Was floating: save current floating rect to prev_floating_rect
            self.set_prev_floating_rect(key, Some(current));
        } else {
            // Was tiled: remove key from tree (it will float as maximized)
            if let Some(ref mut layout) = self.managed_layout {
                layout.remove_window(key);
            }
            self.set_prev_floating_rect(key, None);
        }

        self.set_floating_rect(key, Some(full));
        if let Some(w) = self.windows.get_mut(key) {
            w.set_maximized(true);
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
        let policy = self
            .window(key)
            .map(|w| w.close_policy())
            .unwrap_or_default();
        self.transition_window(key, WindowState::Unmapped);

        if policy == ClosePolicy::Destroy {
            if let Some(w) = self.windows.get_mut(key) {
                if let Some(c) = self.components.get_mut(w.component_key()) {
                    c.destroy();
                }
                self.components.remove(w.component_key());
            }
            self.windows.remove(key);
        }
    }
}
