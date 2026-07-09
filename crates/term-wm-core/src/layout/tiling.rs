use crate::Rect;
use crate::layout::{Constraint, Direction, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};
use term_wm_layout_engine::LayoutRect;

static VOID_ID_COUNTER: AtomicUsize = AtomicUsize::new(1);

use super::{FloatingPane, RegionMap, gap_size, rect_contains};

#[derive(Debug, Clone)]
pub enum LayoutNode<Id: Copy + Eq + Ord> {
    Leaf(Id),
    Void(usize),
    Split {
        direction: Direction,
        children: Vec<LayoutNode<Id>>,
        weights: Vec<f32>,
        constraints: Vec<Constraint>,
        resizable: bool,
    },
}

pub use term_wm_layout_engine::InsertPosition;

impl<Id: Copy + Eq + Ord> From<term_wm_layout_engine::BspNode<Id>> for LayoutNode<Id> {
    fn from(bsp: term_wm_layout_engine::BspNode<Id>) -> Self {
        match bsp {
            term_wm_layout_engine::BspNode::Leaf(id) => LayoutNode::leaf(id),
            term_wm_layout_engine::BspNode::Split {
                orientation,
                left,
                right,
                ratio,
            } => {
                let direction = match orientation {
                    term_wm_layout_engine::Orientation::Horizontal => Direction::Horizontal,
                    term_wm_layout_engine::Orientation::Vertical => Direction::Vertical,
                };
                let left_node: LayoutNode<Id> = LayoutNode::from(*left);
                let right_node: LayoutNode<Id> = LayoutNode::from(*right);
                let total = u32::from(ratio.total());
                let weights = if total == 0 {
                    vec![1.0, 1.0]
                } else {
                    vec![
                        f32::from(ratio.left_part()) / total as f32,
                        f32::from(ratio.right_part()) / total as f32,
                    ]
                };
                LayoutNode::Split {
                    direction,
                    children: vec![left_node, right_node],
                    weights,
                    constraints: Vec::new(),
                    resizable: true,
                }
            }
        }
    }
}

impl<Id: Copy + Eq + Ord> LayoutNode<Id> {
    pub fn leaf(id: Id) -> Self {
        Self::Leaf(id)
    }

    pub fn split(
        direction: Direction,
        constraints: Vec<Constraint>,
        children: Vec<LayoutNode<Id>>,
    ) -> Self {
        Self::Split {
            direction,
            children,
            weights: Vec::new(),
            constraints,
            resizable: true,
        }
    }

    pub fn split_resizable(
        direction: Direction,
        constraints: Vec<Constraint>,
        children: Vec<LayoutNode<Id>>,
        resizable: bool,
    ) -> Self {
        Self::Split {
            direction,
            children,
            weights: Vec::new(),
            constraints,
            resizable,
        }
    }

    pub fn unwrap_leaf(&self) -> Option<Id> {
        match self {
            LayoutNode::Leaf(id) => Some(*id),
            _ => None,
        }
    }

    pub fn layout(&self, area: Rect) -> Vec<(Id, Rect)> {
        let (regions, _) = self.layout_with_handles(area);
        regions
    }

    pub fn layout_with_handles(&self, area: Rect) -> (Vec<(Id, Rect)>, Vec<SplitHandle>) {
        let mut regions = Vec::new();
        let mut handles = Vec::new();
        self.layout_recursive(area, &mut regions, &mut handles, &mut Vec::new());
        (regions, handles)
    }

    pub fn node_at_path(&self, path: &[usize]) -> Option<&LayoutNode<Id>> {
        let mut current = self;
        for &idx in path {
            let LayoutNode::Split { children, .. } = current else {
                return None;
            };
            current = children.get(idx)?;
        }
        Some(current)
    }

    pub fn subtree_any<F>(&self, mut predicate: F) -> bool
    where
        F: FnMut(Id) -> bool,
    {
        fn walk<Id: Copy + Eq + Ord, F: FnMut(Id) -> bool>(
            node: &LayoutNode<Id>,
            predicate: &mut F,
        ) -> bool {
            match node {
                LayoutNode::Leaf(id) => predicate(*id),
                LayoutNode::Void(_) => false,
                LayoutNode::Split { children, .. } => {
                    children.iter().any(|child| walk(child, predicate))
                }
            }
        }

        walk(self, &mut predicate)
    }

    pub fn hit_test_handle(&self, area: Rect, column: u16, row: u16) -> Option<SplitHandle> {
        let (_, handles) = self.layout_with_handles(area);
        handles.into_iter().find(|handle| {
            let lr = LayoutRect {
                x: handle.rect.x,
                y: handle.rect.y,
                width: handle.rect.width,
                height: handle.rect.height,
            };
            rect_contains(lr, column, row)
        })
    }

    pub fn apply_drag(
        &mut self,
        area: Rect,
        path: &[usize],
        index: usize,
        direction: Direction,
        delta: i16,
    ) -> bool {
        let Some(split_area) = split_area_for_path(self, area, path) else {
            return false;
        };
        let Some(split) = split_at_path_mut(self, path) else {
            return false;
        };
        let LayoutNode::Split {
            weights,
            children,
            constraints,
            resizable,
            ..
        } = split
        else {
            return false;
        };
        if !*resizable || children.len() < 2 || index + 1 >= children.len() {
            return false;
        }
        let sizes = split_sizes(
            split_area,
            direction,
            weights,
            constraints,
            children.len(),
            *resizable,
        );
        if sizes.is_empty() {
            return false;
        }
        let mut sizes = sizes.into_iter().map(|v| v as i16).collect::<Vec<_>>();
        let min_size: i16 = 4;
        let total_pair = sizes[index] + sizes[index + 1];
        let mut left = sizes[index] + delta;
        let min_left = min_size;
        let max_left = (total_pair - min_size).max(min_size);
        left = left.clamp(min_left, max_left);
        let right = total_pair - left;
        sizes[index] = left;
        sizes[index + 1] = right;
        *weights = sizes.iter().map(|v| (*v).max(1) as f32).collect();
        true
    }

    /// Remove the leaf matching `target` by replacing it with a `Void` node.
    ///
    /// Returns `true` if the target was found and replaced.  Does NOT contract
    /// the parent split — call `contract_tree` as a post-processing pass after
    /// all removals are complete.
    pub fn remove_leaf(&mut self, target: Id) -> bool {
        match self {
            LayoutNode::Leaf(id) if *id == target => {
                *self = LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed));
                true
            }
            LayoutNode::Split { children, .. } => {
                let mut found = false;
                for child in children.iter_mut() {
                    if child.remove_leaf(target) {
                        found = true;
                    }
                }
                found
            }
            _ => false,
        }
    }

    /// Post-order contraction pass: collapse degenerate splits.
    ///
    /// - **0 children** → replaced with `Void`.
    /// - **1 child** → replaced with that sole child.
    /// - **2+ children** → left intact, preserving intentional `Void`
    ///   placeholders (snap-assist drop targets, corner insert zones).
    ///
    /// This is safe: `std::mem::replace` takes ownership of `self` so no
    /// outstanding field borrows prevent writing back the result.
    pub fn contract_tree(&mut self) {
        let replacement = match std::mem::replace(
            self,
            LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed)),
        ) {
            LayoutNode::Split { children, .. }
                if children.is_empty() =>
            {
                LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
            }
            LayoutNode::Split { mut children, .. }
                if children.len() == 1 =>
            {
                children[0].contract_tree();
                children.swap_remove(0)
            }
            LayoutNode::Split { direction, mut children, weights, constraints, resizable } => {
                for child in &mut children {
                    child.contract_tree();
                }
                LayoutNode::Split {
                    direction,
                    children,
                    weights,
                    constraints,
                    resizable,
                }
            }
            other => other,
        };
        *self = replacement;
    }

    pub fn insert_leaf(&mut self, target: Id, insert: Id, position: InsertPosition) -> bool {
        match self {
            LayoutNode::Leaf(current) => {
                if *current != target {
                    return false;
                }
                match position {
                    InsertPosition::Left => {
                        *self = LayoutNode::Split {
                            direction: Direction::Horizontal,
                            children: vec![LayoutNode::leaf(insert), LayoutNode::leaf(*current)],
                            weights: vec![1.0, 1.0],
                            constraints: Vec::new(),
                            resizable: true,
                        };
                    }
                    InsertPosition::Right => {
                        *self = LayoutNode::Split {
                            direction: Direction::Horizontal,
                            children: vec![LayoutNode::leaf(*current), LayoutNode::leaf(insert)],
                            weights: vec![1.0, 1.0],
                            constraints: Vec::new(),
                            resizable: true,
                        };
                    }
                    InsertPosition::Top => {
                        *self = LayoutNode::Split {
                            direction: Direction::Vertical,
                            children: vec![LayoutNode::leaf(insert), LayoutNode::leaf(*current)],
                            weights: vec![1.0, 1.0],
                            constraints: Vec::new(),
                            resizable: true,
                        };
                    }
                    InsertPosition::Bottom => {
                        *self = LayoutNode::Split {
                            direction: Direction::Vertical,
                            children: vec![LayoutNode::leaf(*current), LayoutNode::leaf(insert)],
                            weights: vec![1.0, 1.0],
                            constraints: Vec::new(),
                            resizable: true,
                        };
                    }
                    InsertPosition::TopLeft => {
                        let inner = LayoutNode::Split {
                            direction: Direction::Vertical,
                            children: vec![LayoutNode::leaf(insert), LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed))],
                            weights: vec![1.0, 1.0],
                            constraints: Vec::new(),
                            resizable: true,
                        };
                        *self = LayoutNode::Split {
                            direction: Direction::Horizontal,
                            children: vec![inner, LayoutNode::leaf(*current)],
                            weights: vec![1.0, 1.0],
                            constraints: Vec::new(),
                            resizable: true,
                        };
                    }
                    InsertPosition::TopRight => {
                        let inner = LayoutNode::Split {
                            direction: Direction::Vertical,
                            children: vec![LayoutNode::leaf(insert), LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed))],
                            weights: vec![1.0, 1.0],
                            constraints: Vec::new(),
                            resizable: true,
                        };
                        *self = LayoutNode::Split {
                            direction: Direction::Horizontal,
                            children: vec![LayoutNode::leaf(*current), inner],
                            weights: vec![1.0, 1.0],
                            constraints: Vec::new(),
                            resizable: true,
                        };
                    }
                    InsertPosition::BottomLeft => {
                        let inner = LayoutNode::Split {
                            direction: Direction::Vertical,
                            children: vec![LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed)), LayoutNode::leaf(insert)],
                            weights: vec![1.0, 1.0],
                            constraints: Vec::new(),
                            resizable: true,
                        };
                        *self = LayoutNode::Split {
                            direction: Direction::Horizontal,
                            children: vec![inner, LayoutNode::leaf(*current)],
                            weights: vec![1.0, 1.0],
                            constraints: Vec::new(),
                            resizable: true,
                        };
                    }
                    InsertPosition::BottomRight => {
                        let inner = LayoutNode::Split {
                            direction: Direction::Vertical,
                            children: vec![LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed)), LayoutNode::leaf(insert)],
                            weights: vec![1.0, 1.0],
                            constraints: Vec::new(),
                            resizable: true,
                        };
                        *self = LayoutNode::Split {
                            direction: Direction::Horizontal,
                            children: vec![LayoutNode::leaf(*current), inner],
                            weights: vec![1.0, 1.0],
                            constraints: Vec::new(),
                            resizable: true,
                        };
                    }
                }
                true
            }
            LayoutNode::Void(_) => false,
            LayoutNode::Split { children, .. } => {
                for child in children.iter_mut() {
                    if child.insert_leaf(target, insert, position) {
                        return true;
                    }
                }
                false
            }
        }
    }

    fn layout_recursive(
        &self,
        area: Rect,
        regions: &mut Vec<(Id, Rect)>,
        handles: &mut Vec<SplitHandle>,
        path: &mut Vec<usize>,
    ) {
        match self {
            LayoutNode::Leaf(id) => {
                regions.push((*id, area));
            }
            LayoutNode::Void(_) => {}
            LayoutNode::Split {
                direction,
                children,
                weights,
                constraints,
                resizable,
            } => {
                let (rects, gaps) = split_rects_with_gaps(
                    *direction,
                    area,
                    weights,
                    constraints,
                    children.len(),
                    *resizable,
                );
                for (idx, (child, rect)) in children.iter().zip(rects.iter().copied()).enumerate() {
                    path.push(idx);
                    child.layout_recursive(rect, regions, handles, path);
                    path.pop();
                }
                if *resizable && children.len() > 1 {
                    for (index, handle_rect) in gaps.into_iter().enumerate() {
                        handles.push(SplitHandle {
                            rect: handle_rect,
                            path: path.clone(),
                            index,
                            direction: *direction,
                        });
                    }
                }
            }
        }
    }
}

impl<Id: Copy + Eq + Ord> LayoutNode<Id> {
    pub fn void_regions(&self, area: Rect) -> Vec<(usize, Rect)> {
        let mut rects = Vec::new();
        self.void_regions_recursive(area, &mut rects);
        rects
    }

    fn void_regions_recursive(&self, area: Rect, out: &mut Vec<(usize, Rect)>) {
        match self {
            LayoutNode::Void(id) => out.push((*id, area)),
            LayoutNode::Split { direction, children, weights, constraints, resizable } => {
                let (rects, _) = split_rects_with_gaps(*direction, area, weights, constraints, children.len(), *resizable);
                for (child, sub) in children.iter().zip(rects) {
                    child.void_regions_recursive(sub, out);
                }
            }
            _ => {}
        }
    }

    pub fn replace_void_by_id(&mut self, void_id: usize, new_leaf: LayoutNode<Id>) -> bool {
        match self {
            LayoutNode::Void(id) if *id == void_id => {
                *self = new_leaf;
                true
            }
            LayoutNode::Split { children, .. } => {
                for child in children.iter_mut() {
                    if child.replace_void_by_id(void_id, new_leaf.clone()) {
                        return true;
                    }
                }
                false
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SplitHandle {
    pub rect: Rect,
    pub path: Vec<usize>,
    pub index: usize,
    pub direction: Direction,
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
}

impl<Id: Copy + Eq + Ord> TilingLayout<Id> {
    pub fn new(root: LayoutNode<Id>) -> Self {
        Self {
            root,
            drag: None,
            hover: None,
        }
    }

    pub fn root(&self) -> &LayoutNode<Id> {
        &self.root
    }

    pub fn root_mut(&mut self) -> &mut LayoutNode<Id> {
        &mut self.root
    }

    pub fn split_root(&mut self, insert: Id, position: InsertPosition) {
        if matches!(self.root, LayoutNode::Void(_)) {
            self.root = LayoutNode::leaf(insert);
            return;
        }
        self.root = match position {
            InsertPosition::Left => LayoutNode::Split {
                direction: Direction::Horizontal,
                children: vec![LayoutNode::leaf(insert), self.root.clone()],
                weights: vec![1.0, 1.0],
                constraints: Vec::new(),
                resizable: true,
            },
            InsertPosition::Right => LayoutNode::Split {
                direction: Direction::Horizontal,
                children: vec![self.root.clone(), LayoutNode::leaf(insert)],
                weights: vec![1.0, 1.0],
                constraints: Vec::new(),
                resizable: true,
            },
            InsertPosition::Top => LayoutNode::Split {
                direction: Direction::Vertical,
                children: vec![LayoutNode::leaf(insert), self.root.clone()],
                weights: vec![1.0, 1.0],
                constraints: Vec::new(),
                resizable: true,
            },
            InsertPosition::Bottom => LayoutNode::Split {
                direction: Direction::Vertical,
                children: vec![self.root.clone(), LayoutNode::leaf(insert)],
                weights: vec![1.0, 1.0],
                constraints: Vec::new(),
                resizable: true,
            },
            InsertPosition::TopLeft => {
                let inner = LayoutNode::Split {
                    direction: Direction::Vertical,
                    children: vec![LayoutNode::leaf(insert), LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed))],
                    weights: vec![1.0, 1.0],
                    constraints: Vec::new(),
                    resizable: true,
                };
                LayoutNode::Split {
                    direction: Direction::Horizontal,
                    children: vec![inner, self.root.clone()],
                    weights: vec![1.0, 1.0],
                    constraints: Vec::new(),
                    resizable: true,
                }
            }
            InsertPosition::TopRight => {
                let inner = LayoutNode::Split {
                    direction: Direction::Vertical,
                    children: vec![LayoutNode::leaf(insert), LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed))],
                    weights: vec![1.0, 1.0],
                    constraints: Vec::new(),
                    resizable: true,
                };
                LayoutNode::Split {
                    direction: Direction::Horizontal,
                    children: vec![self.root.clone(), inner],
                    weights: vec![1.0, 1.0],
                    constraints: Vec::new(),
                    resizable: true,
                }
            }
            InsertPosition::BottomLeft => {
                let inner = LayoutNode::Split {
                    direction: Direction::Vertical,
                    children: vec![LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed)), LayoutNode::leaf(insert)],
                    weights: vec![1.0, 1.0],
                    constraints: Vec::new(),
                    resizable: true,
                };
                LayoutNode::Split {
                    direction: Direction::Horizontal,
                    children: vec![inner, self.root.clone()],
                    weights: vec![1.0, 1.0],
                    constraints: Vec::new(),
                    resizable: true,
                }
            }
            InsertPosition::BottomRight => {
                let inner = LayoutNode::Split {
                    direction: Direction::Vertical,
                    children: vec![LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed)), LayoutNode::leaf(insert)],
                    weights: vec![1.0, 1.0],
                    constraints: Vec::new(),
                    resizable: true,
                };
                LayoutNode::Split {
                    direction: Direction::Horizontal,
                    children: vec![self.root.clone(), inner],
                    weights: vec![1.0, 1.0],
                    constraints: Vec::new(),
                    resizable: true,
                }
            }
        };
    }

    pub fn regions(&self, area: Rect) -> Vec<(Id, Rect)> {
        self.root.layout(area)
    }

    pub fn void_regions(&self, area: Rect) -> Vec<(usize, Rect)> {
        self.root.void_regions(area)
    }

    pub fn replace_void_by_id(&mut self, void_id: usize, new_leaf: LayoutNode<Id>) -> bool {
        self.root.replace_void_by_id(void_id, new_leaf)
    }

    pub fn project_insert_void(
        &self,
        insert: Id,
        void_id: usize,
        area: Rect,
    ) -> Option<Rect> {
        let mut root = self.root.clone();
        root.remove_leaf(insert);
        if root.replace_void_by_id(void_id, LayoutNode::leaf(insert)) {
            root.layout(area).into_iter()
                .find(|(id, _)| *id == insert)
                .map(|(_, r)| r)
        } else {
            None
        }
    }

    /// Dry-run insert into a cloned layout. Returns the exact `Rect` the
    /// inserted leaf would occupy after `apply_snap`.
    pub fn project_insert(
        &self,
        target: Option<Id>,
        insert: Id,
        position: InsertPosition,
        area: Rect,
    ) -> Option<Rect> {
        let mut root = self.root.clone();
        // Cull stale leaf before re-insertion — the window may still be in
        // the tree (detach_to_floating does not remove it).
        root.remove_leaf(insert);
        let success = match target {
            Some(t) => root.insert_leaf(t, insert, position),
            None => false,
        };
        if !success {
            let mut dummy_layout = TilingLayout::new(root);
            dummy_layout.split_root(insert, position);
            root = dummy_layout.root().clone();
        }
        root.layout(area)
            .into_iter()
            .find(|(id, _)| *id == insert)
            .map(|(_, r)| r)
    }

    pub fn handles(&self, area: Rect) -> Vec<SplitHandle> {
        let (_, handles) = self.root.layout_with_handles(area);
        handles
    }

    pub fn hovered_handle(&self, area: Rect) -> Option<SplitHandle> {
        let (column, row) = self.hover?;
        self.root.hit_test_handle(area, column, row)
    }

    pub fn handle_event(&mut self, event: &crate::events::Event, area: Rect) -> bool {
        use crate::events::MouseEventKind;
        let crate::events::Event::Mouse(mouse) = event else {
            return false;
        };
        self.hover = Some((mouse.column, mouse.row));
        match mouse.kind {
            MouseEventKind::Press(_) => {
                if let Some(handle) = self.root.hit_test_handle(area, mouse.column, mouse.row) {
                    self.drag = Some(DragState {
                        path: handle.path,
                        index: handle.index,
                        direction: handle.direction,
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
        for (id, rect) in self.root.layout(area) {
            regions.set(id, rect);
        }
        for floating in &self.floating {
            regions.set(floating.key, floating.rect.resolve(area));
        }
        regions
    }
}

fn split_rects(
    direction: Direction,
    area: Rect,
    weights: &[f32],
    constraints: &[Constraint],
    child_count: usize,
) -> Vec<Rect> {
    if weights.len() == child_count && weights.iter().any(|value| *value > 0.0) {
        return split_rects_weighted(direction, area, weights, child_count);
    }
    if constraints.len() == child_count {
        return Layout::default()
            .direction(direction)
            .constraints(constraints.to_vec())
            .split(area)
            .to_vec();
    }
    split_rects_weighted(direction, area, weights, child_count)
}

fn split_rects_with_gaps(
    direction: Direction,
    area: Rect,
    weights: &[f32],
    constraints: &[Constraint],
    child_count: usize,
    resizable: bool,
) -> (Vec<Rect>, Vec<Rect>) {
    let gap = gap_size(direction, area, child_count, resizable);
    if gap == 0 || child_count < 2 {
        return (
            split_rects(direction, area, weights, constraints, child_count),
            Vec::new(),
        );
    }
    let gap_total = gap.saturating_mul(child_count.saturating_sub(1) as u16);
    let mut shrunk = area;
    match direction {
        Direction::Horizontal => {
            shrunk.width = area.width.saturating_sub(gap_total);
        }
        Direction::Vertical => {
            shrunk.height = area.height.saturating_sub(gap_total);
        }
    }
    let raw = split_rects(direction, shrunk, weights, constraints, child_count);
    let mut rects = Vec::with_capacity(raw.len());
    for (idx, rect) in raw.into_iter().enumerate() {
        let offset = gap.saturating_mul(idx as u16);
        let shifted = match direction {
            Direction::Horizontal => Rect {
                x: rect.x.saturating_add(i32::from(offset)),
                ..rect
            },
            Direction::Vertical => Rect {
                y: rect.y.saturating_add(i32::from(offset)),
                ..rect
            },
        };
        rects.push(shifted);
    }
    let mut gaps = Vec::with_capacity(child_count.saturating_sub(1));
    for rect in rects.iter().take(child_count.saturating_sub(1)) {
        let rect = match direction {
            Direction::Horizontal => Rect {
                x: rect.x.saturating_add(i32::from(rect.width)),
                y: area.y,
                width: gap,
                height: area.height,
            },
            Direction::Vertical => Rect {
                x: area.x,
                y: rect.y.saturating_add(i32::from(rect.height)),
                width: area.width,
                height: gap,
            },
        };
        gaps.push(rect);
    }
    (rects, gaps)
}

fn split_rects_weighted(
    direction: Direction,
    area: Rect,
    weights: &[f32],
    child_count: usize,
) -> Vec<Rect> {
    let count = child_count.max(1);
    let weights = if weights.len() == child_count {
        weights.to_vec()
    } else {
        vec![1.0; child_count]
    };
    let total_weight: f32 = weights.iter().sum::<f32>().max(1.0);
    let total = match direction {
        Direction::Horizontal => area.width,
        Direction::Vertical => area.height,
    };
    // If weights correspond exactly to pixels (common during resize), use them directly to avoid float drift.
    let exact_match = (total_weight - total as f32).abs() < 0.01;

    let mut sizes = Vec::with_capacity(count);
    let mut used: u16 = 0;
    for (idx, weight) in weights.iter().enumerate() {
        let size = if idx + 1 == count {
            total.saturating_sub(used)
        } else if exact_match {
            let s = weight.round() as u16;
            used = used.saturating_add(s);
            s
        } else {
            let portion = ((*weight / total_weight) * total as f32).floor() as u16;
            used = used.saturating_add(portion);
            portion
        };
        sizes.push(size);
    }
    build_rects_from_sizes(direction, area, &sizes)
}

fn split_sizes(
    area: Rect,
    direction: Direction,
    weights: &[f32],
    constraints: &[Constraint],
    child_count: usize,
    resizable: bool,
) -> Vec<u16> {
    let (rects, _) = split_rects_with_gaps(
        direction,
        area,
        weights,
        constraints,
        child_count,
        resizable,
    );
    rects
        .iter()
        .map(|rect| match direction {
            Direction::Horizontal => rect.width,
            Direction::Vertical => rect.height,
        })
        .collect()
}

fn build_rects_from_sizes(direction: Direction, area: Rect, sizes: &[u16]) -> Vec<Rect> {
    let mut rects = Vec::with_capacity(sizes.len());
    let mut cursor_x = area.x;
    let mut cursor_y = area.y;
    for size in sizes {
        let rect = match direction {
            Direction::Horizontal => {
                let rect = Rect {
                    x: cursor_x,
                    y: area.y,
                    width: *size,
                    height: area.height,
                };
                cursor_x = cursor_x.saturating_add(i32::from(*size));
                rect
            }
            Direction::Vertical => {
                let rect = Rect {
                    x: area.x,
                    y: cursor_y,
                    width: area.width,
                    height: *size,
                };
                cursor_y = cursor_y.saturating_add(i32::from(*size));
                rect
            }
        };
        rects.push(rect);
    }
    rects
}

fn split_area_for_path<Id: Copy + Eq + Ord>(
    node: &LayoutNode<Id>,
    area: Rect,
    path: &[usize],
) -> Option<Rect> {
    let mut area = area;
    let mut current = node;
    for &idx in path {
        let LayoutNode::Split {
            direction,
            children,
            weights,
            constraints,
            resizable,
            ..
        } = current
        else {
            return None;
        };
        let (rects, _) = split_rects_with_gaps(
            *direction,
            area,
            weights,
            constraints,
            children.len(),
            *resizable,
        );
        area = *rects.get(idx)?;
        current = children.get(idx)?;
    }
    Some(area)
}

fn split_at_path_mut<'a, Id: Copy + Eq + Ord>(
    node: &'a mut LayoutNode<Id>,
    path: &[usize],
) -> Option<&'a mut LayoutNode<Id>> {
    let mut current = node;
    for &idx in path {
        let LayoutNode::Split { children, .. } = current else {
            return None;
        };
        current = children.get_mut(idx)?;
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Direction;
    #[test]
    fn build_rects_from_sizes_horizontal() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 3,
        };
        let sizes = [3u16, 7u16];
        let rects = build_rects_from_sizes(Direction::Horizontal, area, &sizes);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].width, 3);
        assert_eq!(rects[1].width, 7);
        assert_eq!(rects[0].x, 0);
        assert_eq!(rects[1].x, 3);
    }

    #[test]
    fn build_rects_from_sizes_vertical() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 9,
        };
        let sizes = [2u16, 3u16, 4u16];
        let rects = build_rects_from_sizes(Direction::Vertical, area, &sizes);
        assert_eq!(rects.len(), 3);
        assert_eq!(rects[0].height, 2);
        assert_eq!(rects[1].height, 3);
        assert_eq!(rects[2].height, 4);
        assert_eq!(rects[2].y, 5);
    }

    #[test]
    fn split_rects_weighted_even() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 11,
            height: 1,
        };
        let weights = [1.0f32, 1.0f32];
        let rects = split_rects_weighted(Direction::Horizontal, area, &weights, 2);
        assert_eq!(rects.len(), 2);
        // floor division: first portion floor((1/2)*11)=5, remainder 6
        assert_eq!(rects[0].width, 5);
        assert_eq!(rects[1].width, 6);
    }

    #[test]
    fn insert_and_remove_leaf_and_split_area_for_path() {
        let mut node: LayoutNode<usize> = LayoutNode::leaf(1);
        // insert a new leaf to the right of 1
        assert!(node.insert_leaf(1, 2, InsertPosition::Right));
        // now root should be a split
        if let LayoutNode::Split { children, .. } = &node {
            assert_eq!(children.len(), 2);
            // first child should be leaf(1)
            assert_eq!(children[0].unwrap_leaf(), Some(1));
            assert_eq!(children[1].unwrap_leaf(), Some(2));
        } else {
            panic!("expected split after insert");
        }

        // compute area for the second child (path [1])
        let area = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 4,
        };
        let sub = split_area_for_path(&node, area, &[1]).expect("should get area for path");
        // since weights default to equal, second rect x should be > 0
        assert!(sub.x > 0);

        // remove leaf 2
        assert!(node.remove_leaf(2));
        // after removal, node should simplify back to leaf(1)
        assert_eq!(node.unwrap_leaf(), Some(1));
    }

    #[test]
    fn hit_test_handle_finds_gap() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let node = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::Leaf(1), LayoutNode::Leaf(2)],
            weights: vec![1.0, 1.0],
            constraints: vec![Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)],
            resizable: true,
        };
        let (_, handles) = node.layout_with_handles(area);
        assert_eq!(handles.len(), 1, "2-window split must produce 1 handle");
        let handle = &handles[0];
        assert_eq!(handle.direction, Direction::Horizontal);
        assert_eq!(handle.index, 0);
        // The gap rect should be at the split point
        assert!(handle.rect.width > 0);
        assert_eq!(handle.rect.height, 24);
        // hit_test_handle at the gap center should find it
        let center_col = (handle.rect.x + i32::from(handle.rect.width) / 2) as u16;
        let center_row = (handle.rect.y + i32::from(handle.rect.height) / 2) as u16;
        let found = node.hit_test_handle(area, center_col, center_row);
        assert!(found.is_some(), "hit_test_handle must find the gap");
        assert_eq!(found.unwrap().direction, Direction::Horizontal);
    }

    #[test]
    fn tiling_handle_event_direct() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let mut layout = TilingLayout::new(LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::Leaf(1), LayoutNode::Leaf(2)],
            weights: vec![1.0, 1.0],
            constraints: vec![Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)],
            resizable: true,
        });

        // Find the gap position
        let handles = layout.handles(area);
        assert_eq!(handles.len(), 1);
        let gap = &handles[0].rect;
        let gap_col = (gap.x + i32::from(gap.width) / 2) as u16;
        let gap_row = (gap.y + i32::from(gap.height) / 2) as u16;

        // Down at the gap
        let down = crate::events::Event::Mouse(crate::events::MouseEvent {
            kind: crate::events::MouseEventKind::Press(crate::events::MouseButton::Left),
            column: gap_col,
            row: gap_row,
            modifiers: crate::events::KeyModifiers::NONE,
        });
        assert!(layout.handle_event(&down, area), "Down must hit the handle");

        // Drag right by 10 columns
        let drag = crate::events::Event::Mouse(crate::events::MouseEvent {
            kind: crate::events::MouseEventKind::Drag(crate::events::MouseButton::Left),
            column: gap_col + 10,
            row: gap_row,
            modifiers: crate::events::KeyModifiers::NONE,
        });
        assert!(layout.handle_event(&drag, area), "Drag must adjust split");

        // Up to release
        let up = crate::events::Event::Mouse(crate::events::MouseEvent {
            kind: crate::events::MouseEventKind::Release(crate::events::MouseButton::Left),
            column: gap_col + 10,
            row: gap_row,
            modifiers: crate::events::KeyModifiers::NONE,
        });
        assert!(layout.handle_event(&up, area), "Up must clear drag state");

        // Verify layout changed: get new regions
        let (regions, _) = layout.root.layout_with_handles(area);
        assert_eq!(regions.len(), 2);
        // Left window should now be wider than right (we dragged right)
        let left_width = regions[0].1.width;
        let right_width = regions[1].1.width;
        assert!(
            left_width > right_width,
            "after dragging split right, left ({}) must be wider than right ({})",
            left_width,
            right_width
        );
    }
}
