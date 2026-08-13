//! Probe: can an argtuner-style TUI use the new `view!` types?
//!
//! Mirrors `../rust-argtuner/src/cli/tui/mod.rs`: a `TermWmApp<AppComponent>`
//! where `AppComponent` is a delegate enum (via `impl_component_delegate!`)
//! holding one concrete component per window, opened as `Custom` windows. This
//! exercises the two realistic integration styles:
//!
//! 1. **All-owned `&self` view** (`DashboardWindow`) — `view(&self)` builds a
//!    `<Box>`/`<Grid>` tree (stateless children), so `desired_height(&self)`
//!    can call `self.view().desired_height(width)` — fully dynamic, no consts.
//! 2. **`&mut self` view with a borrowed stateful child** (`ChartsWindow`,
//!    like argtuner's `ChartsView`) — `view(&mut self)` injects
//!    `{ &mut self.pane }`; `desired_height` can't build the view, so a static
//!    layout reports a const.
//!
//! `view!` is invoked through the umbrella `::term_wm::` path style (argtuner
//! depends on `term-wm`).

use std::collections::VecDeque;

use term_wm::actions::{EventResult, TermWmAction};
use term_wm::component_context::ComponentContext;
use term_wm::components::AppRootComponent;
use term_wm::events::Event;
use term_wm::helpers::{downcast_ratatui, layout_rect_to_clipped_rect};
use term_wm::term_wm_app::TermWmApp;
use term_wm::window::WindowKey;
use term_wm::Component;
use term_wm::view;
use term_wm_core::impl_component_delegate;
use term_wm_layout_engine::LayoutRect;

/// A stateful custom child, like argtuner's `ChartsView` (not cloneable).
struct ChartsPane {
    rows: usize,
}

impl Component<TermWmAction> for ChartsPane {
    fn desired_height(&self, _width: u16) -> u16 {
        self.rows as u16
    }

    fn render(
        &mut self,
        backend: &mut dyn term_wm::RenderBackend,
        area: LayoutRect,
        _ctx: &ComponentContext,
        _registry: &mut term_wm::hitbox_registry::HitboxRegistry,
    ) {
        let rect = layout_rect_to_clipped_rect(area);
        let backend = downcast_ratatui(backend);
        let _ = rect;
        let _ = backend;
    }

    fn handle_events(&mut self, _event: &Event, _ctx: &ComponentContext) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }

    fn update(
        &mut self,
        _action: TermWmAction,
        _ctx: &ComponentContext,
        _actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
    }
}

/// An all-owned `&self` view: `<Box>` + `<Grid>` with stateless children.
struct DashboardWindow;

impl DashboardWindow {
    fn view(&self) -> impl Component<TermWmAction> + '_ {
        view! {
            <Box title="Dashboard" padding=1>
                <Grid cols="16 1fr" rows="3">
                    <Label text="Trials:" />
                    <Button label=" Reset " action={TermWmAction::Quit} />
                </Grid>
            </Box>
        }
    }
}

impl Component<TermWmAction> for DashboardWindow {
    fn render(
        &mut self,
        backend: &mut dyn term_wm::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm::hitbox_registry::HitboxRegistry,
    ) {
        self.view().render(backend, area, ctx, registry);
    }

    fn handle_events(&mut self, event: &Event, ctx: &ComponentContext) -> EventResult<TermWmAction> {
        self.view().handle_events(event, ctx)
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        self.view().update(action, ctx, actions);
    }

    fn destroy(&mut self) {
        self.view().destroy();
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.view().desired_height(width)
    }
}

/// A `&mut self` view with a borrowed stateful child, like argtuner's
/// `ChartsView` wrapped in a card.
struct ChartsWindow {
    pane: ChartsPane,
}

impl ChartsWindow {
    fn view(&mut self) -> impl Component<TermWmAction> + '_ {
        view! {
            <Box title="Charts" padding=1>
                <Grid cols="16 1fr" rows="3">
                    <Label text="View:" />
                    { &mut self.pane }
                </Grid>
            </Box>
        }
    }
}

impl Component<TermWmAction> for ChartsWindow {
    fn render(
        &mut self,
        backend: &mut dyn term_wm::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm::hitbox_registry::HitboxRegistry,
    ) {
        self.view().render(backend, area, ctx, registry);
    }

    fn handle_events(&mut self, event: &Event, ctx: &ComponentContext) -> EventResult<TermWmAction> {
        self.view().handle_events(event, ctx)
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        self.view().update(action, ctx, actions);
    }

    fn destroy(&mut self) {
        self.view().destroy();
    }

    // `view()` needs `&mut self` (for `{ &mut self.pane }`), so it can't be
    // called from `desired_height(&self)`. The layout is static: Box border (2)
    // + padding (2) + a 3-row grid. This is the case where a const is the
    // correct zero-cost answer.
    fn desired_height(&self, _width: u16) -> u16 {
        2 + 2 + 3
    }
}

/// Delegate enum, like argtuner's `AppComponent`.
enum AppComponent {
    Dashboard(DashboardWindow),
    Charts(ChartsWindow),
}

impl_component_delegate!(AppComponent { Dashboard, Charts });

#[test]
fn argtuner_style_consumer_works_with_view_trees() {
    let mut app: TermWmApp<AppComponent> =
        TermWmApp::new_custom(term_wm::AppContext::new("probe", "0.0.0"));

    let dash_key = app.open_window(AppRootComponent::Custom(AppComponent::Dashboard(DashboardWindow)));
    let charts_key = app.open_window(AppRootComponent::Custom(AppComponent::Charts(
        ChartsWindow {
            pane: ChartsPane { rows: 3 },
        },
    )));

    // All-owned `&self` view: dynamic height, Box + Grid render.
    {
        let area = LayoutRect { x: 0, y: 0, width: 30, height: 10 };
        let buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 30, 10));
        let mut backend = term_wm_console::RatatuiBackend::new_simple(
            buffer,
            ratatui::layout::Rect::new(0, 0, 30, 10),
        );
        let ctx = ComponentContext::new(true).with_screen_area(area);
        let mut registry = term_wm::hitbox_registry::HitboxRegistry::new();
        app.wm()
            .component_for_key_mut(dash_key)
            .expect("window")
            .render(&mut backend, area, &ctx, &mut registry);

        let content: String = backend
            .buffer
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(content.contains("╭─ Dashboard"), "Box border + title: {content:?}");
        assert!(content.contains("Trials:"), "Grid label: {content:?}");
        assert_eq!(
            app.wm().component_for_key_mut(dash_key).unwrap().desired_height(30),
            7,
            "dynamic height (Box 2+2 + grid 3)"
        );
    }

    // `&mut self` view: borrowed stateful child in a Box.
    {
        let area = LayoutRect { x: 0, y: 0, width: 30, height: 10 };
        let buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 30, 10));
        let mut backend = term_wm_console::RatatuiBackend::new_simple(
            buffer,
            ratatui::layout::Rect::new(0, 0, 30, 10),
        );
        let ctx = ComponentContext::new(true).with_screen_area(area);
        let mut registry = term_wm::hitbox_registry::HitboxRegistry::new();
        app.wm()
            .component_for_key_mut(charts_key)
            .expect("window")
            .render(&mut backend, area, &ctx, &mut registry);

        let content: String = backend
            .buffer
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(content.contains("╭─ Charts"), "Box border + title: {content:?}");
        assert_eq!(
            app.wm().component_for_key_mut(charts_key).unwrap().desired_height(30),
            7,
            "static height (Box 2+2 + pane 3)"
        );
    }
}
