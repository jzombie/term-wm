use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

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
