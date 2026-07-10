use std::collections::VecDeque;

use ratatui::style::Color;
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext, SelectionStatus};
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;
use term_wm_ui_components::{
    ButtonComponent, LabelComponent, ScrollViewComponent, VerticalStackComponent,
};

/// A system panel with utility buttons, built from declarative components.
pub struct WmSystemPanelComponent {
    scroll_view: ScrollViewComponent<VerticalStackComponent>,
}

impl WmSystemPanelComponent {
    pub fn new() -> Self {
        let mut stack = VerticalStackComponent::new();
        stack.add(Box::new(
            LabelComponent::new("Notification test panel").with_color(Color::DarkGray),
        ));
        stack.add(Box::new(SpacerComponent::new(1)));
        stack.add(Box::new(
            LabelComponent::new("Click below to send a test toast:").with_color(Color::DarkGray),
        ));
        stack.add(Box::new(SpacerComponent::new(1)));
        stack.add(Box::new(ButtonComponent::new(
            "  Send Notification  ",
            TermWmAction::SendNotification("Hello from System Panel!".to_string()),
        )));

        let scroll_view = ScrollViewComponent::new(stack);
        Self { scroll_view }
    }
}

impl Default for WmSystemPanelComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component<TermWmAction> for WmSystemPanelComponent {
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        self.scroll_view.render(backend, area, ctx, registry);
    }

    fn handle_events(
        &mut self,
        event: &term_wm_core::events::Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        self.scroll_view.handle_events(event, ctx)
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        self.scroll_view.update(action, ctx, actions);
    }

    fn destroy(&mut self) {}

    fn selection_status(&self) -> SelectionStatus {
        self.scroll_view.selection_status()
    }

    fn selection_text(&self) -> Option<String> {
        self.scroll_view.selection_text()
    }
}

/// A simple spacer component that takes up a fixed number of rows.
struct SpacerComponent {
    height: u16,
}

impl SpacerComponent {
    fn new(height: u16) -> Self {
        Self { height }
    }
}

impl Component<TermWmAction> for SpacerComponent {
    fn desired_height(&self, _width: u16) -> u16 {
        self.height
    }

    fn render(
        &mut self,
        _backend: &mut dyn term_wm_render::RenderBackend,
        _area: LayoutRect,
        _ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
    }

    fn handle_events(
        &mut self,
        _event: &term_wm_core::events::Event,
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

    fn destroy(&mut self) {}
}
