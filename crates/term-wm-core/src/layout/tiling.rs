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

    pub fn swap_nodes(&mut self, source: &Id, target: &Id) -> bool {
        self.root.swap_leaves(source, target)
    }

    pub fn insert_window_balanced(&mut self, insert: Id, area: Rect) {
        let regions = self.regions(area);
        if regions.is_empty() {
            self.split_root(insert, InsertPosition::Right);
            return;
        }

        let (largest_id, largest_rect) = regions
            .iter()
            .max_by_key(|(_, r)| (r.width as u32) * (r.height as u32))
            .copied()
            .unwrap();

        let pos = if largest_rect.width / 2 < crate::constants::MIN_TILE_WIDTH {
            InsertPosition::Bottom
        } else if largest_rect.height / 2 < crate::constants::MIN_TILE_HEIGHT {
            InsertPosition::Right
        } else {
            let visual_h = (largest_rect.height as u32) * crate::constants::CELL_ASPECT_RATIO;
            let visual_w = largest_rect.width as u32;
            if visual_w >= visual_h {
                InsertPosition::Right
            } else {
                InsertPosition::Bottom
            }
        };

        if !self.root.insert_leaf(largest_id, insert, pos) {
            self.split_root(insert, pos);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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
