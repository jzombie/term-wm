use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use term_wm_layout_engine::LayoutRect;

/// Safely converts a signed LayoutRect into an unsigned Ratatui Rect for rendering.
/// If the LayoutRect is partially off-screen (negative x/y), it crops the
/// invisible portion and shrinks width/height to fit the screen.
pub fn layout_rect_to_clipped_rect(area: LayoutRect) -> Rect {
    let x = area.x.max(0) as u16;
    let y = area.y.max(0) as u16;
    let crop_left = area.x.min(0).unsigned_abs() as u16;
    let crop_top = area.y.min(0).unsigned_abs() as u16;
    Rect {
        x,
        y,
        width: area.width.saturating_sub(crop_left),
        height: area.height.saturating_sub(crop_top),
    }
}

/// Translates a global screen coordinate into a component-local coordinate.
/// Returns None if the coordinate falls outside the component's bounds.
/// Use this for click targets, link hits, and other "reject if outside" cases.
pub fn localize_coordinate(
    area: LayoutRect,
    global_col: u16,
    global_row: u16,
) -> Option<(u16, u16)> {
    let g_col = i32::from(global_col);
    let g_row = i32::from(global_row);
    let max_x = area.x.saturating_add(i32::from(area.width));
    let max_y = area.y.saturating_add(i32::from(area.height));
    if g_col < area.x || g_col >= max_x || g_row < area.y || g_row >= max_y {
        return None;
    }
    Some(((g_col - area.x) as u16, (g_row - area.y) as u16))
}

/// Translates a global screen coordinate into a component-local coordinate,
/// clamping to the nearest edge if it falls outside bounds.
/// Use this for text selection dragging or scrollbar dragging where
/// dragging past the edge means "select to the edge".
pub fn localize_coordinate_clamped(
    area: LayoutRect,
    global_col: u16,
    global_row: u16,
) -> Option<(u16, u16)> {
    if area.width == 0 || area.height == 0 {
        return None;
    }
    let g_col = i32::from(global_col);
    let g_row = i32::from(global_row);
    let max_x = area.x.saturating_add(i32::from(area.width)).saturating_sub(1);
    let max_y = area.y.saturating_add(i32::from(area.height)).saturating_sub(1);
    let clamped_col = g_col.clamp(area.x, max_x);
    let clamped_row = g_row.clamp(area.y, max_y);
    Some(((clamped_col - area.x) as u16, (clamped_row - area.y) as u16))
}

pub fn color_to_ratatui(c: term_wm_core::theme::Color) -> Color {
    match c {
        term_wm_core::theme::Color::Black => Color::Black,
        term_wm_core::theme::Color::Red => Color::Red,
        term_wm_core::theme::Color::Green => Color::Green,
        term_wm_core::theme::Color::Yellow => Color::Yellow,
        term_wm_core::theme::Color::Blue => Color::Blue,
        term_wm_core::theme::Color::Magenta => Color::Magenta,
        term_wm_core::theme::Color::Cyan => Color::Cyan,
        term_wm_core::theme::Color::White => Color::White,
        term_wm_core::theme::Color::Gray => Color::Gray,
        term_wm_core::theme::Color::DarkGray => Color::DarkGray,
        term_wm_core::theme::Color::LightRed => Color::LightRed,
        term_wm_core::theme::Color::LightGreen => Color::LightGreen,
        term_wm_core::theme::Color::LightYellow => Color::LightYellow,
        term_wm_core::theme::Color::LightBlue => Color::LightBlue,
        term_wm_core::theme::Color::LightMagenta => Color::LightMagenta,
        term_wm_core::theme::Color::LightCyan => Color::LightCyan,
        term_wm_core::theme::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
        term_wm_core::theme::Color::Indexed(i) => Color::Indexed(i),
    }
}

pub fn safe_set_string(
    buffer: &mut Buffer,
    bounds: Rect,
    x: u16,
    y: u16,
    text: &str,
    style: Style,
) {
    if y < bounds.y || y >= bounds.y.saturating_add(bounds.height) {
        return;
    }
    let mut col = x;
    for ch in text.chars() {
        if col >= bounds.x.saturating_add(bounds.width) {
            break;
        }
        if let Some(cell) = buffer.cell_mut((col, y)) {
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf);
            cell.set_symbol(s).set_style(style);
        }
        col = col.saturating_add(1);
    }
}

pub fn decorate_link_style(mut style: Style, theme: &term_wm_core::theme::Theme) -> Style {
    if theme.link_underline {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style.fg(color_to_ratatui(theme.link_color))
}

pub fn downcast_ratatui(
    backend: &mut dyn term_wm_render::RenderBackend,
) -> &mut term_wm_console::RatatuiBackend {
    backend
        .as_any_mut()
        .downcast_mut::<term_wm_console::RatatuiBackend>()
        .expect("expected RatatuiBackend")
}

pub fn map_rgb_to_ratatui(r: u8, g: u8, b: u8) -> Color {
    color_to_ratatui(term_wm_core::term_color::map_rgb_to_color(r, g, b))
}

pub fn style_to_role(
    style: &Style,
    _theme: &term_wm_core::theme::Theme,
) -> term_wm_core::theme::SemanticRole {
    use ratatui::style::Color as RColor;
    if style.add_modifier.intersects(Modifier::BOLD) {
        return term_wm_core::theme::SemanticRole::Bold;
    }
    if style.add_modifier.intersects(Modifier::ITALIC) {
        return term_wm_core::theme::SemanticRole::Italic;
    }
    if style.add_modifier.intersects(Modifier::UNDERLINED) {
        return term_wm_core::theme::SemanticRole::Underlined;
    }
    if style.add_modifier.intersects(Modifier::DIM) {
        return term_wm_core::theme::SemanticRole::Dimmed;
    }
    match style.fg {
        Some(RColor::Yellow) => term_wm_core::theme::SemanticRole::Warning,
        Some(RColor::Red) => term_wm_core::theme::SemanticRole::Error,
        _ => term_wm_core::theme::SemanticRole::Normal,
    }
}

pub fn role_to_style(
    role: term_wm_core::theme::SemanticRole,
    theme: &term_wm_core::theme::Theme,
) -> Style {
    use ratatui::style::Color as RColor;
    match role {
        term_wm_core::theme::SemanticRole::Bold => Style::default().add_modifier(Modifier::BOLD),
        term_wm_core::theme::SemanticRole::Italic => {
            Style::default().add_modifier(Modifier::ITALIC)
        }
        term_wm_core::theme::SemanticRole::Underlined => {
            Style::default().add_modifier(Modifier::UNDERLINED)
        }
        term_wm_core::theme::SemanticRole::Dimmed => Style::default().add_modifier(Modifier::DIM),
        term_wm_core::theme::SemanticRole::Warning => Style::default().fg(RColor::Yellow),
        term_wm_core::theme::SemanticRole::Error => Style::default().fg(RColor::Red),
        term_wm_core::theme::SemanticRole::Link => decorate_link_style(Style::default(), theme),
        term_wm_core::theme::SemanticRole::Highlight => {
            Style::default().fg(color_to_ratatui(theme.menu_selected_fg))
        }
        term_wm_core::theme::SemanticRole::Success => Style::default().fg(RColor::Green),
        term_wm_core::theme::SemanticRole::Muted | term_wm_core::theme::SemanticRole::Disabled => {
            Style::default().add_modifier(Modifier::DIM)
        }
        term_wm_core::theme::SemanticRole::Normal => Style::default(),
    }
}

pub fn linkified_to_text(
    linkified: term_wm_core::utils::linkifier::LinkifiedText,
    theme: &term_wm_core::theme::Theme,
) -> (
    ratatui::text::Text<'static>,
    term_wm_core::utils::linkifier::LinkMap,
) {
    let lines: Vec<ratatui::text::Line<'static>> = linkified
        .styled_lines
        .into_iter()
        .zip(linkified.link_map.iter())
        .map(|(spans, links)| {
            let ratatui_spans: Vec<ratatui::text::Span<'static>> = spans
                .into_iter()
                .enumerate()
                .map(|(i, (text, role))| {
                    let mut style = role_to_style(role, theme);
                    // Apply link styling if this span is a hyperlink
                    if links.get(i).and_then(|l| l.as_ref()).is_some() {
                        style = decorate_link_style(style, theme);
                    }
                    ratatui::text::Span::styled(text, style)
                })
                .collect();
            ratatui::text::Line::from(ratatui_spans)
        })
        .collect();
    (ratatui::text::Text::from(lines), linkified.link_map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use term_wm_core::theme::{Color as TColor, NOIR, SemanticRole, Theme};
    use term_wm_layout_engine::LayoutRect;

    #[test]
    fn layout_rect_to_clipped_rect_on_screen() {
        let lr = LayoutRect {
            x: 5,
            y: 10,
            width: 20,
            height: 15,
        };
        let r = layout_rect_to_clipped_rect(lr);
        assert_eq!(r.x, 5);
        assert_eq!(r.y, 10);
        assert_eq!(r.width, 20);
        assert_eq!(r.height, 15);
    }

    #[test]
    fn layout_rect_to_clipped_rect_off_screen() {
        let lr = LayoutRect {
            x: -10,
            y: -5,
            width: 80,
            height: 40,
        };
        let r = layout_rect_to_clipped_rect(lr);
        assert_eq!(r.x, 0);
        assert_eq!(r.y, 0);
        assert_eq!(r.width, 70);
        assert_eq!(r.height, 35);
    }

    #[test]
    fn localize_coordinate_inside() {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let result = localize_coordinate(area, 3, 5);
        assert_eq!(result, Some((3, 5)));
    }

    #[test]
    fn localize_coordinate_outside() {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        assert_eq!(localize_coordinate(area, 15, 5), None);
        assert_eq!(localize_coordinate(area, 5, 15), None);
    }

    #[test]
    fn localize_coordinate_negative_offset() {
        let area = LayoutRect {
            x: -10,
            y: -5,
            width: 80,
            height: 40,
        };
        let result = localize_coordinate(area, 0, 0);
        assert_eq!(result, Some((10, 5)));
    }

    #[test]
    fn localize_coordinate_clamped_outside() {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let result = localize_coordinate_clamped(area, 15, 5);
        assert_eq!(result, Some((9, 5)));
    }

    #[test]
    fn localize_coordinate_clamped_inside() {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let result = localize_coordinate_clamped(area, 3, 5);
        assert_eq!(result, Some((3, 5)));
    }

    #[test]
    fn localize_coordinate_clamped_zero_area() {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        assert_eq!(localize_coordinate_clamped(area, 5, 5), None);
    }

    #[test]
    fn color_to_ratatui_all_variants() {
        assert_eq!(color_to_ratatui(TColor::Black), Color::Black);
        assert_eq!(color_to_ratatui(TColor::Red), Color::Red);
        assert_eq!(color_to_ratatui(TColor::Green), Color::Green);
        assert_eq!(color_to_ratatui(TColor::Yellow), Color::Yellow);
        assert_eq!(color_to_ratatui(TColor::Blue), Color::Blue);
        assert_eq!(color_to_ratatui(TColor::Magenta), Color::Magenta);
        assert_eq!(color_to_ratatui(TColor::Cyan), Color::Cyan);
        assert_eq!(color_to_ratatui(TColor::White), Color::White);
        assert_eq!(color_to_ratatui(TColor::Gray), Color::Gray);
        assert_eq!(color_to_ratatui(TColor::DarkGray), Color::DarkGray);
        assert_eq!(color_to_ratatui(TColor::LightRed), Color::LightRed);
        assert_eq!(color_to_ratatui(TColor::LightGreen), Color::LightGreen);
        assert_eq!(color_to_ratatui(TColor::LightYellow), Color::LightYellow);
        assert_eq!(color_to_ratatui(TColor::LightBlue), Color::LightBlue);
        assert_eq!(color_to_ratatui(TColor::LightMagenta), Color::LightMagenta);
        assert_eq!(color_to_ratatui(TColor::LightCyan), Color::LightCyan);
        assert_eq!(color_to_ratatui(TColor::Rgb(1, 2, 3)), Color::Rgb(1, 2, 3));
        assert_eq!(color_to_ratatui(TColor::Indexed(42)), Color::Indexed(42));
    }

    #[test]
    fn safe_set_string_writes_within_bounds() {
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        let bounds = area;
        safe_set_string(&mut buf, bounds, 2, 1, "hello", Style::default());
        assert_eq!(buf[(2, 1)].symbol(), "h");
        assert_eq!(buf[(6, 1)].symbol(), "o");
        //超出 bounds should not panic
        safe_set_string(&mut buf, bounds, 8, 1, "xyz", Style::default());
        assert_eq!(buf[(8, 1)].symbol(), "x");
    }

    #[test]
    fn safe_set_string_skips_out_of_bounds_y() {
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        safe_set_string(&mut buf, area, 0, 5, "hello", Style::default());
        // should not write anything
        assert_eq!(buf[(0, 0)].symbol(), " ");
    }

    #[test]
    fn safe_set_string_wide_chars() {
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        safe_set_string(&mut buf, area, 0, 0, "AB", Style::default());
        assert_eq!(buf[(0, 0)].symbol(), "A");
        assert_eq!(buf[(1, 0)].symbol(), "B");
    }

    #[test]
    fn decorate_link_style_with_underline() {
        let theme = Theme {
            link_underline: true,
            link_color: TColor::Cyan,
            ..NOIR
        };
        let style = Style::default();
        let decorated = decorate_link_style(style, &theme);
        assert!(decorated.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(decorated.fg, Some(Color::Cyan));
    }

    #[test]
    fn decorate_link_style_without_underline() {
        let theme = Theme {
            link_underline: false,
            link_color: TColor::Blue,
            ..NOIR
        };
        let style = Style::default();
        let decorated = decorate_link_style(style, &theme);
        assert!(!decorated.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(decorated.fg, Some(Color::Blue));
    }

    #[test]
    fn style_to_role_bold() {
        let style = Style::default().add_modifier(Modifier::BOLD);
        let role = style_to_role(&style, &NOIR);
        assert_eq!(role, SemanticRole::Bold);
    }

    #[test]
    fn style_to_role_italic() {
        let style = Style::default().add_modifier(Modifier::ITALIC);
        let role = style_to_role(&style, &NOIR);
        assert_eq!(role, SemanticRole::Italic);
    }

    #[test]
    fn style_to_role_underlined() {
        let style = Style::default().add_modifier(Modifier::UNDERLINED);
        let role = style_to_role(&style, &NOIR);
        assert_eq!(role, SemanticRole::Underlined);
    }

    #[test]
    fn style_to_role_dim() {
        let style = Style::default().add_modifier(Modifier::DIM);
        let role = style_to_role(&style, &NOIR);
        assert_eq!(role, SemanticRole::Dimmed);
    }

    #[test]
    fn style_to_role_yellow_is_warning() {
        let style = Style::default().fg(Color::Yellow);
        let role = style_to_role(&style, &NOIR);
        assert_eq!(role, SemanticRole::Warning);
    }

    #[test]
    fn style_to_role_red_is_error() {
        let style = Style::default().fg(Color::Red);
        let role = style_to_role(&style, &NOIR);
        assert_eq!(role, SemanticRole::Error);
    }

    #[test]
    fn style_to_role_plain_is_normal() {
        let style = Style::default();
        let role = style_to_role(&style, &NOIR);
        assert_eq!(role, SemanticRole::Normal);
    }

    #[test]
    fn role_to_style_all_roles() {
        let theme = &NOIR;
        let _ = role_to_style(SemanticRole::Bold, theme);
        let _ = role_to_style(SemanticRole::Italic, theme);
        let _ = role_to_style(SemanticRole::Underlined, theme);
        let _ = role_to_style(SemanticRole::Dimmed, theme);
        let _ = role_to_style(SemanticRole::Warning, theme);
        let _ = role_to_style(SemanticRole::Error, theme);
        let _ = role_to_style(SemanticRole::Link, theme);
        let _ = role_to_style(SemanticRole::Highlight, theme);
        let _ = role_to_style(SemanticRole::Success, theme);
        let _ = role_to_style(SemanticRole::Muted, theme);
        let _ = role_to_style(SemanticRole::Disabled, theme);
        let _ = role_to_style(SemanticRole::Normal, theme);
    }

    #[test]
    fn linkified_to_text_converts() {
        use term_wm_core::utils::linkifier::{LinkMap, LinkifiedText};
        let linkified = LinkifiedText {
            lines: vec!["hello".to_string()],
            styled_lines: vec![vec![("hello".to_string(), SemanticRole::Normal)]],
            link_map: LinkMap::from(vec![vec![Some("https://example.com".to_string())]]),
        };
        let (text, link_map) = linkified_to_text(linkified, &NOIR);
        assert_eq!(text.lines.len(), 1);
        assert_eq!(text.lines[0].spans.len(), 1);
        assert_eq!(text.lines[0].spans[0].content, "hello");
        assert!(!link_map.is_empty());
    }

    #[test]
    fn map_rgb_to_ratatui_returns_color() {
        let color = map_rgb_to_ratatui(255, 0, 0);
        // Should return either Rgb or Indexed depending on COLORTERM
        match color {
            Color::Rgb(r, g, b) => {
                assert_eq!((r, g, b), (255, 0, 0));
            }
            Color::Indexed(_) => {} // acceptable fallback
            _ => panic!("unexpected color type"),
        }
    }
}
