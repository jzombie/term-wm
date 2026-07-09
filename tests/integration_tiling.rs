use std::sync::Arc;
use term_wm::layout::tiling::{InsertPosition, LayoutNode, TilingLayout};
use term_wm::layout::Direction;
use term_wm::window::{FloatRectSpec, WindowKey, WindowManager};
use term_wm::wm_config::WmConfig;
use term_wm::AppContext;
use term_wm_layout_engine::{detect_corner_snap, detect_edge_snap, edge_preview_rect, LayoutRect};

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
            children: vec![LayoutNode::leaf(1usize), LayoutNode::leaf(2), LayoutNode::leaf(3)],
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
        let total_area: u32 = regions.iter().map(|(_, r)| r.width as u32 * r.height as u32).sum();
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
        assert!(matches!(root, LayoutNode::Leaf(2)), "removing last leaf from collapsed tree");
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
        assert!(diff <= 1, "widths should be within 1px: {} vs {}", r1.width, r2.width);
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
                LayoutNode::Split { children: inner, .. } => match &inner[1] {
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
        assert_eq!(rect_after_left.x, rect_before_left.x, "left sibling x must not change");
        assert_eq!(rect_after_left.y, rect_before_left.y, "left sibling y must not change");
        assert_eq!(rect_after_left.width, rect_before_left.width, "left sibling width must not change");
        assert_eq!(rect_after_left.height, rect_before_left.height, "left sibling height must not change");
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
        assert_eq!(root_dir, Direction::Horizontal, "root must remain Horizontal");

        root.insert_leaf(3, 4, InsertPosition::BottomRight);
        let root_dir_after = match &root {
            LayoutNode::Split { direction, .. } => *direction,
            _ => panic!("expected root Split"),
        };
        assert_eq!(root_dir_after, Direction::Horizontal, "root direction must not change after quadrant insert");
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
        assert_eq!(r3_before.1.x, r3_after.1.x, "unrelated sibling x must not change");
        assert_eq!(r3_before.1.y, r3_after.1.y, "unrelated sibling y must not change");
        assert_eq!(r3_before.1.width, r3_after.1.width, "unrelated sibling width must not change");
        assert_eq!(r3_before.1.height, r3_after.1.height, "unrelated sibling height must not change");
    }
}

// ─── Module 5: Drag-Snap Pipeline ────────────────────────────────────

#[cfg(test)]
mod drag_snap_pipeline {
    use super::*;
    use term_wm::events::{MouseButton, MouseEventKind};

    fn setup() -> (WindowManager, [WindowKey; 2]) {
        wm_with_two_windows()
    }

    /// Execute the production render pipeline against an in-memory backend.
    /// This forces the window manager to naturally populate its HitboxRegistry
    /// via the same code path used in production.  Must be called before
    /// every mouse interaction phase, since HitboxRegistry is immediate-mode
    /// and cleared/rebuilt each frame.
    fn setup_with_render(wm: &mut WindowManager) {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect as RatatuiRect;
        use term_wm::render_app;
        use term_wm_console::draw_plan_renderer::DrawPlanRenderer;
        use term_wm_console::RatatuiBackend;
        use term_wm_core::engine::CoreEngine;

        let area = RatatuiRect {
            x: 0,
            y: 0,
            width: AREA.width,
            height: AREA.height,
        };
        let buf = Buffer::empty(area);
        let mut backend = RatatuiBackend::new(buf, area);
        let mut engine = CoreEngine::new();
        let mut renderer = DrawPlanRenderer::new();

        render_app(&mut backend, wm, &mut engine, &mut renderer);
    }

    #[test]
    fn drag_to_right_edge_snaps() {
        let (mut wm, keys) = setup();
        setup_with_render(&mut wm);
        let header = header_rect(&wm, keys[0]);
        let down = make_mouse(MouseEventKind::Press(MouseButton::Left), header.x as u16, header.y as u16);
        wm.dispatch_mouse(&down);

        let right_edge = (AREA.x + i32::from(AREA.width) - 1) as u16;
        let drag = make_mouse(MouseEventKind::Drag(MouseButton::Left), right_edge, header.y as u16);
        wm.dispatch_mouse(&drag);

        let up = make_mouse(MouseEventKind::Release(MouseButton::Left), right_edge, header.y as u16);
        wm.dispatch_mouse(&up);

        let r = wm.region(keys[0]);
        assert_eq!(r.x, AREA.x + i32::from(AREA.width / 2), "right-snapped window x");
        assert_eq!(r.y, AREA.y, "right-snapped window y");
        assert_eq!(r.width, AREA.width / 2, "right-snapped window width");
        assert_eq!(r.height, AREA.height, "right-snapped window height");
    }

    #[test]
    fn drag_to_left_edge_snaps() {
        let (mut wm, keys) = setup();
        setup_with_render(&mut wm);
        let header = header_rect(&wm, keys[0]);
        let down = make_mouse(MouseEventKind::Press(MouseButton::Left), header.x as u16, header.y as u16);
        wm.dispatch_mouse(&down);

        let left_edge = AREA.x as u16;
        let drag = make_mouse(MouseEventKind::Drag(MouseButton::Left), left_edge, header.y as u16);
        wm.dispatch_mouse(&drag);

        let up = make_mouse(MouseEventKind::Release(MouseButton::Left), left_edge, header.y as u16);
        wm.dispatch_mouse(&up);

        let r = wm.region(keys[0]);
        assert_eq!(r.x, AREA.x, "left-snapped window x");
        assert_eq!(r.y, AREA.y, "left-snapped window y");
        assert_eq!(r.width, AREA.width / 2, "left-snapped window width");
        assert_eq!(r.height, AREA.height, "left-snapped window height");
    }

    #[test]
    fn drag_to_top_maximizes() {
        let (mut wm, keys) = setup();
        setup_with_render(&mut wm);
        let header = header_rect(&wm, keys[0]);
        let down = make_mouse(MouseEventKind::Press(MouseButton::Left), header.x as u16, header.y as u16);
        wm.dispatch_mouse(&down);

        let drag = make_mouse(MouseEventKind::Drag(MouseButton::Left), header.x as u16, 0);
        wm.dispatch_mouse(&drag);

        let up = make_mouse(MouseEventKind::Release(MouseButton::Left), header.x as u16, 0);
        wm.dispatch_mouse(&up);

        let r = wm.region(keys[0]);
        assert_eq!(r.x, AREA.x, "maximized x");
        assert_eq!(r.y, AREA.y, "maximized y");
        assert_eq!(r.width, AREA.width, "maximized width");
        assert_eq!(r.height, AREA.height, "maximized height");
    }

    #[test]
    fn drag_to_corner_quadrant() {
        let (mut wm, keys) = setup();
        setup_with_render(&mut wm);
        let header = header_rect(&wm, keys[0]);
        let down = make_mouse(MouseEventKind::Press(MouseButton::Left), header.x as u16, header.y as u16);
        wm.dispatch_mouse(&down);

        let corner_x = (AREA.x + i32::from(AREA.width) - 1) as u16;
        let corner_y = (AREA.y + i32::from(AREA.height) - 1) as u16;
        let drag = make_mouse(MouseEventKind::Drag(MouseButton::Left), corner_x, corner_y);
        wm.dispatch_mouse(&drag);

        let up = make_mouse(MouseEventKind::Release(MouseButton::Left), corner_x, corner_y);
        wm.dispatch_mouse(&up);

        let r = wm.region(keys[0]);
        assert_eq!(r.x, AREA.x + i32::from(AREA.width / 2), "corner x");
        assert_eq!(r.y, AREA.y + i32::from(AREA.height / 2), "corner y");
        assert_eq!(r.width, AREA.width / 2, "corner width");
        assert_eq!(r.height, AREA.height / 2, "corner height");
    }

    #[test]
    fn drag_away_restores_float_geometry() {
        let (mut wm, keys) = setup();
        setup_with_render(&mut wm);
        let header = header_rect(&wm, keys[0]);

        let down = make_mouse(MouseEventKind::Press(MouseButton::Left), header.x as u16, header.y as u16);
        wm.dispatch_mouse(&down);

        let right_edge = (AREA.x + i32::from(AREA.width) - 1) as u16;
        let drag = make_mouse(MouseEventKind::Drag(MouseButton::Left), right_edge, header.y as u16);
        wm.dispatch_mouse(&drag);

        let up = make_mouse(MouseEventKind::Release(MouseButton::Left), right_edge, header.y as u16);
        wm.dispatch_mouse(&up);

        let snapped = wm.region(keys[0]);
        let pre_w = snapped.width;
        let pre_h = snapped.height;

        // Re-render to refresh hitboxes after layout mutation
        setup_with_render(&mut wm);
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
        let (_, float_spec) = float_panes.iter().find(|(k, _)| *k == keys[0]).expect("window should be floating");
        if let FloatRectSpec::Absolute(fr) = float_spec {
            assert_eq!(fr.width, pre_w, "restored width must match pre-snap");
            assert_eq!(fr.height, pre_h, "restored height must match pre-snap");
            let new_cursor_offset_x = away_x as i32 - fr.x;
            let new_cursor_offset_y = away_y as i32 - fr.y;
            assert_eq!(new_cursor_offset_x, cursor_offset_x, "cursor offset x must match");
            assert_eq!(new_cursor_offset_y, cursor_offset_y, "cursor offset y must match");
        } else {
            panic!("expected absolute float rect");
        }
    }

    #[test]
    fn double_snap_converges() {
        let (mut wm, keys) = setup();
        setup_with_render(&mut wm);
        let header = header_rect(&wm, keys[0]);

        // Phase 1: snap right
        let down = make_mouse(MouseEventKind::Press(MouseButton::Left), header.x as u16, header.y as u16);
        wm.dispatch_mouse(&down);
        let right_edge = (AREA.x + i32::from(AREA.width) - 1) as u16;
        let drag = make_mouse(MouseEventKind::Drag(MouseButton::Left), right_edge, header.y as u16);
        wm.dispatch_mouse(&drag);
        let up = make_mouse(MouseEventKind::Release(MouseButton::Left), right_edge, header.y as u16);
        wm.dispatch_mouse(&up);

        // Re-render for phase 2
        setup_with_render(&mut wm);
        let header2 = header_rect(&wm, keys[0]);
        let down2 = make_mouse(MouseEventKind::Press(MouseButton::Left), header2.x as u16, header2.y as u16);
        wm.dispatch_mouse(&down2);
        let away_x = (AREA.x + 10) as u16;
        let drag2 = make_mouse(MouseEventKind::Drag(MouseButton::Left), away_x, header2.y as u16);
        wm.dispatch_mouse(&drag2);
        let up2 = make_mouse(MouseEventKind::Release(MouseButton::Left), away_x, header2.y as u16);
        wm.dispatch_mouse(&up2);

        // Re-render for phase 3
        setup_with_render(&mut wm);
        let header3 = header_rect(&wm, keys[0]);
        let down3 = make_mouse(MouseEventKind::Press(MouseButton::Left), header3.x as u16, header3.y as u16);
        wm.dispatch_mouse(&down3);
        let left_edge = AREA.x as u16;
        let drag3 = make_mouse(MouseEventKind::Drag(MouseButton::Left), left_edge, header3.y as u16);
        wm.dispatch_mouse(&drag3);
        let up3 = make_mouse(MouseEventKind::Release(MouseButton::Left), left_edge, header3.y as u16);
        wm.dispatch_mouse(&up3);

        let r = wm.region(keys[0]);
        assert_eq!(r.x, AREA.x, "double-snap x must be origin");
        assert_eq!(r.y, AREA.y, "double-snap y must be origin");
        assert_eq!(r.width, AREA.width / 2, "double-snap width");
        assert_eq!(r.height, AREA.height, "double-snap height");
    }
}

// ─── Module 6: Property Tests ────────────────────────────────────────

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    fn area_strategy() -> impl Strategy<Value = Rect> {
        (100u16..200, 40u16..80).prop_map(|(w, h)| Rect { x: 0, y: 0, width: w, height: h })
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
            ).prop_map(move |(_, ops)| {
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
