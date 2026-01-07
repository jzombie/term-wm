use std::str;

use crossterm::event::{Event, KeyCode};
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Clear};

use crate::components::{Component, DialogOverlayComponent, MarkdownViewerComponent};
use crate::ui::UiFrame;

static HELP_MD: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/help.md"));

#[derive(Debug)]
pub struct HelpOverlayComponent {
    dialog: DialogOverlayComponent,
    visible: bool,
    viewer: MarkdownViewerComponent,
}

impl Component for HelpOverlayComponent {
    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, _focused: bool) {
        if !self.visible || area.width == 0 || area.height == 0 {
            return;
        }
        // If the dialog requests a dim backdrop, apply it across the full frame
        // before clearing and drawing the help dialog contents.
        self.dialog.render_backdrop(frame, area);
        let rect = self.dialog.rect_for(area);
        frame.render_widget(Clear, rect);
        let title = "About / Help";
        let block = Block::default().title(title).borders(Borders::ALL);
        let inner = Rect {
            x: rect.x.saturating_add(1),
            y: rect.y.saturating_add(1),
            width: rect.width.saturating_sub(2),
            height: rect.height.saturating_sub(2),
        };
        frame.render_widget(block, rect);
        self.viewer.render_content(frame, inner);
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        // need area to pass to viewer; approximate using dialog rect against full frame
        // The caller (WindowManager) routes events while overlay visible; here just return false
        // Actual routing is handled in WindowManager where available area is known.
        self.handle_help_event(event)
    }
}

impl HelpOverlayComponent {
    pub fn new() -> Self {
        let mut overlay = Self {
            dialog: DialogOverlayComponent::new(),
            visible: false,
            viewer: MarkdownViewerComponent::new(),
        };
        overlay.dialog.set_size(70, 20);
        overlay.dialog.set_dim_backdrop(true);
        overlay.dialog.set_bg(crate::theme::dialog_bg());
        // substitute package/version placeholders and set markdown
        if let Ok(raw) = str::from_utf8(HELP_MD) {
            let s = raw
                .replace("%PACKAGE%", env!("CARGO_PKG_NAME"))
                .replace("%VERSION%", env!("CARGO_PKG_VERSION"))
                .replace("%REPOSITORY%", env!("CARGO_PKG_REPOSITORY"));
            overlay.viewer.set_markdown(&s);
        }
        overlay
    }

    pub fn set_visible(&mut self, v: bool) {
        self.visible = v;
        // enable/disable keyboard handling for the viewer when visibility changes
        self.viewer.set_keyboard_enabled(v);
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn handle_help_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(key) => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                    self.visible = false;
                    true
                }
                _ => self.viewer.handle_key_event(key),
            },
            Event::Mouse(_) => self.viewer.handle_pointer_event(event),
            _ => false,
        }
    }

    /// Manually set keyboard handling for the underlying viewer.
    pub fn set_keyboard_enabled(&mut self, enabled: bool) {
        self.viewer.set_keyboard_enabled(enabled);
    }
}

impl Default for HelpOverlayComponent {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_constructs() {
        let h = HelpOverlayComponent::new();
        // should create without panic
        let _ = h;
    }
}
