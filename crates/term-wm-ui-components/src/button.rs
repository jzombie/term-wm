use std::collections::VecDeque;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::events::MouseButton;
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;

use crate::helpers::layout_rect_to_rect;

/// A clickable button rendered as styled borders + label.
#[derive(Debug)]
pub struct ButtonComponent {
    label: String,
    action: TermWmAction,
}

impl ButtonComponent {
    pub fn new(label: impl Into<String>, action: TermWmAction) -> Self {
        Self {
            label: label.into(),
            action,
        }
    }
}

impl Component<TermWmAction> for ButtonComponent {
    fn desired_height(&self, _width: u16) -> u16 {
        3
    }

    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        _ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        if area.width == 0 || area.height < 3 {
            return;
        }
        let rect = layout_rect_to_rect(area);
        let backend = crate::helpers::downcast_ratatui(backend);

        // Top border
        let top = format!("╭{}╮", "─".repeat(area.width.saturating_sub(2) as usize));
        Paragraph::new(Line::from(Span::styled(
            top,
            Style::default().fg(Color::Cyan),
        )))
        .render(
            ratatui::layout::Rect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: 1,
            },
            &mut backend.buffer,
        );

        // Label row
        let label_line = format!(
            "│{:^width$}│",
            self.label,
            width = area.width.saturating_sub(2) as usize
        );
        Paragraph::new(Line::from(Span::styled(
            label_line,
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(30, 30, 50))
                .add_modifier(Modifier::BOLD),
        )))
        .render(
            ratatui::layout::Rect {
                x: rect.x,
                y: rect.y + 1,
                width: rect.width,
                height: 1,
            },
            &mut backend.buffer,
        );

        // Bottom border
        let bottom = format!("╰{}╯", "─".repeat(area.width.saturating_sub(2) as usize));
        Paragraph::new(Line::from(Span::styled(
            bottom,
            Style::default().fg(Color::Cyan),
        )))
        .render(
            ratatui::layout::Rect {
                x: rect.x,
                y: rect.y + 2,
                width: rect.width,
                height: 1,
            },
            &mut backend.buffer,
        );
    }

    fn handle_events(
        &mut self,
        event: &term_wm_core::events::Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        if ctx.localize_mouse_click(event, MouseButton::Left).is_some() {
            return EventResult::Action(self.action.clone());
        }
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
