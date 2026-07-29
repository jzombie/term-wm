pub mod ansi;
pub mod keyboard_normalizer;
pub mod linkifier;
pub mod selectable_text;

pub use keyboard_normalizer::KeyboardNormalizer;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Truncate a string to a maximum cellular width, appending `…` (U+2026)
/// if the string exceeds the bound. Safe for multi-width Unicode characters.
/// Returns an empty string when `max_width` is 0.
pub fn truncate_with_ellipsis(value: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if value.width() <= max_width {
        return value.to_string();
    }
    let target = max_width.saturating_sub(1);
    let mut current = 0;
    let mut out = String::new();
    for c in value.chars() {
        let cw = c.width().unwrap_or(0);
        if current + cw > target {
            break;
        }
        current += cw;
        out.push(c);
    }

    // TODO: Not a fan of Unicode characters (or any strings) being hardcoded
    // into the core, but the Command Palette and several other places
    // currently use them.
    // Unicode ellipsis
    out.push('\u{2026}');
    out
}

/// Truncate a string to a maximum character width.
/// Pure string manipulation — no rendering dependencies.
pub fn truncate_to_width(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    value.chars().take(width).collect()
}
