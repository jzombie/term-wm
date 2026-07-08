pub mod ansi;
pub mod keyboard_normalizer;
pub mod linkifier;
pub mod selectable_text;

pub use keyboard_normalizer::KeyboardNormalizer;

/// Truncate a string to a maximum character width.
/// Pure string manipulation — no rendering dependencies.
pub fn truncate_to_width(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    value.chars().take(width).collect()
}
