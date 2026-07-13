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
    /// A floating (draggable) window.
    FloatingWindow(WindowKey),
    /// System chrome (top panel, bottom panel).
    Panel(PanelPosition),
    /// Transient overlay (help, exit confirm).
    Overlay,
    /// Pulsing border highlight for tap-to-swap targeting.
    TargetHighlight(WindowKey),
    /// Floating action button (FAB) — always visible, exempt from monocle culling.
    Fab,
}

/// Position of a panel in the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelPosition {
    Top,
    Bottom,
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
    /// Whether this region should be hidden (used for monocle mode culling)
    pub hidden: bool,
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

    /// Apply monocle culling: resize focused window to fill screen,
    /// mark other windows as hidden but preserve their logical geometry.
    /// The FAB is EXEMPT from culling — it's the sole mobile navigation mechanism.
    pub fn apply_monocle_culling(&mut self, focused_key: WindowKey, screen: LayoutRect) {
        for region in &mut self.regions {
            match &region.region_type {
                RegionType::Window(key) if *key == focused_key => {
                    // Focused window gets FULL screen area
                    region.bounds = screen;
                    region.dimmed = false;
                }
                RegionType::Window(_) => {
                    region.hidden = true;
                }
                // Hide panels in monocle mode
                RegionType::Panel(_) => {
                    region.hidden = true;
                }
                // FAB is EXEMPT from monocle culling — always visible
                RegionType::Fab => {
                    region.hidden = false;
                }
                _ => {}
            }
        }
    }

    /// Apply monocle Z-order using stratified layering.
    /// Partitions regions into topological layers, then concatenates
    /// them in correct depth order. The focused window is elevated
    /// within its layer but never above overlays.
    pub fn apply_monocle_z_order(&mut self, focused_key: WindowKey) {
        // Partition into strict topological layers
        let mut hidden_tiled: Vec<RenderRegion> = Vec::new();
        let mut focused_region: Option<RenderRegion> = None;
        let mut other_tiled: Vec<RenderRegion> = Vec::new();
        let mut floating: Vec<RenderRegion> = Vec::new();
        let mut overlays: Vec<RenderRegion> = Vec::new();

        for region in self.regions.drain(..) {
            match &region.region_type {
                RegionType::Window(key) if *key == focused_key => {
                    focused_region = Some(region);
                }
                RegionType::Window(_) if region.hidden => {
                    hidden_tiled.push(region);
                }
                RegionType::Window(_) => {
                    other_tiled.push(region);
                }
                RegionType::FloatingWindow(_) => {
                    floating.push(region);
                }
                // FAB goes in overlays layer (above everything)
                RegionType::Fab => {
                    overlays.push(region);
                }
                RegionType::Panel(_) | RegionType::Overlay | RegionType::TargetHighlight(_) => {
                    overlays.push(region);
                }
                _ => {
                    other_tiled.push(region);
                }
            }
        }

        // Reassemble in strict depth order:
        // 1. Hidden tiled windows (background, not rendered)
        // 2. Other visible tiled windows
        // 3. Focused window (elevated within tiled layer)
        // 4. Floating windows (above tiled)
        // 5. Overlays (panels, FAB, session manager — always on top)
        self.regions = hidden_tiled;
        self.regions.append(&mut other_tiled);
        if let Some(region) = focused_region {
            self.regions.push(region);
        }
        self.regions.append(&mut floating);
        self.regions.append(&mut overlays);
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
            hidden: false,
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
            hidden: false,
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

    /// Test helper: create a FAB render region
    pub fn make_fab_region(x: i32, y: i32, width: u16, height: u16, z_index: usize) -> RenderRegion {
        RenderRegion {
            region_type: RegionType::Fab,
            bounds: LayoutRect {
                x,
                y,
                width,
                height,
            },
            z_index,
            dimmed: false,
            hidden: false,
        }
    }

    /// Test helper: create a panel render region
    pub fn make_panel_region(
        position: PanelPosition,
        x: i32,
        y: i32,
        width: u16,
        height: u16,
        z_index: usize,
    ) -> RenderRegion {
        RenderRegion {
            region_type: RegionType::Panel(position),
            bounds: LayoutRect {
                x,
                y,
                width,
                height,
            },
            z_index,
            dimmed: false,
            hidden: false,
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

        #[test]
        fn test_monocle_culling_resizes_focused_window() {
            let key1 = WindowKey::default();
            let key2 = WindowKey::default();
            
            let mut plan = DrawPlan::with_capacity(4);
            plan.push(make_region(key1, 0, 0, 40, 24, 0));
            plan.push(make_region(key2, 40, 0, 40, 24, 0));
            
            let screen = LayoutRect { x: 0, y: 0, width: 80, height: 24 };
            plan.apply_monocle_culling(key1, screen);
            
            // Focused window should fill the screen
            assert_region_bounds(&plan, 0, 0, 0, 80, 24);
            // Other window should be hidden
            assert!(plan.regions()[1].hidden);
        }

        #[test]
        fn test_monocle_culling_hides_panels() {
            let key1 = WindowKey::default();
            
            let mut plan = DrawPlan::with_capacity(4);
            plan.push(make_region(key1, 0, 0, 80, 20, 0));
            plan.push(make_panel_region(PanelPosition::Top, 0, 0, 80, 2, 10));
            plan.push(make_panel_region(PanelPosition::Bottom, 0, 22, 80, 2, 10));
            
            let screen = LayoutRect { x: 0, y: 0, width: 80, height: 24 };
            plan.apply_monocle_culling(key1, screen);
            
            // Panels should be hidden
            assert!(plan.regions()[1].hidden);
            assert!(plan.regions()[2].hidden);
        }

        #[test]
        fn test_monocle_culling_exempt_fab() {
            let key1 = WindowKey::default();
            
            let mut plan = DrawPlan::with_capacity(4);
            plan.push(make_region(key1, 0, 0, 80, 24, 0));
            plan.push(make_fab_region(77, 23, 3, 1, 1000));
            
            let screen = LayoutRect { x: 0, y: 0, width: 80, height: 24 };
            plan.apply_monocle_culling(key1, screen);
            
            // FAB should NOT be hidden (exempt from culling)
            assert!(!plan.regions()[1].hidden);
        }

        #[test]
        fn test_monocle_z_order_places_fab_in_overlays() {
            let key1 = WindowKey::default();
            
            let mut plan = DrawPlan::with_capacity(4);
            plan.push(make_region(key1, 0, 0, 80, 24, 0));
            plan.push(make_fab_region(77, 23, 3, 1, 1000));
            
            plan.apply_monocle_z_order(key1);
            
            // FAB should be in the last position (overlays layer)
            let last = plan.regions().last().unwrap();
            assert!(matches!(last.region_type, RegionType::Fab));
        }
    }
}
