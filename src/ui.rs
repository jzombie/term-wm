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
