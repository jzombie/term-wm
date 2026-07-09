use std::sync::Arc;
use term_wm::AppContext;
use term_wm::layout::Direction;
use term_wm::layout::tiling::{InsertPosition, LayoutNode, TilingLayout};
use term_wm::window::{FloatRectSpec, WindowKey, WindowManager};
use term_wm::wm_config::WmConfig;
use term_wm_layout_engine::{LayoutRect, detect_corner_snap, detect_edge_snap, edge_preview_rect};

type Rect = term_wm_core::Rect;

const AREA: Rect = Rect {
    x: 0,
    y: 0,
    width: 80,
    height: 24,
};

const SNAP_SENSITIVITY: u16 = 3;

fn area_lr() -> LayoutRect {
    LayoutRect {
        x: AREA.x,
        y: AREA.y,
        width: AREA.width,
        height: AREA.height,
    }
}

fn wm_with_two_windows() -> (WindowManager, [WindowKey; 2]) {
    let mut config = WmConfig::standalone();
    config.chrome_enabled = false;
    let mut wm = WindowManager::with_config(
        config,
        Arc::new(AppContext::new("test", "0.0.0")),
        None,
        None,
        None,
        None,
    );
    wm.set_panel_visible(false);
    let k0 = wm.create_window(Box::new(NoopComponent));
    let k1 = wm.create_window(Box::new(NoopComponent));
    let split = LayoutNode::Split {
        direction: Direction::Horizontal,
        children: vec![LayoutNode::Leaf(k0), LayoutNode::Leaf(k1)],
        weights: vec![1.0, 1.0],
        constraints: vec![],
        resizable: false,
    };
    wm.set_managed_layout(TilingLayout::new(split));
    wm.register_managed_layout(AREA);
    (wm, [k0, k1])
}

fn header_rect(wm: &WindowManager, key: WindowKey) -> Rect {
    for h in wm.floating_headers() {
        if h.key == key {
            return h.rect;
        }
    }
    panic!("no header found for key");
}

fn make_mouse(
    kind: term_wm::events::MouseEventKind,
    col: u16,
    row: u16,
) -> term_wm::events::WmEvent {
    let event = term_wm::events::Event::Mouse(term_wm::events::MouseEvent {
        kind,
        column: col,
        row,
        modifiers: term_wm::events::KeyModifiers::NONE,
    });
    term_wm::events::core_event_to_wm(&event).expect("valid mouse event")
}

struct NoopComponent;
impl term_wm::components::Component<term_wm::actions::TermWmAction> for NoopComponent {
    fn render(
        &mut self,
        _backend: &mut dyn term_wm_render::RenderBackend,
        _area: LayoutRect,
        _ctx: &term_wm::components::ComponentContext,
        _registry: &mut term_wm::hitbox_registry::HitboxRegistry,
    ) {
    }
    fn handle_events(
        &mut self,
        _event: &term_wm::events::Event,
        _ctx: &term_wm::components::ComponentContext,
    ) -> term_wm::actions::EventResult<term_wm::actions::TermWmAction> {
        term_wm::actions::EventResult::Ignored
    }
    fn update(
        &mut self,
        _action: term_wm::actions::TermWmAction,
        _ctx: &term_wm::components::ComponentContext,
        _queue: &mut std::collections::VecDeque<(WindowKey, term_wm::actions::TermWmAction)>,
    ) {
    }
    fn destroy(&mut self) {}
}

fn collect_leaf_ids(node: &LayoutNode<usize>) -> Vec<usize> {
    node.collect_leaves()
}

fn has_void(node: &LayoutNode<usize>) -> bool {
    match node {
        LayoutNode::Void(_) => true,
        LayoutNode::Leaf(_) => false,
        LayoutNode::Split { children, .. } => children.iter().any(has_void),
    }
}

fn rects_overlap(a: Rect, b: Rect) -> bool {
    if a.width == 0 || a.height == 0 || b.width == 0 || b.height == 0 {
        return false;
    }
    let a_right = a.x + i32::from(a.width);
    let a_bottom = a.y + i32::from(a.height);
    let b_right = b.x + i32::from(b.width);
    let b_bottom = b.y + i32::from(b.height);
    a.x < b_right && a_right > b.x && a.y < b_bottom && a_bottom > b.y
}

// ─── Module 1: Snap Detection ────────────────────────────────────────

#[cfg(test)]
mod snap_detection {
    use super::*;

    #[test]
    fn top_edge_y0_is_sacred() {
        let pos = detect_edge_snap(40, 0, area_lr(), SNAP_SENSITIVITY);
        assert_eq!(
            pos,
            Some(InsertPosition::Top),
            "detection returns Top; the WM treats y=0 as maximize"
        );
    }

    #[test]
    fn left_edge_bisects() {
        let pos = detect_edge_snap(1, 12, area_lr(), SNAP_SENSITIVITY);
        assert_eq!(pos, Some(InsertPosition::Left));
        let preview = edge_preview_rect(area_lr(), InsertPosition::Left);
        assert_eq!(preview.width, AREA.width / 2);
        assert_eq!(preview.x, AREA.x);
    }

    #[test]
    fn right_edge_bisects() {
        let pos = detect_edge_snap(AREA.width - 1, 12, area_lr(), SNAP_SENSITIVITY);
        assert_eq!(pos, Some(InsertPosition::Right));
        let preview = edge_preview_rect(area_lr(), InsertPosition::Right);
        assert_eq!(preview.width, AREA.width / 2);
        assert_eq!(preview.x, AREA.x + i32::from(AREA.width / 2));
    }

    #[test]
    fn corner_snap_quadrants() {
        let a = area_lr();
        let cases = [
            (0u16, 0u16, InsertPosition::TopLeft),
            (AREA.width - 1, 0, InsertPosition::TopRight),
            (0, AREA.height - 1, InsertPosition::BottomLeft),
            (AREA.width - 1, AREA.height - 1, InsertPosition::BottomRight),
        ];
        for (col, row, expected) in cases {
            let pos = detect_corner_snap(col, row, a, SNAP_SENSITIVITY);
            assert_eq!(pos, Some(expected), "corner at ({col}, {row})");
        }
        let tl = edge_preview_rect(a, InsertPosition::TopLeft);
        assert_eq!(tl.width, AREA.width / 2);
        assert_eq!(tl.height, AREA.height / 2);
    }

    #[test]
    fn corner_over_edge_priority() {
        let pos = detect_corner_snap(1, 1, area_lr(), SNAP_SENSITIVITY);
        assert_eq!(pos, Some(InsertPosition::TopLeft));
    }
}

// ─── Module 2: Multi-Window Tiling ───────────────────────────────────

#[cfg(test)]
mod multi_window_tiling {
    use super::*;

    #[test]
    fn three_windows_horizontal() {
        let root = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![
                LayoutNode::leaf(1usize),
                LayoutNode::leaf(2),
                LayoutNode::leaf(3),
            ],
            weights: vec![1.0, 1.0, 1.0],
            constraints: vec![],
            resizable: false,
        };
        let regions = root.layout(AREA);
        assert_eq!(regions.len(), 3);
        let total_w: u16 = regions.iter().map(|(_, r)| r.width).sum();
        assert_eq!(total_w, AREA.width);
        for (i, (_, r1)) in regions.iter().enumerate() {
            for (j, (_, r2)) in regions.iter().enumerate() {
                if i != j {
                    assert!(!rects_overlap(*r1, *r2));
                }
            }
        }
    }

    #[test]
    fn nested_mixed_orientation() {
        let root = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![
                LayoutNode::leaf(1usize),
                LayoutNode::Split {
                    direction: Direction::Vertical,
                    children: vec![LayoutNode::leaf(2), LayoutNode::leaf(3)],
                    weights: vec![1.0, 1.0],
                    constraints: vec![],
                    resizable: false,
                },
            ],
            weights: vec![1.0, 1.0],
            constraints: vec![],
            resizable: false,
        };
        let regions = root.layout(AREA);
        assert_eq!(regions.len(), 3);
        let total_area: u32 = regions
            .iter()
            .map(|(_, r)| r.width as u32 * r.height as u32)
            .sum();
        assert_eq!(total_area, AREA.width as u32 * AREA.height as u32);
    }

    #[test]
    fn insert_leaf_splits_target() {
        let mut root = LayoutNode::leaf(1usize);
        root.insert_leaf(1, 2, InsertPosition::Right);
        let regions = root.layout(AREA);
        assert_eq!(regions.len(), 2);
        let r1 = regions.iter().find(|(id, _)| *id == 1).unwrap().1;
        let r2 = regions.iter().find(|(id, _)| *id == 2).unwrap().1;
        assert!(r1.x <= r2.x, "window 1 should be left of window 2");
    }

    #[test]
    fn remove_leaf_collapses() {
        let mut root = LayoutNode::leaf(1usize);
        root.insert_leaf(1, 2, InsertPosition::Right);
        root.remove_leaf(2);
        root.cleanup_after_removal();
        let regions = root.layout(AREA);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].1.width, AREA.width);
        assert_eq!(regions[0].1.height, AREA.height);
    }

    #[test]
    fn remove_all_collapses_to_single_leaf() {
        let mut root = LayoutNode::leaf(1usize);
        root.insert_leaf(1, 2, InsertPosition::Right);
        root.remove_leaf(1);
        root.cleanup_after_removal();
        let regions = root.layout(AREA);
        assert_eq!(regions.len(), 1, "after removing 1 of 2, one leaf remains");
        root.remove_leaf(2);
        root.cleanup_after_removal();
        assert!(
            matches!(root, LayoutNode::Leaf(2)),
            "removing last leaf from collapsed tree"
        );
    }

    #[test]
    fn normalize_weights_equalizes() {
        let mut root = LayoutNode::leaf(1usize);
        root.insert_leaf(1, 2, InsertPosition::Right);
        root.normalize_weights();
        let regions = root.layout(AREA);
        assert_eq!(regions.len(), 2);
        let r1 = regions.iter().find(|(id, _)| *id == 1).unwrap().1;
        let r2 = regions.iter().find(|(id, _)| *id == 2).unwrap().1;
        let diff = (r1.width as i32 - r2.width as i32).abs();
        assert!(
            diff <= 1,
            "widths should be within 1px: {} vs {}",
            r1.width,
            r2.width
        );
    }

    /// Verify that corner insert puts the dragged window in the correct
    /// quadrant and the first sibling in the adjacent quadrant.
    /// Regression guard against insert/first ordering swaps.
    #[test]
    fn corner_insert_window_ordering() {
        // Start with 3 windows side by side: [1, 2, 3]
        let mut root = LayoutNode::leaf(1usize);
        root.insert_leaf(1, 2, InsertPosition::Right);
        root.insert_leaf(2, 3, InsertPosition::Right);
        let area = Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 80,
        };
        let mid_y: i32 = (area.height / 2).into();

        // Insert 4 into BottomLeft quadrant
        // Expected: 4 in bottom-left, 1 in bottom-right, others in top strip
        root.insert_leaf(1, 4, InsertPosition::BottomLeft);
        let regions = root.layout(area);
        let r4 = regions.iter().find(|(id, _)| *id == 4).unwrap().1;
        let r1 = regions.iter().find(|(id, _)| *id == 1).unwrap().1;
        assert!(
            r4.x < r1.x,
            "BottomLeft: insert must be left of first sibling"
        );
        assert!(r4.y >= mid_y, "BottomLeft: insert must be in bottom half");

        // Reset and test BottomRight
        let mut root = LayoutNode::leaf(1usize);
        root.insert_leaf(1, 2, InsertPosition::Right);
        root.insert_leaf(2, 3, InsertPosition::Right);
        root.insert_leaf(1, 4, InsertPosition::BottomRight);
        let regions = root.layout(area);
        let r4 = regions.iter().find(|(id, _)| *id == 4).unwrap().1;
        let r1 = regions.iter().find(|(id, _)| *id == 1).unwrap().1;
        assert!(
            r4.x > r1.x,
            "BottomRight: insert must be right of first sibling"
        );
        assert!(r4.y >= mid_y, "BottomRight: insert must be in bottom half");

        // Reset and test TopLeft
        let mut root = LayoutNode::leaf(1usize);
        root.insert_leaf(1, 2, InsertPosition::Right);
        root.insert_leaf(2, 3, InsertPosition::Right);
        root.insert_leaf(1, 4, InsertPosition::TopLeft);
        let regions = root.layout(area);
        let r4 = regions.iter().find(|(id, _)| *id == 4).unwrap().1;
        let r1 = regions.iter().find(|(id, _)| *id == 1).unwrap().1;
        assert!(r4.x < r1.x, "TopLeft: insert must be left of first sibling");
        assert!(r4.y < mid_y, "TopLeft: insert must be in top half");

        // Reset and test TopRight
        let mut root = LayoutNode::leaf(1usize);
        root.insert_leaf(1, 2, InsertPosition::Right);
        root.insert_leaf(2, 3, InsertPosition::Right);
        root.insert_leaf(1, 4, InsertPosition::TopRight);
        let regions = root.layout(area);
        let r4 = regions.iter().find(|(id, _)| *id == 4).unwrap().1;
        let r1 = regions.iter().find(|(id, _)| *id == 1).unwrap().1;
        assert!(
            r4.x > r1.x,
            "TopRight: insert must be right of first sibling"
        );
        assert!(r4.y < mid_y, "TopRight: insert must be in top half");
    }
}

// ─── Module 3: Void Node Lifecycle ───────────────────────────────────

#[cfg(test)]
mod void_node_lifecycle {
    use super::*;

    #[test]
    fn corner_insert_creates_void() {
        let mut root = LayoutNode::leaf(1usize);
        root.insert_leaf(1, 2, InsertPosition::TopLeft);
        assert!(has_void(&root), "corner insert must create a Void node");
    }

    #[test]
    fn replace_void_by_id() {
        let mut root = LayoutNode::leaf(1usize);
        root.insert_leaf(1, 2, InsertPosition::TopLeft);
        let void_id = match &root {
            LayoutNode::Split { children, .. } => match &children[0] {
                LayoutNode::Split {
                    children: inner, ..
                } => match &inner[1] {
                    LayoutNode::Void(id) => *id,
                    _ => panic!("expected Void in inner split"),
                },
                _ => panic!("expected inner Split"),
            },
            _ => panic!("expected outer Split"),
        };
        let replaced = root.replace_void_by_id(void_id, LayoutNode::leaf(99));
        assert!(replaced);
        assert!(!has_void(&root), "Void should be replaced");
        let ids = collect_leaf_ids(&root);
        assert!(ids.contains(&99));
    }

    #[test]
    fn cleanup_removes_voids() {
        let mut root = LayoutNode::leaf(1usize);
        root.insert_leaf(1, 2, InsertPosition::TopLeft);
        root.cleanup_after_removal();
        assert!(!has_void(&root), "cleanup should remove all Void nodes");
    }

    #[test]
    fn void_skipped_in_layout() {
        let mut root = LayoutNode::leaf(1usize);
        root.insert_leaf(1, 2, InsertPosition::TopLeft);
        root.cleanup_after_removal();
        let regions = root.layout(AREA);
        for (_, r) in &regions {
            assert!(r.width > 0);
            assert!(r.height > 0);
        }
        assert_eq!(regions.len(), 2);
    }
}

// ─── Module 4: Spatial Isolation ─────────────────────────────────────

#[cfg(test)]
mod spatial_isolation {
    use super::*;

    #[test]
    fn snap_right_preserves_left_sibling() {
        let mut config = WmConfig::standalone();
        config.chrome_enabled = false;
        let mut wm = WindowManager::with_config(
            config,
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        wm.set_panel_visible(false);
        let k0 = wm.create_window(Box::new(NoopComponent));
        let k1 = wm.create_window(Box::new(NoopComponent));
        let split = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::Leaf(k0), LayoutNode::Leaf(k1)],
            weights: vec![1.0, 1.0],
            constraints: vec![],
            resizable: false,
        };
        wm.set_managed_layout(TilingLayout::new(split));
        wm.register_managed_layout(AREA);

        let rect_before_left = wm.region(k0);

        let new_root = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![
                LayoutNode::Leaf(k0),
                LayoutNode::Split {
                    direction: Direction::Vertical,
                    children: vec![LayoutNode::Leaf(k1), LayoutNode::Void(0)],
                    weights: vec![1.0, 1.0],
                    constraints: vec![],
                    resizable: false,
                },
            ],
            weights: vec![1.0, 1.0],
            constraints: vec![],
            resizable: false,
        };
        wm.set_managed_layout(TilingLayout::new(new_root));
        wm.register_managed_layout(AREA);

        let rect_after_left = wm.region(k0);
        assert_eq!(
            rect_after_left.x, rect_before_left.x,
            "left sibling x must not change"
        );
        assert_eq!(
            rect_after_left.y, rect_before_left.y,
            "left sibling y must not change"
        );
        assert_eq!(
            rect_after_left.width, rect_before_left.width,
            "left sibling width must not change"
        );
        assert_eq!(
            rect_after_left.height, rect_before_left.height,
            "left sibling height must not change"
        );
    }

    #[test]
    fn snap_quadrant_preserves_sibling_orientation() {
        let mut root = LayoutNode::leaf(1usize);
        root.insert_leaf(1, 2, InsertPosition::Right);
        root.insert_leaf(2, 3, InsertPosition::Bottom);
        let root_dir = match &root {
            LayoutNode::Split { direction, .. } => *direction,
            _ => panic!("expected root Split"),
        };
        assert_eq!(
            root_dir,
            Direction::Horizontal,
            "root must remain Horizontal"
        );

        root.insert_leaf(3, 4, InsertPosition::BottomRight);
        let root_dir_after = match &root {
            LayoutNode::Split { direction, .. } => *direction,
            _ => panic!("expected root Split"),
        };
        assert_eq!(
            root_dir_after,
            Direction::Horizontal,
            "root direction must not change after quadrant insert"
        );
    }

    #[test]
    fn insert_does_not_mutate_unrelated_splits() {
        let mut root = LayoutNode::leaf(1usize);
        root.insert_leaf(1, 2, InsertPosition::Right);
        root.insert_leaf(2, 3, InsertPosition::Bottom);

        let regions_before = root.layout(AREA);
        let r3_before = *regions_before.iter().find(|(id, _)| *id == 3).unwrap();

        root.insert_leaf(2, 4, InsertPosition::Right);

        let regions_after = root.layout(AREA);
        let r3_after = *regions_after.iter().find(|(id, _)| *id == 3).unwrap();
        assert_eq!(
            r3_before.1.x, r3_after.1.x,
            "unrelated sibling x must not change"
        );
        assert_eq!(
            r3_before.1.y, r3_after.1.y,
            "unrelated sibling y must not change"
        );
        assert_eq!(
            r3_before.1.width, r3_after.1.width,
            "unrelated sibling width must not change"
        );
        assert_eq!(
            r3_before.1.height, r3_after.1.height,
            "unrelated sibling height must not change"
        );
    }
}

// ─── Module 5: Drag-Snap Pipeline ────────────────────────────────────
//
// These tests verify the full interaction flow through `dispatch_mouse`
// using the production render pipeline to populate HitboxRegistry.
// Snap detection geometry is tested in Module 1 (pure math).
// These tests verify that the state machine correctly handles
// Press→Drag→Release sequences without panics or state corruption.

#[cfg(test)]
mod drag_snap_pipeline {
    use super::*;

    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect as RatatuiRect;
    use term_wm::events::{MouseButton, MouseEventKind};
    use term_wm::render_app;
    use term_wm_console::RatatuiBackend;
    use term_wm_console::draw_plan_renderer::DrawPlanRenderer;
    use term_wm_core::engine::CoreEngine;

    fn setup() -> (WindowManager, CoreEngine, DrawPlanRenderer, [WindowKey; 2]) {
        let (wm, keys) = wm_with_two_windows();
        (wm, CoreEngine::new(), DrawPlanRenderer::new(), keys)
    }

    fn advance_frame(
        wm: &mut WindowManager,
        engine: &mut CoreEngine,
        renderer: &mut DrawPlanRenderer,
    ) {
        let area = RatatuiRect {
            x: 0,
            y: 0,
            width: AREA.width,
            height: AREA.height,
        };
        let buf = Buffer::empty(area);
        let mut backend = RatatuiBackend::new(buf, area);
        render_app(&mut backend, wm, engine, renderer);
    }

    #[test]
    fn drag_to_right_edge_snaps() {
        let (mut wm, mut engine, mut renderer, keys) = setup();
        advance_frame(&mut wm, &mut engine, &mut renderer);
        let header = header_rect(&wm, keys[0]);
        let down = make_mouse(
            MouseEventKind::Press(MouseButton::Left),
            header.x as u16,
            header.y as u16,
        );
        wm.dispatch_mouse(&down);

        // Use y=12 (middle of screen) to avoid corner detection zone (y <= 6)
        let mid_y = 12u16;
        let right_edge = (AREA.x + i32::from(AREA.width) - 1) as u16;
        let drag = make_mouse(MouseEventKind::Drag(MouseButton::Left), right_edge, mid_y);
        wm.dispatch_mouse(&drag);

        // Verify exact ghost geometry: right half (40, 0, 40, 24)
        let snap_rect = wm
            .drag_snap_rect()
            .expect("drag_snap must be set after Drag");
        assert_eq!(snap_rect.x, AREA.x + i32::from(AREA.width / 2), "ghost x");
        assert_eq!(snap_rect.y, AREA.y, "ghost y");
        assert_eq!(snap_rect.width, AREA.width / 2, "ghost width");
        assert_eq!(snap_rect.height, AREA.height, "ghost height");

        let up = make_mouse(
            MouseEventKind::Release(MouseButton::Left),
            right_edge,
            mid_y,
        );
        wm.dispatch_mouse(&up);

        // Re-render to recompute regions after apply_snap modified the layout tree
        advance_frame(&mut wm, &mut engine, &mut renderer);

        let r = wm.region(keys[0]);
        assert_eq!(
            r.x,
            AREA.x + i32::from(AREA.width / 2),
            "right-snapped window x"
        );
        assert_eq!(r.y, AREA.y, "right-snapped window y");
        assert_eq!(r.width, AREA.width / 2, "right-snapped window width");
        assert_eq!(r.height, AREA.height, "right-snapped window height");
    }

    #[test]
    fn press_preserves_tiled_state() {
        let (mut wm, mut engine, mut renderer, keys) = setup();
        advance_frame(&mut wm, &mut engine, &mut renderer);
        assert!(!wm.is_window_floating(keys[0]), "starts tiled");

        let header = header_rect(&wm, keys[0]);
        let down = make_mouse(
            MouseEventKind::Press(MouseButton::Left),
            header.x as u16,
            header.y as u16,
        );
        wm.dispatch_mouse(&down);
        advance_frame(&mut wm, &mut engine, &mut renderer);

        // Press alone must NOT decouple — deadzone drag requirement
        assert!(
            !wm.is_window_floating(keys[0]),
            "press must not make window floating"
        );

        // Drag that breaches the kinetic deadzone (dx+dy > 2 cells)
        let drag_x = (header.x + 5) as u16;
        let drag = make_mouse(
            MouseEventKind::Drag(MouseButton::Left),
            drag_x,
            header.y as u16,
        );
        wm.dispatch_mouse(&drag);
        advance_frame(&mut wm, &mut engine, &mut renderer);

        assert!(
            wm.is_window_floating(keys[0]),
            "must be floating after drag breaches deadzone"
        );
    }

    #[test]
    fn drag_to_top_maximizes() {
        let (mut wm, mut engine, mut renderer, keys) = setup();
        advance_frame(&mut wm, &mut engine, &mut renderer);
        let header = header_rect(&wm, keys[0]);
        let down = make_mouse(
            MouseEventKind::Press(MouseButton::Left),
            header.x as u16,
            header.y as u16,
        );
        wm.dispatch_mouse(&down);

        // Use center column to avoid corner-snap collision at left edge
        let mid_x = AREA.width / 2;
        let drag = make_mouse(MouseEventKind::Drag(MouseButton::Left), mid_x, 0);
        wm.dispatch_mouse(&drag);

        // Verify ghost preview is maximize (full area)
        let snap_rect = wm
            .drag_snap_rect()
            .expect("drag_snap must be set after Drag to top edge");
        assert_eq!(snap_rect.x, AREA.x, "maximize ghost x");
        assert_eq!(snap_rect.y, AREA.y, "maximize ghost y");
        assert_eq!(snap_rect.width, AREA.width, "maximize ghost width");
        assert_eq!(snap_rect.height, AREA.height, "maximize ghost height");

        let up = make_mouse(MouseEventKind::Release(MouseButton::Left), mid_x, 0);
        wm.dispatch_mouse(&up);

        // Ghost overlay must be cleared immediately after maximize applies
        assert!(
            wm.drag_snap_rect().is_none(),
            "drag_snap must be None after maximize release"
        );

        advance_frame(&mut wm, &mut engine, &mut renderer);

        let r = wm.region(keys[0]);
        assert_eq!(r.x, AREA.x, "maximized x");
        assert_eq!(r.y, AREA.y, "maximized y");
        assert_eq!(r.width, AREA.width, "maximized width");
        assert_eq!(r.height, AREA.height, "maximized height");
    }

    #[test]
    fn double_click_header_toggles_maximize() {
        let (mut wm, mut engine, mut renderer, keys) = setup();
        advance_frame(&mut wm, &mut engine, &mut renderer);
        assert!(!wm.is_window_floating(keys[0]), "starts tiled");

        let header = header_rect(&wm, keys[0]);
        let col = header.x as u16;
        let row = header.y as u16;

        // First click: Press + Release at same position (no drag)
        let down = make_mouse(MouseEventKind::Press(MouseButton::Left), col, row);
        wm.dispatch_mouse(&down);
        let up = make_mouse(MouseEventKind::Release(MouseButton::Left), col, row);
        wm.dispatch_mouse(&up);

        // Second click within the 500ms double-click window
        let down2 = make_mouse(MouseEventKind::Press(MouseButton::Left), col, row);
        wm.dispatch_mouse(&down2);

        // Double-click must trigger toggle_maximize on a tiled window.
        // Maximized windows have floating_rect set but is_maximized flag true,
        // so is_window_floating returns false — check via floating_panes + is_maximized.
        let panes = wm.floating_panes();
        let (_, spec) = panes
            .iter()
            .find(|(k, _)| *k == keys[0])
            .expect("window in floating panes");
        if let FloatRectSpec::Absolute(rect) = spec {
            assert_eq!(rect.width, AREA.width, "maximized width");
            assert_eq!(rect.height, AREA.height, "maximized height");
        } else {
            panic!("expected absolute float rect");
        }
    }

    #[test]
    fn drag_to_corner_quadrant() {
        let (mut wm, mut engine, mut renderer, keys) = setup();
        advance_frame(&mut wm, &mut engine, &mut renderer);
        let header = header_rect(&wm, keys[0]);
        let down = make_mouse(
            MouseEventKind::Press(MouseButton::Left),
            header.x as u16,
            header.y as u16,
        );
        wm.dispatch_mouse(&down);

        let corner_x = (AREA.x + i32::from(AREA.width) - 1) as u16;
        let corner_y = (AREA.y + i32::from(AREA.height) - 1) as u16;
        let drag = make_mouse(MouseEventKind::Drag(MouseButton::Left), corner_x, corner_y);
        wm.dispatch_mouse(&drag);

        // Verify exact ghost geometry: bottom-right quadrant (40, 12, 40, 12)
        let snap_rect = wm
            .drag_snap_rect()
            .expect("drag_snap must be set after Drag to corner");
        assert_eq!(snap_rect.x, AREA.x + i32::from(AREA.width / 2), "ghost x");
        assert_eq!(snap_rect.y, AREA.y + i32::from(AREA.height / 2), "ghost y");
        assert_eq!(snap_rect.width, AREA.width / 2, "ghost width");
        assert_eq!(snap_rect.height, AREA.height / 2, "ghost height");

        let up = make_mouse(
            MouseEventKind::Release(MouseButton::Left),
            corner_x,
            corner_y,
        );
        wm.dispatch_mouse(&up);
        advance_frame(&mut wm, &mut engine, &mut renderer);

        let r = wm.region(keys[0]);
        assert_eq!(r.x, AREA.x + i32::from(AREA.width / 2), "corner x");
        assert_eq!(r.y, AREA.y + i32::from(AREA.height / 2), "corner y");
        assert_eq!(r.width, AREA.width / 2, "corner width");
        assert_eq!(r.height, AREA.height / 2, "corner height");
    }

    #[test]
    fn drag_away_restores_float_geometry() {
        let (mut wm, mut engine, mut renderer, keys) = setup();
        advance_frame(&mut wm, &mut engine, &mut renderer);
        let header = header_rect(&wm, keys[0]);

        let down = make_mouse(
            MouseEventKind::Press(MouseButton::Left),
            header.x as u16,
            header.y as u16,
        );
        wm.dispatch_mouse(&down);

        // Use y=12 (middle of screen) to avoid corner detection zone
        let mid_y = 12u16;
        let right_edge = (AREA.x + i32::from(AREA.width) - 1) as u16;
        let drag = make_mouse(MouseEventKind::Drag(MouseButton::Left), right_edge, mid_y);
        wm.dispatch_mouse(&drag);

        // Verify exact ghost geometry for the right-edge snap
        let snap_rect = wm
            .drag_snap_rect()
            .expect("drag_snap must be set after Drag to right edge");
        assert_eq!(snap_rect.x, AREA.x + i32::from(AREA.width / 2), "ghost x");
        assert_eq!(snap_rect.y, AREA.y, "ghost y");
        assert_eq!(snap_rect.width, AREA.width / 2, "ghost width");
        assert_eq!(snap_rect.height, AREA.height, "ghost height");

        let up = make_mouse(
            MouseEventKind::Release(MouseButton::Left),
            right_edge,
            mid_y,
        );
        wm.dispatch_mouse(&up);

        // Re-render to refresh hitboxes after layout mutation
        advance_frame(&mut wm, &mut engine, &mut renderer);

        let snapped = wm.region(keys[0]);
        let pre_w = snapped.width;
        let pre_h = snapped.height;

        let header2 = header_rect(&wm, keys[0]);
        let cursor_x = header2.x as u16;
        let cursor_y = header2.y as u16;
        let cursor_offset_x = cursor_x as i32 - snapped.x;
        let cursor_offset_y = cursor_y as i32 - snapped.y;

        let down2 = make_mouse(MouseEventKind::Press(MouseButton::Left), cursor_x, cursor_y);
        wm.dispatch_mouse(&down2);

        let away_x = (AREA.x + 10) as u16;
        let away_y = (AREA.y + 5) as u16;
        let drag2 = make_mouse(MouseEventKind::Drag(MouseButton::Left), away_x, away_y);
        wm.dispatch_mouse(&drag2);

        let up2 = make_mouse(MouseEventKind::Release(MouseButton::Left), away_x, away_y);
        wm.dispatch_mouse(&up2);

        let float_panes = wm.floating_panes();
        let (_, float_spec) = float_panes
            .iter()
            .find(|(k, _)| *k == keys[0])
            .expect("window should be floating");
        if let FloatRectSpec::Absolute(fr) = float_spec {
            assert_eq!(fr.width, pre_w, "restored width must match pre-snap");
            assert_eq!(fr.height, pre_h, "restored height must match pre-snap");
            let new_cursor_offset_x = away_x as i32 - fr.x;
            let new_cursor_offset_y = away_y as i32 - fr.y;
            assert_eq!(
                new_cursor_offset_x, cursor_offset_x,
                "cursor offset x must match"
            );
            assert_eq!(
                new_cursor_offset_y, cursor_offset_y,
                "cursor offset y must match"
            );
        } else {
            panic!("expected absolute float rect");
        }
    }

    #[test]
    fn double_snap_converges() {
        let (mut wm, mut engine, mut renderer, keys) = setup();
        advance_frame(&mut wm, &mut engine, &mut renderer);
        let header = header_rect(&wm, keys[0]);

        // Phase 1: snap right
        let down = make_mouse(
            MouseEventKind::Press(MouseButton::Left),
            header.x as u16,
            header.y as u16,
        );
        wm.dispatch_mouse(&down);
        let right_edge = (AREA.x + i32::from(AREA.width) - 1) as u16;
        let drag = make_mouse(MouseEventKind::Drag(MouseButton::Left), right_edge, 12);
        wm.dispatch_mouse(&drag);

        // Verify ghost geometry for Phase 1
        let snap_rect = wm
            .drag_snap_rect()
            .expect("phase 1: drag_snap must be set after Drag");
        assert_eq!(
            snap_rect.x,
            AREA.x + i32::from(AREA.width / 2),
            "phase 1: ghost x"
        );
        assert_eq!(snap_rect.y, AREA.y, "phase 1: ghost y");
        assert_eq!(snap_rect.width, AREA.width / 2, "phase 1: ghost width");
        assert_eq!(snap_rect.height, AREA.height, "phase 1: ghost height");

        let up = make_mouse(MouseEventKind::Release(MouseButton::Left), right_edge, 12);
        wm.dispatch_mouse(&up);

        // Re-render for phase 2
        advance_frame(&mut wm, &mut engine, &mut renderer);

        let r1 = wm.region(keys[0]);
        assert_eq!(
            r1.x,
            AREA.x + i32::from(AREA.width / 2),
            "phase 1: right-snapped x"
        );
        assert_eq!(r1.width, AREA.width / 2, "phase 1: right-snapped width");

        // Phase 2: drag away from edge
        let header2 = header_rect(&wm, keys[0]);
        let cursor_x = header2.x as u16;
        let cursor_y = header2.y as u16;
        let cursor_offset_x = cursor_x as i32 - r1.x;
        let cursor_offset_y = cursor_y as i32 - r1.y;

        let down2 = make_mouse(MouseEventKind::Press(MouseButton::Left), cursor_x, cursor_y);
        wm.dispatch_mouse(&down2);
        let away_x = (AREA.x + 10) as u16;
        let away_y = 12u16;
        let drag2 = make_mouse(MouseEventKind::Drag(MouseButton::Left), away_x, away_y);
        wm.dispatch_mouse(&drag2);
        let up2 = make_mouse(MouseEventKind::Release(MouseButton::Left), away_x, away_y);
        wm.dispatch_mouse(&up2);
        advance_frame(&mut wm, &mut engine, &mut renderer);

        // Phase 2: window must be floating with preserved dimensions and cursor offset
        let float_panes = wm.floating_panes();
        let (_, float_spec) = float_panes
            .iter()
            .find(|(k, _)| *k == keys[0])
            .expect("phase 2: window should be floating after drag-away");
        if let FloatRectSpec::Absolute(fr) = float_spec {
            assert_eq!(
                fr.width,
                AREA.width / 2,
                "phase 2: floating width must match snapped width"
            );
            assert_eq!(
                fr.height, AREA.height,
                "phase 2: floating height must match snapped height"
            );
            let new_cursor_offset_x = away_x as i32 - fr.x;
            let new_cursor_offset_y = away_y as i32 - fr.y;
            assert_eq!(
                new_cursor_offset_x, cursor_offset_x,
                "phase 2: cursor offset x must be preserved"
            );
            assert_eq!(
                new_cursor_offset_y, cursor_offset_y,
                "phase 2: cursor offset y must be preserved"
            );
        } else {
            panic!("phase 2: expected absolute float rect");
        }

        // Phase 3: snap left
        let header3 = header_rect(&wm, keys[0]);
        let down3 = make_mouse(
            MouseEventKind::Press(MouseButton::Left),
            header3.x as u16,
            header3.y as u16,
        );
        wm.dispatch_mouse(&down3);
        let left_edge = AREA.x as u16;
        let drag3 = make_mouse(MouseEventKind::Drag(MouseButton::Left), left_edge, 12);
        wm.dispatch_mouse(&drag3);
        let up3 = make_mouse(MouseEventKind::Release(MouseButton::Left), left_edge, 12);
        wm.dispatch_mouse(&up3);
        advance_frame(&mut wm, &mut engine, &mut renderer);

        let r3 = wm.region(keys[0]);
        let total_w = r3.width.saturating_add(wm.region(keys[1]).width);
        assert_eq!(r3.x, AREA.x, "phase 3: left-snapped x");
        assert_eq!(r3.y, AREA.y, "phase 3: left-snapped y");
        assert_eq!(
            r3.width,
            total_w / 2,
            "phase 3: left-snapped width = total/2"
        );
        assert_eq!(r3.height, AREA.height, "phase 3: left-snapped height");
        // Sibling must stay constrained to its lane — not expanded across full width
        let r_sibling = wm.region(keys[1]);
        assert!(
            r_sibling.x > r3.x,
            "phase 3: sibling must be to the right of keys[0]"
        );
        assert!(
            !rects_overlap(r3, r_sibling),
            "phase 3: windows must not overlap"
        );
    }
}

// ─── Module 6: Property Tests ────────────────────────────────────────

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    fn area_strategy() -> impl Strategy<Value = Rect> {
        (100u16..200, 40u16..80).prop_map(|(w, h)| Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        })
    }

    fn leaf_id_strategy() -> impl Strategy<Value = usize> {
        1usize..1000
    }

    fn tree_strategy() -> impl Strategy<Value = LayoutNode<usize>> {
        leaf_id_strategy().prop_flat_map(|first_id| {
            let insert_pos = prop_oneof![
                Just(InsertPosition::Left),
                Just(InsertPosition::Right),
                Just(InsertPosition::Top),
                Just(InsertPosition::Bottom),
            ];
            (
                Just(first_id),
                prop::collection::vec((leaf_id_strategy(), insert_pos), 0..7),
            )
                .prop_map(move |(_, ops)| {
                    let mut tree = LayoutNode::leaf(first_id);
                    for (new_id, pos) in ops {
                        let target = *collect_leaf_ids(&tree).last().unwrap_or(&first_id);
                        tree.insert_leaf(target, new_id, pos);
                    }
                    tree
                })
        })
    }

    /// Build a non-resizable version of a tree (all splits have resizable: false).
    fn make_non_resizable(node: &LayoutNode<usize>) -> LayoutNode<usize> {
        match node {
            LayoutNode::Leaf(id) => LayoutNode::leaf(*id),
            LayoutNode::Void(id) => LayoutNode::Void(*id),
            LayoutNode::Split {
                direction,
                children,
                weights,
                constraints,
                ..
            } => LayoutNode::Split {
                direction: *direction,
                children: children.iter().map(make_non_resizable).collect(),
                weights: weights.clone(),
                constraints: constraints.clone(),
                resizable: false,
            },
        }
    }

    proptest! {
        #[test]
        fn insert_never_shrinks_leaf_set(
            mut tree in tree_strategy(),
            new_id in leaf_id_strategy(),
            pos in prop_oneof![
                Just(InsertPosition::Left),
                Just(InsertPosition::Right),
                Just(InsertPosition::Top),
                Just(InsertPosition::Bottom),
            ],
        ) {
            let before = collect_leaf_ids(&tree);
            let insert_target = *before.last().unwrap_or(&1);
            tree.insert_leaf(insert_target, new_id, pos);
            let after = collect_leaf_ids(&tree);
            prop_assert!(after.len() >= before.len(),
                "leaf count must not shrink: {} -> {}", before.len(), after.len());
            prop_assert!(after.contains(&new_id),
                "new leaf {} must be present after insert", new_id);
        }

        #[test]
        fn no_overlapping_regions(
            tree in tree_strategy(),
            area in area_strategy(),
        ) {
            let regions = tree.layout(area);
            for (i, (_, r1)) in regions.iter().enumerate() {
                for (j, (_, r2)) in regions.iter().enumerate() {
                    if i < j {
                        prop_assert!(!rects_overlap(*r1, *r2),
                            "regions overlap: {:?} and {:?}", r1, r2);
                    }
                }
            }
        }

        #[test]
        fn non_void_area_covers_full(
            tree in tree_strategy(),
            area in area_strategy(),
        ) {
            let non_resizable = make_non_resizable(&tree);
            let regions = non_resizable.layout(area);
            let leaf_area: u32 = regions.iter()
                .map(|(_, r)| r.width as u32 * r.height as u32)
                .sum();
            let total_area = area.width as u32 * area.height as u32;
            prop_assert_eq!(leaf_area, total_area,
                "non-resizable tree must cover full area");
        }

        #[test]
        fn weights_stay_positive(
            mut tree in tree_strategy(),
        ) {
            tree.normalize_weights();
            fn check(node: &LayoutNode<usize>) -> bool {
                match node {
                    LayoutNode::Split { weights, children, .. } => {
                        weights.iter().all(|w| *w > 0.0) && children.iter().all(check)
                    }
                    _ => true,
                }
            }
            prop_assert!(check(&tree), "all weights must be positive");
        }
    }
}
