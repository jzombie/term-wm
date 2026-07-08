use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use term_wm_layout_engine::LayoutRect;

pub fn layout_rect_to_rect(area: LayoutRect) -> Rect {
    Rect {
        x: area.x as u16,
        y: area.y as u16,
        width: area.width,
        height: area.height,
    }
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
