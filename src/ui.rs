//! UiFrame: a thin wrapper around `ratatui::Frame` that clamps drawing to the
//! visible area and centralizes clipping logic.
//!
//! Why this exists
//! - Components and widgets sometimes compute rectangles that drift partially or
//!   fully outside the terminal buffer. Writing out-of-bounds into the underlying
//!   `Buffer` can panic or corrupt rendering. `UiFrame` prevents that by
//!   clipping all draw calls to the visible area.
//!
//! Benefits
//! - Safety: components can call the familiar `render_widget` /
//!   `render_stateful_widget` helpers without needing to guard every draw with
//!   manual bounds checks.
//! - Simplicity: keeps widget code concise and focused on layout rather than
//!   buffer-safety details.
//! - Clear handling: by routing `Clear` widget rendering through `UiFrame`, we
//!   can safely clear regions without exposing a brittle `clear_rect` helper.
//!
//! Usage
//! - In paint closures, construct a `UiFrame` from a `ratatui::Frame` via
//!   `UiFrame::new(&mut frame)`. Use `frame.render_widget(...)` and
//!   `frame.render_stateful_widget(...)` as before. To clear an area, render the
//!   `Clear` widget through the `UiFrame`.
use crate::window::FloatRect;
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

    /// Construct a `UiFrame` directly from an area and buffer.
    ///
    /// This powers offscreen rendering paths where components should draw into
    /// their logical window size before being composited onto the visible
    /// terminal buffer.
    pub(crate) fn from_parts(area: Rect, buffer: &'a mut Buffer) -> Self {
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

    pub fn blit_from(&mut self, src: &Buffer, src_area: Rect) {
        let overlap = src_area.intersection(self.area);
        if overlap.width == 0 || overlap.height == 0 {
            return;
        }
        for y in overlap.y..overlap.y.saturating_add(overlap.height) {
            for x in overlap.x..overlap.x.saturating_add(overlap.width) {
                if let (Some(src_cell), Some(dst_cell)) =
                    (src.cell((x, y)), self.buffer.cell_mut((x, y)))
                {
                    *dst_cell = src_cell.clone();
                }
            }
        }
    }

    pub fn blit_from_signed(&mut self, src: &Buffer, dest: FloatRect) {
        let frame_x0 = self.area.x as i32;
        let frame_y0 = self.area.y as i32;
        let frame_x1 = frame_x0 + self.area.width as i32;
        let frame_y1 = frame_y0 + self.area.height as i32;
        for sy in 0..dest.height as i32 {
            let dy = dest.y + sy;
            if dy < frame_y0 || dy >= frame_y1 {
                continue;
            }
            for sx in 0..dest.width as i32 {
                let dx = dest.x + sx;
                if dx < frame_x0 || dx >= frame_x1 {
                    continue;
                }
                if let (Some(src_cell), Some(dst_cell)) = (
                    src.cell((sx as u16, sy as u16)),
                    self.buffer.cell_mut((dx as u16, dy as u16)),
                ) {
                    *dst_cell = src_cell.clone();
                }
            }
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
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    #[test]
    fn blit_from_signed_clips_negative_offsets() {
        let frame_area = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        };
        let mut dest = Buffer::empty(frame_area);
        let mut frame = UiFrame::from_parts(frame_area, &mut dest);
        let src_area = Rect {
            x: 0,
            y: 0,
            width: 3,
            height: 2,
        };
        let mut src = Buffer::empty(src_area);
        for y in 0..src_area.height {
            for x in 0..src_area.width {
                if let Some(cell) = src.cell_mut((x, y)) {
                    cell.set_symbol("#");
                }
            }
        }
        frame.blit_from_signed(
            &src,
            FloatRect {
                x: -1,
                y: 0,
                width: 3,
                height: 2,
            },
        );
        let buffer = frame.buffer;
        assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "#");
        assert_eq!(buffer.cell((1, 0)).unwrap().symbol(), "#");
        assert_eq!(buffer.cell((2, 0)).unwrap().symbol(), " ");
    }

    #[test]
    fn blit_from_signed_ignores_non_overlapping() {
        let frame_area = Rect {
            x: 0,
            y: 0,
            width: 3,
            height: 3,
        };
        let mut dest = Buffer::empty(frame_area);
        let mut frame = UiFrame::from_parts(frame_area, &mut dest);
        let src_area = Rect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        };
        let mut src = Buffer::empty(src_area);
        for y in 0..src_area.height {
            for x in 0..src_area.width {
                if let Some(cell) = src.cell_mut((x, y)) {
                    cell.set_symbol("#");
                }
            }
        }
        frame.blit_from_signed(
            &src,
            FloatRect {
                x: -5,
                y: -5,
                width: 2,
                height: 2,
            },
        );
        let buffer = frame.buffer;
        for y in 0..frame_area.height {
            for x in 0..frame_area.width {
                assert_eq!(buffer.cell((x, y)).unwrap().symbol(), " ");
            }
        }
    }

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

    #[test]
    fn render_widget_clips_to_frame_area() {
        use ratatui::layout::Rect;

        let area = Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 3,
        };
        let mut buf = Buffer::empty(area);
        let mut ui = UiFrame::from_parts(area, &mut buf);

        struct FillWidget;
        impl Widget for FillWidget {
            fn render(self, area: Rect, buf: &mut Buffer) {
                for y in area.y..area.y.saturating_add(area.height) {
                    for x in area.x..area.x.saturating_add(area.width) {
                        if let Some(cell) = buf.cell_mut((x, y)) {
                            cell.set_symbol("A");
                        }
                    }
                }
            }
        }

        // Request an area that partially lies outside the right edge.
        ui.render_widget(
            FillWidget,
            Rect {
                x: 3,
                y: 1,
                width: 5,
                height: 2,
            },
        );

        // Inside clipped region
        let inside = buf.cell_mut((3, 1)).expect("cell present");
        assert!(inside.symbol().starts_with('A'));

        // Outside clipped region (left of the filled area)
        let outside = buf.cell_mut((2, 1)).expect("cell present");
        assert!(!outside.symbol().starts_with('A'));
    }

    #[test]
    fn render_stateful_widget_clips_to_frame_area() {
        use ratatui::layout::Rect;

        let area = Rect {
            x: 0,
            y: 0,
            width: 6,
            height: 4,
        };
        let mut buf = Buffer::empty(area);
        let mut ui = UiFrame::from_parts(area, &mut buf);

        struct FillStateful;
        impl StatefulWidget for FillStateful {
            type State = usize;
            fn render(self, area: Rect, buf: &mut Buffer, _state: &mut Self::State) {
                for y in area.y..area.y.saturating_add(area.height) {
                    for x in area.x..area.x.saturating_add(area.width) {
                        if let Some(cell) = buf.cell_mut((x, y)) {
                            cell.set_symbol("S");
                        }
                    }
                }
            }
        }

        // Request an area that exceeds bottom edge.
        let mut state = 0usize;
        ui.render_stateful_widget(
            FillStateful,
            Rect {
                x: 1,
                y: 2,
                width: 4,
                height: 4,
            },
            &mut state,
        );

        // Inside clipped region
        let inside = buf.cell_mut((1, 2)).expect("cell present");
        assert!(inside.symbol().starts_with('S'));

        // Outside clipped region (below buffer)
        // Coordinates (1, 6) are outside; ensure we don't panic by checking a nearby in-bounds cell
        let near = buf.cell_mut((1, 3)).expect("cell present");
        assert!(near.symbol().starts_with('S'));
    }

    #[test]
    fn blit_from_copies_overlapping_region() {
        use ratatui::layout::Rect;

        let dest_area = Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 3,
        };
        let mut dest = Buffer::empty(dest_area);
        for y in dest_area.y..dest_area.y.saturating_add(dest_area.height) {
            for x in dest_area.x..dest_area.x.saturating_add(dest_area.width) {
                if let Some(cell) = dest.cell_mut((x, y)) {
                    cell.set_symbol(".");
                }
            }
        }
        let mut frame = UiFrame::from_parts(dest_area, &mut dest);

        let src_area = Rect {
            x: 3,
            y: 1,
            width: 4,
            height: 3,
        };
        let mut src = Buffer::empty(src_area);
        for y in src_area.y..src_area.y.saturating_add(src_area.height) {
            for x in src_area.x..src_area.x.saturating_add(src_area.width) {
                if let Some(cell) = src.cell_mut((x, y)) {
                    cell.set_symbol("Z");
                }
            }
        }

        frame.blit_from(&src, src_area);

        let buffer = frame.buffer_mut();
        assert_eq!(buffer.cell((3, 1)).unwrap().symbol(), "Z");
        assert_eq!(buffer.cell((4, 2)).unwrap().symbol(), "Z");
        assert_eq!(buffer.cell((2, 1)).unwrap().symbol(), ".");
        assert_eq!(buffer.cell((4, 0)).unwrap().symbol(), ".");
    }

    #[test]
    fn blit_from_respects_non_zero_origins() {
        use ratatui::layout::Rect;

        let dest_area = Rect {
            x: 5,
            y: 5,
            width: 4,
            height: 2,
        };
        let mut dest = Buffer::empty(dest_area);
        for y in dest_area.y..dest_area.y.saturating_add(dest_area.height) {
            for x in dest_area.x..dest_area.x.saturating_add(dest_area.width) {
                if let Some(cell) = dest.cell_mut((x, y)) {
                    cell.set_symbol(".");
                }
            }
        }
        let mut frame = UiFrame::from_parts(dest_area, &mut dest);

        let src_area = Rect {
            x: 6,
            y: 6,
            width: 2,
            height: 1,
        };
        let mut src = Buffer::empty(src_area);
        for y in src_area.y..src_area.y.saturating_add(src_area.height) {
            for x in src_area.x..src_area.x.saturating_add(src_area.width) {
                if let Some(cell) = src.cell_mut((x, y)) {
                    cell.set_symbol("Q");
                }
            }
        }

        frame.blit_from(&src, src_area);

        let buffer = frame.buffer_mut();
        assert_eq!(buffer.cell((6, 6)).unwrap().symbol(), "Q");
        assert_eq!(buffer.cell((7, 6)).unwrap().symbol(), "Q");
        assert_eq!(buffer.cell((5, 5)).unwrap().symbol(), ".");
        assert_eq!(buffer.cell((8, 6)).unwrap().symbol(), ".");
    }
}
