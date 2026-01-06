use crossterm::event::{Event, KeyCode, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Paragraph, Wrap};

use crate::components::{Component, DialogOverlay};
use crate::layout::rect_contains;
use crate::ui::safe_set_string;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAction {
    Confirm,
    Cancel,
}

#[derive(Debug, Default)]
pub struct ConfirmOverlay {
    dialog: DialogOverlay,
    visible: bool,
    body: String,
    selected_confirm: bool,
    cancel_rect: Option<Rect>,
    confirm_rect: Option<Rect>,
}

impl ConfirmOverlay {
    pub fn new() -> Self {
        let mut dialog = DialogOverlay::new();
        dialog.set_size(60, 9);
        dialog.set_dim_backdrop(true);
        dialog.set_bg(Color::Black);
        Self {
            dialog,
            visible: false,
            body: String::new(),
            selected_confirm: true,
            cancel_rect: None,
            confirm_rect: None,
        }
    }

    pub fn open(&mut self, title: &str, body: &str) {
        self.dialog.set_title(title);
        self.dialog.set_body("");
        self.dialog.set_visible(true);
        self.visible = true;
        self.body = body.to_string();
        self.selected_confirm = true;
    }

    pub fn close(&mut self) {
        self.dialog.set_visible(false);
        self.visible = false;
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn set_dim_backdrop(&mut self, dim: bool) {
        self.dialog.set_dim_backdrop(dim);
    }
}

impl Component for ConfirmOverlay {
    fn render(&mut self, frame: &mut Frame, area: Rect, _focused: bool) {
        if !self.visible || area.width == 0 || area.height == 0 {
            return;
        }
        self.dialog.render(frame, area, false);
        let rect = self.dialog.rect_for(area);
        if rect.width < 3 || rect.height < 3 {
            return;
        }
        self.cancel_rect = None;
        self.confirm_rect = None;
        let inner = Rect {
            x: rect.x.saturating_add(1),
            y: rect.y.saturating_add(1),
            width: rect.width.saturating_sub(2),
            height: rect.height.saturating_sub(2),
        };
        if inner.height == 0 || inner.width == 0 {
            return;
        }
        let content = Rect {
            x: inner.x.saturating_add(1),
            y: inner.y,
            width: inner.width.saturating_sub(2),
            height: inner.height,
        };
        if content.height < 4 || content.width == 0 {
            return;
        }
        let separator_y = content.y.saturating_add(content.height.saturating_sub(2));
        let button_y = content.y.saturating_add(content.height.saturating_sub(1));
        let body_rect = Rect {
            x: content.x,
            y: content.y,
            width: content.width,
            height: content.height.saturating_sub(3),
        };
        let paragraph = Paragraph::new(self.body.as_str())
            .alignment(Alignment::Left)
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, body_rect);
        let separator_style = Style::default().fg(Color::DarkGray);
        let buffer = frame.buffer_mut();
        let bounds = area.intersection(buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        for x in content.x..content.x.saturating_add(content.width) {
            if let Some(cell) = buffer.cell_mut((x, separator_y)) {
                cell.set_symbol("─");
                cell.set_style(separator_style);
            }
        }
        let cancel = "[ Cancel ]";
        let confirm = "[ Exit ]";
        let cancel_style = if self.selected_confirm {
            Style::default().fg(Color::White).bg(Color::DarkGray)
        } else {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Gray)
                .add_modifier(Modifier::BOLD)
        };
        let confirm_style = if self.selected_confirm {
            Style::default()
                .fg(Color::White)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White).bg(Color::DarkGray)
        };
        let total_width = cancel.len() + 1 + confirm.len();
        let start_x = content
            .x
            .saturating_add(content.width.saturating_sub(total_width as u16));
        safe_set_string(buffer, bounds, start_x, button_y, cancel, cancel_style);
        let confirm_x = start_x.saturating_add(cancel.len() as u16 + 1);
        safe_set_string(buffer, bounds, confirm_x, button_y, confirm, confirm_style);
        self.cancel_rect = Some(Rect {
            x: start_x,
            y: button_y,
            width: cancel.len() as u16,
            height: 1,
        });
        self.confirm_rect = Some(Rect {
            x: confirm_x,
            y: button_y,
            width: confirm.len() as u16,
            height: 1,
        });
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        let Event::Key(key) = event else {
            return false;
        };
        matches!(
            key.code,
            KeyCode::Enter
                | KeyCode::Char('y')
                | KeyCode::Esc
                | KeyCode::Char('n')
                | KeyCode::Tab
                | KeyCode::BackTab
                | KeyCode::Left
                | KeyCode::Right
        )
    }
}

impl ConfirmOverlay {
    pub fn handle_confirm_event(&mut self, event: &Event) -> Option<ConfirmAction> {
        match event {
            Event::Mouse(mouse) if matches!(mouse.kind, MouseEventKind::Down(_)) => {
                if self
                    .confirm_rect
                    .is_some_and(|rect| rect_contains(rect, mouse.column, mouse.row))
                {
                    return Some(ConfirmAction::Confirm);
                }
                if self
                    .cancel_rect
                    .is_some_and(|rect| rect_contains(rect, mouse.column, mouse.row))
                {
                    return Some(ConfirmAction::Cancel);
                }
                None
            }
            Event::Key(key) => match key.code {
                KeyCode::Tab => {
                    self.selected_confirm = !self.selected_confirm;
                    None
                }
                KeyCode::BackTab => {
                    self.selected_confirm = !self.selected_confirm;
                    None
                }
                KeyCode::Left => {
                    self.selected_confirm = false;
                    None
                }
                KeyCode::Right => {
                    self.selected_confirm = true;
                    None
                }
                KeyCode::Enter | KeyCode::Char('y') => {
                    if self.selected_confirm {
                        Some(ConfirmAction::Confirm)
                    } else {
                        Some(ConfirmAction::Cancel)
                    }
                }
                KeyCode::Esc | KeyCode::Char('n') => Some(ConfirmAction::Cancel),
                _ => None,
            },
            _ => None,
        }
    }
}
