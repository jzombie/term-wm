use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use term_wm_core::events::{Event, MouseEventKind};

use std::cell::Cell;
use std::collections::VecDeque;

use crate::dialog_overlay::DialogOverlayComponent;
use crate::helpers::{color_to_ratatui, layout_rect_to_rect, safe_set_string};
use term_wm_core::actions::{ConfirmAction, EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext, Overlay};
use term_wm_core::layout::rect_contains;
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;

#[derive(Debug)]
pub struct ConfirmOverlayComponent {
    dialog: DialogOverlayComponent,
    visible: bool,
    body: String,
    selected_confirm: bool,
    cancel_rect: Cell<Option<Rect>>,
    confirm_rect: Cell<Option<Rect>>,
}

impl Default for ConfirmOverlayComponent {
    fn default() -> Self {
        Self {
            dialog: DialogOverlayComponent::new(),
            visible: false,
            body: String::new(),
            selected_confirm: false,
            cancel_rect: Cell::new(None),
            confirm_rect: Cell::new(None),
        }
    }
}

impl Component<TermWmAction> for ConfirmOverlayComponent {
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        if !self.visible || area.width == 0 || area.height == 0 {
            return;
        }
        let area = layout_rect_to_rect(area);
        let dialog_ctx = ctx.with_overlay(true).with_focus(true);
        let backend = crate::helpers::downcast_ratatui(backend);
        self.dialog.render(
            backend,
            LayoutRect {
                x: area.x as i32,
                y: area.y as i32,
                width: area.width,
                height: area.height,
            },
            &dialog_ctx,
            registry,
        );
        let rect = self.dialog.rect_for(area);
        if rect.width < 3 || rect.height < 3 {
            return;
        }
        self.cancel_rect.set(None);
        self.confirm_rect.set(None);
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
            .style(Style::default().fg(color_to_ratatui(ctx.config().theme.dialog_fg)))
            .wrap(Wrap { trim: true });
        paragraph.render(body_rect, &mut backend.buffer);
        let separator_style =
            Style::default().fg(color_to_ratatui(ctx.config().theme.dialog_separator));
        let backend = crate::helpers::downcast_ratatui(backend);
        let buffer = &mut backend.buffer;
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
            .fg(color_to_ratatui(ctx.config().theme.decorator_header_fg))
            .bg(color_to_ratatui(ctx.config().theme.decorator_header_bg))
            .add_modifier(Modifier::BOLD);
        let unselected_style = Style::default()
            .fg(color_to_ratatui(ctx.config().theme.dialog_fg))
            .bg(color_to_ratatui(ctx.config().theme.panel_bg));

        let (cancel_style, confirm_style) = if self.selected_confirm {
            (unselected_style, selected_style)
        } else {
            (selected_style, unselected_style)
        };
        let total_width = cancel.len() + 1 + confirm.len();
        let start_x = content
            .x
            .saturating_add(content.width.saturating_sub(total_width as u16));
        safe_set_string(buffer, bounds, start_x, button_y, cancel, cancel_style);
        let confirm_x = start_x.saturating_add(cancel.len() as u16 + 1);
        safe_set_string(buffer, bounds, confirm_x, button_y, confirm, confirm_style);
        self.cancel_rect.set(Some(Rect {
            x: start_x,
            y: button_y,
            width: cancel.len() as u16,
            height: 1,
        }));
        self.confirm_rect.set(Some(Rect {
            x: confirm_x,
            y: button_y,
            width: confirm.len() as u16,
            height: 1,
        }));
    }

    fn handle_events(
        &mut self,
        event: &Event,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        if self.handle_confirm_event(event).is_some() {
            return EventResult::Consumed;
        }
        let Event::Key(key) = event else {
            return EventResult::Ignored;
        };
        let kb = term_wm_core::keybindings::KeyBindings::default();
        if kb.matches(TermWmAction::ConfirmToggle, key)
            || kb.matches(TermWmAction::ConfirmLeft, key)
            || kb.matches(TermWmAction::ConfirmRight, key)
        {
            EventResult::Consumed
        } else {
            EventResult::Ignored
        }
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

impl Overlay<TermWmAction> for ConfirmOverlayComponent {
    fn visible(&self) -> bool {
        self.visible
    }
    fn handle_confirm_event(&mut self, event: &Event) -> Option<ConfirmAction> {
        self.handle_confirm_event(event)
    }
}

impl ConfirmOverlayComponent {
    pub fn new() -> Self {
        let mut dialog = DialogOverlayComponent::new();
        dialog.set_bg(term_wm_core::theme::NOIR.dialog_bg);
        dialog.set_auto_close_on_outside_click(false);
        Self {
            dialog,
            visible: false,
            body: String::new(),
            selected_confirm: false,
            cancel_rect: Cell::new(None),
            confirm_rect: Cell::new(None),
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
            Event::Mouse(mouse) if matches!(mouse.kind, MouseEventKind::Press(_)) => {
                if self.confirm_rect.get().is_some_and(|rect| {
                    let lr = LayoutRect {
                        x: rect.x as i32,
                        y: rect.y as i32,
                        width: rect.width,
                        height: rect.height,
                    };
                    rect_contains(lr, mouse.column, mouse.row)
                }) {
                    return Some(ConfirmAction::Confirm);
                }
                if self.cancel_rect.get().is_some_and(|rect| {
                    let lr = LayoutRect {
                        x: rect.x as i32,
                        y: rect.y as i32,
                        width: rect.width,
                        height: rect.height,
                    };
                    rect_contains(lr, mouse.column, mouse.row)
                }) {
                    return Some(ConfirmAction::Cancel);
                }
                None
            }
            Event::Key(key) => {
                let kb = term_wm_core::keybindings::KeyBindings::default();
                if kb.matches(TermWmAction::ConfirmToggle, key) {
                    self.selected_confirm = !self.selected_confirm;
                    None
                } else if kb.matches(TermWmAction::ConfirmLeft, key) {
                    self.selected_confirm = false;
                    None
                } else if kb.matches(TermWmAction::ConfirmRight, key) {
                    self.selected_confirm = true;
                    None
                } else if kb.matches(TermWmAction::ConfirmAccept, key) {
                    if self.selected_confirm {
                        Some(ConfirmAction::Confirm)
                    } else {
                        Some(ConfirmAction::Cancel)
                    }
                } else if kb.matches(TermWmAction::ConfirmCancel, key) {
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
    use term_wm_core::events::{
        Event, KeyCode, KeyEvent, KeyKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    fn ev_for(action: TermWmAction) -> Event {
        use term_wm_core::events::KeyEvent;
        use term_wm_core::keybindings::KeyBindings;
        if let Some(combo) = KeyBindings::default().first_combo(action) {
            Event::Key(KeyEvent::new(combo.code, combo.mods, KeyKind::Press))
        } else {
            // fallback: return an arbitrary key that should still be handled
            Event::Key(KeyEvent::new(
                term_wm_core::events::KeyCode::Esc,
                term_wm_core::events::KeyModifiers::NONE,
                KeyKind::Press,
            ))
        }
    }

    #[test]
    fn handle_event_recognizes_keys() {
        let mut o = ConfirmOverlayComponent::new();
        let ctx = ComponentContext::new(true);
        assert!(
            !o.handle_events(&ev_for(TermWmAction::ConfirmAccept), &ctx)
                .is_ignored()
        );
        assert!(
            !o.handle_events(&ev_for(TermWmAction::ConfirmAccept), &ctx)
                .is_ignored()
        );
        assert!(
            !o.handle_events(&ev_for(TermWmAction::ConfirmCancel), &ctx)
                .is_ignored()
        );
        assert!(
            !o.handle_events(&ev_for(TermWmAction::ConfirmToggle), &ctx)
                .is_ignored()
        );
    }

    #[test]
    fn handle_confirm_event_mouse_and_keys() {
        let mut o = ConfirmOverlayComponent::new();
        // set rects so mouse tests work
        o.confirm_rect.set(Some(ratatui::layout::Rect {
            x: 2,
            y: 3,
            width: 4,
            height: 1,
        }));
        o.cancel_rect.set(Some(ratatui::layout::Rect {
            x: 0,
            y: 3,
            width: 2,
            height: 1,
        }));

        let m = MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 3,
            row: 3,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            o.handle_confirm_event(&Event::Mouse(m)),
            Some(ConfirmAction::Confirm)
        );

        let m2 = MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
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
            o.handle_confirm_event(&Event::Key(KeyEvent::new(
                KeyCode::Tab,
                KeyModifiers::NONE,
                KeyKind::Press
            ))),
            None
        );
        assert!(!o.selected_confirm);

        // Enter uses selected_confirm to decide
        o.selected_confirm = true;
        assert_eq!(
            o.handle_confirm_event(&Event::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
                KeyKind::Press,
            ))),
            Some(ConfirmAction::Confirm)
        );
    }
}
