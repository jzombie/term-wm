use crate::Rect;
use crate::layout::Direction;
use core::sync::atomic::{AtomicUsize, Ordering};
use term_wm_layout_engine::LayoutRect;
use term_wm_layout_engine::Orientation;

static VOID_ID_COUNTER: AtomicUsize = AtomicUsize::new(1);

use super::{FloatingPane, RegionMap, rect_contains};

#[derive(Debug, Clone)]
pub enum LayoutNode<Id: Copy + Eq + Ord> {
    Leaf(Id),
    Void(usize),
    Split {
        direction: Direction,
        children: Vec<LayoutNode<Id>>,
        weights: Vec<u16>,
        resizable: bool,
    },
}

impl From<Direction> for Orientation {
    fn from(d: Direction) -> Self {
        match d {
            Direction::Horizontal => Orientation::Horizontal,
            Direction::Vertical => Orientation::Vertical,
        }
    }
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
                let weights = if ratio.total() == 0 {
                    vec![1u16, 1u16]
                } else {
                    vec![ratio.left_part(), ratio.right_part()]
                };
                LayoutNode::Split {
                    direction,
                    children: vec![left_node, right_node],
                    weights,
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

    pub fn split(direction: Direction, children: Vec<LayoutNode<Id>>) -> Self {
        Self::Split {
            direction,
            children,
            weights: Vec::new(),
            resizable: true,
        }
    }

    pub fn split_resizable(
        direction: Direction,
        children: Vec<LayoutNode<Id>>,
        resizable: bool,
    ) -> Self {
        Self::Split {
            direction,
            children,
            weights: Vec::new(),
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

    /// Collect all leaf IDs in order from the tree.
    pub fn collect_leaves(&self) -> Vec<Id> {
        let mut ids = Vec::new();
        self.collect_leaves_recursive(&mut ids);
        ids
    }

    fn collect_leaves_recursive(&self, out: &mut Vec<Id>) {
        match self {
            LayoutNode::Leaf(id) => out.push(*id),
            LayoutNode::Split { children, .. } => {
                for child in children {
                    child.collect_leaves_recursive(out);
                }
            }
            _ => {}
        }
    }

    /// Swap two leaves in the tree by their IDs.
    /// This preserves split ratios and weights while exchanging positions.
    pub fn swap_leaves(&mut self, source: &Id, target: &Id) -> bool {
        // Find paths to both source and target
        let mut source_path = Vec::new();
        let mut target_path = Vec::new();

        if !self.find_leaf_path(source, &mut source_path, &mut Vec::new()) {
            return false;
        }
        if !self.find_leaf_path(target, &mut target_path, &mut Vec::new()) {
            return false;
        }

        // Get the source node's ID and replace it with a temporary void
        let source_id = {
            let source_node = self.node_at_path_mut(&source_path);
            match source_node {
                Some(LayoutNode::Leaf(id)) => *id,
                _ => return false,
            }
        };

        // Replace source with a temporary void
        {
            let source_node = self.node_at_path_mut(&source_path);
            if let Some(node) = source_node {
                *node = LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed));
            }
        }

        // Get target node's ID and replace it with source
        {
            let target_node = self.node_at_path_mut(&target_path);
            match target_node {
                Some(LayoutNode::Leaf(target_id)) => {
                    let target_id_copy = *target_id;
                    // Replace target with source
                    if let Some(node) = self.node_at_path_mut(&target_path) {
                        *node = LayoutNode::Leaf(source_id);
                    }
                    // Replace the void with target
                    if let Some(node) = self.node_at_path_mut(&source_path) {
                        *node = LayoutNode::Leaf(target_id_copy);
                    }
                    true
                }
                _ => false,
            }
        }
    }

    /// Find the path to a leaf with the given ID.
    fn find_leaf_path(&self, target: &Id, path: &mut Vec<usize>, current: &mut Vec<usize>) -> bool {
        match self {
            LayoutNode::Leaf(id) if id == target => {
                path.extend_from_slice(current);
                true
            }
            LayoutNode::Split { children, .. } => {
                for (idx, child) in children.iter().enumerate() {
                    current.push(idx);
                    if child.find_leaf_path(target, path, current) {
                        return true;
                    }
                    current.pop();
                }
                false
            }
            _ => false,
        }
    }

    /// Get a mutable reference to a node at the given path.
    fn node_at_path_mut(&mut self, path: &[usize]) -> Option<&mut LayoutNode<Id>> {
        let mut current = self;
        for &idx in path {
            let LayoutNode::Split { children, .. } = current else {
                return None;
            };
            current = children.get_mut(idx)?;
        }
        Some(current)
    }

    /// Build a flat split node from a list of leaf IDs.
    pub fn build_flat(direction: Direction, ids: Vec<Id>) -> Self {
        if ids.is_empty() {
            return LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed));
        }
        if ids.len() == 1 {
            return LayoutNode::leaf(ids[0]);
        }
        let n = ids.len();
        LayoutNode::Split {
            direction,
            children: ids.into_iter().map(LayoutNode::leaf).collect(),
            weights: vec![1u16; n],
            resizable: true,
        }
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
            resizable,
            ..
        } = split
        else {
            return false;
        };
        if !*resizable || children.len() < 2 || index + 1 >= children.len() {
            return false;
        }
        let orientation = Orientation::from(direction);
        let total_dim = match direction {
            Direction::Horizontal => split_area.width,
            Direction::Vertical => split_area.height,
        };
        let gap =
            term_wm_layout_engine::gap_size(orientation, total_dim, children.len(), *resizable);
        let sizes = term_wm_layout_engine::split_sizes(
            split_area,
            orientation,
            weights.as_slice(),
            children.len(),
            gap,
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
        *weights = sizes.iter().map(|v| (*v).max(1) as u16).collect();
        true
    }

    /// Remove a leaf by ID from a Split parent.  Returns false if the node
    /// itself is a Leaf (caller must handle via `clear_leaf` or similar).
    pub fn remove_leaf(&mut self, id: Id) -> bool {
        match self {
            LayoutNode::Leaf(_) => false,
            LayoutNode::Void(_) => false,
            LayoutNode::Split {
                children, weights, ..
            } => {
                let mut removed = false;
                let mut index = 0;
                while index < children.len() {
                    let is_target = match &children[index] {
                        LayoutNode::Leaf(i) => *i == id,
                        _ => false,
                    };

                    if is_target {
                        children.remove(index);
                        if index < weights.len() {
                            weights.remove(index);
                        }
                        removed = true;
                        break;
                    }

                    if children[index].remove_leaf(id) {
                        removed = true;
                        let is_empty_split = match &children[index] {
                            LayoutNode::Split { children: s, .. } => s.is_empty(),
                            _ => false,
                        };
                        if is_empty_split {
                            children.remove(index);
                            if index < weights.len() {
                                weights.remove(index);
                            }
                        }
                        break;
                    }

                    index += 1;
                }
                if removed {
                    if children.len() == 1 {
                        let only = children.remove(0);
                        *self = only;
                    } else if children.iter().all(|c| matches!(c, LayoutNode::Void(_))) {
                        *self = LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed));
                        return true;
                    }
                }
                removed
            }
        }
    }

    /// If this node is a `Leaf(id)`, replace it with `Void` and return true.
    /// Useful when `remove_leaf` failed because the target IS the root.
    pub fn clear_leaf(&mut self, id: Id) -> bool {
        if matches!(self, LayoutNode::Leaf(current) if *current == id) {
            *self = LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed));
            true
        } else {
            false
        }
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
                            weights: vec![1u16, 1u16],
                            resizable: true,
                        };
                    }
                    InsertPosition::Right => {
                        *self = LayoutNode::Split {
                            direction: Direction::Horizontal,
                            children: vec![LayoutNode::leaf(*current), LayoutNode::leaf(insert)],
                            weights: vec![1u16, 1u16],
                            resizable: true,
                        };
                    }
                    InsertPosition::Top => {
                        *self = LayoutNode::Split {
                            direction: Direction::Vertical,
                            children: vec![LayoutNode::leaf(insert), LayoutNode::leaf(*current)],
                            weights: vec![1u16, 1u16],
                            resizable: true,
                        };
                    }
                    InsertPosition::Bottom => {
                        *self = LayoutNode::Split {
                            direction: Direction::Vertical,
                            children: vec![LayoutNode::leaf(*current), LayoutNode::leaf(insert)],
                            weights: vec![1u16, 1u16],
                            resizable: true,
                        };
                    }
                    InsertPosition::TopLeft => {
                        let inner = LayoutNode::Split {
                            direction: Direction::Vertical,
                            children: vec![
                                LayoutNode::leaf(insert),
                                LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed)),
                            ],
                            weights: vec![1u16, 1u16],
                            resizable: true,
                        };
                        *self = LayoutNode::Split {
                            direction: Direction::Horizontal,
                            children: vec![inner, LayoutNode::leaf(*current)],
                            weights: vec![1u16, 1u16],
                            resizable: true,
                        };
                    }
                    InsertPosition::TopRight => {
                        let inner = LayoutNode::Split {
                            direction: Direction::Vertical,
                            children: vec![
                                LayoutNode::leaf(insert),
                                LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed)),
                            ],
                            weights: vec![1u16, 1u16],
                            resizable: true,
                        };
                        *self = LayoutNode::Split {
                            direction: Direction::Horizontal,
                            children: vec![LayoutNode::leaf(*current), inner],
                            weights: vec![1u16, 1u16],
                            resizable: true,
                        };
                    }
                    InsertPosition::BottomLeft => {
                        let inner = LayoutNode::Split {
                            direction: Direction::Vertical,
                            children: vec![
                                LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed)),
                                LayoutNode::leaf(insert),
                            ],
                            weights: vec![1u16, 1u16],
                            resizable: true,
                        };
                        *self = LayoutNode::Split {
                            direction: Direction::Horizontal,
                            children: vec![inner, LayoutNode::leaf(*current)],
                            weights: vec![1u16, 1u16],
                            resizable: true,
                        };
                    }
                    InsertPosition::BottomRight => {
                        let inner = LayoutNode::Split {
                            direction: Direction::Vertical,
                            children: vec![
                                LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed)),
                                LayoutNode::leaf(insert),
                            ],
                            weights: vec![1u16, 1u16],
                            resizable: true,
                        };
                        *self = LayoutNode::Split {
                            direction: Direction::Horizontal,
                            children: vec![LayoutNode::leaf(*current), inner],
                            weights: vec![1u16, 1u16],
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
                resizable,
            } => {
                let orientation = Orientation::from(*direction);
                let total_dim = match direction {
                    Direction::Horizontal => area.width,
                    Direction::Vertical => area.height,
                };
                let gap = term_wm_layout_engine::gap_size(
                    orientation,
                    total_dim,
                    children.len(),
                    *resizable,
                );
                let (rects, gaps) = term_wm_layout_engine::split_rects_with_gaps(
                    area,
                    orientation,
                    weights.as_slice(),
                    children.len(),
                    gap,
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
                            hitbox_id: crate::hitbox_registry::HitboxId::new(),
                        });
                    }
                }
            }
        }
    }

    /// Build a BSP tree from non-overlapping rectangles using
    /// Top-Down Floorplan Slicing.  Windows without spatial data
    /// should use `insert_window_balanced` instead.
    pub fn from_rects(rects: &[(Id, crate::Rect)]) -> Self {
        if rects.is_empty() {
            return Self::Void(0);
        }
        if rects.len() == 1 {
            return Self::Leaf(rects[0].0);
        }

        let min_x = rects.iter().map(|(_, r)| r.x).min().unwrap_or(0);
        let min_y = rects.iter().map(|(_, r)| r.y).min().unwrap_or(0);
        let max_x = rects
            .iter()
            .map(|(_, r)| r.x.saturating_add(r.width as i32))
            .max()
            .unwrap_or(0);
        let max_y = rects
            .iter()
            .map(|(_, r)| r.y.saturating_add(r.height as i32))
            .max()
            .unwrap_or(0);

        // 1. Horizontal cut (y coordinate)
        let mut y_candidates: Vec<i32> = rects
            .iter()
            .flat_map(|(_, r)| [r.y, r.y.saturating_add(r.height as i32)])
            .collect();
        y_candidates.sort_unstable();
        y_candidates.dedup();

        for &y in &y_candidates {
            if y <= min_y || y >= max_y {
                continue;
            }
            if rects.iter().any(|(_, r)| {
                r.y < y && r.y.saturating_add(r.height as i32) > y
            }) {
                continue;
            }
            let mut top = Vec::new();
            let mut bottom = Vec::new();
            for &(k, r) in rects {
                if r.y.saturating_add(r.height as i32) <= y {
                    top.push((k, r));
                } else {
                    bottom.push((k, r));
                }
            }
            if !top.is_empty() && !bottom.is_empty() {
                return Self::Split {
                    direction: Direction::Vertical,
                    children: vec![
                        Self::from_rects(&top),
                        Self::from_rects(&bottom),
                    ],
                    weights: vec![
                        (y - min_y).max(1) as u16,
                        (max_y - y).max(1) as u16,
                    ],
                    resizable: true,
                };
            }
        }

        // 2. Vertical cut (x coordinate)
        let mut x_candidates: Vec<i32> = rects
            .iter()
            .flat_map(|(_, r)| [r.x, r.x.saturating_add(r.width as i32)])
            .collect();
        x_candidates.sort_unstable();
        x_candidates.dedup();

        for &x in &x_candidates {
            if x <= min_x || x >= max_x {
                continue;
            }
            if rects.iter().any(|(_, r)| {
                r.x < x && r.x.saturating_add(r.width as i32) > x
            }) {
                continue;
            }
            let mut left = Vec::new();
            let mut right = Vec::new();
            for &(k, r) in rects {
                if r.x.saturating_add(r.width as i32) <= x {
                    left.push((k, r));
                } else {
                    right.push((k, r));
                }
            }
            if !left.is_empty() && !right.is_empty() {
                return Self::Split {
                    direction: Direction::Horizontal,
                    children: vec![
                        Self::from_rects(&left),
                        Self::from_rects(&right),
                    ],
                    weights: vec![
                        (x - min_x).max(1) as u16,
                        (max_x - x).max(1) as u16,
                    ],
                    resizable: true,
                };
            }
        }

        // 3. Fallback: sort by y then x, split list
        let mut sorted = rects.to_vec();
        sorted.sort_unstable_by_key(|(_, r)| (r.y, r.x));
        let mid = sorted.len() / 2;
        Self::Split {
            direction: Direction::Horizontal,
            children: vec![
                Self::from_rects(&sorted[..mid]),
                Self::from_rects(&sorted[mid..]),
            ],
            weights: vec![1, 1],
            resizable: true,
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
            LayoutNode::Split {
                direction,
                children,
                weights,
                resizable,
            } => {
                let orientation = Orientation::from(*direction);
                let total_dim = match direction {
                    Direction::Horizontal => area.width,
                    Direction::Vertical => area.height,
                };
                let gap = term_wm_layout_engine::gap_size(
                    orientation,
                    total_dim,
                    children.len(),
                    *resizable,
                );
                let (rects, _) = term_wm_layout_engine::split_rects_with_gaps(
                    area,
                    orientation,
                    weights.as_slice(),
                    children.len(),
                    gap,
                );
                for (child, sub) in children.iter().zip(rects) {
                    child.void_regions_recursive(sub, out);
                }
            }
            _ => {}
        }
    }

    /// Post-removal cleanup: remove Void children and collapse degenerate splits.
    ///
    /// After removing a leaf from the tree, this pass ensures:
    /// 1. Void children (from remove_leaf) are removed from multi-child splits
    /// 2. Splits with 0 children → Void
    /// 3. Splits with 1 child → replace with that child
    /// 4. Splits where ALL children are Void → replace with Void
    ///
    /// Call this after every `remove_leaf` to keep the layout clean.
    #[allow(clippy::single_match)]
    pub fn cleanup_after_removal(&mut self) {
        match self {
            LayoutNode::Split {
                children, weights, ..
            } => {
                // Recurse first
                for child in children.iter_mut() {
                    child.cleanup_after_removal();
                }
                // Remove Void children and corresponding weights
                let mut i = 0;
                while i < children.len() {
                    if matches!(children[i], LayoutNode::Void(_)) {
                        children.remove(i);
                        if i < weights.len() {
                            weights.remove(i);
                        }
                        // Don't increment — next element shifted into this index
                    } else {
                        i += 1;
                    }
                }
                // Contract degenerate splits
                match children.len() {
                    0 => {
                        *self = LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed));
                    }
                    1 => {
                        let only = children.remove(0);
                        *self = only;
                    }
                    _ => {
                        if children.iter().all(|c| matches!(c, LayoutNode::Void(_))) {
                            *self =
                                LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Reset all split weights to 1.0 so every leaf gets equal space.
    /// Call after any snap insertion to rebalance the layout.
    #[allow(clippy::single_match)]
    pub fn normalize_weights(&mut self) {
        match self {
            LayoutNode::Split {
                weights, children, ..
            } => {
                for w in weights.iter_mut() {
                    *w = 1;
                }
                for child in children.iter_mut() {
                    child.normalize_weights();
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

    /// Check if this node is structurally empty (Void or all children are empty).
    /// Non-allocating O(depth) tree traversal.
    pub fn is_empty(&self) -> bool {
        match self {
            LayoutNode::Void(_) => true,
            LayoutNode::Leaf(_) => false,
            LayoutNode::Split { children, .. } => children.iter().all(|c| c.is_empty()),
        }
    }
}

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
    /// Whether monocle mode is active (terminal width < threshold)
    monocle_active: bool,
    /// Width threshold below which monocle mode activates (default: 80 columns)
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
        let void_id = VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        Self::new(LayoutNode::Void(void_id))
    }

    /// Update monocle mode state based on terminal width.
    pub fn update_monocle_state(&mut self, terminal_width: u16) {
        let should_be_monocle = terminal_width < self.monocle_width_threshold;
        if should_be_monocle != self.monocle_active {
            self.monocle_active = should_be_monocle;
        }
    }

    /// Check if monocle mode is active.
    pub fn is_monocle(&self) -> bool {
        self.monocle_active
    }

    /// Set the monocle width threshold.
    pub fn set_monocle_width_threshold(&mut self, threshold: u16) {
        self.monocle_width_threshold = threshold;
    }

    /// Get the monocle width threshold.
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
        if let LayoutNode::Void(existing_void_id) = self.root {
            self.root = match position {
                InsertPosition::Left | InsertPosition::TopLeft | InsertPosition::BottomLeft => {
                    LayoutNode::Split {
                        direction: Direction::Horizontal,
                        children: vec![
                            LayoutNode::leaf(insert),
                            LayoutNode::Void(existing_void_id),
                        ],
                        weights: vec![1u16, 1u16],
                        resizable: true,
                    }
                }
                InsertPosition::Right | InsertPosition::TopRight | InsertPosition::BottomRight => {
                    LayoutNode::Split {
                        direction: Direction::Horizontal,
                        children: vec![
                            LayoutNode::Void(existing_void_id),
                            LayoutNode::leaf(insert),
                        ],
                        weights: vec![1u16, 1u16],
                        resizable: true,
                    }
                }
                InsertPosition::Top => LayoutNode::Split {
                    direction: Direction::Vertical,
                    children: vec![LayoutNode::leaf(insert), LayoutNode::Void(existing_void_id)],
                    weights: vec![1u16, 1u16],
                    resizable: true,
                },
                InsertPosition::Bottom => LayoutNode::Split {
                    direction: Direction::Vertical,
                    children: vec![LayoutNode::Void(existing_void_id), LayoutNode::leaf(insert)],
                    weights: vec![1u16, 1u16],
                    resizable: true,
                },
            };
            return;
        }
        self.root = match position {
            InsertPosition::Left => LayoutNode::Split {
                direction: Direction::Horizontal,
                children: vec![LayoutNode::leaf(insert), self.root.clone()],
                weights: vec![1u16, 1u16],
                resizable: true,
            },
            InsertPosition::Right => LayoutNode::Split {
                direction: Direction::Horizontal,
                children: vec![self.root.clone(), LayoutNode::leaf(insert)],
                weights: vec![1u16, 1u16],
                resizable: true,
            },
            InsertPosition::Top => LayoutNode::Split {
                direction: Direction::Vertical,
                children: vec![LayoutNode::leaf(insert), self.root.clone()],
                weights: vec![1u16, 1u16],
                resizable: true,
            },
            InsertPosition::Bottom => LayoutNode::Split {
                direction: Direction::Vertical,
                children: vec![self.root.clone(), LayoutNode::leaf(insert)],
                weights: vec![1u16, 1u16],
                resizable: true,
            },
            InsertPosition::TopLeft => {
                // Extract all windows. D goes top-left, first remaining goes
                // top-right, rest fill the bottom half.
                let mut ids = self.root.collect_leaves();
                ids.retain(|id| *id != insert);
                if ids.is_empty() {
                    return self.root = LayoutNode::leaf(insert);
                }
                let first = ids.remove(0);
                if ids.is_empty() {
                    // Exactly 2 windows — split only the dragged window's
                    // side.  The other window stays in its lane at full height.
                    // The unused quadrant becomes a Void placeholder.
                    let void_id = VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
                    self.root = LayoutNode::Split {
                        direction: Direction::Horizontal,
                        children: vec![
                            LayoutNode::Split {
                                direction: Direction::Vertical,
                                children: vec![LayoutNode::leaf(insert), LayoutNode::Void(void_id)],
                                weights: vec![1u16, 1u16],
                                resizable: true,
                            },
                            LayoutNode::leaf(first),
                        ],
                        weights: vec![1u16, 1u16],
                        resizable: true,
                    };
                    return;
                }
                let bottom = LayoutNode::build_flat(Direction::Horizontal, ids);
                LayoutNode::Split {
                    direction: Direction::Vertical,
                    children: vec![
                        LayoutNode::Split {
                            direction: Direction::Horizontal,
                            children: vec![LayoutNode::leaf(insert), LayoutNode::leaf(first)],
                            weights: vec![1u16, 1u16],
                            resizable: true,
                        },
                        bottom,
                    ],
                    weights: vec![1u16, 1u16],
                    resizable: true,
                }
            }
            InsertPosition::TopRight => {
                let mut ids = self.root.collect_leaves();
                ids.retain(|id| *id != insert);
                if ids.is_empty() {
                    return self.root = LayoutNode::leaf(insert);
                }
                let first = ids.remove(0);
                if ids.is_empty() {
                    let void_id = VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
                    self.root = LayoutNode::Split {
                        direction: Direction::Horizontal,
                        children: vec![
                            LayoutNode::leaf(first),
                            LayoutNode::Split {
                                direction: Direction::Vertical,
                                children: vec![LayoutNode::leaf(insert), LayoutNode::Void(void_id)],
                                weights: vec![1u16, 1u16],
                                resizable: true,
                            },
                        ],
                        weights: vec![1u16, 1u16],
                        resizable: true,
                    };
                    return;
                }
                let bottom = LayoutNode::build_flat(Direction::Horizontal, ids);
                LayoutNode::Split {
                    direction: Direction::Vertical,
                    children: vec![
                        LayoutNode::Split {
                            direction: Direction::Horizontal,
                            children: vec![LayoutNode::leaf(first), LayoutNode::leaf(insert)],
                            weights: vec![1u16, 1u16],
                            resizable: true,
                        },
                        bottom,
                    ],
                    weights: vec![1u16, 1u16],
                    resizable: true,
                }
            }
            InsertPosition::BottomLeft => {
                let mut ids = self.root.collect_leaves();
                ids.retain(|id| *id != insert);
                if ids.is_empty() {
                    return self.root = LayoutNode::leaf(insert);
                }
                let first = ids.remove(0);
                if ids.is_empty() {
                    let void_id = VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
                    self.root = LayoutNode::Split {
                        direction: Direction::Horizontal,
                        children: vec![
                            LayoutNode::Split {
                                direction: Direction::Vertical,
                                children: vec![LayoutNode::Void(void_id), LayoutNode::leaf(insert)],
                                weights: vec![1u16, 1u16],
                                resizable: true,
                            },
                            LayoutNode::leaf(first),
                        ],
                        weights: vec![1u16, 1u16],
                        resizable: true,
                    };
                    return;
                }
                let top = LayoutNode::build_flat(Direction::Horizontal, ids);
                LayoutNode::Split {
                    direction: Direction::Vertical,
                    children: vec![
                        top,
                        LayoutNode::Split {
                            direction: Direction::Horizontal,
                            children: vec![LayoutNode::leaf(insert), LayoutNode::leaf(first)],
                            weights: vec![1u16, 1u16],
                            resizable: true,
                        },
                    ],
                    weights: vec![1u16, 1u16],
                    resizable: true,
                }
            }
            InsertPosition::BottomRight => {
                let mut ids = self.root.collect_leaves();
                ids.retain(|id| *id != insert);
                if ids.is_empty() {
                    return self.root = LayoutNode::leaf(insert);
                }
                let first = ids.remove(0);
                if ids.is_empty() {
                    let void_id = VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
                    self.root = LayoutNode::Split {
                        direction: Direction::Horizontal,
                        children: vec![
                            LayoutNode::leaf(first),
                            LayoutNode::Split {
                                direction: Direction::Vertical,
                                children: vec![LayoutNode::Void(void_id), LayoutNode::leaf(insert)],
                                weights: vec![1u16, 1u16],
                                resizable: true,
                            },
                        ],
                        weights: vec![1u16, 1u16],
                        resizable: true,
                    };
                    return;
                }
                let top = LayoutNode::build_flat(Direction::Horizontal, ids);
                LayoutNode::Split {
                    direction: Direction::Vertical,
                    children: vec![
                        top,
                        LayoutNode::Split {
                            direction: Direction::Horizontal,
                            children: vec![LayoutNode::leaf(first), LayoutNode::leaf(insert)],
                            weights: vec![1u16, 1u16],
                            resizable: true,
                        },
                    ],
                    weights: vec![1u16, 1u16],
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

    /// Swap two nodes in the layout tree by their IDs.
    /// This preserves split ratios and weights while exchanging positions.
    pub fn swap_nodes(&mut self, source: &Id, target: &Id) -> bool {
        self.root.swap_leaves(source, target)
    }

    /// Topology-aware insertion: finds the largest leaf by area,
    /// splits along its longer axis, inserts `insert` adjacent.
    /// Used by both preview simulation and commit for balanced tiling.
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

        // Anti-degeneracy: force direction when splitting would produce sub-threshold tiles
        let pos = if largest_rect.width / 2 < crate::constants::MIN_TILE_WIDTH {
            InsertPosition::Bottom // horizontal split would create <20col ribbons
        } else if largest_rect.height / 2 < crate::constants::MIN_TILE_HEIGHT {
            InsertPosition::Right // vertical split would create <6row stubs
        } else {
            // Cell aspect ratio: height is ~2x width, so scale before comparison
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
        let mut root = self.root.clone();
        root.remove_leaf(insert);
        if root.replace_void_by_id(void_id, LayoutNode::leaf(insert)) {
            root.layout(area)
                .into_iter()
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
        let removed = root.remove_leaf(insert);
        if !removed && matches!(&root, LayoutNode::Leaf(id) if *id == insert) {
            // The tree is a single leaf matching insert — cannot remove a
            // leaf from itself.  Replace with Void so split_root doesn't
            // create duplicate entries.
            root = LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed));
        }
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

    /// Encapsulated leaf removal: removes `key` from the tree, cleans up
    /// degenerate splits, and clears the leaf node if it was the root.
    pub fn remove_window(&mut self, key: Id) {
        self.root.remove_leaf(key);
        self.root.cleanup_after_removal();
        self.root.clear_leaf(key);
    }

    /// Check if the layout tree is structurally empty (no live leaves).
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
        for (id, rect) in self.root.layout(area) {
            regions.set(id, rect);
        }
        for floating in &self.floating {
            regions.set(floating.key, floating.rect.resolve(area));
        }
        regions
    }
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
            resizable,
            ..
        } = current
        else {
            return None;
        };
        let orientation = Orientation::from(*direction);
        let total_dim = match direction {
            Direction::Horizontal => area.width,
            Direction::Vertical => area.height,
        };
        let gap =
            term_wm_layout_engine::gap_size(orientation, total_dim, children.len(), *resizable);
        let (rects, _) = term_wm_layout_engine::split_rects_with_gaps(
            area,
            orientation,
            weights.as_slice(),
            children.len(),
            gap,
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
        let rects =
            term_wm_layout_engine::build_rects_from_sizes(area, Orientation::Horizontal, &sizes);
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
        let rects =
            term_wm_layout_engine::build_rects_from_sizes(area, Orientation::Vertical, &sizes);
        assert_eq!(rects.len(), 3);
        assert_eq!(rects[0].height, 2);
        assert_eq!(rects[1].height, 3);
        assert_eq!(rects[2].height, 4);
        assert_eq!(rects[2].y, 5);
    }

    #[test]
    fn split_rects_nary_even() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 11,
            height: 1,
        };
        let weights = [1u16, 1u16];
        let rects =
            term_wm_layout_engine::split_rects_weighted(area, Orientation::Horizontal, &weights, 2);
        assert_eq!(rects.len(), 2);
        // integer division: first portion (11*1/2)=5, remainder 6
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
            weights: vec![1u16, 1u16],

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
            weights: vec![1u16, 1u16],

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
    fn normalize_weights_resets_to_equal() {
        let mut node: LayoutNode<usize> = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::leaf(1), LayoutNode::leaf(2)],
            weights: vec![3u16, 1u16],
            resizable: true,
        };
        node.normalize_weights();
        if let LayoutNode::Split { weights, .. } = &node {
            assert!(weights.iter().all(|w| *w == 1u16));
        } else {
            panic!("expected split");
        }
    }

    #[test]
    fn build_flat_empty_returns_void() {
        let node: LayoutNode<usize> = LayoutNode::build_flat(Direction::Horizontal, vec![]);
        assert!(node.unwrap_leaf().is_none());
    }

    #[test]
    fn build_flat_single_returns_leaf() {
        let node = LayoutNode::build_flat(Direction::Horizontal, vec![42]);
        assert_eq!(node.unwrap_leaf(), Some(42));
    }

    #[test]
    fn build_flat_multiple_returns_split() {
        let node = LayoutNode::build_flat(Direction::Vertical, vec![1, 2, 3]);
        if let LayoutNode::Split {
            children,
            weights,
            direction,
            ..
        } = &node
        {
            assert_eq!(children.len(), 3);
            assert_eq!(*direction, Direction::Vertical);
            assert!(weights.iter().all(|w| *w == 1u16));
        } else {
            panic!("expected split");
        }
    }

    #[test]
    fn void_regions_returns_voids() {
        let node: LayoutNode<usize> = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::leaf(1), LayoutNode::Void(99)],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let voids = node.void_regions(area);
        assert_eq!(voids.len(), 1);
        assert_eq!(voids[0].0, 99);
    }

    #[test]
    fn swap_leaves_exchanges_positions() {
        let mut node: LayoutNode<usize> = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![
                LayoutNode::leaf(1),
                LayoutNode::leaf(2),
                LayoutNode::leaf(3),
            ],
            weights: vec![1u16, 1u16, 1u16],
            resizable: true,
        };
        assert!(node.swap_leaves(&1, &3));
        let leaves = node.collect_leaves();
        assert_eq!(leaves, vec![3, 2, 1]);
    }

    #[test]
    fn swap_leaves_same_id_returns_false() {
        let mut node: LayoutNode<usize> = LayoutNode::leaf(1);
        // Swapping a leaf with itself: source gets replaced with Void,
        // then target lookup finds Void instead of the target leaf → returns false
        assert!(!node.swap_leaves(&1, &1));
    }

    #[test]
    fn swap_leaves_nonexistent_returns_false() {
        let mut node: LayoutNode<usize> = LayoutNode::leaf(1);
        assert!(!node.swap_leaves(&1, &2));
    }

    #[test]
    fn cleanup_after_removes_void_children() {
        let mut node: LayoutNode<usize> = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::leaf(1), LayoutNode::Void(99)],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        node.cleanup_after_removal();
        // After removing void, single child should collapse to leaf
        assert_eq!(node.unwrap_leaf(), Some(1));
    }

    #[test]
    fn cleanup_all_voids_becomes_void() {
        let mut node: LayoutNode<usize> = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::Void(1), LayoutNode::Void(2)],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        node.cleanup_after_removal();
        assert!(matches!(node, LayoutNode::Void(_)));
    }

    #[test]
    fn cleanup_empty_split_becomes_void() {
        let mut node: LayoutNode<usize> = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![],
            weights: vec![],
            resizable: true,
        };
        node.cleanup_after_removal();
        assert!(matches!(node, LayoutNode::Void(_)));
    }

    #[test]
    fn clear_leaf_replaces_with_void() {
        let mut node: LayoutNode<usize> = LayoutNode::leaf(42);
        assert!(node.clear_leaf(42));
        assert!(matches!(node, LayoutNode::Void(_)));
    }

    #[test]
    fn clear_leaf_wrong_id_returns_false() {
        let mut node: LayoutNode<usize> = LayoutNode::leaf(42);
        assert!(!node.clear_leaf(99));
    }

    #[test]
    fn subtree_any_finds_matching_leaf() {
        let node: LayoutNode<usize> = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::leaf(1), LayoutNode::leaf(2)],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        assert!(node.subtree_any(|id| id == 2));
        assert!(!node.subtree_any(|id| id == 99));
    }

    #[test]
    fn node_at_path_returns_none_for_invalid_path() {
        let node: LayoutNode<usize> = LayoutNode::leaf(1);
        assert!(node.node_at_path(&[0]).is_none());
    }

    #[test]
    fn collect_leaves_from_nested() {
        let node: LayoutNode<usize> = LayoutNode::Split {
            direction: Direction::Vertical,
            children: vec![
                LayoutNode::leaf(1),
                LayoutNode::Split {
                    direction: Direction::Horizontal,
                    children: vec![LayoutNode::leaf(2), LayoutNode::leaf(3)],
                    weights: vec![1u16, 1u16],
                    resizable: true,
                },
            ],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        assert_eq!(node.collect_leaves(), vec![1, 2, 3]);
    }

    #[test]
    fn insert_leaf_left_on_single() {
        let mut node: LayoutNode<usize> = LayoutNode::leaf(1);
        assert!(node.insert_leaf(1, 2, InsertPosition::Left));
        let leaves = node.collect_leaves();
        assert_eq!(leaves, vec![2, 1]);
    }

    #[test]
    fn insert_leaf_top_on_single() {
        let mut node: LayoutNode<usize> = LayoutNode::leaf(1);
        assert!(node.insert_leaf(1, 2, InsertPosition::Top));
        let leaves = node.collect_leaves();
        assert_eq!(leaves, vec![2, 1]);
    }

    #[test]
    fn insert_leaf_bottom_on_single() {
        let mut node: LayoutNode<usize> = LayoutNode::leaf(1);
        assert!(node.insert_leaf(1, 2, InsertPosition::Bottom));
        let leaves = node.collect_leaves();
        assert_eq!(leaves, vec![1, 2]);
    }

    #[test]
    fn insert_leaf_nonexistent_target_returns_false() {
        let mut node: LayoutNode<usize> = LayoutNode::leaf(1);
        assert!(!node.insert_leaf(99, 2, InsertPosition::Right));
    }

    #[test]
    fn insert_leaf_in_nested_split() {
        let mut node: LayoutNode<usize> = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::leaf(1), LayoutNode::leaf(2)],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        assert!(node.insert_leaf(2, 3, InsertPosition::Right));
        let leaves = node.collect_leaves();
        assert_eq!(leaves, vec![1, 2, 3]);
    }

    #[test]
    fn tiling_layout_split_root_void_to_left() {
        let mut layout = TilingLayout::new_void();
        layout.split_root(1, InsertPosition::Left);
        let leaves = layout.root().collect_leaves();
        assert_eq!(leaves, vec![1]);
    }

    #[test]
    fn tiling_layout_split_root_void_to_right() {
        let mut layout = TilingLayout::new_void();
        layout.split_root(1, InsertPosition::Right);
        let leaves = layout.root().collect_leaves();
        assert_eq!(leaves, vec![1]);
    }

    #[test]
    fn tiling_layout_split_root_void_to_top() {
        let mut layout = TilingLayout::new_void();
        layout.split_root(1, InsertPosition::Top);
        let leaves = layout.root().collect_leaves();
        assert_eq!(leaves, vec![1]);
    }

    #[test]
    fn tiling_layout_split_root_void_to_bottom() {
        let mut layout = TilingLayout::new_void();
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
        assert!(!node.apply_drag(area, &[0], 0, Direction::Horizontal, 5));
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

    #[test]
    fn void_id_counter_increments() {
        let a = VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        let b = VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        assert!(b > a);
    }
}
