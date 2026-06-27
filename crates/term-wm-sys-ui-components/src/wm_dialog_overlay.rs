use crossterm::event::Event;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Clear};

use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::keybindings::{Action, KeyBindings};
use term_wm_core::ui::UiFrame;
use term_wm_ui_components::{DialogOverlayComponent, ScrollViewComponent};

#[derive(Debug)]
pub struct WmDialogOverlayComponent<C: Component> {
    dialog: DialogOverlayComponent,
    content: ScrollViewComponent<C>,
    area: Rect,
    keybindings: KeyBindings,
    close_action: Action,
}

impl<C: Component> WmDialogOverlayComponent<C> {
    pub fn new(
        content: ScrollViewComponent<C>,
        keybindings: KeyBindings,
        close_action: Action,
    ) -> Self {
        let mut dialog = DialogOverlayComponent::new();
        dialog.set_dim_backdrop(true);
        dialog.set_auto_close_on_outside_click(true);
        dialog.set_bg(term_wm_core::theme::dialog_bg());
        Self {
            dialog,
            content,
            area: Rect::default(),
            keybindings,
            close_action,
        }
    }

    pub fn show(&mut self) {
        self.dialog.set_visible(true);
        self.content.set_keyboard_enabled(true);
    }

    pub fn close(&mut self) {
        self.dialog.set_visible(false);
        self.content.set_keyboard_enabled(false);
    }

    pub fn visible(&self) -> bool {
        self.dialog.visible()
    }

    pub fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, title: &str) {
        self.area = area;
        if !self.visible() || area.width == 0 || area.height == 0 {
            return;
        }
        self.dialog.render_backdrop(frame, area, None);
        let rect = self.dialog.rect_for(area);
        frame.render_widget(Clear, rect);
        let block = Block::default().title(title).borders(Borders::ALL);
        let inner = Rect {
            x: rect.x.saturating_add(1),
            y: rect.y.saturating_add(1),
            width: rect.width.saturating_sub(2),
            height: rect.height.saturating_sub(2),
        };
        frame.render_widget(block, rect);
        let ctx = ComponentContext::new(true).with_overlay(true);
        self.content.resize(inner, &ctx);
        self.content.render(frame, inner, &ctx);
    }

    pub fn handle_event(&mut self, event: &Event, ctx: &ComponentContext) -> bool {
        if !self.visible() {
            return false;
        }
        match event {
            Event::Key(key) => {
                if self.keybindings.matches(self.close_action, key) {
                    self.close();
                    true
                } else {
                    self.content.handle_event(event, ctx)
                }
            }
            Event::Mouse(_) => {
                if self.dialog.handle_click_outside(event, self.area) {
                    self.close();
                    return true;
                }
                self.content.handle_event(event, ctx)
            }
            _ => false,
        }
    }

    pub fn set_selection_enabled(&mut self, enabled: bool) {
        self.content.content.set_selection_enabled(enabled);
    }

    pub fn dialog_mut(&mut self) -> &mut DialogOverlayComponent {
        &mut self.dialog
    }

    pub fn content_mut(&mut self) -> &mut ScrollViewComponent<C> {
        &mut self.content
    }
}
