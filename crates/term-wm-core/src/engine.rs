use crate::actions::TermWmAction;
use crate::components::{Component, Overlay, WmComponent};
use crate::draw_plan::{DrawPlan, RegionType, RenderRegion, ZLayer};
use crate::window::{ComponentTag, WindowManager};
use term_wm_layout_engine::LayoutRect;

/// Pre-allocated capacity for the draw plan
const INITIAL_DRAW_PLAN_CAPACITY: usize = 256;

/// The core engine that manages draw plan generation.
/// Produces spatial IR (DrawPlan) without any rendering dependencies.
pub struct CoreEngine {
    /// Pre-allocated draw plan buffer (cleared, not deallocated, each frame)
    draw_plan: DrawPlan,
    /// Dirty flag for fast path
    is_dirty: bool,
}

impl CoreEngine {
    pub fn new() -> Self {
        Self {
            draw_plan: DrawPlan::with_capacity(INITIAL_DRAW_PLAN_CAPACITY),
            is_dirty: true,
        }
    }

    /// Project the current draw plan without causing heap allocation.
    /// Returns a reference to the draw plan struct.
    pub fn project_draw_plan<
        C: Component<TermWmAction> + 'static,
        L: WmComponent,
        O: Overlay<TermWmAction>,
    >(
        &mut self,
        width: u32,
        height: u32,
        wm: &mut WindowManager<C, L, O>,
    ) -> &DrawPlan {
        // Check if either the engine or the WindowManager has changed
        if !self.is_dirty && !wm.layout_dirty() {
            self.draw_plan.sort_by_layer();
            return &self.draw_plan;
        }

        // Clear plan (retains capacity, no allocation)
        self.draw_plan.clear();

        // Generate new regions from layout state
        self.generate_regions(width, height, wm);

        // Apply monocle mode if active
        // This hides non-focused windows and reorders Z-indices
        // without mutating WindowManager.z_order
        if wm.is_monocle() {
            let focused_key = wm.focused_window();
            let screen = wm.managed_area();
            self.draw_plan.apply_monocle_culling(focused_key, screen);
            self.draw_plan.apply_monocle_z_order(focused_key);
        }

        // Sort by layer for correct layering
        self.draw_plan.sort_by_layer();

        // Mark as clean
        self.is_dirty = false;
        wm.clear_layout_dirty();

        &self.draw_plan
    }

    /// Generate render regions from current layout state.
    fn generate_regions<
        C: Component<TermWmAction> + 'static,
        L: WmComponent,
        O: Overlay<TermWmAction>,
    >(
        &mut self,
        _width: u32,
        _height: u32,
        wm: &mut WindowManager<C, L, O>,
    ) {
        // 1. Generate terminal window regions
        for &window_key in &wm.managed_draw_order {
            let region = wm.full_region_for_key(window_key);
            if region.width == 0 || region.height == 0 {
                continue;
            }

            let is_focused = wm.focused_window() == window_key;

            // Convert ratatui::Rect to LayoutRect
            let layout_rect = LayoutRect {
                x: region.x,
                y: region.y,
                width: region.width,
                height: region.height,
            };

            self.draw_plan.push(RenderRegion {
                bounds: layout_rect,
                layer: ZLayer::TiledWindow,
                dimmed: !is_focused,
                region_type: RegionType::Window(window_key),
                hidden: false,
            });
        }

        // TODO: Remove?
        // 2. Generate panel regions (top and bottom)
        // Panels are rendered by the WindowManager, not as window regions
        // Their z-index is higher than windows

        // TODO: Remove?
        // 3. Generate overlay regions (if active)
        // Overlays are rendered by the WindowManager
        // Their z-index is highest

        // 4. Generate notification toast regions
        generate_notification_regions(&mut self.draw_plan, wm);
    }

    /// Mark the engine as needing re-projection.
    pub fn mark_dirty(&mut self) {
        self.is_dirty = true;
    }

    /// Check if the engine is dirty.
    pub fn is_dirty(&self) -> bool {
        self.is_dirty
    }

    /// Get the current draw plan (read-only).
    pub fn draw_plan(&self) -> &DrawPlan {
        &self.draw_plan
    }
}

/// Generate notification toast regions and append them to the draw plan.
///
/// Extracted as a standalone function so that the geometric circuit-breaker
/// early return only skips notification layers — not the entire pipeline.
fn generate_notification_regions<
    C: Component<TermWmAction> + 'static,
    L: WmComponent,
    O: Overlay<TermWmAction>,
>(
    plan: &mut DrawPlan,
    wm: &WindowManager<C, L, O>,
) {
    use std::sync::Arc;
    use textwrap::Options;

    const TOAST_W: u16 = 40;
    const H_MARGIN: u16 = 2;
    const Y_OFFSET: u16 = 0;
    /// Rows to shift toasts down when the top panel overlays row 0 without
    /// reserving layout space (cramped monocle, #357). The value is the
    /// top panel's single-row height invariant (the overlay renders exactly
    /// one row and `split_area` claims `self.height`, default 1); it cannot
    /// be derived from `top_claimed_area()`, which is 0 in cramped mode by
    /// design. In every other mode `managed.y` already accounts for the
    /// panel, so no shift applies.
    const OVERLAY_PANEL_Y_SHIFT: u16 = 1;
    const GAP: u16 = 0;

    let managed = wm.managed_area();
    let notif_count = wm.notifications().len();
    if notif_count == 0 {
        return;
    }

    // Circuit breaker — terminal too narrow; skip notification layers only.
    if managed.width <= H_MARGIN.saturating_mul(2).saturating_add(2) {
        return;
    }

    let actual_w = TOAST_W.min(managed.width.saturating_sub(H_MARGIN.saturating_mul(2)));
    let inner_w = actual_w.saturating_sub(2) as usize;
    let wrap_opts = Options::new(inner_w);

    // Shift only when the panel actually paints row 0 as an overlay: cramped
    // monocle with a registered top panel component. The overlay render path
    // paints unconditionally (it ignores the component `visible` flag), so
    // presence — not visibility — is the sound gate. `top_claimed_area()`
    // cannot be used: it is 0 in cramped mode by design.
    let panel_overlays_top_row =
        wm.is_monocle_cramped() && wm.get_semantic_component(ComponentTag::TopPanel).is_some();
    let mut y_offset: u16 = if panel_overlays_top_row {
        OVERLAY_PANEL_Y_SHIFT
    } else {
        Y_OFFSET
    };

    for notification in wm.notifications().renderable().rev() {
        let lines = textwrap::wrap(&notification.message, &wrap_opts);
        let h = (lines.len() as u16).saturating_add(2);
        let h = h.min(
            managed
                .height
                .saturating_sub(y_offset.saturating_add(H_MARGIN)),
        );
        if h < 3 {
            break;
        }

        let x = managed
            .x
            .saturating_add(managed.width as i32)
            .saturating_sub(actual_w as i32)
            .saturating_sub(H_MARGIN as i32);

        plan.push(RenderRegion {
            bounds: LayoutRect {
                x,
                y: managed.y.saturating_add(y_offset as i32),
                width: actual_w,
                height: h,
            },
            layer: ZLayer::Notification,
            dimmed: false,
            region_type: RegionType::Notification(Arc::clone(&notification.message)),
            hidden: false,
        });

        y_offset = y_offset.saturating_add(h).saturating_add(GAP);
    }
}

impl Default for CoreEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_core_engine_new() {
        let engine = CoreEngine::new();
        assert!(engine.is_dirty());
        assert!(engine.draw_plan().is_empty());
    }

    #[test]
    fn test_mark_dirty() {
        let mut engine = CoreEngine::new();
        engine.mark_dirty();
        assert!(engine.is_dirty());
    }

    #[test]
    fn test_draw_plan_capacity_reuse() {
        let mut engine = CoreEngine::new();

        // Initially dirty
        assert!(engine.is_dirty());

        // Mark as clean
        engine.is_dirty = false;
        assert!(!engine.is_dirty());

        // Mark dirty again
        engine.mark_dirty();
        assert!(engine.is_dirty());
    }

    fn make_wm() -> WindowManager<crate::components::NoopComponent> {
        use std::sync::Arc;
        WindowManager::<crate::components::NoopComponent>::with_config(
            crate::wm_config::WmConfig::default(),
            Arc::new(crate::app_context::AppContext::new("test", "0.1.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        )
    }

    fn make_wm_with_top_panel() -> WindowManager<crate::components::NoopComponent> {
        use std::sync::Arc;
        let mut layer_manager = crate::window::LayerManager::new();
        let id = layer_manager.insert(
            crate::components::NoopWmComponent,
            crate::window::ZPlane::Background,
        );
        let mut semantic_registry = std::collections::HashMap::new();
        semantic_registry.insert(crate::window::ComponentTag::TopPanel, id);
        WindowManager::<crate::components::NoopComponent>::with_config(
            crate::wm_config::WmConfig::default(),
            Arc::new(crate::app_context::AppContext::new("test", "0.1.0")),
            None,
            layer_manager,
            semantic_registry,
        )
    }

    fn toast_bounds_y(
        wm: &WindowManager<crate::components::NoopComponent>,
        message: &str,
    ) -> Vec<i32> {
        let mut plan = DrawPlan::with_capacity(4);
        generate_notification_regions(&mut plan, wm);
        plan.regions()
            .iter()
            .filter(|r| {
                matches!(&r.region_type, RegionType::Notification(msg) if msg.as_ref() == message)
            })
            .map(|r| r.bounds.y)
            .collect()
    }

    #[test]
    fn toast_starts_at_top_outside_monocle() {
        let mut wm = make_wm();
        wm.managed_area = crate::Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert!(!wm.is_monocle());
        wm.push_notification("hello", std::time::Duration::from_secs(60));
        assert_eq!(toast_bounds_y(&wm, "hello"), vec![0]);
    }

    #[test]
    fn toast_not_shifted_in_non_cramped_monocle() {
        // Manual monocle on a wide viewport: the panel claims row 0, so
        // managed.y is already 1 and no extra shift may apply (#357 review).
        let mut wm = make_wm();
        wm.managed_area = crate::Rect {
            x: 0,
            y: 1,
            width: 80,
            height: 23,
        };
        wm.monocle_mode = crate::window::window_manager::MonocleMode::On;
        assert!(wm.is_monocle());
        assert!(!wm.is_monocle_cramped());
        wm.push_notification("hello", std::time::Duration::from_secs(60));
        assert_eq!(toast_bounds_y(&wm, "hello"), vec![1]);
    }

    #[test]
    fn toast_shifts_down_a_row_in_cramped_monocle() {
        // Constrained viewport: auto monocle with the panel overlaying row 0
        // without reserving layout space, so toasts must clear it (#357).
        let mut wm = make_wm_with_top_panel();
        wm.managed_area = crate::Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        wm.update_monocle_mode(50);
        assert!(wm.is_monocle());
        assert!(wm.is_monocle_cramped());
        wm.push_notification("hello", std::time::Duration::from_secs(60));
        assert_eq!(toast_bounds_y(&wm, "hello"), vec![1]);
    }

    #[test]
    fn toast_not_shifted_in_cramped_monocle_without_panel() {
        // No registered top panel component means nothing paints row 0,
        // so toasts stay at the managed origin even when cramped.
        let mut wm = make_wm();
        wm.managed_area = crate::Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        wm.update_monocle_mode(50);
        assert!(wm.is_monocle_cramped());
        wm.push_notification("hello", std::time::Duration::from_secs(60));
        assert_eq!(toast_bounds_y(&wm, "hello"), vec![0]);
    }

    #[test]
    fn stacked_toasts_keep_offset_in_cramped_monocle() {
        let mut wm = make_wm_with_top_panel();
        wm.managed_area = crate::Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        wm.update_monocle_mode(50);
        assert!(wm.is_monocle_cramped());
        wm.push_notification("one", std::time::Duration::from_secs(60));
        wm.push_notification("two", std::time::Duration::from_secs(60));
        // Newest first: 3-row toasts ("two" on top at the shifted origin).
        assert_eq!(toast_bounds_y(&wm, "two"), vec![1]);
        assert_eq!(toast_bounds_y(&wm, "one"), vec![4]);
    }
}
