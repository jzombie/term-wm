#![cfg_attr(not(feature = "std"), no_std)]

//! Pure-math window layout engine for term-wm.
//!
//! Provides tiling (BSP/N-ary tree) and floating window layout math with
//! zero runtime dependencies. Supports `no_std` via the `std` feature
//! (enabled by default).

#[cfg(not(feature = "std"))]
extern crate alloc;

mod floating;
mod hit_test;
mod layout;
mod node;
mod ordering;
mod orientation;
mod rect;
mod region_map;
mod scroll;
mod snap;
mod split;

pub use floating::{
    DragHandle, FLOATING_MIN_HEIGHT, FLOATING_MIN_WIDTH, HeaderDrag, ResizeDrag, ResizeEdge,
    ResizeHandle, apply_resize_drag_signed, floating_header_for_region, resize_handles_for_region,
};
pub use hit_test::{detect_tiled_quadrant, hit_test_leaf};
pub use layout::LayoutEngine;
pub use node::{BspNode, NaryNode};
pub use ordering::{FocusRing, ZOrder};
pub use orientation::{LongestSide, OrientationHeuristic, Spiral};
pub use rect::{
    LayoutError, LayoutRect, Orientation, Ratio, RectSpec, SizeConstraints, gap_insert,
    inset, rect_contains,
};
pub use region_map::RegionMap;
pub use scroll::ScrollState;
pub use snap::{
    EdgeResistance, InsertPosition, SnapPreview, SnapTarget, corner_preview_rect,
    detect_corner_snap, detect_edge_snap, edge_preview_rect, tiled_preview_rect,
};
pub use split::{
    build_rects_from_sizes, gap_size, handle_thickness, split_rect_bsp, split_rects_nary,
    split_rects_weighted, split_rects_with_gaps, split_sizes,
};
