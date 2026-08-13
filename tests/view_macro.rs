//! Integration tests for the declarative `view!` macro.
//!
//! The all-owned pattern: `view!` produces a `'static` `Component` that is
//! handed straight to `open_window` — no wrapper type.

use std::sync::Arc;

use term_wm::config::AppBuilder;
use term_wm::view;
use term_wm::Component;
use term_wm_core::components::NoopOverlay;
use term_wm_layout_engine::LayoutRect;
use term_wm_ui_facade::layer_component::LayerComponent;

#[test]
fn view_macro_all_owned_window_renders_label_and_button() {
    let ctx = Arc::new(term_wm::AppContext::new("test", "0.0.0"));
    let mut wm = AppBuilder::<LayerComponent>::new()
        .app_ctx(ctx)
        .build::<_, NoopOverlay>()
        .expect("test build");

    let key = wm.open_window(view! {
        <VerticalStack gap=1>
            <Label text="Hello view!" />
            <Button label="Quit" action={term_wm::TermWmAction::Quit} />
        </VerticalStack>
    });

    let comp = wm.component_for_key_mut(key).expect("window component");
    let buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 10));
    let area = LayoutRect {
        x: 0,
        y: 0,
        width: 40,
        height: 10,
    };
    let mut backend =
        term_wm_console::RatatuiBackend::new_simple(buffer, ratatui::layout::Rect::new(0, 0, 40, 10));
    let ctx = term_wm::ComponentContext::new(true).with_screen_area(area);
    let mut registry = term_wm::hitbox_registry::HitboxRegistry::new();
    comp.render(&mut backend, area, &ctx, &mut registry);

    let content: String = backend
        .buffer
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(content.contains("Hello view!"), "label should render: {content:?}");
    assert!(content.contains("Quit"), "button label should render");
}

#[test]
fn view_macro_grid_and_center_layout() {
    let ctx = Arc::new(term_wm::AppContext::new("test", "0.0.0"));
    let mut wm = AppBuilder::<LayerComponent>::new()
        .app_ctx(ctx)
        .build::<_, NoopOverlay>()
        .expect("test build");

    let key = wm.open_window(view! {
        <Center width=20 height=3>
            <Grid cols="1fr 1fr" rows="1fr">
                <Label text="L" />
                <Label text="R" />
            </Grid>
        </Center>
    });

    let comp = wm.component_for_key_mut(key).expect("window component");
    let buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 10));
    let area = LayoutRect {
        x: 0,
        y: 0,
        width: 40,
        height: 10,
    };
    let mut backend =
        term_wm_console::RatatuiBackend::new_simple(buffer, ratatui::layout::Rect::new(0, 0, 40, 10));
    let ctx = term_wm::ComponentContext::new(true).with_screen_area(area);
    let mut registry = term_wm::hitbox_registry::HitboxRegistry::new();
    comp.render(&mut backend, area, &ctx, &mut registry);

    let content: String = backend
        .buffer
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(content.contains('L'), "left grid cell should render");
    assert!(content.contains('R'), "right grid cell should render");
}
