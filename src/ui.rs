use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{StatefulWidget, Widget};

/// Wrapper around `ratatui::Frame` that clamps drawing to the visible area.
///
/// Components render through this type so they can keep calling familiar
/// `render_widget` / `render_stateful_widget` helpers while automatically
/// clipping any rectangles that drift outside the buffer.
pub struct UiFrame<'a> {
    area: Rect,
    buffer: &'a mut Buffer,
}

impl<'a> UiFrame<'a> {
    pub fn new(frame: &'a mut Frame<'_>) -> Self {
        let area = frame.area();
        let buffer = frame.buffer_mut();
        Self { area, buffer }
    }

    pub fn area(&self) -> Rect {
        self.area
    }

    pub fn buffer_mut(&mut self) -> &mut Buffer {
        self.buffer
    }

    fn clip_rect(&self, rect: Rect) -> Option<Rect> {
        let clipped = rect.intersection(self.area);
        if clipped.width == 0 || clipped.height == 0 {
            None
        } else {
            Some(clipped)
        }
    }

    pub fn render_widget<W>(&mut self, widget: W, area: Rect)
    where
        W: Widget,
    {
        if let Some(clipped) = self.clip_rect(area) {
            widget.render(clipped, self.buffer);
        }
    }

    pub fn render_stateful_widget<W>(&mut self, widget: W, area: Rect, state: &mut W::State)
    where
        W: StatefulWidget,
    {
        if let Some(clipped) = self.clip_rect(area) {
            widget.render(clipped, self.buffer, state);
        }
    }
}

pub(crate) fn safe_set_string(
    buffer: &mut Buffer,
    bounds: Rect,
    x: u16,
    y: u16,
    text: &str,
    style: Style,
) {
    if bounds.width == 0 || bounds.height == 0 {
        return;
    }
    let max_x = bounds.x.saturating_add(bounds.width);
    let max_y = bounds.y.saturating_add(bounds.height);
    if x < bounds.x || x >= max_x || y < bounds.y || y >= max_y {
        return;
    }
    let available = max_x.saturating_sub(x);
    if available == 0 {
        return;
    }
    let text = truncate_to_width(text, available as usize);
    buffer.set_string(x, y, text, style);
}

pub(crate) fn truncate_to_width(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    value.chars().take(width).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::style::Style;

    #[test]
    fn truncate_to_width_short_and_long() {
        assert_eq!(truncate_to_width("abc", 5), "abc");
        assert_eq!(truncate_to_width("abcdef", 3), "abc");
    }

    #[test]
    fn safe_set_string_writes_within_bounds() {
        let bounds = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 2,
        };
        let mut buf = Buffer::empty(bounds);
        safe_set_string(&mut buf, bounds, 1, 0, "hello", Style::default());
        let cell = buf.cell_mut((1, 0)).expect("cell present");
        let first = cell.symbol().chars().next().unwrap();
        assert_eq!(first, 'h');

        // outside bounds should be ignored (no panic)
        safe_set_string(&mut buf, bounds, 100, 0, "x", Style::default());
    }
}
