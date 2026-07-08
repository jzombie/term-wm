use crate::draw_plan::{DrawPlan, RenderRegion};
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

        // Sort by z-index for correct layering
        self.draw_plan.sort_by_z_index();

        // Mark as clean
        self.is_dirty = false;
        wm.clear_layout_dirty();

        &self.draw_plan
    }

    /// Generate render regions from current layout state.
    fn generate_regions(&mut self, _width: u32, _height: u32, wm: &WindowManager) {
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
                key: window_key,
                bounds: layout_rect,
                z_index: 0, // Windows at base layer
                dimmed: !is_focused,
            });
        }

        // 2. Generate panel regions (top and bottom)
        // Panels are rendered by the WindowManager, not as window regions
        // Their z-index is higher than windows

        // 3. Generate overlay regions (if active)
        // Overlays are rendered by the WindowManager
        // Their z-index is highest
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
