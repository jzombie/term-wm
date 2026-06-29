use crate::rect::{LayoutRect, Orientation, Ratio, SizeConstraints, LayoutError};
use crate::snap::InsertPosition;
use crate::split;

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
            Self::Split { orientation, left, right, ratio } => {
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
        _constraints: &SizeConstraints,
    ) -> Result<(), LayoutError> {
        match self {
            Self::Leaf(current) => {
                if *current != target {
                    return Err(LayoutError::NotFound);
                }
                let (orientation, left_child, right_child) = match position {
                    InsertPosition::Left => {
                        (Orientation::Horizontal, Self::leaf(insert), Self::leaf(*current))
                    }
                    InsertPosition::Right => {
                        (Orientation::Horizontal, Self::leaf(*current), Self::leaf(insert))
                    }
                    InsertPosition::Top => {
                        (Orientation::Vertical, Self::leaf(insert), Self::leaf(*current))
                    }
                    InsertPosition::Bottom => {
                        (Orientation::Vertical, Self::leaf(*current), Self::leaf(insert))
                    }
                };
                *self = Self::Split {
                    orientation,
                    left: Box::new(left_child),
                    right: Box::new(right_child),
                    ratio: Ratio::half(),
                };
                Ok(())
            }
            Self::Split { left, right, .. } => {
                left.insert_leaf(target, insert, position, _constraints)
                    .or_else(|_| right.insert_leaf(target, insert, position, _constraints))
            }
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
                if *id == target { Some(Vec::new()) } else { None }
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

    pub fn subtree_any(&self, predicate: &mut impl FnMut(Id) -> bool) -> bool {
        match self {
            Self::Leaf(id) => predicate(*id),
            Self::Container { children, .. } => {
                children.iter().any(|c| c.subtree_any(predicate))
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
            Self::Container { orientation, children, weights } => {
                let sub_rects = split::split_rects_nary(area, *orientation, weights, children.len());
                for (child, sub) in children.iter().zip(sub_rects) {
                    child.layout_recursive(sub, regions);
                }
            }
        }
    }

    pub fn insert_leaf(
        &mut self,
        target: Id,
        insert: Id,
        position: InsertPosition,
        _constraints: &SizeConstraints,
    ) -> Result<(), LayoutError> {
        match self {
            Self::Leaf(current) => {
                if *current != target {
                    return Err(LayoutError::NotFound);
                }
                let (orientation, left_child, right_child) = match position {
                    InsertPosition::Left => {
                        (Orientation::Horizontal, Self::leaf(insert), Self::leaf(*current))
                    }
                    InsertPosition::Right => {
                        (Orientation::Horizontal, Self::leaf(*current), Self::leaf(insert))
                    }
                    InsertPosition::Top => {
                        (Orientation::Vertical, Self::leaf(insert), Self::leaf(*current))
                    }
                    InsertPosition::Bottom => {
                        (Orientation::Vertical, Self::leaf(*current), Self::leaf(insert))
                    }
                };
                *self = Self::Container {
                    orientation,
                    children: vec![left_child, right_child],
                    weights: vec![1, 1],
                };
                Ok(())
            }
            Self::Container { children, .. } => {
                for child in children.iter_mut() {
                    if child.insert_leaf(target, insert, position, _constraints).is_ok() {
                        return Ok(());
                    }
                }
                Err(LayoutError::NotFound)
            }
        }
    }

    pub fn remove_leaf(&mut self, id: Id) -> Result<(), LayoutError> {
        match self {
            Self::Leaf(_) => Err(LayoutError::NotFound),
            Self::Container { children, weights, .. } => {
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
                        let empty_container = matches!(child, Self::Container { children: c, .. } if c.is_empty());
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

    #[test]
    fn bsp_insert_and_remove() {
        let mut node: BspNode<usize> = BspNode::leaf(1);
        let constraints = SizeConstraints { min_width: 2, min_height: 2 };

        assert!(node.insert_leaf(1, 2, InsertPosition::Right, &constraints).is_ok());
        assert_eq!(node.all_leaf_ids(), vec![1, 2]);

        assert!(node.remove_leaf(2).is_ok());
        assert_eq!(node.unwrap_leaf(), Some(1));
    }

    #[test]
    fn nary_insert_and_remove() {
        let mut node: NaryNode<usize> = NaryNode::leaf(1);
        let constraints = SizeConstraints { min_width: 2, min_height: 2 };

        assert!(node.insert_leaf(1, 2, InsertPosition::Right, &constraints).is_ok());
        assert!(node.insert_leaf(1, 3, InsertPosition::Left, &constraints).is_ok());

        assert!(node.remove_leaf(2).is_ok());
        assert!(node.remove_leaf(3).is_ok());
        assert_eq!(node.unwrap_leaf(), Some(1));
    }

    #[test]
    fn bsp_insert_nonexistent_target_returns_not_found() {
        let mut node: BspNode<usize> = BspNode::leaf(1);
        let constraints = SizeConstraints { min_width: 2, min_height: 2 };
        assert_eq!(
            node.insert_leaf(99, 2, InsertPosition::Right, &constraints),
            Err(LayoutError::NotFound)
        );
    }

    #[test]
    fn bsp_layout_returns_all_ids() {
        let mut node: BspNode<&str> = BspNode::leaf("a");
        let constraints = SizeConstraints { min_width: 2, min_height: 2 };
        node.insert_leaf("a", "b", InsertPosition::Right, &constraints).unwrap();
        node.insert_leaf("b", "c", InsertPosition::Top, &constraints).unwrap();

        let area = LayoutRect { x: 0, y: 0, width: 80, height: 24 };
        let regions = node.layout(area);
        let ids: Vec<&&str> = regions.iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec![&"a", &"c", &"b"]);
    }

    #[test]
    fn bsp_find_path_returns_correct_route() {
        let mut node: BspNode<usize> = BspNode::leaf(1);
        let constraints = SizeConstraints { min_width: 2, min_height: 2 };
        node.insert_leaf(1, 2, InsertPosition::Right, &constraints).unwrap();

        assert_eq!(node.find_path(1), Some(vec![false]));
        assert_eq!(node.find_path(2), Some(vec![true]));
    }

    #[test]
    fn nary_subtree_any() {
        let mut node: NaryNode<usize> = NaryNode::leaf(1);
        let constraints = SizeConstraints { min_width: 2, min_height: 2 };
        node.insert_leaf(1, 2, InsertPosition::Right, &constraints).unwrap();

        assert!(node.subtree_any(&mut |id| id == 2));
        assert!(!node.subtree_any(&mut |id| id == 99));
    }
}
