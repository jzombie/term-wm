use std::collections::VecDeque;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use term_wm_layout_engine::LayoutRect;

use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::events::MouseButton;
use term_wm_core::window::WindowKey;
use term_wm_ui_components::helpers::layout_rect_to_rect;

/// A system panel with utility buttons.
///
/// Currently contains a "Send Notification" button that pushes a test toast.
#[derive(Debug)]
pub struct WmSystemPanelComponent {
    /// Cached screen-space rect of the button for hit-testing.
    button_rect: Option<LayoutRect>,
}

const BUTTON_WIDTH: u16 = 24;
const BUTTON_HEIGHT: u16 = 3;

impl WmSystemPanelComponent {
    pub fn new() -> Self {
        Self {
            button_rect: None,
        }
    }

    /// Compute the button's LayoutRect given the panel area.
    fn button_layout(&self, area: LayoutRect) -> LayoutRect {
        let btn_x = area
            .x
            .saturating_add(area.width as i32 / 2)
            .saturating_sub(BUTTON_WIDTH as i32 / 2);
        let btn_y = area.y.saturating_add(4);
        LayoutRect {
            x: btn_x,
            y: btn_y,
            width: BUTTON_WIDTH,
            height: BUTTON_HEIGHT,
        }
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
        _ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let ratatui_area = layout_rect_to_rect(area);
        let backend = term_wm_ui_components::helpers::downcast_ratatui(backend);
        let buffer = &mut backend.buffer;

        // Clear the full area
        Clear.render(ratatui_area, buffer);

        // Outer block with title
        let block = Block::default()
            .title("System Panel")
            .borders(Borders::ALL)
            .style(Style::default().bg(Color::Black).fg(Color::White));
        block.render(ratatui_area, buffer);

        // Inner area (inside borders)
        let inner = LayoutRect {
            x: area.x.saturating_add(1),
            y: area.y.saturating_add(1),
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };

        // Render description text
        if inner.height >= 2 {
            let desc = Paragraph::new(Line::from(vec![Span::styled(
                "Notification test panel",
                Style::default().fg(Color::DarkGray),
            )]));
            let desc_rect = layout_rect_to_rect(LayoutRect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: 1,
            });
            desc.render(desc_rect, buffer);
        }

        // Render the button
        let btn = self.button_layout(area);
        let btn_rect = layout_rect_to_rect(btn);
        self.button_rect = Some(btn);

        let btn_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .style(Style::default().bg(Color::Rgb(30, 30, 50)));
        btn_block.render(btn_rect, buffer);

        let btn_label = Line::from(Span::styled(
            " Send Notification ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));
        // Center the label inside the button
        let label_x = btn.x + (btn.width as i32 / 2).saturating_sub(11);
        let label_y = btn.y + 1;
        let label_rect = layout_rect_to_rect(LayoutRect {
            x: label_x,
            y: label_y,
            width: BUTTON_WIDTH.saturating_sub(2),
            height: 1,
        });
        let label_para = Paragraph::new(btn_label);
        label_para.render(label_rect, buffer);

        // Render hint text below button
        if inner.height >= 6 {
            let hint_y = btn.y + BUTTON_HEIGHT as i32 + 1;
            if hint_y < area.y + area.height as i32 {
                let hint = Paragraph::new(Line::from(vec![Span::styled(
                    "Click to send a test toast",
                    Style::default().fg(Color::DarkGray),
                )]));
                let hint_rect = layout_rect_to_rect(LayoutRect {
                    x: inner.x,
                    y: hint_y,
                    width: inner.width,
                    height: 1,
                });
                hint.render(hint_rect, buffer);
            }
        }
    }

    fn on_mouse_press(
        &mut self,
        local_x: u16,
        local_y: u16,
        button: MouseButton,
        _modifiers: term_wm_core::events::KeyModifiers,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        if button != MouseButton::Left {
            return EventResult::Ignored;
        }

        let Some(screen_area) = ctx.screen_area() else {
            return EventResult::Ignored;
        };
        let Some(btn) = self.button_rect else {
            return EventResult::Ignored;
        };

        // Convert local coordinates to screen coordinates
        let screen_x = screen_area.x.saturating_add(local_x as i32);
        let screen_y = screen_area.y.saturating_add(local_y as i32);

        // Check if click is within the button bounds
        if screen_x >= btn.x
            && screen_x < btn.x + btn.width as i32
            && screen_y >= btn.y
            && screen_y < btn.y + btn.height as i32
        {
            return EventResult::Action(TermWmAction::SendNotification(
                "Hello from System Panel!".to_string(),
            ));
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
