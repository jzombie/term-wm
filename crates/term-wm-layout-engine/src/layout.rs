use crate::node::BspNode;
use crate::node::NaryNode;
use crate::rect::{LayoutError, LayoutRect, Orientation, SizeConstraints};
use crate::snap::InsertPosition;

/// Common interface for tree-based layout engines (BSP and N-ary).
///
/// All mutation methods accept `SizeConstraints` to enforce minimum
/// dimensions and return `Result<(), LayoutError>` on violation.
pub trait LayoutEngine<Id: Copy + Eq + Ord> {
    fn layout(&self, area: LayoutRect) -> Vec<(Id, LayoutRect)>;
    fn insert_leaf(
        &mut self,
        target: Id,
        insert: Id,
        position: InsertPosition,
        area: LayoutRect,
        constraints: &SizeConstraints,
    ) -> Result<(), LayoutError>;
    fn remove_leaf(&mut self, id: Id) -> Result<(), LayoutError>;
    fn all_leaf_ids(&self) -> Vec<Id>;
    fn subtree_any(&self, predicate: &mut impl FnMut(Id) -> bool) -> bool;
    fn apply_drag(
        &mut self,
        area: LayoutRect,
        path: &[usize],
        index: usize,
        orientation: Orientation,
        delta: i16,
        constraints: &SizeConstraints,
    ) -> bool;
}

impl<Id: Copy + Eq + Ord> LayoutEngine<Id> for BspNode<Id> {
    fn layout(&self, area: LayoutRect) -> Vec<(Id, LayoutRect)> {
        self.layout(area)
    }

    fn insert_leaf(
        &mut self,
        target: Id,
        insert: Id,
        position: InsertPosition,
        area: LayoutRect,
        constraints: &SizeConstraints,
    ) -> Result<(), LayoutError> {
        self.insert_leaf(target, insert, position, area, constraints)
    }

    fn remove_leaf(&mut self, id: Id) -> Result<(), LayoutError> {
        self.remove_leaf(id)
    }

    fn all_leaf_ids(&self) -> Vec<Id> {
        self.all_leaf_ids()
    }

    fn subtree_any(&self, predicate: &mut impl FnMut(Id) -> bool) -> bool {
        self.subtree_any(predicate)
    }

    fn apply_drag(
        &mut self,
        area: LayoutRect,
        path: &[usize],
        _index: usize,
        orientation: Orientation,
        delta: i16,
        constraints: &SizeConstraints,
    ) -> bool {
        let bool_path: Vec<bool> = path.iter().map(|&i| i != 0).collect();
        self.apply_drag(area, &bool_path, orientation, delta, constraints)
    }
}

impl<Id: Copy + Eq + Ord> LayoutEngine<Id> for NaryNode<Id> {
    fn layout(&self, area: LayoutRect) -> Vec<(Id, LayoutRect)> {
        self.layout(area)
    }

    fn insert_leaf(
        &mut self,
        target: Id,
        insert: Id,
        position: InsertPosition,
        area: LayoutRect,
        constraints: &SizeConstraints,
    ) -> Result<(), LayoutError> {
        self.insert_leaf(target, insert, position, area, constraints)
    }

    fn remove_leaf(&mut self, id: Id) -> Result<(), LayoutError> {
        self.remove_leaf(id)
    }

    fn all_leaf_ids(&self) -> Vec<Id> {
        self.all_leaf_ids()
    }

    fn subtree_any(&self, predicate: &mut impl FnMut(Id) -> bool) -> bool {
        self.subtree_any(predicate)
    }

    fn apply_drag(
        &mut self,
        area: LayoutRect,
        path: &[usize],
        index: usize,
        orientation: Orientation,
        delta: i16,
        constraints: &SizeConstraints,
    ) -> bool {
        self.apply_drag(area, path, index, orientation, delta, constraints)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::BspNode;
    use crate::node::NaryNode;

    #[test]
    fn layout_engine_trait_works_with_bsp_node() {
        let mut node: BspNode<u8> = BspNode::leaf(1);
        let constraints = SizeConstraints {
            min_width: 2,
            min_height: 2,
        };
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };

        assert!(
            LayoutEngine::insert_leaf(&mut node, 1, 2, InsertPosition::Right, area, &constraints)
                .is_ok()
        );

        let regions = LayoutEngine::layout(&node, area);
        assert_eq!(regions.len(), 2);
    }

    #[test]
    fn layout_engine_trait_works_with_nary_node() {
        let mut node: NaryNode<u8> = NaryNode::leaf(1);
        let constraints = SizeConstraints {
            min_width: 2,
            min_height: 2,
        };
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };

        assert!(
            LayoutEngine::insert_leaf(&mut node, 1, 2, InsertPosition::Right, area, &constraints)
                .is_ok()
        );

        let regions = LayoutEngine::layout(&node, area);
        assert_eq!(regions.len(), 2);
    }
}
