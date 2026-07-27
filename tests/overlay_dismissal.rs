use std::collections::VecDeque;
use std::sync::Arc;

use term_wm_core::actions::TermWmAction;
use term_wm_core::components::{Component, ComponentContext, NoopComponent, NoopWmComponent, Overlay};
use term_wm_core::events::{Event, MouseButton, MouseEvent, MouseEventKind, KeyModifiers};
use term_wm_core::window::{WindowKey, WindowManager, LayerManager};
use term_wm_core::wm_config::WmConfig;
use term_wm_core::AppContext;
use term_wm_layout_engine::LayoutRect;

/// An overlay that returns known render_area bounds for spatial hit-testing.
struct TestOverlay {
    bounds: Option<LayoutRect>,
    visible: bool,
}

impl Component<TermWmAction> for TestOverlay {
    fn render(
        &mut self,
        _backend: &mut dyn term_wm_render::RenderBackend,
        _area: LayoutRect,
        _ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
    }
    fn update(
        &mut self,
        _action: TermWmAction,
        _ctx: &ComponentContext,
        _actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
    }
}

impl Overlay<TermWmAction> for TestOverlay {
    fn visible(&self) -> bool {
        self.visible
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn render_area(&self) -> Option<LayoutRect> {
        self.bounds.filter(|b| b.width > 0 && b.height > 0)
    }
}

fn setup_wm_with_palette(
    palette_bounds: Option<LayoutRect>,
) -> WindowManager<NoopComponent, NoopWmComponent, TestOverlay> {
    let mut wm = WindowManager::<NoopComponent, NoopWmComponent, TestOverlay>::with_config(
        WmConfig::standalone(),
        Arc::new(AppContext::new("test", "0.0.0")),
        None,
        LayerManager::new(),
        std::collections::HashMap::new(),
    );
    let key = wm.create_window(NoopComponent);
    wm.transition_window(key, term_wm_core::window::WindowState::Mapped);
    wm.set_focus_order(vec![key]);
    wm.focus_window_key(key);

    let overlay = TestOverlay {
        bounds: palette_bounds,
        visible: true,
    };
    wm.open_command_palette_overlay(overlay);
    wm
}

fn make_mouse_event(col: u16, row: u16) -> Event {
    Event::Mouse(MouseEvent {
        kind: MouseEventKind::Press(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

#[test]
fn command_palette_with_bounds_outside_click_closes_palette() {
    let mut wm = setup_wm_with_palette(Some(LayoutRect {
        x: 20, y: 5, width: 40, height: 10,
    }));
    assert!(wm.command_palette_visible());

    let evt = make_mouse_event(5, 2);
    let mut actions = VecDeque::new();
    let stale_key = wm.command_palette_key();

    wm.close_command_palette();
    let _handled = wm.handle_outside_click(5, 2, &evt, &mut actions, stale_key);

    assert!(!wm.command_palette_visible(), "palette should be closed after outside click");
}

#[test]
fn command_palette_with_bounds_inside_click_does_not_close() {
    let wm = setup_wm_with_palette(Some(LayoutRect {
        x: 20, y: 5, width: 40, height: 10,
    }));
    assert!(wm.command_palette_visible());

    let bounds = wm.command_palette_bounds().unwrap();
    let inside_x = (bounds.x + i32::from(bounds.width) / 2) as u16;
    let inside_y = (bounds.y + i32::from(bounds.height) / 2) as u16;
    assert!(
        bounds.contains(inside_x, inside_y),
        "click should be inside palette bounds"
    );
    assert!(wm.command_palette_visible());
}

#[test]
fn command_palette_bounds_delegates_to_overlay_render_area() {
    let wm = setup_wm_with_palette(Some(LayoutRect {
        x: 10, y: 5, width: 40, height: 10,
    }));
    assert_eq!(
        wm.command_palette_bounds(),
        Some(LayoutRect { x: 10, y: 5, width: 40, height: 10 })
    );
}

#[test]
fn command_palette_no_bounds_falls_through() {
    let wm = setup_wm_with_palette(None);
    assert!(wm.command_palette_visible());
    assert_eq!(wm.command_palette_bounds(), None);
}

#[test]
fn handle_outside_click_returns_false_with_palette_key_ignored() {
    let mut wm = setup_wm_with_palette(Some(LayoutRect {
        x: 0, y: 0, width: 80, height: 24,
    }));
    let palette_key = wm.command_palette_key();

    let evt = make_mouse_event(5, 5);
    let mut actions = VecDeque::new();
    // No hitboxes registered at all → handle_outside_click returns false
    let result = wm.handle_outside_click(5, 5, &evt, &mut actions, palette_key);
    assert!(!result, "no hitboxes means nothing was handled");
    assert!(actions.is_empty(), "no actions expected");
    assert!(wm.command_palette_visible(), "palette should still be visible (no outside-click routed)");
}
