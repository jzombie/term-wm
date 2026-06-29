use ratatui::prelude::Rect;

use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::ui::UiFrame;

pub struct CenterComponent<C> {
    content: C,
    content_size: (u16, u16),
}

impl<C: Component> CenterComponent<C> {
    pub fn new(content: C, width: u16, height: u16) -> Self {
        Self {
            content,
            content_size: (width, height),
        }
    }

    fn inner_rect(&self, area: Rect) -> Rect {
        let width = self.content_size.0.min(area.width);
        let height = self.content_size.1.min(area.height);
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        Rect { x, y, width, height }
    }
}

impl<C: Component> Component for CenterComponent<C> {
    fn resize(&mut self, area: Rect, ctx: &ComponentContext) {
        let inner = self.inner_rect(area);
        self.content.resize(inner, ctx);
    }

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, ctx: &ComponentContext) {
        let inner = self.inner_rect(area);
        self.content.render(frame, inner, ctx);
    }

    fn handle_event(&mut self, event: &crossterm::event::Event, ctx: &ComponentContext) -> bool {
        self.content.handle_event(event, ctx)
    }
}
