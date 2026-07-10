use std::sync::Arc;

use term_wm_layout_engine::LayoutRect;

use crate::window::WindowKey;

/// Discriminator for the payload carried by a render region.
///
/// Moving the `WindowKey` into the `Window` variant makes the illegal
/// state (a window region without a key) unrepresentable at compile time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegionType {
    /// A standard window component to render.
    Window(WindowKey),
    /// A transient toast notification.
    Notification(Arc<str>),
}

/// A single render region in the draw plan.
/// The `region_type` carries the semantic payload; spatial bounds are separate.
#[derive(Debug, Clone)]
pub struct RenderRegion {
    /// Bounding box in screen coordinates
    pub bounds: LayoutRect,
    /// Z-ordering for layering (higher = rendered on top)
    pub z_index: usize,
    /// Whether this region should be dimmed (unfocused windows)
    pub dimmed: bool,
    /// Semantic discriminator — carries the key for windows, the message for notifications.
    pub region_type: RegionType,
}

/// The complete draw plan for a frame.
/// Core engine produces this; app layer consumes it.
#[derive(Debug, Clone)]
pub struct DrawPlan {
    /// Render regions sorted by z-index (low to high)
    regions: Vec<RenderRegion>,
}

impl DrawPlan {
    /// Create a new draw plan with pre-allocated capacity.
    /// Uses native Vec amortization — clear() retains capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            regions: Vec::with_capacity(capacity),
        }
    }

    /// Clear the plan for reuse (retains allocated capacity).
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    /// Add a render region to the plan.
    pub fn push(&mut self, region: RenderRegion) {
        self.regions.push(region);
    }

    /// Get regions sorted by z-index.
    pub fn regions(&self) -> &[RenderRegion] {
        &self.regions
    }

    /// Get mutable access for sorting.
    pub fn regions_mut(&mut self) -> &mut [RenderRegion] {
        &mut self.regions
    }

    /// Sort regions by z-index (stable sort).
    pub fn sort_by_z_index(&mut self) {
        self.regions.sort_by_key(|r| r.z_index);
    }

    /// Current number of regions.
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// Check if plan is empty.
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Get the current capacity of the regions vector.
    pub fn capacity(&self) -> usize {
        self.regions.capacity()
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Test helper: create a simple render region for a window
    pub fn make_region(
        key: WindowKey,
        x: i32,
        y: i32,
        width: u16,
        height: u16,
        z_index: usize,
    ) -> RenderRegion {
        RenderRegion {
            region_type: RegionType::Window(key),
            bounds: LayoutRect {
                x,
                y,
                width,
                height,
            },
            z_index,
            dimmed: false,
        }
    }

    /// Test helper: create a dimmed render region for a window
    pub fn make_dimmed_region(
        key: WindowKey,
        x: i32,
        y: i32,
        width: u16,
        height: u16,
        z_index: usize,
    ) -> RenderRegion {
        RenderRegion {
            region_type: RegionType::Window(key),
            bounds: LayoutRect {
                x,
                y,
                width,
                height,
            },
            z_index,
            dimmed: true,
        }
    }

    /// Test helper: assert that a draw plan has the expected number of regions
    pub fn assert_region_count(plan: &DrawPlan, expected: usize) {
        assert_eq!(
            plan.len(),
            expected,
            "Expected {} regions, got {}",
            expected,
            plan.len()
        );
    }

    /// Test helper: assert that a region has the expected bounds
    pub fn assert_region_bounds(
        plan: &DrawPlan,
        index: usize,
        x: i32,
        y: i32,
        width: u16,
        height: u16,
    ) {
        let region = &plan.regions()[index];
        assert_eq!(
            region.bounds.x, x,
            "Region {} x: expected {}, got {}",
            index, x, region.bounds.x
        );
        assert_eq!(
            region.bounds.y, y,
            "Region {} y: expected {}, got {}",
            index, y, region.bounds.y
        );
        assert_eq!(
            region.bounds.width, width,
            "Region {} width: expected {}, got {}",
            index, width, region.bounds.width
        );
        assert_eq!(
            region.bounds.height, height,
            "Region {} height: expected {}, got {}",
            index, height, region.bounds.height
        );
    }

    /// Test helper: assert that a region has the expected z-index
    pub fn assert_region_z_index(plan: &DrawPlan, index: usize, expected: usize) {
        let region = &plan.regions()[index];
        assert_eq!(
            region.z_index, expected,
            "Region {} z_index: expected {}, got {}",
            index, expected, region.z_index
        );
    }

    /// Test helper: assert that a region is dimmed
    pub fn assert_region_dimmed(plan: &DrawPlan, index: usize, expected: bool) {
        let region = &plan.regions()[index];
        assert_eq!(
            region.dimmed, expected,
            "Region {} dimmed: expected {}, got {}",
            index, expected, region.dimmed
        );
    }

    /// Test helper: assert that regions are sorted by z-index
    pub fn assert_sorted_by_z_index(plan: &DrawPlan) {
        for window in plan.regions().windows(2) {
            assert!(
                window[0].z_index <= window[1].z_index,
                "Regions not sorted by z_index: {} > {}",
                window[0].z_index,
                window[1].z_index
            );
        }
    }

    /// Test helper: assert that no two regions overlap
    pub fn assert_no_overlap(plan: &DrawPlan) {
        for i in 0..plan.regions().len() {
            for j in (i + 1)..plan.regions().len() {
                let a = &plan.regions()[i].bounds;
                let b = &plan.regions()[j].bounds;

                // No overlap if: a is completely left/right/above/below b
                let no_overlap = a.x + i32::from(a.width) <= b.x
                    || b.x + i32::from(b.width) <= a.x
                    || a.y + i32::from(a.height) <= b.y
                    || b.y + i32::from(b.height) <= a.y;

                assert!(
                    no_overlap,
                    "Regions {} and {} overlap: {:?} and {:?}",
                    i, j, a, b
                );
            }
        }
    }

    /// Test helper: assert that all regions are within screen bounds
    pub fn assert_within_screen(plan: &DrawPlan, screen_width: i32, screen_height: i32) {
        for (i, region) in plan.regions().iter().enumerate() {
            assert!(
                region.bounds.x + region.bounds.width as i32 <= screen_width,
                "Region {} exceeds screen width: {} + {} > {}",
                i,
                region.bounds.x,
                region.bounds.width,
                screen_width
            );
            assert!(
                region.bounds.y + region.bounds.height as i32 <= screen_height,
                "Region {} exceeds screen height: {} + {} > {}",
                i,
                region.bounds.y,
                region.bounds.height,
                screen_height
            );
        }
    }

    #[cfg(test)]
    #[allow(clippy::module_inception)]
    mod tests {
        use super::*;
        use crate::window::WindowKey;

        #[test]
        fn test_draw_plan_basic() {
            let key1 = WindowKey::default();
            let key2 = WindowKey::default();

            let mut plan = DrawPlan::with_capacity(4);
            plan.push(make_region(key1, 0, 0, 40, 24, 0));
            plan.push(make_region(key2, 40, 0, 40, 24, 0));

            assert_region_count(&plan, 2);
            assert_region_bounds(&plan, 0, 0, 0, 40, 24);
            assert_region_bounds(&plan, 1, 40, 0, 40, 24);
        }

        #[test]
        fn test_draw_plan_z_index_sorting() {
            let key1 = WindowKey::default();
            let key2 = WindowKey::default();
            let key3 = WindowKey::default();

            let mut plan = DrawPlan::with_capacity(4);
            plan.push(make_region(key1, 0, 0, 40, 24, 20));
            plan.push(make_region(key2, 0, 0, 80, 24, 0));
            plan.push(make_region(key3, 0, 0, 80, 24, 10));

            plan.sort_by_z_index();

            assert_sorted_by_z_index(&plan);
            assert_region_z_index(&plan, 0, 0);
            assert_region_z_index(&plan, 1, 10);
            assert_region_z_index(&plan, 2, 20);
        }

        #[test]
        fn test_draw_plan_no_overlap() {
            let key1 = WindowKey::default();
            let key2 = WindowKey::default();

            let mut plan = DrawPlan::with_capacity(4);
            plan.push(make_region(key1, 0, 0, 40, 24, 0));
            plan.push(make_region(key2, 40, 0, 40, 24, 0));

            assert_no_overlap(&plan);
        }

        #[test]
        fn test_draw_plan_within_screen() {
            let key1 = WindowKey::default();
            let key2 = WindowKey::default();

            let mut plan = DrawPlan::with_capacity(4);
            plan.push(make_region(key1, 0, 0, 40, 24, 0));
            plan.push(make_region(key2, 40, 0, 40, 24, 0));

            assert_within_screen(&plan, 80, 24);
        }

        #[test]
        fn test_draw_plan_capacity_reuse() {
            let mut plan = DrawPlan::with_capacity(4);
            let key = WindowKey::default();

            // Fill the plan
            plan.push(make_region(key, 0, 0, 80, 24, 0));
            let capacity = plan.regions.capacity();

            // Clear and refill
            plan.clear();
            plan.push(make_region(key, 0, 0, 80, 24, 0));

            // Capacity should be reused
            assert_eq!(plan.regions.capacity(), capacity);
        }
    }
}
