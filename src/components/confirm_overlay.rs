use crossterm::event::{Event, MouseEventKind};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Paragraph, Wrap};

use crate::components::{Component, DialogOverlayComponent};
use crate::layout::rect_contains;
use crate::ui::{UiFrame, safe_set_string};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAction {
    Confirm,
    Cancel,
}

#[derive(Debug, Default)]
pub struct ConfirmOverlayComponent {
    dialog: DialogOverlayComponent,
    visible: bool,
    body: String,
    selected_confirm: bool,
    cancel_rect: Option<Rect>,
    confirm_rect: Option<Rect>,
    area: Rect,
}

impl Component for ConfirmOverlayComponent {
    fn resize(&mut self, area: Rect) {
        self.area = area;
    }

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, _focused: bool) {
        self.area = area;
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
            .style(Style::default().fg(crate::theme::dialog_fg()))
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, body_rect);
        let separator_style = Style::default().fg(crate::theme::dialog_separator());
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
        let selected_style = Style::default()
            .fg(crate::theme::decorator_header_fg())
            .bg(crate::theme::decorator_header_bg())
            .add_modifier(Modifier::BOLD);
        let unselected_style = Style::default()
            .fg(crate::theme::dialog_fg())
            .bg(crate::theme::panel_bg());

        let (cancel_style, confirm_style) = if self.selected_confirm {
            // confirm is selected
            (unselected_style, selected_style)
        } else {
            // cancel is selected
            (selected_style, unselected_style)
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
        if self.handle_confirm_event(event).is_some() {
            return true;
        }
        let Event::Key(key) = event else {
            return false;
        };
        let kb = crate::keybindings::KeyBindings::default();
        kb.matches(crate::keybindings::Action::ConfirmToggle, &key)
            || kb.matches(crate::keybindings::Action::ConfirmLeft, &key)
            || kb.matches(crate::keybindings::Action::ConfirmRight, &key)
    }
}

impl ConfirmOverlayComponent {
    pub fn new() -> Self {
        let mut dialog = DialogOverlayComponent::new();
        dialog.set_bg(crate::theme::dialog_bg());
        dialog.set_auto_close_on_outside_click(false);
        Self {
            dialog,
            visible: false,
            body: String::new(),
            selected_confirm: false,
            cancel_rect: None,
            confirm_rect: None,
            area: Rect::default(),
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

impl ConfirmOverlayComponent {
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
            Event::Key(key) => {
                let kb = crate::keybindings::KeyBindings::default();
                if kb.matches(crate::keybindings::Action::ConfirmToggle, &key) {
                    self.selected_confirm = !self.selected_confirm;
                    None
                } else if kb.matches(crate::keybindings::Action::ConfirmLeft, &key) {
                    self.selected_confirm = false;
                    None
                } else if kb.matches(crate::keybindings::Action::ConfirmRight, &key) {
                    self.selected_confirm = true;
                    None
                } else if kb.matches(crate::keybindings::Action::ConfirmAccept, &key) {
                    if self.selected_confirm {
                        Some(ConfirmAction::Confirm)
                    } else {
                        Some(ConfirmAction::Cancel)
                    }
                } else if kb.matches(crate::keybindings::Action::ConfirmCancel, &key) {
                    Some(ConfirmAction::Cancel)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{
        Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    fn ev_for(action: crate::keybindings::Action) -> Event {
        use crate::keybindings::KeyBindings;
        use crossterm::event::KeyEvent;
        if let Some(combo) = KeyBindings::default().first_combo(action) {
            Event::Key(KeyEvent::new(combo.code, combo.mods))
        } else {
            // fallback: return an arbitrary key that should still be handled
            Event::Key(KeyEvent::new(
                crossterm::event::KeyCode::Esc,
                crossterm::event::KeyModifiers::NONE,
            ))
        }
    }

    #[test]
    fn handle_event_recognizes_keys() {
        let mut o = ConfirmOverlayComponent::new();
        assert!(o.handle_event(&ev_for(crate::keybindings::Action::ConfirmAccept)));
        assert!(o.handle_event(&ev_for(crate::keybindings::Action::ConfirmAccept)));
        assert!(o.handle_event(&ev_for(crate::keybindings::Action::ConfirmCancel)));
        assert!(o.handle_event(&ev_for(crate::keybindings::Action::ConfirmToggle)));
    }

    #[test]
    fn handle_confirm_event_mouse_and_keys() {
        let mut o = ConfirmOverlayComponent::new();
        // set rects so mouse tests work
        o.confirm_rect = Some(ratatui::layout::Rect {
            x: 2,
            y: 3,
            width: 4,
            height: 1,
        });
        o.cancel_rect = Some(ratatui::layout::Rect {
            x: 0,
            y: 3,
            width: 2,
            height: 1,
        });

        let m = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 3,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            o.handle_confirm_event(&Event::Mouse(m)),
            Some(ConfirmAction::Confirm)
        );

        let m2 = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 3,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            o.handle_confirm_event(&Event::Mouse(m2)),
            Some(ConfirmAction::Cancel)
        );

        // Tab toggles selection
        o.selected_confirm = true;
        assert_eq!(
            o.handle_confirm_event(&Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))),
            None
        );
        assert!(!o.selected_confirm);

        // Enter uses selected_confirm to decide
        o.selected_confirm = true;
        assert_eq!(
            o.handle_confirm_event(&Event::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE
            ))),
            Some(ConfirmAction::Confirm)
        );
    }
}
