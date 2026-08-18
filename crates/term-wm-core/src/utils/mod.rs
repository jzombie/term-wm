pub mod ansi;
pub mod debounce;
pub mod keyboard_normalizer;
pub mod linkifier;
pub mod selectable_text;

pub use debounce::DelayedReleaseBool;
pub use debounce::KeyedTaskDebouncer;
pub use keyboard_normalizer::KeyboardNormalizer;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::constants::ELLIPSIS_CHAR;

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

    out.push(ELLIPSIS_CHAR);
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

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_returns_empty() {
        assert_eq!(truncate_with_ellipsis("", 10), "");
    }

    #[test]
    fn zero_max_width_returns_empty() {
        assert_eq!(truncate_with_ellipsis("hello", 0), "");
    }

    #[test]
    fn fits_exactly_returns_unchanged() {
        assert_eq!(truncate_with_ellipsis("hello", 5), "hello");
    }

    #[test]
    fn truncates_with_ellipsis() {
        assert_eq!(truncate_with_ellipsis("hello world", 8), "hello w…");
    }

    #[test]
    fn single_char_target() {
        assert_eq!(truncate_with_ellipsis("ab", 1), "…");
    }

    #[test]
    fn cjk_fits_one_char_plus_ellipsis() {
        assert_eq!(truncate_with_ellipsis("日本語", 3), "日…");
    }

    #[test]
    fn cjk_rejected_when_no_room() {
        assert_eq!(truncate_with_ellipsis("日本語", 2), "…");
    }

    #[test]
    fn cjk_mixed_truncation() {
        assert_eq!(truncate_with_ellipsis("ab日本語", 6), "ab日…");
    }
}
