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
                // Was tiled: restore Void → Leaf in layout tree
                self.clear_floating_rect(key);
                let void_id = self.window(key).and_then(|w| w.void_id());

                let mut restored = false;
                if let Some(vid) = void_id
                    && let Some(ref mut layout) = self.managed_layout
                {
                    restored = layout.replace_void_by_id(vid, LayoutNode::leaf(key));
                }

                // Force re-attachment if the Void node was destroyed (e.g., during minimize)
                if !restored && self.window_state(key) == Some(WindowState::Mapped) {
                    self.reattach_to_tiling_layout(key);
                }
            }

            if let Some(w) = self.windows.get_mut(key) {
                w.set_maximized(false);
                w.clear_void_id();
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
            // Was tiled: replace Leaf with Void in-place, preserving tree
            if let Some(ref mut layout) = self.managed_layout
                && let Some(void_id) = layout.replace_leaf_with_void(key)
                && let Some(w) = self.windows.get_mut(key)
            {
                w.set_void_id(Some(void_id));
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
        let w = match self.window(key) {
            Some(w) => w,
            None => {
                tracing::warn!(window_key = ?key, "close_window invoked on unknown or destroyed window");
                return;
            }
        };
        if !w.closable() {
            tracing::debug!(window_key = ?key, "ignoring close: window is not closable");
            return;
        }
        tracing::debug!(window_key = ?key, "closing window");
        let policy = w.close_policy();
        self.transition_window(key, WindowState::Unmapped);

        if policy == ClosePolicy::Destroy {
            if let Some(w) = self.windows.get_mut(key) {
                if let Some(c) = self.components.get_mut(w.component_key()) {
                    c.destroy();
                }
                self.components.remove(w.component_key());
            }
            self.windows.remove(key);
            // Drop any pending Direct Mode toast debounce for the removed
            // window; an already-armed flush task fires as a harmless no-op.
            self.direct_mode_debounce.cancel(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rect;
    use crate::app_context::AppContext;
    use crate::components::NoopComponent;
    use crate::window::WindowState;
    use crate::wm_config::WmConfig;
    use std::sync::Arc;

    fn make_wm() -> WindowManager<NoopComponent> {
        WindowManager::<NoopComponent>::with_config(
            WmConfig::default(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        )
    }

    #[test]
    fn maximize_minimize_unmaximize_retains_window() {
        let mut wm = make_wm();

        let key_a = wm.create_window(NoopComponent);
        let key_b = wm.create_window(NoopComponent);
        wm.transition_window(key_a, WindowState::Mapped);
        wm.transition_window(key_b, WindowState::Mapped);

        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });

        // 1. Maximize window A (creates Void in tree, stores void_id)
        wm.toggle_maximize(key_a);
        assert!(wm.window(key_a).is_some_and(|w| w.is_maximized()));
        assert!(wm.is_window_floating(key_a));

        // 2. Minimize window A (destroys Void, clears void_id)
        wm.minimize_window(key_a);
        assert_eq!(wm.window_state(key_a), Some(WindowState::Iconic));

        // 3. Restore window A (state -> Mapped)
        wm.restore_minimized(key_a);
        assert_eq!(wm.window_state(key_a), Some(WindowState::Mapped));

        // 4. Unmaximize window A (toggle_maximize — void_id pointer is now stale/None)
        wm.toggle_maximize(key_a);
        assert!(!wm.window(key_a).is_some_and(|w| w.is_maximized()));
        assert!(!wm.is_window_floating(key_a));

        // Assert: Window A was forcefully reattached to tiling layout via fallback
        assert!(
            wm.layout_contains(key_a),
            "Window A must be reattached to tiling layout despite stale void_id"
        );
        assert!(
            wm.z_order.contains(&key_a),
            "Window A must remain in z_order"
        );

        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        let reg = wm.region(key_a);
        assert!(
            reg.width > 0 && reg.height > 0,
            "Window A must have a valid rendered region after unmaximize"
        );
    }
}
