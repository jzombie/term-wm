use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

#[derive(Debug, Clone)]
pub struct DialogOverlay {
    title: String,
    body: String,
    visible: bool,
    width: u16,
    height: u16,
    bg: Color,
    dim_backdrop: bool,
}

impl DialogOverlay {
    pub fn new() -> Self {
        Self {
            title: "Dialog".to_string(),
            body: String::new(),
            visible: false,
            width: 70,
            height: 9,
            bg: Color::Black,
            dim_backdrop: false,
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    pub fn set_body(&mut self, body: impl Into<String>) {
        self.body = body.into();
    }

    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn set_size(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
    }

    pub fn set_bg(&mut self, bg: Color) {
        self.bg = bg;
    }

    pub fn set_dim_backdrop(&mut self, dim: bool) {
        self.dim_backdrop = dim;
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Clamp dialog size to the available area to avoid drawing outside the buffer
    /// when the terminal is smaller than the preferred minimums.
    pub fn rect_for(&self, area: Rect) -> Rect {
        let mut width = area.width.min(self.width).max(1);
        let mut height = area.height.min(self.height).max(1);
        if area.width >= 24 {
            width = width.max(24);
        }
        if area.height >= 5 {
            height = height.max(5);
        }
        let x = area.x.saturating_add(area.width.saturating_sub(width) / 2);
        let y = area
            .y
            .saturating_add(area.height.saturating_sub(height) / 2);
        Rect {
            x,
            y,
            width,
            height,
        }
    }
}

impl Default for DialogOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl super::Component for DialogOverlay {
    fn render(&mut self, frame: &mut Frame, area: Rect, _focused: bool) {
        if !self.visible || area.width == 0 || area.height == 0 {
            return;
        }
        if self.dim_backdrop {
            let buffer = frame.buffer_mut();
            let dim_style = Style::default().add_modifier(Modifier::DIM);
            for y in area.y..area.y.saturating_add(area.height) {
                for x in area.x..area.x.saturating_add(area.width) {
                    if let Some(cell) = buffer.cell_mut((x, y)) {
                        cell.set_style(dim_style);
                    }
                }
            }
        }
        let rect = self.rect_for(area);
        frame.render_widget(Clear, rect);
        let block = Block::default()
            .title(self.title.as_str())
            .borders(Borders::ALL);
        let paragraph = Paragraph::new(self.body.as_str())
            .style(Style::default().bg(self.bg))
            .block(block)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, rect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_for_clamps_sizes() {
        let dlg = DialogOverlay::new();
        // tiny area smaller than min width/height
        let area = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 2,
        };
        let r = dlg.rect_for(area);
        assert!(r.width >= 1);
        assert!(r.height >= 1);

        // larger area should enforce minimum preferred
        let area2 = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 10,
        };
        let r2 = dlg.rect_for(area2);
        assert!(r2.width >= 24);
        assert!(r2.height >= 5);
    }
}
