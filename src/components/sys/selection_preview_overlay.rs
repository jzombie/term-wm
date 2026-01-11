use crossterm::event::Event;
use ratatui::layout::Rect;
use ratatui::text::Text;
use ratatui::widgets::{Block, Borders, Clear};

use crate::components::{
    Component, ComponentContext, DialogOverlayComponent, ScrollViewComponent, TextRendererComponent,
};
use crate::keybindings::{Action, KeyBindings};
use crate::ui::UiFrame;

#[derive(Debug)]
pub struct SelectionPreviewOverlayComponent {
    dialog: DialogOverlayComponent,
    visible: bool,
    viewer: ScrollViewComponent<TextRendererComponent>,
    area: Rect,
}

impl Component for SelectionPreviewOverlayComponent {
    fn resize(&mut self, area: Rect, _ctx: &ComponentContext) {
        self.area = area;
    }

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, _ctx: &ComponentContext) {
        self.area = area;
        if !self.visible || area.width == 0 || area.height == 0 {
            return;
        }
        self.dialog.render_backdrop(frame, area);
        let rect = self.dialog.rect_for(area);
        frame.render_widget(Clear, rect);
        let block = Block::default().title("Selection Preview").borders(Borders::ALL);
        let inner = Rect {
            x: rect.x.saturating_add(1),
            y: rect.y.saturating_add(1),
            width: rect.width.saturating_sub(2),
            height: rect.height.saturating_sub(2),
        };
        frame.render_widget(block, rect);
        let viewer_ctx = ComponentContext::new(true).with_overlay(true);
        self.viewer.render(frame, inner, &viewer_ctx);
    }

    fn handle_event(&mut self, event: &Event, _ctx: &ComponentContext) -> bool {
        if !self.visible {
            return false;
        }
        match event {
            Event::Key(key) => {
                let kb = KeyBindings::default();
                if kb.matches(Action::CloseHelp, key) {
                    self.close();
                    true
                } else {
                    let viewer_ctx = ComponentContext::new(true).with_overlay(true);
                    self.viewer.handle_event(event, &viewer_ctx)
                }
            }
            Event::Mouse(_) => {
                if self.dialog.handle_click_outside(event, self.area) {
                    self.close();
                    return true;
                }
                let viewer_ctx = ComponentContext::new(true).with_overlay(true);
                self.viewer.handle_event(event, &viewer_ctx)
            }
            _ => false,
        }
    }
}

impl SelectionPreviewOverlayComponent {
    pub fn new() -> Self {
        let mut overlay = Self {
            dialog: DialogOverlayComponent::new(),
            visible: false,
            viewer: ScrollViewComponent::new(TextRendererComponent::new()),
            area: Rect::default(),
        };
        overlay.dialog.set_size(70, 20);
        overlay.dialog.set_dim_backdrop(true);
        overlay.dialog.set_auto_close_on_outside_click(true);
        overlay.dialog.set_bg(crate::theme::dialog_bg());
        overlay.viewer.set_keyboard_enabled(true);
        overlay.viewer.content.set_selection_enabled(false);
        overlay
    }

    pub fn set_text(&mut self, text: String) {
        self.viewer.content.set_text(Text::from(text));
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.viewer.set_keyboard_enabled(true);
        self.dialog.set_visible(true);
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.viewer.set_keyboard_enabled(false);
        self.dialog.set_visible(false);
        self.viewer.content.reset();
    }

    pub fn visible(&self) -> bool {
        self.visible
    }
}

impl Default for SelectionPreviewOverlayComponent {
    fn default() -> Self {
        Self::new()
    }
}
