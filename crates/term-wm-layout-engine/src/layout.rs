use crate::node::BspNode;
use crate::rect::{LayoutRect, SizeConstraints, LayoutError};
use crate::snap::InsertPosition;

pub trait LayoutEngine<Id: Copy + Eq + Ord> {
    fn layout(&self, area: LayoutRect) -> Vec<(Id, LayoutRect)>;
    fn insert_leaf(
        &mut self,
        target: Id,
        insert: Id,
        position: InsertPosition,
        constraints: &SizeConstraints,
    ) -> Result<(), LayoutError>;
    fn remove_leaf(&mut self, id: Id) -> Result<(), LayoutError>;
    fn all_leaf_ids(&self) -> Vec<Id>;
    fn subtree_any(&self, predicate: &mut impl FnMut(Id) -> bool) -> bool;
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
        constraints: &SizeConstraints,
    ) -> Result<(), LayoutError> {
        self.insert_leaf(target, insert, position, constraints)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::BspNode;

    #[test]
    fn layout_engine_trait_works_with_bsp_node() {
        let mut node: BspNode<u8> = BspNode::leaf(1);
        let constraints = SizeConstraints { min_width: 2, min_height: 2 };

        assert!(LayoutEngine::insert_leaf(&mut node, 1, 2, InsertPosition::Right, &constraints).is_ok());

        let area = LayoutRect { x: 0, y: 0, width: 80, height: 24 };
        let regions = LayoutEngine::layout(&node, area);
        assert_eq!(regions.len(), 2);
    }
}
