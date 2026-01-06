use ratatui::Frame;
use ratatui::prelude::Rect;
use ratatui::style::{Color, Modifier, Style};

pub trait WindowDecorator: std::fmt::Debug {
    fn render_window(
        &self,
        frame: &mut Frame,
        rect: Rect,
        bounds: Rect,
        title: &str,
        focused: bool,
        is_obscured: &dyn Fn(u16, u16) -> bool,
    );
}

#[derive(Debug)]
pub struct OpenStepDecorator;

impl WindowDecorator for OpenStepDecorator {
    fn render_window(
        &self,
        frame: &mut Frame,
        rect: Rect,
        bounds: Rect,
        title: &str,
        focused: bool,
        is_obscured: &dyn Fn(u16, u16) -> bool,
    ) {
        let buffer = frame.buffer_mut();

        let focused_header_style = Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);
        let normal_header_style = Style::default().bg(Color::DarkGray).fg(Color::White);
        let border_style = Style::default().fg(Color::DarkGray).bg(Color::Reset);

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
        if header_y >= bounds.y && header_y < bounds.y + bounds.height {
            for x in outer_left.saturating_add(1)..outer_right {
                if x >= bounds.x
                    && x < bounds.x + bounds.width
                    && !is_obscured(x, header_y)
                    && let Some(cell) = buffer.cell_mut((x, header_y))
                {
                    cell.set_symbol(" ");
                    cell.set_style(header_style);
                }
            }
            let title_len = title.len() as u16;
            let header_width = outer_right.saturating_sub(outer_left).saturating_sub(1);
            if title_len <= header_width {
                let start_x = outer_left + 1 + (header_width - title_len) / 2;
                if start_x >= bounds.x && start_x + title_len <= bounds.x + bounds.width {
                    for (idx, ch) in title.chars().enumerate() {
                        let x = start_x + idx as u16;
                        if x < bounds.x + bounds.width
                            && !is_obscured(x, header_y)
                            && let Some(cell) = buffer.cell_mut((x, header_y))
                        {
                            cell.set_symbol(&ch.to_string());
                            cell.set_style(header_style);
                        }
                    }
                }
            }
        }

        // Borders
        // Top
        if outer_top >= bounds.y && outer_top < bounds.y + bounds.height {
            for x in outer_left..=outer_right {
                if x >= bounds.x
                    && x < bounds.x + bounds.width
                    && !is_obscured(x, outer_top)
                    && let Some(cell) = buffer.cell_mut((x, outer_top))
                {
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
        }
        // Bottom
        if outer_bottom >= bounds.y && outer_bottom < bounds.y + bounds.height {
            for x in outer_left..=outer_right {
                if x >= bounds.x
                    && x < bounds.x + bounds.width
                    && !is_obscured(x, outer_bottom)
                    && let Some(cell) = buffer.cell_mut((x, outer_bottom))
                {
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
        }
        // Left
        if outer_left >= bounds.x && outer_left < bounds.x + bounds.width {
            for y in outer_top.saturating_add(1)..outer_bottom {
                if y >= bounds.y
                    && y < bounds.y + bounds.height
                    && !is_obscured(outer_left, y)
                    && let Some(cell) = buffer.cell_mut((outer_left, y))
                {
                    cell.set_symbol("│");
                    cell.set_style(border_style);
                }
            }
        }
        // Right
        if outer_right >= bounds.x && outer_right < bounds.x + bounds.width {
            for y in outer_top.saturating_add(1)..outer_bottom {
                if y >= bounds.y
                    && y < bounds.y + bounds.height
                    && !is_obscured(outer_right, y)
                    && let Some(cell) = buffer.cell_mut((outer_right, y))
                {
                    cell.set_symbol("│");
                    cell.set_style(border_style);
                }
            }
        }
    }
}
