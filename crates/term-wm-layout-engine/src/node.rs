use crate::rect::{LayoutError, LayoutRect, Orientation, Ratio, SizeConstraints};
use crate::snap::InsertPosition;
use crate::split;

/// A strict binary (BSP) split tree.
///
/// Each `Split` node divides its area into two children using an integer
/// ratio.  Remainder isolation guarantees no dead zones:
/// `sum(child widths) == parent width`.
#[derive(Debug, Clone)]
pub enum BspNode<Id: Copy + Eq + Ord> {
    Leaf(Id),
    Split {
        orientation: Orientation,
        left: Box<BspNode<Id>>,
        right: Box<BspNode<Id>>,
        ratio: Ratio,
    },
}

impl<Id: Copy + Eq + Ord> BspNode<Id> {
    pub fn leaf(id: Id) -> Self {
        Self::Leaf(id)
    }

    pub fn is_leaf(&self) -> bool {
        matches!(self, Self::Leaf(_))
    }

    pub fn unwrap_leaf(&self) -> Option<Id> {
        match self {
            Self::Leaf(id) => Some(*id),
            _ => None,
        }
    }

    pub fn subtree_any(&self, predicate: &mut impl FnMut(Id) -> bool) -> bool {
        match self {
            Self::Leaf(id) => predicate(*id),
            Self::Split { left, right, .. } => {
                left.subtree_any(predicate) || right.subtree_any(predicate)
            }
        }
    }

    pub fn all_leaf_ids(&self) -> Vec<Id> {
        let mut ids = Vec::new();
        self.collect_ids(&mut ids);
        ids
    }

    fn collect_ids(&self, ids: &mut Vec<Id>) {
        match self {
            Self::Leaf(id) => ids.push(*id),
            Self::Split { left, right, .. } => {
                left.collect_ids(ids);
                right.collect_ids(ids);
            }
        }
    }

    pub fn layout(&self, area: LayoutRect) -> Vec<(Id, LayoutRect)> {
        let mut regions = Vec::new();
        self.layout_recursive(area, &mut regions);
        regions
    }

    fn layout_recursive(&self, area: LayoutRect, regions: &mut Vec<(Id, LayoutRect)>) {
        match self {
            Self::Leaf(id) => {
                regions.push((*id, area));
            }
            Self::Split {
                orientation,
                left,
                right,
                ratio,
            } => {
                let (left_area, right_area) = split::split_rect_bsp(area, *orientation, *ratio);
                left.layout_recursive(left_area, regions);
                right.layout_recursive(right_area, regions);
            }
        }
    }

    pub fn insert_leaf(
        &mut self,
        target: Id,
        insert: Id,
        position: InsertPosition,
        area: LayoutRect,
        constraints: &SizeConstraints,
    ) -> Result<(), LayoutError> {
        match self {
            Self::Leaf(current) => {
                if *current != target {
                    return Err(LayoutError::NotFound);
                }
                let (orientation, left_child, right_child) = match position {
                    InsertPosition::Left => (
                        Orientation::Horizontal,
                        Self::leaf(insert),
                        Self::leaf(*current),
                    ),
                    InsertPosition::Right => (
                        Orientation::Horizontal,
                        Self::leaf(*current),
                        Self::leaf(insert),
                    ),
                    InsertPosition::Top => (
                        Orientation::Vertical,
                        Self::leaf(insert),
                        Self::leaf(*current),
                    ),
                    InsertPosition::Bottom => (
                        Orientation::Vertical,
                        Self::leaf(*current),
                        Self::leaf(insert),
                    ),
                    // Corners: use vertical split (top portion for new window)
                    InsertPosition::TopLeft
                    | InsertPosition::TopRight => (
                        Orientation::Vertical,
                        Self::leaf(insert),
                        Self::leaf(*current),
                    ),
                    InsertPosition::BottomLeft
                    | InsertPosition::BottomRight => (
                        Orientation::Vertical,
                        Self::leaf(*current),
                        Self::leaf(insert),
                    ),
                };
                let half_dim = match orientation {
                    Orientation::Horizontal => area.width / 2,
                    Orientation::Vertical => area.height / 2,
                };
                if orientation == Orientation::Horizontal {
                    if half_dim < constraints.min_width {
                        return Err(LayoutError::ConstraintViolated(*constraints));
                    }
                    if area.height < constraints.min_height {
                        return Err(LayoutError::ConstraintViolated(*constraints));
                    }
                } else {
                    if half_dim < constraints.min_height {
                        return Err(LayoutError::ConstraintViolated(*constraints));
                    }
                    if area.width < constraints.min_width {
                        return Err(LayoutError::ConstraintViolated(*constraints));
                    }
                }
                *self = Self::Split {
                    orientation,
                    left: Box::new(left_child),
                    right: Box::new(right_child),
                    ratio: Ratio::half(),
                };
                Ok(())
            }
            Self::Split {
                orientation,
                left,
                right,
                ratio,
            } => {
                let (left_area, right_area) = split::split_rect_bsp(area, *orientation, *ratio);
                left.insert_leaf(target, insert, position, left_area, constraints)
                    .or_else(|_| {
                        right.insert_leaf(target, insert, position, right_area, constraints)
                    })
            }
        }
    }

    pub fn apply_drag(
        &mut self,
        area: LayoutRect,
        path: &[bool],
        orientation: Orientation,
        delta: i16,
        constraints: &SizeConstraints,
    ) -> bool {
        if path.is_empty() {
            return false;
        }
        match self {
            Self::Split {
                orientation: split_orient,
                left,
                right,
                ratio,
            } => {
                if path.len() == 1 {
                    if *split_orient != orientation {
                        return false;
                    }
                    let total = match orientation {
                        Orientation::Horizontal => u32::from(area.width),
                        Orientation::Vertical => u32::from(area.height),
                    };
                    if total == 0 {
                        return false;
                    }
                    let min_first = match orientation {
                        Orientation::Horizontal => u32::from(constraints.min_width),
                        Orientation::Vertical => u32::from(constraints.min_height),
                    };
                    let min_second = min_first;
                    let current =
                        u32::from(ratio.left_part()) * total / u32::from(ratio.total()).max(1);
                    let new_pos = (current as i32).saturating_add(i32::from(delta));
                    let new_pos = new_pos.max(i32::from(min_first as u16));
                    let bound = (total as i32).saturating_sub(i32::from(min_second as u16));
                    let new_pos = new_pos.min(bound) as u32;
                    let new_ratio_left = new_pos * u32::from(ratio.total()).max(1) / total.max(1);
                    let new_ratio_left = new_ratio_left.max(1) as u16;
                    let new_ratio_total = ratio.total().max(1);
                    *ratio = Ratio(new_ratio_left, new_ratio_total);
                    true
                } else {
                    let (left_area, right_area) =
                        split::split_rect_bsp(area, *split_orient, *ratio);
                    let rest = &path[1..];
                    if !path[0] {
                        left.apply_drag(left_area, rest, orientation, delta, constraints)
                    } else {
                        right.apply_drag(right_area, rest, orientation, delta, constraints)
                    }
                }
            }
            Self::Leaf(_) => false,
        }
    }

    pub fn remove_leaf(&mut self, id: Id) -> Result<(), LayoutError> {
        match self {
            Self::Leaf(current) => {
                if *current == id {
                    return Err(LayoutError::NotFound);
                }
                Err(LayoutError::NotFound)
            }
            Self::Split { left, right, .. } => {
                let left_is_target = left.as_ref().unwrap_leaf() == Some(id);
                let right_is_target = right.as_ref().unwrap_leaf() == Some(id);

                if left_is_target {
                    *self = *right.clone();
                    return Ok(());
                }
                if right_is_target {
                    *self = *left.clone();
                    return Ok(());
                }

                left.remove_leaf(id).or_else(|_| right.remove_leaf(id))?;

                if left.is_leaf() && right.is_leaf() {
                    return Ok(());
                }

                Ok(())
            }
        }
    }

    pub fn find_path(&self, target: Id) -> Option<Vec<bool>> {
        match self {
            Self::Leaf(id) => {
                if *id == target {
                    Some(Vec::new())
                } else {
                    None
                }
            }
            Self::Split { left, right, .. } => {
                if let Some(mut path) = left.find_path(target) {
                    path.insert(0, false);
                    return Some(path);
                }
                if let Some(mut path) = right.find_path(target) {
                    path.insert(0, true);
                    return Some(path);
                }
                None
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum NaryNode<Id: Copy + Eq + Ord> {
    Leaf(Id),
    Container {
        orientation: Orientation,
        children: Vec<NaryNode<Id>>,
        weights: Vec<u16>,
    },
}

impl<Id: Copy + Eq + Ord> NaryNode<Id> {
    pub fn leaf(id: Id) -> Self {
        Self::Leaf(id)
    }

    pub fn is_leaf(&self) -> bool {
        matches!(self, Self::Leaf(_))
    }

    pub fn unwrap_leaf(&self) -> Option<Id> {
        match self {
            Self::Leaf(id) => Some(*id),
            _ => None,
        }
    }

    pub fn all_leaf_ids(&self) -> Vec<Id> {
        let mut ids = Vec::new();
        self.collect_ids(&mut ids);
        ids
    }

    fn collect_ids(&self, ids: &mut Vec<Id>) {
        match self {
            Self::Leaf(id) => ids.push(*id),
            Self::Container { children, .. } => {
                for child in children {
                    child.collect_ids(ids);
                }
            }
        }
    }

    pub fn subtree_any(&self, predicate: &mut impl FnMut(Id) -> bool) -> bool {
        match self {
            Self::Leaf(id) => predicate(*id),
            Self::Container { children, .. } => children.iter().any(|c| c.subtree_any(predicate)),
        }
    }

    pub fn layout(&self, area: LayoutRect) -> Vec<(Id, LayoutRect)> {
        let mut regions = Vec::new();
        self.layout_recursive(area, &mut regions);
        regions
    }

    fn layout_recursive(&self, area: LayoutRect, regions: &mut Vec<(Id, LayoutRect)>) {
        match self {
            Self::Leaf(id) => {
                regions.push((*id, area));
            }
            Self::Container {
                orientation,
                children,
                weights,
            } => {
                let sub_rects =
                    split::split_rects_nary(area, *orientation, weights, children.len());
                for (child, sub) in children.iter().zip(sub_rects) {
                    child.layout_recursive(sub, regions);
                }
            }
        }
    }

    pub fn find_path(&self, target: Id) -> Option<Vec<usize>> {
        match self {
            Self::Leaf(id) => {
                if *id == target {
                    Some(Vec::new())
                } else {
                    None
                }
            }
            Self::Container { children, .. } => {
                for (i, child) in children.iter().enumerate() {
                    if let Some(mut path) = child.find_path(target) {
                        path.insert(0, i);
                        return Some(path);
                    }
                }
                None
            }
        }
    }

    pub fn node_at_path(&self, path: &[usize]) -> Option<&Self> {
        let mut node = self;
        for &idx in path {
            match node {
                Self::Container { children, .. } => {
                    node = children.get(idx)?;
                }
                _ => return None,
            }
        }
        Some(node)
    }

    pub fn node_at_path_mut(&mut self, path: &[usize]) -> Option<&mut Self> {
        let mut node = self;
        for &idx in path {
            match node {
                Self::Container { children, .. } => {
                    node = children.get_mut(idx)?;
                }
                _ => return None,
            }
        }
        Some(node)
    }

    pub fn layout_with_gaps(&self, area: LayoutRect, gap: u16) -> Vec<(Id, LayoutRect)> {
        let mut regions = Vec::new();
        self.layout_with_gaps_recursive(area, gap, &mut regions);
        regions
    }

    fn layout_with_gaps_recursive(
        &self,
        area: LayoutRect,
        gap: u16,
        regions: &mut Vec<(Id, LayoutRect)>,
    ) {
        match self {
            Self::Leaf(id) => {
                regions.push((*id, area));
            }
            Self::Container {
                orientation,
                children,
                weights,
            } => {
                let (sub_rects, _gap_rects) =
                    split::split_rects_with_gaps(area, *orientation, weights, children.len(), gap);
                for (child, sub) in children.iter().zip(sub_rects) {
                    child.layout_with_gaps_recursive(sub, gap, regions);
                }
            }
        }
    }

    pub fn split_area_for_path(&self, area: LayoutRect, path: &[usize]) -> Option<LayoutRect> {
        let mut current_area = area;
        let mut node = self;
        for &idx in path {
            match node {
                Self::Container {
                    orientation,
                    children,
                    weights,
                } => {
                    let rects = split::split_rects_nary(
                        current_area,
                        *orientation,
                        weights,
                        children.len(),
                    );
                    let sub_area = rects.get(idx).copied()?;
                    current_area = sub_area;
                    node = children.get(idx)?;
                }
                _ => return None,
            }
        }
        Some(current_area)
    }

    pub fn insert_leaf(
        &mut self,
        target: Id,
        insert: Id,
        position: InsertPosition,
        area: LayoutRect,
        constraints: &SizeConstraints,
    ) -> Result<(), LayoutError> {
        match self {
            Self::Leaf(current) => {
                if *current != target {
                    return Err(LayoutError::NotFound);
                }
                let (orientation, left_child, right_child) = match position {
                    InsertPosition::Left => (
                        Orientation::Horizontal,
                        Self::leaf(insert),
                        Self::leaf(*current),
                    ),
                    InsertPosition::Right => (
                        Orientation::Horizontal,
                        Self::leaf(*current),
                        Self::leaf(insert),
                    ),
                    InsertPosition::Top => (
                        Orientation::Vertical,
                        Self::leaf(insert),
                        Self::leaf(*current),
                    ),
                    InsertPosition::Bottom => (
                        Orientation::Vertical,
                        Self::leaf(*current),
                        Self::leaf(insert),
                    ),
                    // Corners: use vertical split (top portion for new window)
                    InsertPosition::TopLeft
                    | InsertPosition::TopRight => (
                        Orientation::Vertical,
                        Self::leaf(insert),
                        Self::leaf(*current),
                    ),
                    InsertPosition::BottomLeft
                    | InsertPosition::BottomRight => (
                        Orientation::Vertical,
                        Self::leaf(*current),
                        Self::leaf(insert),
                    ),
                };
                let half_dim = match orientation {
                    Orientation::Horizontal => area.width / 2,
                    Orientation::Vertical => area.height / 2,
                };
                if orientation == Orientation::Horizontal {
                    if half_dim < constraints.min_width {
                        return Err(LayoutError::ConstraintViolated(*constraints));
                    }
                    if area.height < constraints.min_height {
                        return Err(LayoutError::ConstraintViolated(*constraints));
                    }
                } else {
                    if half_dim < constraints.min_height {
                        return Err(LayoutError::ConstraintViolated(*constraints));
                    }
                    if area.width < constraints.min_width {
                        return Err(LayoutError::ConstraintViolated(*constraints));
                    }
                }
                *self = Self::Container {
                    orientation,
                    children: vec![left_child, right_child],
                    weights: vec![1, 1],
                };
                Ok(())
            }
            Self::Container {
                orientation: node_orient,
                children,
                weights,
            } => {
                let sub_rects =
                    split::split_rects_nary(area, *node_orient, weights, children.len());
                for (child, sub) in children.iter_mut().zip(sub_rects) {
                    if child
                        .insert_leaf(target, insert, position, sub, constraints)
                        .is_ok()
                    {
                        return Ok(());
                    }
                }
                Err(LayoutError::NotFound)
            }
        }
    }

    pub fn apply_drag(
        &mut self,
        area: LayoutRect,
        path: &[usize],
        index: usize,
        orientation: Orientation,
        delta: i16,
        constraints: &SizeConstraints,
    ) -> bool {
        match self {
            Self::Container {
                orientation: cont_orient,
                children,
                weights,
            } => {
                if path.is_empty() {
                    if *cont_orient != orientation {
                        return false;
                    }
                    if index >= weights.len().saturating_sub(1) || weights.is_empty() {
                        return false;
                    }
                    let total = match orientation {
                        Orientation::Horizontal => u32::from(area.width),
                        Orientation::Vertical => u32::from(area.height),
                    };
                    if total == 0 {
                        return false;
                    }
                    let total_weight: u32 =
                        weights.iter().map(|w| u32::from(*w)).sum::<u32>().max(1);
                    let current0 = u32::from(weights[index]);
                    let current1 = u32::from(weights[index + 1]);
                    let min_weight = 1u16;
                    let new0 = (current0 as i32)
                        .saturating_add(i32::from(delta))
                        .max(i32::from(min_weight));
                    let new1 = (current1 as i32)
                        .saturating_sub(i32::from(delta))
                        .max(i32::from(min_weight));
                    let sum_before = current0.saturating_add(current1);
                    let sum_after = (new0 as u32).saturating_add(new1 as u32);
                    if sum_before != sum_after {
                        let diff = sum_before.saturating_sub(sum_after);
                        weights[index] = new0 as u16;
                        weights[index + 1] = (new1 as u32).saturating_add(diff) as u16;
                    } else {
                        weights[index] = new0 as u16;
                        weights[index + 1] = new1 as u16;
                    }
                    let min_first = match orientation {
                        Orientation::Horizontal => u32::from(constraints.min_width),
                        Orientation::Vertical => u32::from(constraints.min_height),
                    };
                    let child_frac = u32::from(weights[index]) * total / total_weight;
                    if child_frac < min_first {
                        let correction = min_first.saturating_sub(child_frac) as u16;
                        weights[index] = weights[index].saturating_add(correction);
                        weights[index + 1] = weights[index + 1]
                            .saturating_sub(correction)
                            .max(min_weight);
                    }
                    return true;
                }
                let idx = path[0];
                if idx >= children.len() {
                    return false;
                }
                let sub_rects =
                    split::split_rects_nary(area, *cont_orient, weights, children.len());
                if let Some(sub) = sub_rects.get(idx) {
                    children[idx].apply_drag(
                        *sub,
                        &path[1..],
                        index,
                        orientation,
                        delta,
                        constraints,
                    )
                } else {
                    false
                }
            }
            Self::Leaf(_) => false,
        }
    }

    pub fn remove_leaf(&mut self, id: Id) -> Result<(), LayoutError> {
        match self {
            Self::Leaf(_) => Err(LayoutError::NotFound),
            Self::Container {
                children, weights, ..
            } => {
                let pos = children.iter().position(|c| c.unwrap_leaf() == Some(id));
                if let Some(idx) = pos {
                    children.remove(idx);
                    if idx < weights.len() {
                        weights.remove(idx);
                    }
                    if children.len() == 1 {
                        let only = children.remove(0);
                        *self = only;
                    }
                    return Ok(());
                }

                for child in children.iter_mut() {
                    if child.remove_leaf(id).is_ok() {
                        let empty_container =
                            matches!(child, Self::Container { children: c, .. } if c.is_empty());
                        if empty_container {
                            let idx = children.iter().position(|c| c.unwrap_leaf().is_none() && matches!(c, Self::Container { children: cc, .. } if cc.is_empty()));
                            if let Some(idx) = idx {
                                children.remove(idx);
                                if idx < weights.len() {
                                    weights.remove(idx);
                                }
                            }
                        }
                        if children.len() == 1 {
                            let only = children.remove(0);
                            *self = only;
                        }
                        return Ok(());
                    }
                }
                Err(LayoutError::NotFound)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InsertPosition;

    fn default_area() -> LayoutRect {
        LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        }
    }

    fn constraints() -> SizeConstraints {
        SizeConstraints {
            min_width: 2,
            min_height: 2,
        }
    }

    #[test]
    fn bsp_insert_and_remove() {
        let mut node: BspNode<usize> = BspNode::leaf(1);
        assert!(
            node.insert_leaf(1, 2, InsertPosition::Right, default_area(), &constraints())
                .is_ok()
        );
        assert_eq!(node.all_leaf_ids(), vec![1, 2]);
        assert!(node.remove_leaf(2).is_ok());
        assert_eq!(node.unwrap_leaf(), Some(1));
    }

    #[test]
    fn nary_insert_and_remove() {
        let mut node: NaryNode<usize> = NaryNode::leaf(1);
        assert!(
            node.insert_leaf(1, 2, InsertPosition::Right, default_area(), &constraints())
                .is_ok()
        );
        assert!(
            node.insert_leaf(1, 3, InsertPosition::Left, default_area(), &constraints())
                .is_ok()
        );
        assert!(node.remove_leaf(2).is_ok());
        assert!(node.remove_leaf(3).is_ok());
        assert_eq!(node.unwrap_leaf(), Some(1));
    }

    #[test]
    fn bsp_insert_nonexistent_target_returns_not_found() {
        let mut node: BspNode<usize> = BspNode::leaf(1);
        assert_eq!(
            node.insert_leaf(99, 2, InsertPosition::Right, default_area(), &constraints()),
            Err(LayoutError::NotFound)
        );
    }

    #[test]
    fn bsp_insert_constraints_too_small() {
        let mut node: BspNode<usize> = BspNode::leaf(1);
        let small_area = LayoutRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let tight = SizeConstraints {
            min_width: 10,
            min_height: 10,
        };
        assert_eq!(
            node.insert_leaf(1, 2, InsertPosition::Right, small_area, &tight),
            Err(LayoutError::ConstraintViolated(tight))
        );
    }

    #[test]
    fn bsp_layout_returns_all_ids() {
        let mut node: BspNode<&str> = BspNode::leaf("a");
        node.insert_leaf(
            "a",
            "b",
            InsertPosition::Right,
            default_area(),
            &constraints(),
        )
        .unwrap();
        node.insert_leaf(
            "b",
            "c",
            InsertPosition::Top,
            default_area(),
            &constraints(),
        )
        .unwrap();

        let area = default_area();
        let regions = node.layout(area);
        let ids: Vec<&&str> = regions.iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec![&"a", &"c", &"b"]);
    }

    #[test]
    fn bsp_find_path_returns_correct_route() {
        let mut node: BspNode<usize> = BspNode::leaf(1);
        node.insert_leaf(1, 2, InsertPosition::Right, default_area(), &constraints())
            .unwrap();

        assert_eq!(node.find_path(1), Some(vec![false]));
        assert_eq!(node.find_path(2), Some(vec![true]));
    }

    #[test]
    fn bsp_apply_drag_adjusts_ratio() {
        let mut node: BspNode<usize> = BspNode::leaf(1);
        node.insert_leaf(1, 2, InsertPosition::Right, default_area(), &constraints())
            .unwrap();
        let path = vec![false];
        let result = node.apply_drag(
            default_area(),
            &path,
            Orientation::Horizontal,
            10,
            &constraints(),
        );
        assert!(result);
        let regions = node.layout(default_area());
        let (left_id, left) = regions.iter().find(|(id, _)| *id == 1).unwrap();
        assert!(*left_id == 1);
        // delta=10 rightward → left shrinks from 40 to ~26
        assert!(left.width < 40);
    }

    #[test]
    fn bsp_apply_drag_wrong_orientation_returns_false() {
        let mut node: BspNode<usize> = BspNode::leaf(1);
        node.insert_leaf(1, 2, InsertPosition::Right, default_area(), &constraints())
            .unwrap();
        let result = node.apply_drag(
            default_area(),
            &[false],
            Orientation::Vertical,
            10,
            &constraints(),
        );
        assert!(!result);
    }

    #[test]
    fn nary_subtree_any() {
        let mut node: NaryNode<usize> = NaryNode::leaf(1);
        node.insert_leaf(1, 2, InsertPosition::Right, default_area(), &constraints())
            .unwrap();
        assert!(node.subtree_any(&mut |id| id == 2));
        assert!(!node.subtree_any(&mut |id| id == 99));
    }

    #[test]
    fn nary_find_path_and_node_at_path() {
        let mut node: NaryNode<usize> = NaryNode::leaf(1);
        node.insert_leaf(1, 2, InsertPosition::Right, default_area(), &constraints())
            .unwrap();
        node.insert_leaf(2, 3, InsertPosition::Bottom, default_area(), &constraints())
            .unwrap();

        let path = node.find_path(3);
        assert!(path.is_some());
        let p = path.unwrap();
        let found = node.node_at_path(&p);
        assert!(found.is_some());
        assert_eq!(found.unwrap().unwrap_leaf(), Some(3));
    }

    #[test]
    fn nary_insert_constraints_too_small() {
        let mut node: NaryNode<usize> = NaryNode::leaf(1);
        let small_area = LayoutRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let tight = SizeConstraints {
            min_width: 10,
            min_height: 10,
        };
        assert_eq!(
            node.insert_leaf(1, 2, InsertPosition::Right, small_area, &tight),
            Err(LayoutError::ConstraintViolated(tight))
        );
    }

    #[test]
    fn nary_apply_drag_adjusts_weights() {
        let mut node: NaryNode<usize> = NaryNode::leaf(1);
        node.insert_leaf(1, 2, InsertPosition::Right, default_area(), &constraints())
            .unwrap();
        let path: Vec<usize> = Vec::new();
        let result = node.apply_drag(
            default_area(),
            &path,
            0,
            Orientation::Horizontal,
            5,
            &constraints(),
        );
        assert!(result);
        if let NaryNode::Container { weights, .. } = &node {
            assert_eq!(weights[0], 6);
            assert_eq!(weights[1], 1);
        } else {
            panic!("Expected Container");
        }
    }

    #[test]
    fn nary_layout_with_gaps_includes_gap() {
        let mut node: NaryNode<usize> = NaryNode::leaf(1);
        node.insert_leaf(1, 2, InsertPosition::Right, default_area(), &constraints())
            .unwrap();
        let regions = node.layout_with_gaps(default_area(), 2);
        assert_eq!(regions.len(), 2);
        let total_w: u16 = regions.iter().map(|(_, r)| r.width).sum();
        assert!(total_w < 80);
    }

    #[test]
    fn nary_split_area_for_path() {
        let mut node: NaryNode<usize> = NaryNode::leaf(1);
        node.insert_leaf(1, 2, InsertPosition::Right, default_area(), &constraints())
            .unwrap();
        let sub = node.split_area_for_path(default_area(), &[0]);
        assert!(sub.is_some());
        assert_eq!(sub.unwrap().width, 40);
    }

    #[cfg(feature = "std")]
    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn bsp_layout_sum_equals_area(
                id1 in 0u8..10u8,
                id2 in 10u8..20u8,
                w in 4u16..200u16,
                h in 4u16..200u16,
            ) {
                prop_assume!(id1 != id2);
                let constraints = SizeConstraints { min_width: 2, min_height: 2 };
                let area = LayoutRect { x: 0, y: 0, width: w, height: h };
                let mut node: BspNode<u8> = BspNode::leaf(id1);
                let _ = node.insert_leaf(id1, id2, InsertPosition::Right, area, &constraints);
                let regions = node.layout(area);
                let mut sum_w = 0u16;
                for (_, r) in &regions {
                    sum_w = sum_w.saturating_add(r.width);
                    // In a horizontal split all children share the parent height.
                    prop_assert_eq!(r.height, h);
                }
                prop_assert_eq!(sum_w, w, "sum of child widths must equal parent width");
            }
        }
    }
}
