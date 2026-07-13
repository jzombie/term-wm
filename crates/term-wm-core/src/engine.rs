use crate::constants::NOTIFICATION_Z_INDEX;
use crate::draw_plan::{DrawPlan, RegionType, RenderRegion};
use crate::window::WindowManager;
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
    pub fn project_draw_plan(
        &mut self,
        width: u32,
        height: u32,
        wm: &mut WindowManager,
    ) -> &DrawPlan {
        // Check if either the engine or the WindowManager has changed
        if !self.is_dirty && !wm.layout_dirty() {
            self.draw_plan.sort_by_z_index();
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
            let screen = LayoutRect {
                x: 0,
                y: 0,
                width: width as u16,
                height: height as u16,
            };
            self.draw_plan.apply_monocle_culling(focused_key, screen);
            self.draw_plan.apply_monocle_z_order(focused_key);
        }

        // Sort by z-index for correct layering
        self.draw_plan.sort_by_z_index();

        // Mark as clean
        self.is_dirty = false;
        wm.clear_layout_dirty();

        &self.draw_plan
    }

    /// Generate render regions from current layout state.
    fn generate_regions(&mut self, width: u32, height: u32, wm: &mut WindowManager) {
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
                z_index: 0, // Windows at base layer
                dimmed: !is_focused,
                region_type: RegionType::Window(window_key),
                hidden: false,
            });
        }

        // 2. Generate panel regions (top and bottom)
        // Panels are rendered by the WindowManager, not as window regions
        // Their z-index is higher than windows

        // 3. Generate overlay regions (if active)
        // Overlays are rendered by the WindowManager
        // Their z-index is highest

        // 4. Generate notification toast regions
        generate_notification_regions(&mut self.draw_plan, wm);

        // 5. Generate FAB region (bottom-right corner, always visible)
        if wm.fab_component_mut().is_some() {
            let fab_width = 3;
            let fab_height = 1;
            let fab_x = width as i32 - fab_width as i32 - 1;  // 1 col margin
            let fab_y = height as i32 - fab_height as i32 - 1; // 1 row margin
            
            self.draw_plan.push(RenderRegion {
                bounds: LayoutRect {
                    x: fab_x,
                    y: fab_y,
                    width: fab_width,
                    height: fab_height,
                },
                z_index: 1000,  // Above everything
                dimmed: false,
                region_type: RegionType::Fab,
                hidden: false,
            });
        }
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
fn generate_notification_regions(plan: &mut DrawPlan, wm: &WindowManager) {
    use std::sync::Arc;
    use textwrap::Options;

    const TOAST_W: u16 = 40;
    const MARGIN: u16 = 2;
    const GAP: u16 = 1;

    let managed = wm.managed_area();
    let notif_count = wm.notifications().len();
    if notif_count == 0 {
        return;
    }

    // Circuit breaker — terminal too narrow; skip notification layers only.
    if managed.width <= MARGIN.saturating_mul(2).saturating_add(2) {
        return;
    }

    let actual_w = TOAST_W.min(managed.width.saturating_sub(MARGIN.saturating_mul(2)));
    let inner_w = actual_w.saturating_sub(2) as usize;
    let wrap_opts = Options::new(inner_w);

    let mut y_offset: u16 = MARGIN;

    for notification in wm.notifications().renderable().rev() {
        let lines = textwrap::wrap(&notification.message, &wrap_opts);
        let h = (lines.len() as u16).saturating_add(2);
        let h = h.min(
            managed
                .height
                .saturating_sub(y_offset.saturating_add(MARGIN)),
        );
        if h < 3 {
            break;
        }

        let x = managed
            .x
            .saturating_add(managed.width as i32)
            .saturating_sub(actual_w as i32)
            .saturating_sub(MARGIN as i32);

        plan.push(RenderRegion {
            bounds: LayoutRect {
                x,
                y: managed.y.saturating_add(y_offset as i32),
                width: actual_w,
                height: h,
            },
            z_index: NOTIFICATION_Z_INDEX,
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
}
