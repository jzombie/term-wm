use ratatui::style::Color;

// Centralized theme colors. Keep these as small helpers so we can
// map RGB to the terminal-supported color via `crate::colors` when
// appropriate.

pub const ACCENT_RGB: (u8, u8, u8) = (200, 100, 0);
pub const ACCENT_ALT_RGB: (u8, u8, u8) = (255, 165, 0);

pub fn rgb_to_color(rgb: (u8, u8, u8)) -> Color {
    crate::term_color::map_rgb_to_color(rgb.0, rgb.1, rgb.2)
}

pub fn accent() -> Color {
    rgb_to_color(ACCENT_RGB)
}

pub fn accent_alt() -> Color {
    rgb_to_color(ACCENT_ALT_RGB)
}

// Panel / menu
pub fn panel_bg() -> Color {
    Color::DarkGray
}
pub fn panel_fg() -> Color {
    Color::Black
}
pub fn panel_inactive_fg() -> Color {
    Color::DarkGray
}
pub fn panel_active_bg() -> Color {
    Color::Gray
}
pub fn panel_active_fg() -> Color {
    Color::Black
}

// Menu
pub fn menu_bg() -> Color {
    Color::DarkGray
}
pub fn menu_fg() -> Color {
    Color::White
}
pub fn menu_selected_bg() -> Color {
    Color::Gray
}
pub fn menu_selected_fg() -> Color {
    Color::Black
}

// Success / indicator
pub fn success_bg() -> Color {
    Color::Green
}
pub fn success_fg() -> Color {
    Color::Black
}

// Dialog / confirm
pub fn dialog_bg() -> Color {
    Color::Black
}
pub fn dialog_fg() -> Color {
    Color::White
}
pub fn dialog_separator() -> Color {
    Color::DarkGray
}

// Decorator
pub fn decorator_header_bg() -> Color {
    Color::Blue
}
pub fn decorator_header_fg() -> Color {
    Color::White
}
pub fn decorator_border() -> Color {
    Color::DarkGray
}

// Debug log highlight
pub fn debug_highlight() -> Color {
    // Use accent alt for a bright highlight
    accent_alt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn accent_returns_a_color_variant() {
        let a = accent();
        match a {
            Color::Rgb(_, _, _) | Color::Indexed(_) => {}
            _ => panic!("unexpected color variant"),
        }
    }

    #[test]
    fn accent_alt_returns_a_color_variant() {
        let a = accent_alt();
        match a {
            Color::Rgb(_, _, _) | Color::Indexed(_) => {}
            _ => panic!("unexpected color variant"),
        }
    }
}
