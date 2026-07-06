use std::collections::VecDeque;

use crossterm::event::Event;
use ratatui::prelude::Rect;

use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::ui::UiFrame;
use term_wm_core::window::WindowKey;

pub struct CenterComponent<C> {
    content: C,
    content_size: (u16, u16),
}

impl<C: Component<TermWmAction>> CenterComponent<C> {
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
        Rect {
            x,
            y,
            width,
            height,
        }
    }
}

impl<C: Component<TermWmAction>> Component<TermWmAction> for CenterComponent<C> {
    fn render(
        &self,
        frame: &mut UiFrame<'_>,
        area: Rect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        let inner = self.inner_rect(area);
        self.content.render(frame, inner, ctx, registry);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        self.content.handle_events(event, ctx)
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        self.content.update(action, ctx, actions);
    }

    fn destroy(&mut self) {
        self.content.destroy();
    }
}
