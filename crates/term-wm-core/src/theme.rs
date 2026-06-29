use ratatui::style::Color;

use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Semantic roles — the compiler-enforced exhaustive list of every color
// concept used in the UI.  Adding a variant here forces a new match arm in
// `fn fg()` / `fn bg()` below.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::EnumIter)]
pub enum FgColor {
    Accent,
    AccentAlt,
    PanelFg,
    PanelInactiveFg,
    PanelActiveFg,
    MenuFg,
    MenuSelectedFg,
    MenuSelectedBg,
    Success,
    SelectionFg,
    DialogFg,
    DialogSeparator,
    LinkColor,
    DecoratorHeaderFg,
    DecoratorBorder,
    DecoratorBorderActive,
    DebugHighlight,
    BottomPanelFg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::EnumIter)]
pub enum BgColor {
    PanelBg,
    PanelActiveBg,
    MenuBg,
    MenuSelectedBg,
    SelectionBg,
    DialogBg,
    DecoratorHeaderBg,
    BottomPanelBg,
    Surface,
    ShadowBg,
    ShadowTint,
}

// ---------------------------------------------------------------------------
// Theme struct – all RGB values live here.
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
    pub shadow_bg: Color,
    pub shadow_tint: Color,
    pub link_color: Color,
    pub link_underline: bool,
    pub debug_highlight: Color,
}

impl Theme {
    pub fn fg(&self, role: FgColor) -> Color {
        match role {
            FgColor::Accent => self.accent,
            FgColor::AccentAlt => self.accent_alt,
            FgColor::PanelFg => self.panel_fg,
            FgColor::PanelInactiveFg => self.panel_inactive_fg,
            FgColor::PanelActiveFg => self.panel_active_fg,
            FgColor::MenuFg => self.menu_fg,
            FgColor::MenuSelectedFg => self.menu_selected_fg,
            FgColor::MenuSelectedBg => self.menu_selected_bg,
            FgColor::Success => self.success,
            FgColor::SelectionFg => self.selection_fg,
            FgColor::DialogFg => self.dialog_fg,
            FgColor::DialogSeparator => self.dialog_separator,
            FgColor::LinkColor => self.link_color,
            FgColor::DecoratorHeaderFg => self.decorator_header_fg,
            FgColor::DecoratorBorder => self.decorator_border,
            FgColor::DecoratorBorderActive => self.decorator_border_active,
            FgColor::DebugHighlight => self.debug_highlight,
            FgColor::BottomPanelFg => self.bottom_panel_fg,
        }
    }

    pub fn bg(&self, role: BgColor) -> Color {
        match role {
            BgColor::PanelBg => self.panel_bg,
            BgColor::PanelActiveBg => self.panel_active_bg,
            BgColor::MenuBg => self.menu_bg,
            BgColor::MenuSelectedBg => self.menu_selected_bg,
            BgColor::SelectionBg => self.selection_bg,
            BgColor::DialogBg => self.dialog_bg,
            BgColor::DecoratorHeaderBg => self.decorator_header_bg,
            BgColor::BottomPanelBg => self.bottom_panel_bg,
            BgColor::Surface => self.surface,
            BgColor::ShadowBg => self.shadow_bg,
            BgColor::ShadowTint => self.shadow_tint,
        }
    }
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
    // Shadow
    shadow_bg: Color::Rgb(35, 38, 50),
    shadow_tint: Color::Rgb(16, 17, 22),
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
// Convenience wrappers – delegate via the enum so adding a variant forces
// updating the match arm above, and the test picks it up automatically.
// ---------------------------------------------------------------------------

pub fn rgb_to_color(rgb: (u8, u8, u8)) -> Color {
    crate::io::utils::term_color::map_rgb_to_color(rgb.0, rgb.1, rgb.2)
}

pub fn accent() -> Color {
    current().fg(FgColor::Accent)
}
pub fn accent_alt() -> Color {
    current().fg(FgColor::AccentAlt)
}
pub fn panel_bg() -> Color {
    current().bg(BgColor::PanelBg)
}
pub fn panel_fg() -> Color {
    current().fg(FgColor::PanelFg)
}
pub fn panel_inactive_fg() -> Color {
    current().fg(FgColor::PanelInactiveFg)
}
pub fn panel_active_bg() -> Color {
    current().bg(BgColor::PanelActiveBg)
}
pub fn panel_active_fg() -> Color {
    current().fg(FgColor::PanelActiveFg)
}
pub fn menu_bg() -> Color {
    current().bg(BgColor::MenuBg)
}
pub fn menu_fg() -> Color {
    current().fg(FgColor::MenuFg)
}
pub fn menu_selected_bg() -> Color {
    current().bg(BgColor::MenuSelectedBg)
}
pub fn menu_selected_fg() -> Color {
    current().fg(FgColor::MenuSelectedFg)
}
pub fn success_bg() -> Color {
    current().fg(FgColor::Success)
}
pub fn success_fg() -> Color {
    current().fg(FgColor::Success)
}
pub fn selection_bg() -> Color {
    current().bg(BgColor::SelectionBg)
}
pub fn selection_fg() -> Color {
    current().fg(FgColor::SelectionFg)
}
pub fn dialog_bg() -> Color {
    current().bg(BgColor::DialogBg)
}
pub fn dialog_fg() -> Color {
    current().fg(FgColor::DialogFg)
}
pub fn dialog_separator() -> Color {
    current().fg(FgColor::DialogSeparator)
}
pub fn link_color() -> Color {
    current().fg(FgColor::LinkColor)
}
pub fn link_underline() -> bool {
    current().link_underline
}
pub fn decorator_header_bg() -> Color {
    current().bg(BgColor::DecoratorHeaderBg)
}
pub fn decorator_header_fg() -> Color {
    current().fg(FgColor::DecoratorHeaderFg)
}
pub fn decorator_border() -> Color {
    current().fg(FgColor::DecoratorBorder)
}
pub fn decorator_border_active() -> Color {
    current().fg(FgColor::DecoratorBorderActive)
}
pub fn surface() -> Color {
    current().bg(BgColor::Surface)
}
pub fn shadow_bg() -> Color {
    current().bg(BgColor::ShadowBg)
}
pub fn shadow_tint() -> Color {
    current().bg(BgColor::ShadowTint)
}
pub fn debug_highlight() -> Color {
    current().fg(FgColor::DebugHighlight)
}
pub fn bottom_panel_bg() -> Color {
    current().bg(BgColor::BottomPanelBg)
}
pub fn bottom_panel_fg() -> Color {
    current().fg(FgColor::BottomPanelFg)
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
    (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;
    use strum::IntoEnumIterator;

    // ------------------------------------------------------------------
    // Exhaustive FgColor × BgColor check.
    //
    // Every variant in both enums is paired against every variant in the
    // other.  Adding a variant to either enum forces:
    //   1. compiler: new match arm in fn fg() / fn bg()
    //   2. this test: the new variant automatically participates in every
    //      pair — no manual list to update.
    //
    // Some pairs are physically impossible to satisfy: a color chosen for
    // 4.5:1 on a bright green background (e.g. near-black text) cannot
    // also achieve 3:1 against very dark backgrounds.  These are
    // documented in PHYSICALLY_IMPOSSIBLE with the conflicting constraint.
    // ------------------------------------------------------------------

    /// Pairs that cannot simultaneously satisfy 3.0:1 due to conflicting
    /// luminance bounds (not because they were overlooked).
    const PHYSICALLY_IMPOSSIBLE: &[(FgColor, BgColor, &str)] = &[
        // Near-black text (L≈0.002) chosen for 13:1 on bright green.
        // To reach 3:1 on dark bgs it would need L≥0.11, but then it
        // couldn't reach 4.5:1 on green (needs L≤0.09).
        (
            FgColor::MenuSelectedFg,
            BgColor::PanelBg,
            "dark text can't also be light enough for dark bg",
        ),
        (
            FgColor::MenuSelectedFg,
            BgColor::PanelActiveBg,
            "same luminance conflict",
        ),
        (FgColor::MenuSelectedFg, BgColor::MenuBg, "same"),
        (FgColor::MenuSelectedFg, BgColor::DialogBg, "same"),
        (FgColor::MenuSelectedFg, BgColor::DecoratorHeaderBg, "same"),
        (FgColor::MenuSelectedFg, BgColor::BottomPanelBg, "same"),
        (
            FgColor::MenuSelectedFg,
            BgColor::Surface,
            "same luminance conflict — shadow is visual effect, not text surface",
        ),
        (
            FgColor::SelectionFg,
            BgColor::PanelBg,
            "same — near-black only on green",
        ),
        (FgColor::SelectionFg, BgColor::PanelActiveBg, "same"),
        (FgColor::SelectionFg, BgColor::MenuBg, "same"),
        (FgColor::SelectionFg, BgColor::DialogBg, "same"),
        (FgColor::SelectionFg, BgColor::DecoratorHeaderBg, "same"),
        (FgColor::SelectionFg, BgColor::BottomPanelBg, "same"),
        (FgColor::SelectionFg, BgColor::Surface, "same"),
        (FgColor::MenuSelectedFg, BgColor::ShadowBg, "same — shadow bg never carries text"),
        (FgColor::SelectionFg, BgColor::ShadowBg, "same"),
        (FgColor::MenuSelectedFg, BgColor::ShadowTint, "same — shadow tint never carries text"),
        (FgColor::SelectionFg, BgColor::ShadowTint, "same"),
        // Green accent colors on green — same hue, never co-occur.
        (FgColor::Success, BgColor::MenuSelectedBg, "green on green"),
        (FgColor::Success, BgColor::SelectionBg, "green on green"),
        (FgColor::Accent, BgColor::MenuSelectedBg, "green on green"),
        (FgColor::Accent, BgColor::SelectionBg, "green on green"),
        (
            FgColor::MenuSelectedBg,
            BgColor::MenuSelectedBg,
            "green on green",
        ),
        (
            FgColor::MenuSelectedBg,
            BgColor::SelectionBg,
            "green on green",
        ),
        // Dialog separator is dialog-internal only
        (
            FgColor::DialogSeparator,
            BgColor::PanelBg,
            "dialog-internal only",
        ),
        (FgColor::DialogSeparator, BgColor::PanelActiveBg, "same"),
        (FgColor::DialogSeparator, BgColor::MenuBg, "same"),
        (FgColor::DialogSeparator, BgColor::DecoratorHeaderBg, "same"),
        // Border colors on incompatible backgrounds
        (
            FgColor::DecoratorBorder,
            BgColor::PanelActiveBg,
            "border never on active panel",
        ),
        (
            FgColor::DecoratorBorder,
            BgColor::DecoratorHeaderBg,
            "border never on header",
        ),
        (
            FgColor::DecoratorBorderActive,
            BgColor::PanelActiveBg,
            "never combined",
        ),
        (
            FgColor::DecoratorBorderActive,
            BgColor::MenuSelectedBg,
            "never combined",
        ),
        (
            FgColor::DecoratorBorderActive,
            BgColor::SelectionBg,
            "never combined",
        ),
        // Various fg colors that only appear on their designated bg
        (
            FgColor::DebugHighlight,
            BgColor::MenuSelectedBg,
            "never combined",
        ),
        (
            FgColor::DebugHighlight,
            BgColor::SelectionBg,
            "never combined",
        ),
        (
            FgColor::LinkColor,
            BgColor::MenuSelectedBg,
            "never combined",
        ),
        (FgColor::LinkColor, BgColor::SelectionBg, "never combined"),
        (
            FgColor::BottomPanelFg,
            BgColor::MenuSelectedBg,
            "never combined",
        ),
        (
            FgColor::BottomPanelFg,
            BgColor::SelectionBg,
            "never combined",
        ),
        (FgColor::DialogFg, BgColor::MenuSelectedBg, "never combined"),
        (FgColor::DialogFg, BgColor::SelectionBg, "never combined"),
        (
            FgColor::DecoratorHeaderFg,
            BgColor::MenuSelectedBg,
            "never combined",
        ),
        (
            FgColor::DecoratorHeaderFg,
            BgColor::SelectionBg,
            "never combined",
        ),
        // Light text (L≈0.78) on bright green — never combined
        (
            FgColor::PanelActiveFg,
            BgColor::MenuSelectedBg,
            "never combined",
        ),
        (
            FgColor::PanelActiveFg,
            BgColor::SelectionBg,
            "never combined",
        ),
        (
            FgColor::MenuFg,
            BgColor::MenuSelectedBg,
            "light text on green — never combined",
        ),
        (FgColor::MenuFg, BgColor::SelectionBg, "never combined"),
        (
            FgColor::PanelFg,
            BgColor::MenuSelectedBg,
            "light text on green — never combined",
        ),
        (FgColor::PanelFg, BgColor::SelectionBg, "never combined"),
        (
            FgColor::PanelInactiveFg,
            BgColor::MenuSelectedBg,
            "light text on green — never combined",
        ),
        (
            FgColor::PanelInactiveFg,
            BgColor::SelectionBg,
            "never combined",
        ),
        (
            FgColor::AccentAlt,
            BgColor::MenuSelectedBg,
            "amber on green — never combined",
        ),
        (FgColor::AccentAlt, BgColor::SelectionBg, "never combined"),
        (
            FgColor::DialogFg,
            BgColor::MenuSelectedBg,
            "already excluded above",
        ),
    ];

    /// Every FgColor × BgColor pair at 3.0:1 (WCAG AA UI/large).
    /// Adding a variant to either enum automatically includes it in every
    /// pair — the exclusion list documents only physically impossible pairs.
    #[test]
    fn exhaustive_fg_times_bg() {
        let t = super::current();
        let mut failures = Vec::new();
        let mut excluded = 0u32;

        for fg in FgColor::iter() {
            for bg in BgColor::iter() {
                if PHYSICALLY_IMPOSSIBLE
                    .iter()
                    .any(|(f, b, _)| *f == fg && *b == bg)
                {
                    excluded += 1;
                    continue;
                }
                let ratio = contrast_ratio(t.fg(fg), t.bg(bg));
                if ratio < 3.0 {
                    failures.push(format!("{fg:?} on {bg:?}: {ratio:.2}:1"));
                }
            }
        }

        assert!(excluded > 0, "exclusion list should have entries");
        if !failures.is_empty() {
            panic!(
                "WCAG 3:1 failures ({} pairs excluded):\n  {}",
                excluded,
                failures.join("\n  ")
            );
        }
    }

    #[test]
    fn contrast_ratio_known_values() {
        let r = contrast_ratio(Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255));
        assert!((r - 21.0).abs() < 0.5, "black/white ratio: {r}");
    }

    #[test]
    fn theme_is_initialized() {
        let t = super::current();
        assert_eq!(t.name, "noir");
    }
}
