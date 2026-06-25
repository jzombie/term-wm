use ratatui::style::Color;

use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Theme struct
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    pub background: Color,
    pub surface: Color,
    pub panel_bg: Color,
    pub panel_fg: Color,
    pub panel_inactive_fg: Color,
    pub panel_active_bg: Color,
    pub panel_active_fg: Color,
    pub text: Color,
    pub text_muted: Color,
    pub text_disabled: Color,
    pub accent: Color,
    pub accent_alt: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub decorator_header_bg: Color,
    pub decorator_header_fg: Color,
    pub decorator_border: Color,
    pub decorator_border_active: Color,
    pub menu_bg: Color,
    pub menu_fg: Color,
    pub menu_selected_bg: Color,
    pub menu_selected_fg: Color,
    pub bottom_panel_bg: Color,
    pub bottom_panel_fg: Color,
    pub dialog_bg: Color,
    pub dialog_fg: Color,
    pub dialog_separator: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,
    pub link_color: Color,
    pub link_underline: bool,
    pub debug_highlight: Color,
}

// ---------------------------------------------------------------------------
// NoirCast-inspired dark theme
// ---------------------------------------------------------------------------

pub const NOIR: Theme = Theme {
    name: "noir",
    // Core surfaces
    background: Color::Rgb(10, 10, 15),
    surface: Color::Rgb(20, 22, 30),
    panel_bg: Color::Rgb(30, 32, 42),
    panel_fg: Color::Rgb(225, 225, 235),
    panel_inactive_fg: Color::Rgb(140, 142, 152),
    panel_active_bg: Color::Rgb(48, 50, 65),
    panel_active_fg: Color::Rgb(225, 225, 235),
    // Text hierarchy
    text: Color::Rgb(225, 225, 235),
    text_muted: Color::Rgb(140, 142, 152),
    text_disabled: Color::Rgb(90, 92, 102),
    // Semantic accents
    accent: Color::Rgb(0, 230, 118),
    accent_alt: Color::Rgb(255, 168, 0),
    success: Color::Rgb(0, 200, 83),
    warning: Color::Rgb(255, 193, 7),
    error: Color::Rgb(255, 61, 61),
    // Chrome
    decorator_header_bg: Color::Rgb(38, 42, 58),
    decorator_header_fg: Color::Rgb(225, 225, 235),
    decorator_border: Color::Rgb(105, 110, 125),
    decorator_border_active: Color::Rgb(110, 118, 140),
    // Menu
    menu_bg: Color::Rgb(25, 28, 40),
    menu_fg: Color::Rgb(225, 225, 235),
    menu_selected_bg: Color::Rgb(0, 230, 118),
    menu_selected_fg: Color::Rgb(10, 10, 15),
    // Bottom status bar
    bottom_panel_bg: Color::Rgb(15, 17, 24),
    bottom_panel_fg: Color::Rgb(140, 142, 152),
    // Dialog
    dialog_bg: Color::Rgb(20, 22, 30),
    dialog_fg: Color::Rgb(225, 225, 235),
    dialog_separator: Color::Rgb(100, 102, 115),
    // Selection
    selection_bg: Color::Rgb(0, 230, 118),
    selection_fg: Color::Rgb(10, 10, 15),
    // Link
    link_color: Color::Rgb(100, 180, 255),
    link_underline: true,
    // Debug
    debug_highlight: Color::Rgb(255, 168, 0),
};

// ---------------------------------------------------------------------------
// Global accessor
// ---------------------------------------------------------------------------

static CURRENT_THEME: OnceLock<Theme> = OnceLock::new();

pub fn current() -> &'static Theme {
    CURRENT_THEME.get_or_init(|| NOIR)
}

// ---------------------------------------------------------------------------
// Convenience wrappers – exact same signatures as before so every call site
// in term-wm-core and term-wm-ui-components works without changes.
// ---------------------------------------------------------------------------

pub fn rgb_to_color(rgb: (u8, u8, u8)) -> Color {
    crate::io::utils::term_color::map_rgb_to_color(rgb.0, rgb.1, rgb.2)
}

pub fn accent() -> Color {
    current().accent
}

pub fn accent_alt() -> Color {
    current().accent_alt
}

pub fn panel_bg() -> Color {
    current().panel_bg
}

pub fn panel_fg() -> Color {
    current().panel_fg
}

pub fn panel_inactive_fg() -> Color {
    current().panel_inactive_fg
}

pub fn panel_active_bg() -> Color {
    current().panel_active_bg
}

pub fn panel_active_fg() -> Color {
    current().panel_active_fg
}

pub fn menu_bg() -> Color {
    current().menu_bg
}

pub fn menu_fg() -> Color {
    current().menu_fg
}

pub fn menu_selected_bg() -> Color {
    current().menu_selected_bg
}

pub fn menu_selected_fg() -> Color {
    current().menu_selected_fg
}

pub fn success_bg() -> Color {
    current().success
}

pub fn success_fg() -> Color {
    current().success
}

pub fn selection_bg() -> Color {
    current().selection_bg
}

pub fn selection_fg() -> Color {
    current().selection_fg
}

pub fn dialog_bg() -> Color {
    current().dialog_bg
}

pub fn dialog_fg() -> Color {
    current().dialog_fg
}

pub fn dialog_separator() -> Color {
    current().dialog_separator
}

pub fn link_color() -> Color {
    current().link_color
}

pub fn link_underline() -> bool {
    current().link_underline
}

pub fn decorator_header_bg() -> Color {
    current().decorator_header_bg
}

pub fn decorator_header_fg() -> Color {
    current().decorator_header_fg
}

pub fn decorator_border() -> Color {
    current().decorator_border
}

pub fn decorator_border_active() -> Color {
    current().decorator_border_active
}

pub fn debug_highlight() -> Color {
    current().debug_highlight
}

pub fn bottom_panel_bg() -> Color {
    current().bottom_panel_bg
}

pub fn bottom_panel_fg() -> Color {
    current().bottom_panel_fg
}

// ---------------------------------------------------------------------------
// WCAG contrast helpers & tests
// ---------------------------------------------------------------------------

fn srgb_linearize(c: u8) -> f64 {
    let v = c as f64 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn relative_luminance(color: Color) -> f64 {
    let (r, g, b) = match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => return 0.5,
    };
    0.2126 * srgb_linearize(r) + 0.7152 * srgb_linearize(g) + 0.0722 * srgb_linearize(b)
}

pub fn contrast_ratio(a: Color, b: Color) -> f64 {
    let l1 = relative_luminance(a);
    let l2 = relative_luminance(b);
    let lighter = l1.max(l2);
    let darker = l1.min(l2);
    (lighter + 0.05) / (darker + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    type ColorFn = fn() -> Color;

    // Every actual foreground+background pair rendered in the UI.
    // Thresholds: 4.5:1 for text (WCAG AA normal), 3.0:1 for UI/large.
    const PAIRS: &[(ColorFn, ColorFn, f64, &str)] = &[
        // === Top panel bar (bg = bottom_panel_bg) ===
        (
            super::bottom_panel_fg,
            super::bottom_panel_bg,
            4.5,
            "top panel fill",
        ),
        (super::menu_fg, super::menu_bg, 4.5, "menu icon open"),
        (
            super::menu_selected_fg,
            super::menu_selected_bg,
            4.5,
            "focused window tab",
        ),
        (
            super::panel_inactive_fg,
            super::bottom_panel_bg,
            4.5,
            "unfocused window tab",
        ),
        (
            super::success_bg,
            super::bottom_panel_bg,
            3.0,
            "active indicator",
        ),
        (
            super::accent,
            super::bottom_panel_bg,
            3.0,
            "copied indicator",
        ),
        (
            super::panel_inactive_fg,
            super::bottom_panel_bg,
            3.0,
            "inactive indicator",
        ),
        // === Bottom status bar (bg = bottom_panel_bg) ===
        (
            super::bottom_panel_fg,
            super::bottom_panel_bg,
            4.5,
            "status text",
        ),
        (
            super::menu_selected_fg,
            super::menu_selected_bg,
            4.5,
            "keybinding combo",
        ),
        // === Window decorator ===
        (
            super::decorator_header_fg,
            super::decorator_header_bg,
            4.5,
            "focused header text",
        ),
        (
            super::decorator_header_fg,
            super::panel_bg,
            4.5,
            "unfocused header text",
        ),
        (
            super::decorator_border,
            super::panel_bg,
            3.0,
            "inactive window border",
        ),
        (
            super::decorator_border_active,
            super::panel_bg,
            3.0,
            "active window border",
        ),
        // === Menu dropdown ===
        (super::menu_fg, super::menu_bg, 4.5, "menu item text"),
        (
            super::menu_selected_fg,
            super::menu_selected_bg,
            4.5,
            "selected menu item",
        ),
        (
            super::menu_fg,
            super::panel_active_bg,
            4.5,
            "hovered menu item",
        ),
        // === Dialogs ===
        (super::dialog_fg, super::dialog_bg, 4.5, "dialog body text"),
        (
            super::dialog_fg,
            super::panel_bg,
            4.5,
            "unselected dialog button",
        ),
        (
            super::decorator_header_fg,
            super::decorator_header_bg,
            4.5,
            "selected dialog button",
        ),
        (
            super::dialog_separator,
            super::dialog_bg,
            3.0,
            "dialog separator",
        ),
        // === Selection ===
        (
            super::selection_fg,
            super::selection_bg,
            4.5,
            "selection highlight",
        ),
        // === Links (appear in dialogs and panels) ===
        (super::link_color, super::dialog_bg, 4.5, "link in dialog"),
        (super::link_color, super::panel_bg, 4.5, "link in panel"),
        // === Focused list/toggle-list borders ===
        (
            super::success_fg,
            super::panel_bg,
            3.0,
            "focused list border",
        ),
        // === Layout indicators ===
        (
            super::decorator_border_active,
            super::panel_bg,
            3.0,
            "tiling handle",
        ),
        (
            super::menu_selected_bg,
            super::panel_bg,
            3.0,
            "hovered tiling handle",
        ),
        (
            super::accent_alt,
            super::panel_bg,
            3.0,
            "handle hover border / resize outline",
        ),
        // === Drag-snap preview ===
        (super::accent, super::panel_bg, 3.0, "drag-snap fill"),
        // === Debug log ===
        (
            super::debug_highlight,
            super::panel_bg,
            3.0,
            "debug highlight",
        ),
    ];

    #[test]
    fn contrast_ratio_known_values() {
        let r = contrast_ratio(Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255));
        assert!((r - 21.0).abs() < 0.5, "black/white ratio: {r}");
    }

    #[test]
    fn wcag_aa_contrast_compliance() {
        let mut failures = Vec::new();
        for &(fg_fn, bg_fn, min_ratio, label) in PAIRS {
            let fg = fg_fn();
            let bg = bg_fn();
            let ratio = contrast_ratio(fg, bg);
            if ratio < min_ratio {
                failures.push(format!(
                    "{label}: {ratio:.2}:1 < {min_ratio}:1 (fg={fg:?}, bg={bg:?})"
                ));
            }
        }
        if !failures.is_empty() {
            panic!("WCAG AA contrast failures:\n  {}", failures.join("\n  "));
        }
    }

    #[test]
    fn theme_is_initialized() {
        let t = super::current();
        assert_eq!(t.name, "noir");
        assert_eq!(t.background, Color::Rgb(10, 10, 15));
    }

    #[test]
    fn accent_returns_rgb() {
        match accent() {
            Color::Rgb(r, g, b) => {
                assert_eq!(r, 0);
                assert_eq!(g, 230);
                assert_eq!(b, 118);
            }
            other => panic!("expected Rgb, got {other:?}"),
        }
    }

    #[test]
    fn accent_alt_returns_rgb() {
        match accent_alt() {
            Color::Rgb(r, g, b) => {
                assert_eq!(r, 255);
                assert_eq!(g, 168);
                assert_eq!(b, 0);
            }
            other => panic!("expected Rgb, got {other:?}"),
        }
    }
}
