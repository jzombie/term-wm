use ratatui::prelude::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::ui::UiFrame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderAction {
    Minimize,
    Maximize,
    Close,
    Drag,
    None,
}

pub trait WindowDecorator: std::fmt::Debug {
    fn render_window(
        &self,
        frame: &mut UiFrame<'_>,
        rect: Rect,
        title: &str,
        focused: bool,
    );

    fn hit_test(&self, window_rect: Rect, x: u16, y: u16) -> HeaderAction;
}

#[derive(Debug)]
pub struct DefaultDecorator;

impl WindowDecorator for DefaultDecorator {
    fn hit_test(&self, rect: Rect, x: u16, y: u16) -> HeaderAction {
        let outer_left = rect.x;
        let outer_right = rect.x.saturating_add(rect.width).saturating_sub(1);
        let header_y = rect.y.saturating_add(1);

        // Check if inside header row
        if y != header_y {
            return HeaderAction::None;
        }
        // Check if within horizontal bounds
        if x <= outer_left || x >= outer_right {
            return HeaderAction::None;
        }

        let close_x = outer_right.saturating_sub(1);
        let max_x = close_x.saturating_sub(2);
        let min_x = max_x.saturating_sub(2);

        if x == close_x {
            HeaderAction::Close
        } else if x == max_x {
            HeaderAction::Maximize
        } else if x == min_x {
            HeaderAction::Minimize
        } else {
            HeaderAction::Drag
        }
    }

    fn render_window(
        &self,
        frame: &mut UiFrame<'_>,
        rect: Rect,
        title: &str,
        focused: bool,
    ) {
        let buffer = frame.buffer_mut();

        let focused_header_style = Style::default()
            .bg(crate::theme::decorator_header_bg())
            .fg(crate::theme::decorator_header_fg())
            .add_modifier(Modifier::BOLD);
        let normal_header_style = Style::default()
            .bg(crate::theme::panel_bg())
            .fg(crate::theme::decorator_header_fg());
        let border_style = Style::default()
            .fg(crate::theme::decorator_border())
            .bg(Color::Reset);

        let header_style = if focused {
            focused_header_style
        } else {
            normal_header_style
        };

        let outer_left = rect.x;
        let outer_top = rect.y;
        let outer_right = rect.x.saturating_add(rect.width).saturating_sub(1);
        let outer_bottom = rect.y.saturating_add(rect.height).saturating_sub(1);
        let header_y = rect.y.saturating_add(1);

        // Header Background & Title
        for x in outer_left.saturating_add(1)..outer_right {
            if let Some(cell) = buffer.cell_mut((x, header_y)) {
                cell.set_symbol(" ");
                cell.set_style(header_style);
            }
        }
        let title_len = title.len() as u16;
        let header_width = outer_right.saturating_sub(outer_left).saturating_sub(1);
        if title_len <= header_width {
            let start_x = outer_left + 1 + (header_width - title_len) / 2;
            for (idx, ch) in title.chars().enumerate() {
                let x = start_x + idx as u16;
                if let Some(cell) = buffer.cell_mut((x, header_y)) {
                    cell.set_symbol(&ch.to_string());
                    cell.set_style(header_style);
                }
            }
        }
        let close_x = outer_right.saturating_sub(1);
        let max_x = close_x.saturating_sub(2);
        let min_x = max_x.saturating_sub(2);
        let buttons = [(min_x, "_"), (max_x, "▢"), (close_x, "✖")];
        for (bx, sym) in buttons {
            if let Some(cell) = buffer.cell_mut((bx, header_y)) {
                cell.set_symbol(sym);
                cell.set_style(header_style);
            }
        }

        // Borders
        // Top
        for x in outer_left..=outer_right {
            if let Some(cell) = buffer.cell_mut((x, outer_top)) {
                if x == outer_left {
                    cell.set_symbol("┌");
                } else if x == outer_right {
                    cell.set_symbol("┐");
                } else {
                    cell.set_symbol("─");
                }
                cell.set_style(border_style);
            }
        }
        // Bottom
        for x in outer_left..=outer_right {
            if let Some(cell) = buffer.cell_mut((x, outer_bottom)) {
                if x == outer_left {
                    cell.set_symbol("└");
                } else if x == outer_right {
                    cell.set_symbol("┘");
                } else {
                    cell.set_symbol("─");
                }
                cell.set_style(border_style);
            }
        }
        // Left
        for y in outer_top.saturating_add(1)..outer_bottom {
            if let Some(cell) = buffer.cell_mut((outer_left, y)) {
                cell.set_symbol("│");
                cell.set_style(border_style);
            }
        }
        // Right
        for y in outer_top.saturating_add(1)..outer_bottom {
            if let Some(cell) = buffer.cell_mut((outer_right, y)) {
                cell.set_symbol("│");
                cell.set_style(border_style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_step_decorator_debug_format() {
        let dec = DefaultDecorator;
        let s = format!("{:?}", dec);
        assert!(s.contains("DefaultDecorator"));
    }

    #[test]
    fn hit_test_returns_expected_actions() {
        let dec = DefaultDecorator;
        let rect = Rect {
            x: 10,
            y: 5,
            width: 10,
            height: 6,
        };
        // header_y = y + 1 = 6
        let header_y = 6;

        // outside header row
        assert_eq!(dec.hit_test(rect, 11, 5), HeaderAction::None);

        // left/right edges
        assert_eq!(dec.hit_test(rect, 10, header_y), HeaderAction::None);
        assert_eq!(dec.hit_test(rect, 19, header_y), HeaderAction::None);

        // buttons: compute positions
        let outer_right = rect.x + rect.width - 1;
        let close_x = outer_right.saturating_sub(1);
        let max_x = close_x.saturating_sub(2);
        let min_x = max_x.saturating_sub(2);

        assert_eq!(dec.hit_test(rect, close_x, header_y), HeaderAction::Close);
        assert_eq!(dec.hit_test(rect, max_x, header_y), HeaderAction::Maximize);
        assert_eq!(dec.hit_test(rect, min_x, header_y), HeaderAction::Minimize);

        // middle area -> drag
        let mid = rect.x + rect.width / 2;
        assert_eq!(dec.hit_test(rect, mid, header_y), HeaderAction::Drag);
    }
}
