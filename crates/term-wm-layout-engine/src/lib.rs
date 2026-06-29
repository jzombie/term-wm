mod layout;
mod node;
mod orientation;
mod rect;
mod snap;
mod split;
mod hit_test;

pub use layout::LayoutEngine;
pub use node::{BspNode, NaryNode};
pub use orientation::{LongestSide, OrientationHeuristic, Spiral};
pub use rect::{LayoutError, LayoutRect, Orientation, Quadrant, Ratio, SizeConstraints};
pub use snap::{EdgeResistance, InsertPosition, SnapPreview, SnapTarget, detect_edge_snap, edge_preview_rect, tiled_preview_rect};
pub use split::{split_rect_bsp, split_rects_nary};
pub use hit_test::{hit_test_leaf, detect_quadrant};
