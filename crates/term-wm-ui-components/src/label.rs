use std::collections::VecDeque;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;

use crate::helpers::layout_rect_to_rect;

/// A single-line text label.
#[derive(Debug)]
pub struct LabelComponent {
    text: String,
    color: Color,
}

impl LabelComponent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            color: Color::White,
        }
    }

    pub fn with_color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }
}

impl Component<TermWmAction> for LabelComponent {
    fn desired_height(&self, _width: u16) -> u16 {
        1
    }

    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        _ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let rect = layout_rect_to_rect(area);
        let backend = crate::helpers::downcast_ratatui(backend);
        let para = Paragraph::new(Line::from(Span::styled(
            self.text.as_str(),
            Style::default().fg(self.color),
        )));
        para.render(rect, &mut backend.buffer);
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
