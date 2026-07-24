use core::sync::atomic::{AtomicUsize, Ordering};

use crate::BspNode;
use crate::rect::{LayoutRect, Orientation, rect_contains as engine_rect_contains};
use crate::snap::InsertPosition;
use crate::split;

static VOID_ID_COUNTER: AtomicUsize = AtomicUsize::new(1);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    #[default]
    Horizontal,
    Vertical,
}

impl From<Direction> for Orientation {
    fn from(d: Direction) -> Self {
        match d {
            Direction::Horizontal => Orientation::Horizontal,
            Direction::Vertical => Orientation::Vertical,
        }
    }
}

impl From<Orientation> for Direction {
    fn from(o: Orientation) -> Self {
        match o {
            Orientation::Horizontal => Direction::Horizontal,
            Orientation::Vertical => Direction::Vertical,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SplitGap {
    pub rect: LayoutRect,
    pub path: Vec<usize>,
    pub index: usize,
    pub direction: Direction,
}

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

impl<Id: Copy + Eq + Ord> From<BspNode<Id>> for LayoutNode<Id> {
    fn from(bsp: BspNode<Id>) -> Self {
        match bsp {
            BspNode::Leaf(id) => LayoutNode::leaf(id),
            BspNode::Split {
                orientation,
                left,
                right,
                ratio,
            } => {
                let direction = Direction::from(orientation);
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

    pub fn layout_rects(&self, area: LayoutRect) -> Vec<(Id, LayoutRect)> {
        self.layout_with_gaps(area).0
    }

    pub fn layout_with_gaps(&self, area: LayoutRect) -> (Vec<(Id, LayoutRect)>, Vec<SplitGap>) {
        let mut regions = Vec::new();
        let mut gaps = Vec::new();
        self.layout_recursive(area, &mut regions, &mut gaps, &mut Vec::new());
        (regions, gaps)
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

    pub fn swap_leaves(&mut self, source: &Id, target: &Id) -> bool {
        let mut source_path = Vec::new();
        let mut target_path = Vec::new();
        if !self.find_leaf_path(source, &mut source_path, &mut Vec::new()) {
            return false;
        }
        if !self.find_leaf_path(target, &mut target_path, &mut Vec::new()) {
            return false;
        }
        let source_id = {
            let source_node = self.node_at_path_mut(&source_path);
            match source_node {
                Some(LayoutNode::Leaf(id)) => *id,
                _ => return false,
            }
        };
        {
            let source_node = self.node_at_path_mut(&source_path);
            if let Some(node) = source_node {
                *node = LayoutNode::Void(VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed));
            }
        }
        {
            let target_node = self.node_at_path_mut(&target_path);
            match target_node {
                Some(LayoutNode::Leaf(target_id)) => {
                    let target_id_copy = *target_id;
                    if let Some(node) = self.node_at_path_mut(&target_path) {
                        *node = LayoutNode::Leaf(source_id);
                    }
                    if let Some(node) = self.node_at_path_mut(&source_path) {
                        *node = LayoutNode::Leaf(target_id_copy);
                    }
                    true
                }
                _ => false,
            }
        }
    }

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

    pub fn hit_test_gap(&self, area: LayoutRect, column: u16, row: u16) -> Option<SplitGap> {
        let (_, gaps) = self.layout_with_gaps(area);
        gaps.into_iter()
            .find(|gap| engine_rect_contains(&gap.rect, column, row))
    }

    pub fn apply_drag(
        &mut self,
        area: LayoutRect,
        path: &[usize],
        index: usize,
        direction: Direction,
        delta: i16,
        min_size: i16,
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
        let gap = split::gap_size(orientation, total_dim, children.len(), *resizable);
        let sizes = split::split_sizes(
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

    pub fn void_regions(&self, area: LayoutRect) -> Vec<(usize, LayoutRect)> {
        let mut rects = Vec::new();
        self.void_regions_recursive(area, &mut rects);
        rects
    }

    fn void_regions_recursive(&self, area: LayoutRect, out: &mut Vec<(usize, LayoutRect)>) {
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
                let gap = split::gap_size(orientation, total_dim, children.len(), *resizable);
                let (rects, _) = split::split_rects_with_gaps(
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

    #[allow(clippy::single_match)]
    pub fn cleanup_after_removal(&mut self) {
        match self {
            LayoutNode::Split {
                children, weights, ..
            } => {
                for child in children.iter_mut() {
                    child.cleanup_after_removal();
                }
                let mut i = 0;
                while i < children.len() {
                    if matches!(children[i], LayoutNode::Void(_)) {
                        children.remove(i);
                        if i < weights.len() {
                            weights.remove(i);
                        }
                    } else {
                        i += 1;
                    }
                }
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

    pub fn is_empty(&self) -> bool {
        match self {
            LayoutNode::Void(_) => true,
            LayoutNode::Leaf(_) => false,
            LayoutNode::Split { children, .. } => children.iter().all(|c| c.is_empty()),
        }
    }

    fn layout_recursive(
        &self,
        area: LayoutRect,
        regions: &mut Vec<(Id, LayoutRect)>,
        gaps: &mut Vec<SplitGap>,
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
                let gap = split::gap_size(orientation, total_dim, children.len(), *resizable);
                let (rects, split_gaps) = split::split_rects_with_gaps(
                    area,
                    orientation,
                    weights.as_slice(),
                    children.len(),
                    gap,
                );
                for (idx, (child, rect)) in children.iter().zip(rects.iter().copied()).enumerate() {
                    path.push(idx);
                    child.layout_recursive(rect, regions, gaps, path);
                    path.pop();
                }
                if *resizable && children.len() > 1 {
                    for (index, gap_rect) in split_gaps.into_iter().enumerate() {
                        gaps.push(SplitGap {
                            rect: gap_rect,
                            path: path.clone(),
                            index,
                            direction: *direction,
                        });
                    }
                }
            }
        }
    }

    /// Build a BSP tree from rectangles using straddle-tolerant cut selection.
    /// Straddled windows are assigned to the side containing more of their area.
    /// Weights are count-based to ensure each window gets equal space.
    /// Fallback uses aspect-aware direction for the sort axis.
    pub fn from_rects(rects: &[(Id, LayoutRect)]) -> Self {
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

        struct CutCandidate<Id: Copy + Eq + Ord> {
            direction: Direction,
            straddles: usize,
            balance_delta: usize,
            part_a: Vec<(Id, LayoutRect)>,
            part_b: Vec<(Id, LayoutRect)>,
            weight_a: u16,
            weight_b: u16,
        }

        let mut candidates: Vec<CutCandidate<Id>> = Vec::new();

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
            let mut top = Vec::new();
            let mut bottom = Vec::new();
            let mut straddles = 0;

            for &(k, r) in rects {
                let r_bottom = r.y.saturating_add(r.height as i32);
                if r_bottom <= y {
                    top.push((k, r));
                } else if r.y >= y {
                    bottom.push((k, r));
                } else {
                    straddles += 1;
                    let mid = r.y + (r.height as i32 / 2);
                    if mid < y {
                        top.push((k, r));
                    } else {
                        bottom.push((k, r));
                    }
                }
            }

            if !top.is_empty() && !bottom.is_empty() {
                let balance_delta = (top.len() as isize - bottom.len() as isize).unsigned_abs();
                let top_span = {
                    let min = top.iter().map(|(_, r)| r.y).min().unwrap_or(min_y);
                    let max = top
                        .iter()
                        .map(|(_, r)| r.y.saturating_add(r.height as i32))
                        .max()
                        .unwrap_or(y);
                    max.saturating_sub(min).clamp(1, i32::from(u16::MAX)) as u16
                };
                let bot_span = {
                    let min = bottom.iter().map(|(_, r)| r.y).min().unwrap_or(y);
                    let max = bottom
                        .iter()
                        .map(|(_, r)| r.y.saturating_add(r.height as i32))
                        .max()
                        .unwrap_or(max_y);
                    max.saturating_sub(min).clamp(1, i32::from(u16::MAX)) as u16
                };
                candidates.push(CutCandidate {
                    direction: Direction::Vertical,
                    straddles,
                    balance_delta,
                    weight_a: top_span,
                    weight_b: bot_span,
                    part_a: top,
                    part_b: bottom,
                });
            }
        }

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
            let mut left = Vec::new();
            let mut right = Vec::new();
            let mut straddles = 0;

            for &(k, r) in rects {
                let r_right = r.x.saturating_add(r.width as i32);
                if r_right <= x {
                    left.push((k, r));
                } else if r.x >= x {
                    right.push((k, r));
                } else {
                    straddles += 1;
                    let mid = r.x + (r.width as i32 / 2);
                    if mid < x {
                        left.push((k, r));
                    } else {
                        right.push((k, r));
                    }
                }
            }

            if !left.is_empty() && !right.is_empty() {
                let balance_delta = (left.len() as isize - right.len() as isize).unsigned_abs();
                let left_span = {
                    let min = left.iter().map(|(_, r)| r.x).min().unwrap_or(min_x);
                    let max = left
                        .iter()
                        .map(|(_, r)| r.x.saturating_add(r.width as i32))
                        .max()
                        .unwrap_or(x);
                    max.saturating_sub(min).clamp(1, i32::from(u16::MAX)) as u16
                };
                let right_span = {
                    let min = right.iter().map(|(_, r)| r.x).min().unwrap_or(x);
                    let max = right
                        .iter()
                        .map(|(_, r)| r.x.saturating_add(r.width as i32))
                        .max()
                        .unwrap_or(max_x);
                    max.saturating_sub(min).clamp(1, i32::from(u16::MAX)) as u16
                };
                candidates.push(CutCandidate {
                    direction: Direction::Horizontal,
                    straddles,
                    balance_delta,
                    weight_a: left_span,
                    weight_b: right_span,
                    part_a: left,
                    part_b: right,
                });
            }
        }

        if let Some(best) = candidates
            .into_iter()
            .min_by_key(|c| (c.straddles, c.balance_delta))
        {
            return Self::Split {
                direction: best.direction,
                children: vec![
                    Self::from_rects(&best.part_a),
                    Self::from_rects(&best.part_b),
                ],
                weights: vec![best.weight_a, best.weight_b],
                resizable: true,
            };
        }

        let total_w = max_x - min_x;
        let total_h = (max_y - min_y) * 2;

        let mut sorted = rects.to_vec();
        let direction = if total_w >= total_h {
            sorted.sort_unstable_by_key(|(_, r)| (r.x, r.y));
            Direction::Horizontal
        } else {
            sorted.sort_unstable_by_key(|(_, r)| (r.y, r.x));
            Direction::Vertical
        };

        let mid = sorted.len() / 2;
        let left_slice = &sorted[..mid];
        let right_slice = &sorted[mid..];
        let (weight_a, weight_b) = if direction == Direction::Horizontal {
            let left_span = {
                let min = left_slice.iter().map(|(_, r)| r.x).min().unwrap_or(min_x);
                let max = left_slice
                    .iter()
                    .map(|(_, r)| r.x.saturating_add(r.width as i32))
                    .max()
                    .unwrap_or(max_x);
                max.saturating_sub(min).clamp(1, i32::from(u16::MAX)) as u16
            };
            let right_span = {
                let min = right_slice.iter().map(|(_, r)| r.x).min().unwrap_or(min_x);
                let max = right_slice
                    .iter()
                    .map(|(_, r)| r.x.saturating_add(r.width as i32))
                    .max()
                    .unwrap_or(max_x);
                max.saturating_sub(min).clamp(1, i32::from(u16::MAX)) as u16
            };
            (left_span, right_span)
        } else {
            let top_span = {
                let min = left_slice.iter().map(|(_, r)| r.y).min().unwrap_or(min_y);
                let max = left_slice
                    .iter()
                    .map(|(_, r)| r.y.saturating_add(r.height as i32))
                    .max()
                    .unwrap_or(max_y);
                max.saturating_sub(min).clamp(1, i32::from(u16::MAX)) as u16
            };
            let bot_span = {
                let min = right_slice.iter().map(|(_, r)| r.y).min().unwrap_or(min_y);
                let max = right_slice
                    .iter()
                    .map(|(_, r)| r.y.saturating_add(r.height as i32))
                    .max()
                    .unwrap_or(max_y);
                max.saturating_sub(min).clamp(1, i32::from(u16::MAX)) as u16
            };
            (top_span, bot_span)
        };
        Self::Split {
            direction,
            children: vec![Self::from_rects(left_slice), Self::from_rects(right_slice)],
            weights: vec![weight_a, weight_b],
            resizable: true,
        }
    }
}

pub fn split_area_for_path<Id: Copy + Eq + Ord>(
    node: &LayoutNode<Id>,
    area: LayoutRect,
    path: &[usize],
) -> Option<LayoutRect> {
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
        let gap = split::gap_size(orientation, total_dim, children.len(), *resizable);
        let (rects, _) = split::split_rects_with_gaps(
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

pub fn split_at_path_mut<'a, Id: Copy + Eq + Ord>(
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

    #[test]
    fn void_id_counter_increments() {
        let a = VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        let b = VOID_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        assert!(b > a);
    }

    #[test]
    fn from_rects_empty_returns_void() {
        let result = LayoutNode::<i32>::from_rects(&[]);
        assert!(matches!(result, LayoutNode::Void(_)));
    }

    #[test]
    fn from_rects_single_leaf() {
        let rect = LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 24,
        };
        let result = LayoutNode::from_rects(&[(1, rect)]);
        assert!(matches!(result, LayoutNode::Leaf(1)));
    }

    #[test]
    fn from_rects_gapped_windows_equal_columns() {
        let a = LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 24,
        };
        let b = LayoutRect {
            x: 100,
            y: 0,
            width: 40,
            height: 24,
        };
        let result = LayoutNode::from_rects(&[(1, a), (2, b)]);
        match result {
            LayoutNode::Split {
                direction: Direction::Horizontal,
                children,
                weights,
                ..
            } => {
                assert_eq!(children.len(), 2);
                assert_eq!(
                    weights,
                    vec![40, 40],
                    "gapped windows should get equal bounding spans"
                );
            }
            other => panic!("Expected Split, got {:?}", other),
        }
    }

    #[test]
    fn from_rects_unequal_widths_preserves_proportion() {
        let a = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let b = LayoutRect {
            x: 80,
            y: 0,
            width: 20,
            height: 24,
        };
        let result = LayoutNode::from_rects(&[(1, a), (2, b)]);
        match result {
            LayoutNode::Split {
                direction: Direction::Horizontal,
                children,
                weights,
                ..
            } => {
                assert_eq!(children.len(), 2);
                assert_eq!(
                    weights,
                    vec![80, 20],
                    "bounding span weights match window widths"
                );
            }
            other => panic!("Expected Horizontal Split, got {:?}", other),
        }
    }

    #[test]
    fn from_rects_3_windows_top_bottom() {
        let a = LayoutRect {
            x: 0,
            y: 0,
            width: 100,
            height: 25,
        };
        let b = LayoutRect {
            x: 0,
            y: 25,
            width: 50,
            height: 25,
        };
        let c = LayoutRect {
            x: 50,
            y: 25,
            width: 50,
            height: 25,
        };
        let result = LayoutNode::from_rects(&[(1, a), (2, b), (3, c)]);
        match result {
            LayoutNode::Split {
                direction: Direction::Vertical,
                children,
                weights,
                ..
            } => {
                assert_eq!(children.len(), 2);
                assert_eq!(weights, vec![25, 25], "both sides have 25px Y-extent");
            }
            other => panic!("Expected Vertical Split, got {:?}", other),
        }
    }

    #[test]
    fn from_rects_1v3_stacked_equal_width() {
        let a = LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 48,
        };
        let b = LayoutRect {
            x: 40,
            y: 0,
            width: 40,
            height: 16,
        };
        let c = LayoutRect {
            x: 40,
            y: 16,
            width: 40,
            height: 16,
        };
        let d = LayoutRect {
            x: 40,
            y: 32,
            width: 40,
            height: 16,
        };
        let result = LayoutNode::from_rects(&[(1, a), (2, b), (3, c), (4, d)]);
        match result {
            LayoutNode::Split {
                direction: Direction::Horizontal,
                children,
                weights,
                ..
            } => {
                assert_eq!(
                    children.len(),
                    2,
                    "should split into left=[A], right=[B,C,D]"
                );
                assert_eq!(
                    weights,
                    vec![40, 40],
                    "1-vs-3 stacked with same width = equal X-span"
                );
            }
            other => panic!("Expected Horizontal Split, got {:?}", other),
        }
    }

    #[test]
    fn from_rects_overlapping_fallback() {
        let a = LayoutRect {
            x: 0,
            y: 0,
            width: 50,
            height: 50,
        };
        let b = LayoutRect {
            x: 10,
            y: 10,
            width: 50,
            height: 50,
        };
        let c = LayoutRect {
            x: 20,
            y: 20,
            width: 50,
            height: 50,
        };
        let result = LayoutNode::from_rects(&[(1, a), (2, b), (3, c)]);
        assert!(matches!(result, LayoutNode::Split { .. }));
    }

    #[test]
    fn from_rects_with_layout_consistency() {
        let rects = [
            (
                1,
                LayoutRect {
                    x: 0,
                    y: 0,
                    width: 40,
                    height: 24,
                },
            ),
            (
                2,
                LayoutRect {
                    x: 60,
                    y: 0,
                    width: 40,
                    height: 24,
                },
            ),
        ];
        let node = LayoutNode::from_rects(&rects);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 100,
            height: 24,
        };
        let (regions, _) = node.layout_with_gaps(area);
        assert_eq!(regions.len(), 2);
        let sum_w: u16 = regions.iter().map(|(_, r)| r.width).sum();
        assert!(
            sum_w == 100 || sum_w == 99,
            "regions should fill the full width (got {})",
            sum_w
        );
    }
}
