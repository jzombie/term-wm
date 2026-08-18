pub use term_wm_layout_engine::Direction;
pub use term_wm_layout_engine::InsertPosition;
pub use term_wm_layout_engine::LayoutNode;
pub use term_wm_layout_engine::SplitGap;
pub use term_wm_layout_engine::split_area_for_path;
pub use term_wm_layout_engine::split_at_path_mut;

use super::{FloatingPane, RegionMap};

use crate::Rect;

const SPLIT_DRAG_MIN_SIZE: i16 = 4;

#[derive(Debug, Clone)]
pub struct SplitHandle {
    pub rect: Rect,
    pub path: Vec<usize>,
    pub index: usize,
    pub direction: Direction,
    pub hitbox_id: crate::hitbox_registry::HitboxId,
}

#[derive(Debug)]
pub struct DragState {
    pub path: Vec<usize>,
    pub index: usize,
    pub direction: Direction,
    pub last_col: u16,
    pub last_row: u16,
}

#[derive(Debug)]
pub struct TilingLayout<Id: Copy + Eq + Ord> {
    root: LayoutNode<Id>,
    drag: Option<DragState>,
    hover: Option<(u16, u16)>,
    monocle_active: bool,
    monocle_width_threshold: u16,
}

impl<Id: Copy + Eq + Ord> TilingLayout<Id> {
    pub fn new(root: LayoutNode<Id>) -> Self {
        Self {
            root,
            drag: None,
            hover: None,
            monocle_active: false,
            monocle_width_threshold: crate::constants::MONOCLE_WIDTH_THRESHOLD,
        }
    }

    pub fn new_void() -> Self {
        Self::new(LayoutNode::void())
    }

    pub fn update_monocle_state(&mut self, terminal_width: u16) {
        let should_be_monocle = terminal_width < self.monocle_width_threshold;
        if should_be_monocle != self.monocle_active {
            self.monocle_active = should_be_monocle;
        }
    }

    pub fn is_monocle(&self) -> bool {
        self.monocle_active
    }

    pub fn set_monocle_width_threshold(&mut self, threshold: u16) {
        self.monocle_width_threshold = threshold;
    }

    pub fn monocle_width_threshold(&self) -> u16 {
        self.monocle_width_threshold
    }

    pub fn root(&self) -> &LayoutNode<Id> {
        &self.root
    }

    pub fn root_mut(&mut self) -> &mut LayoutNode<Id> {
        &mut self.root
    }

    pub fn split_root(&mut self, insert: Id, position: InsertPosition) {
        self.root.split_root(insert, position);
    }

    pub fn regions(&self, area: Rect) -> Vec<(Id, Rect)> {
        self.root.layout_rects(area)
    }

    pub fn void_regions(&self, area: Rect) -> Vec<(usize, Rect)> {
        self.root.void_regions(area)
    }

    pub fn replace_void_by_id(&mut self, void_id: usize, new_leaf: LayoutNode<Id>) -> bool {
        self.root.replace_void_by_id(void_id, new_leaf)
    }

    pub fn replace_leaf_with_void(&mut self, key: Id) -> Option<usize> {
        self.root.replace_leaf_with_void(key)
    }

    pub fn remove_void_by_id(&mut self, void_id: usize) -> bool {
        self.root.remove_void_by_id(void_id)
    }

    /// Unified topological fallback: consumes the first available Void node.
    /// Returns true if a Void was successfully filled.
    pub fn consume_first_void(&mut self, insert: Id, area: Rect) -> bool {
        let voids = self.void_regions(area);
        if !voids.is_empty() {
            let target_void_id = voids[0].0;
            self.replace_void_by_id(target_void_id, LayoutNode::leaf(insert));
            true
        } else {
            false
        }
    }

    pub fn swap_nodes(&mut self, source: &Id, target: &Id) -> bool {
        self.root.swap_leaves(source, target)
    }

    pub fn insert_window_balanced(&mut self, insert: Id, area: Rect) {
        // Startup inserts can run before the first render pass, when
        // `managed_area` is still `Rect { 0, 0, 0, 0 }`; a degenerate area would
        // force every split to `InsertPosition::Bottom` (a vertical strip stack).
        // Fall back to a standard terminal size so the tree is a real 2D grid.
        let area = if area.width == 0 || area.height == 0 {
            Rect {
                x: 0,
                y: 0,
                width: crate::constants::DEFAULT_FLOAT_WIDTH,
                height: crate::constants::DEFAULT_FLOAT_HEIGHT,
            }
        } else {
            area
        };

        // 1. Unified topological fallback — fill voids before splitting
        if self.consume_first_void(insert, area) {
            self.root.reweight_by_leaf_count();
            return;
        }

        // 2. Existing largest-leaf split logic
        let regions = self.regions(area);
        if regions.is_empty() {
            self.split_root(insert, InsertPosition::Right);
            self.root.reweight_by_leaf_count();
            return;
        }

        let (largest_id, largest_rect) = regions
            .iter()
            .max_by_key(|(_, r)| (r.width as u32) * (r.height as u32))
            .copied()
            .expect("tiling: regions non-empty");

        // Split direction: only one axis fits → that axis; both fit (or neither)
        // → decide by visual aspect ratio. Checking each axis independently avoids
        // the old `width/2` first bias that forced vertical stacking on small areas.
        let can_split_h = largest_rect.width / 2 >= crate::constants::MIN_TILE_WIDTH;
        let can_split_v = largest_rect.height / 2 >= crate::constants::MIN_TILE_HEIGHT;
        let pos = match (can_split_h, can_split_v) {
            (true, false) => InsertPosition::Right,
            (false, true) => InsertPosition::Bottom,
            _ => {
                let visual_h = (largest_rect.height as u32) * crate::constants::CELL_ASPECT_RATIO;
                let visual_w = largest_rect.width as u32;
                // Horizontal splits halve the tile's width; bias them to only
                // fire when the region is clearly wider than tall (>= 1.5x
                // visual height), so they never create narrow vertical strips.
                if visual_w * crate::constants::TILING_HORIZONTAL_BIAS_DENOMINATOR
                    >= visual_h * crate::constants::TILING_HORIZONTAL_BIAS_NUMERATOR
                {
                    InsertPosition::Right
                } else {
                    InsertPosition::Bottom
                }
            }
        };

        if !self.root.insert_leaf(largest_id, insert, pos) {
            self.split_root(insert, pos);
        }
        // Equal-area rebalancing: every leaf gets 1/N regardless of tree depth.
        self.root.reweight_by_leaf_count();
    }

    pub fn project_insert_void(&self, insert: Id, void_id: usize, area: Rect) -> Option<Rect> {
        self.root.project_insert_void(insert, void_id, area)
    }

    pub fn project_insert(
        &self,
        target: Option<Id>,
        insert: Id,
        position: InsertPosition,
        area: Rect,
    ) -> Option<Rect> {
        self.root.project_insert(target, insert, position, area)
    }

    pub fn handles(&self, area: Rect) -> Vec<SplitHandle> {
        let (_, gaps) = self.root.layout_with_gaps(area);
        gaps.into_iter()
            .map(|g| SplitHandle {
                rect: g.rect,
                path: g.path,
                index: g.index,
                direction: g.direction,
                hitbox_id: crate::hitbox_registry::HitboxId::new(),
            })
            .collect()
    }

    pub fn hovered_handle(&self, area: Rect) -> Option<SplitHandle> {
        let (column, row) = self.hover?;
        let gap = self.root.hit_test_gap(area, column, row)?;
        Some(SplitHandle {
            rect: gap.rect,
            path: gap.path,
            index: gap.index,
            direction: gap.direction,
            hitbox_id: crate::hitbox_registry::HitboxId::new(),
        })
    }

    pub fn handle_event(&mut self, event: &crate::events::Event, area: Rect) -> bool {
        use crate::events::MouseEventKind;
        let crate::events::Event::Mouse(mouse) = event else {
            return false;
        };
        self.hover = Some((mouse.column, mouse.row));
        match mouse.kind {
            MouseEventKind::Press(_) => {
                if let Some(gap) = self.root.hit_test_gap(area, mouse.column, mouse.row) {
                    self.drag = Some(DragState {
                        path: gap.path,
                        index: gap.index,
                        direction: gap.direction,
                        last_col: mouse.column,
                        last_row: mouse.row,
                    });
                    return true;
                }
            }
            MouseEventKind::Drag(_) => {
                if let Some(state) = self.drag.as_mut() {
                    let delta = match state.direction {
                        Direction::Horizontal => mouse.column as i16 - state.last_col as i16,
                        Direction::Vertical => mouse.row as i16 - state.last_row as i16,
                    };
                    state.last_col = mouse.column;
                    state.last_row = mouse.row;
                    return self.root.apply_drag(
                        area,
                        &state.path,
                        state.index,
                        state.direction,
                        delta,
                        SPLIT_DRAG_MIN_SIZE,
                    );
                }
            }
            MouseEventKind::Moved => {}
            MouseEventKind::Release(_) if self.drag.is_some() => {
                self.drag = None;
                return true;
            }
            _ => {}
        }
        false
    }

    pub fn remove_window(&mut self, key: Id) {
        self.root.remove_leaf(key);
        self.root.cleanup_after_removal();
        self.root.clear_leaf(key);
        // Rebalance the remaining leaves; the pre-removal weights are stale.
        self.root.reweight_by_leaf_count();
    }

    pub fn is_empty(&self) -> bool {
        self.root.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct LayoutPlan<Id: Copy + Eq + Ord> {
    pub root: LayoutNode<Id>,
    pub floating: Vec<FloatingPane<Id>>,
}

impl<Id: Copy + Eq + Ord> LayoutPlan<Id> {
    pub fn new(root: LayoutNode<Id>) -> Self {
        Self {
            root,
            floating: Vec::new(),
        }
    }

    pub fn regions(&self, area: Rect) -> RegionMap<Id> {
        let mut regions = RegionMap::default();
        for (id, rect) in self.root.layout_rects(area) {
            regions.set(id, rect);
        }
        for floating in &self.floating {
            regions.set(floating.key, floating.rect.resolve(area));
        }
        regions
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_window_balanced_consumes_void_before_splitting_leaf() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };

        // Construct tree: Split [ Leaf(1), Void(42) ]
        let root = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::leaf(1), LayoutNode::Void(42)],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        let mut layout = TilingLayout::new(root);

        assert_eq!(
            layout.void_regions(area).len(),
            1,
            "Initial tree must contain 1 void"
        );

        // Insert window 2 into tree with void
        layout.insert_window_balanced(2, area);

        // Assert: Void node was consumed, no new splits were created
        assert_eq!(
            layout.void_regions(area).len(),
            0,
            "Void must be completely consumed"
        );
        let leaves = layout.root().collect_leaves();
        assert_eq!(leaves, vec![1, 2], "Window 2 must occupy the vacant slot");

        if let LayoutNode::Split { children, .. } = layout.root() {
            assert_eq!(
                children.len(),
                2,
                "Topology must remain a 2-child split (no nesting)"
            );
            assert_eq!(children[0].unwrap_leaf(), Some(1));
            assert_eq!(children[1].unwrap_leaf(), Some(2));
        } else {
            panic!("Root must remain a Split");
        }
    }

    /// Assert every tiled leaf region occupies near-equal area (within a few
    /// percent; integer rounding and split gaps cause small deviations, but a
    /// real imbalance such as ½ vs ¼ must fail).
    fn assert_equal_tiled_areas(layout: &TilingLayout<usize>, area: Rect) {
        let regions = layout.regions(area);
        assert!(!regions.is_empty(), "expected at least one tiled leaf");
        let areas: Vec<u32> = regions
            .iter()
            .map(|(_, r)| (r.width as u32) * (r.height as u32))
            .collect();
        let min_a = *areas.iter().min().unwrap();
        let max_a = *areas.iter().max().unwrap();
        assert!(
            (max_a as u64) * 100 <= (min_a as u64) * 115,
            "tiled leaves must be near equal-area, got {areas:?}"
        );
    }

    #[test]
    fn insert_window_balanced_yields_equal_areas() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        // First window becomes a single leaf (as the window manager does).
        let mut layout = TilingLayout::new(LayoutNode::leaf(1));
        for id in 2..=4 {
            layout.insert_window_balanced(id, area);
        }
        assert_eq!(layout.regions(area).len(), 4);
        assert_equal_tiled_areas(&layout, area);
    }

    #[test]
    fn insert_window_balanced_avoids_narrow_vertical_strips() {
        // A half-screen column (60 cols x 24 rows, e.g. in a 120x24 terminal)
        // must split vertically: a horizontal split would leave two 30-wide
        // full-height strips (the "narrow column" bug).
        let area = Rect {
            x: 0,
            y: 0,
            width: 60,
            height: 24,
        };
        let mut layout = TilingLayout::new(LayoutNode::leaf(1));
        layout.insert_window_balanced(2, area);
        let regions = layout.regions(area);
        assert_eq!(regions.len(), 2);
        let r1 = regions.iter().find(|(id, _)| *id == 1).unwrap().1;
        let r2 = regions.iter().find(|(id, _)| *id == 2).unwrap().1;
        assert_eq!(r1.x, 0, "window 1 starts at column 0");
        assert_eq!(r2.x, 0, "window 2 starts at column 0");
        assert_eq!(r1.width, 60, "window 1 spans full width");
        assert_eq!(r2.width, 60, "window 2 spans full width");
        assert!(r1.y < r2.y, "windows must stack, not form narrow columns");
    }

    #[test]
    fn insert_window_balanced_splits_square_panes_horizontally() {
        // A genuinely wide tile (96 cols x 24 rows) still splits side-by-side
        // into two ~48-wide square panes (a 1-col resize gap is subtracted).
        let area = Rect {
            x: 0,
            y: 0,
            width: 96,
            height: 24,
        };
        let mut layout = TilingLayout::new(LayoutNode::leaf(1));
        layout.insert_window_balanced(2, area);
        let regions = layout.regions(area);
        assert_eq!(regions.len(), 2);
        let r1 = regions.iter().find(|(id, _)| *id == 1).unwrap().1;
        let r2 = regions.iter().find(|(id, _)| *id == 2).unwrap().1;
        assert!(r1.x < r2.x, "windows must be side by side on a wide tile");
        assert!(
            r1.width >= 47 && r2.width >= 47,
            "each pane must be ~half the tile, got {} and {}",
            r1.width,
            r2.width
        );
    }

    #[test]
    fn insert_window_balanced_two_up_landscape_stays_side_by_side() {
        // Regression guard: the standard 80x24 two-window layout must remain a
        // side-by-side split, not regress to stacked.
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let mut layout = TilingLayout::new(LayoutNode::leaf(1));
        layout.insert_window_balanced(2, area);
        let regions = layout.regions(area);
        assert_eq!(regions.len(), 2);
        let r1 = regions.iter().find(|(id, _)| *id == 1).unwrap().1;
        let r2 = regions.iter().find(|(id, _)| *id == 2).unwrap().1;
        assert!(r1.x < r2.x, "80x24 two-up must stay side by side");
    }

    #[test]
    fn remove_window_rebalances_remaining_leaves() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let mut layout = TilingLayout::new(LayoutNode::leaf(1));
        for id in 2..=4 {
            layout.insert_window_balanced(id, area);
        }
        assert_equal_tiled_areas(&layout, area);

        // Removing one of four windows must rebalance the remaining three.
        layout.remove_window(1);
        assert_eq!(layout.regions(area).len(), 3);
        assert_equal_tiled_areas(&layout, area);
    }

    #[test]
    fn insert_four_windows_uninitialized_area_forms_balanced_2d_grid() {
        // Startup inserts happen before the first render pass when the managed
        // area is still 0x0; they must still form a balanced 2D grid, not a
        // vertical strip stack (the SEV-1 degenerate-area failure).
        let uninitialized = Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        let viewport = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };

        let mut layout = TilingLayout::new_void();
        for id in 1..=4 {
            layout.insert_window_balanced(id, uninitialized);
        }

        let regions = layout.regions(viewport);
        assert_eq!(regions.len(), 4);
        // Equal-area rebalance, and horizontally partitioned (not a full-width
        // vertical strip stack where every window has the same x).
        assert_equal_tiled_areas(&layout, viewport);
        let x_coords: std::collections::BTreeSet<_> = regions.iter().map(|(_, r)| r.x).collect();
        assert!(
            x_coords.len() > 1,
            "must have horizontal division, not vertical strips"
        );
    }

    #[test]
    fn tiling_handle_event_direct() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let root = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::Leaf(1), LayoutNode::Leaf(2)],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        let mut layout = TilingLayout::new(root);
        let handles = layout.handles(area);
        assert_eq!(handles.len(), 1);
        let gap = &handles[0].rect;
        let gap_col = (gap.x + i32::from(gap.width) / 2) as u16;
        let gap_row = (gap.y + i32::from(gap.height) / 2) as u16;
        let down = crate::events::Event::Mouse(crate::events::MouseEvent {
            kind: crate::events::MouseEventKind::Press(crate::events::MouseButton::Left),
            column: gap_col,
            row: gap_row,
            modifiers: crate::events::KeyModifiers::NONE,
        });
        assert!(layout.handle_event(&down, area), "Down must hit the handle");
        let drag = crate::events::Event::Mouse(crate::events::MouseEvent {
            kind: crate::events::MouseEventKind::Drag(crate::events::MouseButton::Left),
            column: gap_col + 10,
            row: gap_row,
            modifiers: crate::events::KeyModifiers::NONE,
        });
        assert!(layout.handle_event(&drag, area), "Drag must adjust split");
        let up = crate::events::Event::Mouse(crate::events::MouseEvent {
            kind: crate::events::MouseEventKind::Release(crate::events::MouseButton::Left),
            column: gap_col + 10,
            row: gap_row,
            modifiers: crate::events::KeyModifiers::NONE,
        });
        assert!(layout.handle_event(&up, area), "Up must clear drag state");
        let regions = layout.regions(area);
        assert_eq!(regions.len(), 2);
        assert!(
            regions[0].1.width > regions[1].1.width,
            "after dragging split right, left must be wider"
        );
    }

    #[test]
    fn monocle_mode_toggles_on_narrow_terminal() {
        let root = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::Leaf(1), LayoutNode::Leaf(2)],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        let mut layout = TilingLayout::new(root);
        assert!(!layout.is_monocle());
        layout.update_monocle_state(60);
        assert!(layout.is_monocle());
    }

    #[test]
    fn monocle_mode_deactivates_on_wide_terminal() {
        let root = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::Leaf(1), LayoutNode::Leaf(2)],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        let mut layout = TilingLayout::new(root);
        layout.update_monocle_state(60);
        assert!(layout.is_monocle());
        layout.update_monocle_state(120);
        assert!(!layout.is_monocle());
    }

    #[test]
    fn tiling_layout_split_root_void_to_left() {
        let mut layout = TilingLayout::<usize>::new_void();
        layout.split_root(1, InsertPosition::Left);
        let leaves = layout.root().collect_leaves();
        assert_eq!(leaves, vec![1]);
    }

    #[test]
    fn tiling_layout_split_root_void_to_right() {
        let mut layout = TilingLayout::<usize>::new_void();
        layout.split_root(1, InsertPosition::Right);
        let leaves = layout.root().collect_leaves();
        assert_eq!(leaves, vec![1]);
    }

    #[test]
    fn tiling_layout_split_root_void_to_top() {
        let mut layout = TilingLayout::<usize>::new_void();
        layout.split_root(1, InsertPosition::Top);
        let leaves = layout.root().collect_leaves();
        assert_eq!(leaves, vec![1]);
    }

    #[test]
    fn tiling_layout_split_root_void_to_bottom() {
        let mut layout = TilingLayout::<usize>::new_void();
        layout.split_root(1, InsertPosition::Bottom);
        let leaves = layout.root().collect_leaves();
        assert_eq!(leaves, vec![1]);
    }

    #[test]
    fn tiling_layout_split_root_existing_to_right() {
        let root = LayoutNode::leaf(1);
        let mut layout = TilingLayout::new(root);
        layout.split_root(2, InsertPosition::Right);
        let leaves = layout.root().collect_leaves();
        assert_eq!(leaves, vec![1, 2]);
    }

    #[test]
    fn tiling_layout_split_root_existing_to_left() {
        let root = LayoutNode::leaf(1);
        let mut layout = TilingLayout::new(root);
        layout.split_root(2, InsertPosition::Left);
        let leaves = layout.root().collect_leaves();
        assert_eq!(leaves, vec![2, 1]);
    }

    #[test]
    fn tiling_layout_split_root_existing_to_top() {
        let root = LayoutNode::leaf(1);
        let mut layout = TilingLayout::new(root);
        layout.split_root(2, InsertPosition::Top);
        let leaves = layout.root().collect_leaves();
        assert_eq!(leaves, vec![2, 1]);
    }

    #[test]
    fn tiling_layout_split_root_existing_to_bottom() {
        let root = LayoutNode::leaf(1);
        let mut layout = TilingLayout::new(root);
        layout.split_root(2, InsertPosition::Bottom);
        let leaves = layout.root().collect_leaves();
        assert_eq!(leaves, vec![1, 2]);
    }

    #[test]
    fn tiling_layout_regions_returns_all_leaves() {
        let root = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::leaf(1), LayoutNode::leaf(2)],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        let layout = TilingLayout::new(root);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let regions = layout.regions(area);
        assert_eq!(regions.len(), 2);
    }

    #[test]
    fn tiling_layout_replace_void() {
        let root = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::leaf(1), LayoutNode::Void(42)],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        let mut layout = TilingLayout::new(root);
        assert!(layout.replace_void_by_id(42, LayoutNode::leaf(2)));
        let leaves = layout.root().collect_leaves();
        assert_eq!(leaves, vec![1, 2]);
    }

    #[test]
    fn tiling_layout_swap_nodes() {
        let root = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::leaf(1), LayoutNode::leaf(2)],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        let mut layout = TilingLayout::new(root);
        assert!(layout.swap_nodes(&1, &2));
        let leaves = layout.root().collect_leaves();
        assert_eq!(leaves, vec![2, 1]);
    }

    #[test]
    fn tiling_layout_handles_returns_split_handles() {
        let root = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::leaf(1), LayoutNode::leaf(2)],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        let layout = TilingLayout::new(root);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let handles = layout.handles(area);
        assert_eq!(handles.len(), 1);
    }

    #[test]
    fn tiling_layout_project_insert() {
        let root = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::leaf(1), LayoutNode::leaf(2)],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        let layout = TilingLayout::new(root);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let rect = layout.project_insert(Some(1), 3, InsertPosition::Right, area);
        assert!(rect.is_some());
    }

    #[test]
    fn tiling_layout_project_insert_void() {
        let root = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::leaf(1), LayoutNode::Void(42)],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        let layout = TilingLayout::new(root);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let rect = layout.project_insert_void(2, 42, area);
        assert!(rect.is_some());
    }

    #[test]
    fn layout_plan_regions_includes_floating() {
        use crate::layout::FloatingPane;
        use crate::layout::RectSpec;
        let root = LayoutNode::leaf(1);
        let mut plan = LayoutPlan::new(root);
        plan.floating.push(FloatingPane {
            key: 2,
            rect: RectSpec::Absolute(Rect {
                x: 10,
                y: 10,
                width: 20,
                height: 10,
            }),
        });
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let regions = plan.regions(area);
        assert!(regions.get(1).is_some());
        assert!(regions.get(2).is_some());
    }

    #[test]
    fn monocle_width_threshold_getter_setter() {
        let root = LayoutNode::leaf(1);
        let mut layout = TilingLayout::new(root);
        assert_eq!(
            layout.monocle_width_threshold(),
            crate::constants::MONOCLE_WIDTH_THRESHOLD
        );
        layout.set_monocle_width_threshold(60);
        assert_eq!(layout.monocle_width_threshold(), 60);
    }

    #[test]
    fn apply_drag_invalid_path_returns_false() {
        let root = LayoutNode::leaf(1);
        let mut node = root.clone();
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert!(!node.apply_drag(area, &[0], 0, Direction::Horizontal, 5, 4));
    }

    #[test]
    fn handle_event_non_mouse_returns_false() {
        let root = LayoutNode::leaf(1);
        let mut layout = TilingLayout::new(root);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let key_event = crate::events::Event::Key(crate::events::KeyEvent {
            code: crate::events::KeyCode::Char('a'),
            modifiers: crate::events::KeyModifiers::NONE,
            kind: crate::events::KeyKind::Press,
        });
        assert!(!layout.handle_event(&key_event, area));
    }
}
