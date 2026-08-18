#![allow(clippy::unwrap_used)]

//! Probe: exercise the `impl_view_component!` integration styles in the
//! window-host shape used by `rust-argtuner/src/cli/tui/mod.rs`: a
//! `TermWmApp<AppComponent>` where `AppComponent` is a delegate enum (via
//! `impl_component_delegate!`) holding one window struct per pane, opened as
//! `Custom` windows. argtuner's own panes return their stateful child directly
//! from `view()` and rely on the `child:` delegation form; this probe also
//! covers the all-owned `&self` style. The two styles:
//!
//! 1. **All-owned `&self` view** (`DashboardWindow`) — `view(&self)` builds a
//!    `<Box>`/`<Grid>` tree (stateless children), so `desired_height(&self)`
//!    can call `self.view().desired_height(width)` — fully dynamic, no consts.
//! 2. **`&mut self` view with a borrowed stateful child** (`ChartsWindow`,
//!    like argtuner's `ChartsView`) — `view(&mut self)` injects
//!    `{ &mut self.pane }` via `impl_view_component!(ChartsWindow, child: pane)`,
//!    which forwards the lifecycle to the view and delegates `desired_height` +
//!    selection/hitbox to `self.pane`.
//!
//! `view!` is invoked through the umbrella `::term_wm::` path style (argtuner
//! depends on `term-wm`).

use std::collections::VecDeque;

use term_wm::Component;
use term_wm::actions::{EventResult, TermWmAction};
use term_wm::component_context::ComponentContext;
use term_wm::components::AppRootComponent;
use term_wm::components::SelectionStatus;
use term_wm::events::Event;
use term_wm::helpers::{downcast_ratatui, layout_rect_to_clipped_rect};
use term_wm::term_wm_app::TermWmApp;
use term_wm::view;
use term_wm::window::WindowKey;
use term_wm_core::impl_component_delegate;
use term_wm_core::impl_view_component;
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

    fn handle_events(
        &mut self,
        _event: &Event,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
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

// All-owned `&self` view: forwards everything, including a dynamic
// `desired_height` (no constants).
impl_view_component!(DashboardWindow);

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

// `&mut self` view (borrows the stateful pane): the `child:` form forwards the
// lifecycle to the view and delegates `desired_height` + selection/hitbox to
// `self.pane`.
impl_view_component!(ChartsWindow, child: pane);

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

    let dash_key = app.open_window(AppRootComponent::Custom(AppComponent::Dashboard(
        DashboardWindow,
    )));
    let charts_key = app.open_window(AppRootComponent::Custom(AppComponent::Charts(
        ChartsWindow {
            pane: ChartsPane { rows: 3 },
        },
    )));

    // All-owned `&self` view: dynamic height, Box + Grid render.
    {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 30,
            height: 10,
        };
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
        assert!(
            content.contains("╭─ Dashboard"),
            "Box border + title: {content:?}"
        );
        assert!(content.contains("Trials:"), "Grid label: {content:?}");
        assert_eq!(
            app.wm()
                .component_for_key_mut(dash_key)
                .unwrap()
                .desired_height(30),
            7,
            "dynamic height (Box 2+2 + grid 3)"
        );
    }

    // `&mut self` view: borrowed stateful child in a Box.
    {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 30,
            height: 10,
        };
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
        assert!(
            content.contains("╭─ Charts"),
            "Box border + title: {content:?}"
        );
        let AppRootComponent::Custom(AppComponent::Charts(charts)) = app
            .wm()
            .component_for_key_mut(charts_key)
            .expect("charts window")
        else {
            panic!("expected charts window");
        };
        assert_eq!(charts.desired_height(30), 3, "delegated height (pane.rows)");
        assert_eq!(
            charts.selection_status().active,
            charts.pane.selection_status().active,
            "selection status forwarded to child"
        );
        assert_eq!(
            charts.selection_text(),
            charts.pane.selection_text(),
            "selection text forwarded to child"
        );
    }
}

/// A mock selectable component for the multi-child aggregation test.
struct MockSelectable {
    active: bool,
    dragging: bool,
    text: Option<String>,
    selection_enabled: bool,
    pasted: Vec<String>,
}

impl Default for MockSelectable {
    fn default() -> Self {
        Self {
            active: false,
            dragging: false,
            text: None,
            selection_enabled: true,
            pasted: Vec::new(),
        }
    }
}

impl MockSelectable {
    fn set_active_selection(&mut self, text: &str) {
        self.active = true;
        self.dragging = false;
        self.text = Some(text.to_string());
    }
}

impl Component<TermWmAction> for MockSelectable {
    fn desired_height(&self, _width: u16) -> u16 {
        0
    }

    fn render(
        &mut self,
        _b: &mut dyn term_wm::RenderBackend,
        _a: LayoutRect,
        _c: &ComponentContext,
        _r: &mut term_wm::hitbox_registry::HitboxRegistry,
    ) {
    }

    fn handle_events(&mut self, _e: &Event, _c: &ComponentContext) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }

    fn selection_status(&self) -> SelectionStatus {
        SelectionStatus {
            active: self.active,
            dragging: self.dragging,
        }
    }

    fn selection_text(&self) -> Option<String> {
        self.text.clone()
    }

    fn clear_selection(&mut self) {
        self.active = false;
        self.dragging = false;
        self.text = None;
    }

    fn set_selection_enabled(&mut self, enabled: bool) {
        self.selection_enabled = enabled;
    }

    fn paste(&mut self, text: &str) -> bool {
        self.pasted.push(text.to_string());
        true
    }
}

/// A `&mut self` view with TWO selectable child fields, aggregated via
/// `impl_view_component!(…, height = 0, child: pane_a, pane_b)`.
struct MultiSelectWindow {
    pane_a: MockSelectable,
    pane_b: MockSelectable,
}

impl MultiSelectWindow {
    fn view(&mut self) -> impl Component<TermWmAction> + '_ {
        term_wm::LabelComponent::new("multi")
    }
}

impl_view_component!(MultiSelectWindow, height = 0, child: pane_a, pane_b);

#[test]
fn test_multi_child_selection_aggregation() {
    let mut win = MultiSelectWindow {
        pane_a: MockSelectable::default(),
        pane_b: MockSelectable::default(),
    };

    // 1. No active selection anywhere -> aggregated status is inactive.
    assert!(!win.selection_status().active);
    assert_eq!(win.selection_text(), None);

    // 2. Activate a selection on pane_b (the SECOND field) -> aggregation
    //    surfaces pane_b's status + text.
    win.pane_b.set_active_selection("Selected in B");
    assert!(win.selection_status().active);
    assert_eq!(win.selection_text().unwrap(), "Selected in B");

    // 3. Activate pane_a too -> the FIRST active field wins.
    win.pane_a.set_active_selection("Selected in A");
    assert!(win.selection_status().active);
    assert_eq!(win.selection_text().unwrap(), "Selected in A");

    // 4. clear_selection fans out to BOTH children.
    win.clear_selection();
    assert!(!win.pane_a.selection_status().active);
    assert!(!win.pane_b.selection_status().active);

    // 5. set_selection_enabled fans out to both.
    win.set_selection_enabled(false);
    assert!(!win.pane_a.selection_enabled);
    assert!(!win.pane_b.selection_enabled);

    // 6. paste is tried on each child in order until one consumes it
    //    (pane_a first).
    assert!(win.paste("hello"));
    assert_eq!(win.pane_a.pasted, vec!["hello".to_string()]);
    assert!(win.pane_b.pasted.is_empty());
}
