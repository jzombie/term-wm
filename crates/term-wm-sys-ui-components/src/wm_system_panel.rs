use std::collections::VecDeque;

use ratatui::style::Color;
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::component_context::ComponentContext;
use term_wm_core::components::{Component, SelectionStatus};
use term_wm_core::impl_component_delegate;
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;
use term_wm_ui_components::{CanvasSizingPolicy, 
    ButtonComponent, CanvasScrollView, LabelComponent, ScrollViewComponent, VerticalStackComponent,
};

/// Local enum wrapping all child types used in the system panel stack.
enum PanelChild {
    Label(LabelComponent),
    Button(ButtonComponent),
    Spacer(SpacerComponent),
}

impl_component_delegate!(PanelChild {
    Label,
    Button,
    Spacer,
});

/// A system panel with utility buttons, built from declarative components.
pub struct WmSystemPanelComponent {
    scroll_view: ScrollViewComponent<CanvasScrollView<VerticalStackComponent<PanelChild>>>,
}

impl WmSystemPanelComponent {
    pub fn new() -> Self {
        let mut stack = VerticalStackComponent::<PanelChild>::new();
        stack.add(PanelChild::Label(
            LabelComponent::new("Notification test panel").with_color(Color::DarkGray),
        ));
        stack.add(PanelChild::Spacer(SpacerComponent::new(1)));
        stack.add(PanelChild::Label(
            LabelComponent::new("Click below to send a test toast:").with_color(Color::DarkGray),
        ));
        stack.add(PanelChild::Spacer(SpacerComponent::new(1)));
        stack.add(PanelChild::Button(ButtonComponent::new(
            "  Send Notification  ",
            TermWmAction::SendNotification("Hello from System Panel!".to_string()),
        )));
        stack.add(PanelChild::Spacer(SpacerComponent::new(1)));
        stack.add(PanelChild::Label(
            LabelComponent::new("Debug utilities:").with_color(Color::DarkGray),
        ));
        stack.add(PanelChild::Spacer(SpacerComponent::new(1)));
        stack.add(PanelChild::Button(ButtonComponent::new(
            "  Trigger Panic  ",
            TermWmAction::Callback(|| panic!("Manual panic from system panel")),
        )));

        let scroll_view = ScrollViewComponent::new(CanvasScrollView::new(stack, CanvasSizingPolicy::FitViewportWidth));
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use term_wm_core::events::{Event, KeyCode, KeyEvent, KeyKind, KeyModifiers};

    #[test]
    fn system_panel_new_constructs() {
        let panel = WmSystemPanelComponent::new();
        // Panel wraps a ScrollViewComponent<VerticalStackComponent>; just verify it builds
        let _ = &panel;
    }

    #[test]
    fn system_panel_default_is_same_as_new() {
        let panel = WmSystemPanelComponent::default();
        let _ = &panel;
    }

    #[test]
    fn system_panel_render_does_not_panic() {
        let mut panel = WmSystemPanelComponent::new();
        let buffer = Buffer::empty(Rect::new(0, 0, 60, 20));
        let mut backend = term_wm_console::RatatuiBackend::new(buffer, Rect::new(0, 0, 60, 20));
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 60,
            height: 20,
        });
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        panel.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 60,
                height: 20,
            },
            &ctx,
            &mut registry,
        );
    }

    #[test]
    fn system_panel_handle_events_ignores_key() {
        let mut panel = WmSystemPanelComponent::new();
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 60,
            height: 20,
        });
        let event = Event::Key(KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyKind::Press,
        ));
        assert!(panel.handle_events(&event, &ctx).is_ignored());
    }

    #[test]
    fn system_panel_update_is_noop() {
        let mut panel = WmSystemPanelComponent::new();
        let ctx = ComponentContext::new(true);
        let mut actions = VecDeque::new();
        panel.update(TermWmAction::Quit, &ctx, &mut actions);
    }

    #[test]
    fn system_panel_selection_status() {
        let panel = WmSystemPanelComponent::new();
        let _ = panel.selection_status();
    }

    #[test]
    fn system_panel_selection_text() {
        let panel = WmSystemPanelComponent::new();
        let _ = panel.selection_text();
    }

    #[test]
    fn system_panel_destroy_is_noop() {
        let mut panel = WmSystemPanelComponent::new();
        panel.destroy();
    }

    #[test]
    fn spacer_desired_height() {
        let spacer = SpacerComponent::new(5);
        assert_eq!(spacer.desired_height(40), 5);
    }

    #[test]
    fn spacer_render_is_noop() {
        let mut spacer = SpacerComponent::new(3);
        let buffer = Buffer::empty(Rect::new(0, 0, 40, 10));
        let mut backend = term_wm_console::RatatuiBackend::new(buffer, Rect::new(0, 0, 40, 10));
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        spacer.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
            &mut registry,
        );
    }

    #[test]
    fn spacer_handle_events_ignored() {
        let mut spacer = SpacerComponent::new(3);
        let ctx = ComponentContext::new(true);
        let event = Event::Key(KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyKind::Press,
        ));
        assert!(spacer.handle_events(&event, &ctx).is_ignored());
    }

    #[test]
    fn spacer_update_and_destroy_are_noops() {
        let mut spacer = SpacerComponent::new(3);
        let ctx = ComponentContext::new(true);
        let mut actions = VecDeque::new();
        spacer.update(TermWmAction::Quit, &ctx, &mut actions);
        spacer.destroy();
    }
}
