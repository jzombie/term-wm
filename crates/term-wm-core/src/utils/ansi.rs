pub fn strip_ansi_escapes(s: &str) -> String {
    console::strip_ansi_codes(s).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_passthrough() {
        assert_eq!(strip_ansi_escapes("hello world"), "hello world");
    }

    #[test]
    fn empty_string() {
        assert_eq!(strip_ansi_escapes(""), "");
    }

    #[test]
    fn simple_sgr_reset() {
        assert_eq!(strip_ansi_escapes("\x1b[0m"), "");
    }

    #[test]
    fn sgr_foreground_color() {
        assert_eq!(strip_ansi_escapes("\x1b[34m"), "");
    }

    #[test]
    fn sgr_dim() {
        assert_eq!(strip_ansi_escapes("\x1b[2m"), "");
    }

    #[test]
    fn text_with_ansi_prefix() {
        assert_eq!(strip_ansi_escapes("\x1b[32mDEBUG\x1b[0m ok"), "DEBUG ok");
    }

    #[test]
    fn text_with_ansi_infix() {
        assert_eq!(
            strip_ansi_escapes("foo\x1b[1;34mbar\x1b[0mbaz"),
            "foobarbaz"
        );
    }

    #[test]
    fn multiple_params() {
        assert_eq!(strip_ansi_escapes("\x1b[1;31;42m"), "");
    }

    #[test]
    fn escape_without_bracket_left_alone() {
        assert_eq!(strip_ansi_escapes("esc\x1bX"), "esc\x1bX");
    }

    #[test]
    fn lone_escape_left_alone() {
        assert_eq!(strip_ansi_escapes("abc\x1b"), "abc\x1b");
    }

    #[test]
    fn non_sgr_csi_sequences() {
        assert_eq!(strip_ansi_escapes("\x1b[2J"), "");
        assert_eq!(strip_ansi_escapes("\x1b[1A"), "");
        assert_eq!(strip_ansi_escapes("\x1b[?25l"), "");
    }

    #[test]
    fn mixed_content() {
        let input = "\x1b[2m[2024-01-01T12:00:00Z DEBUG my_module]\x1b[0m hello world";
        let expected = "[2024-01-01T12:00:00Z DEBUG my_module] hello world";
        assert_eq!(strip_ansi_escapes(input), expected);
    }

    #[test]
    fn unicode_preserved() {
        assert_eq!(strip_ansi_escapes("\x1b[31mhéllo\x1b[0m"), "héllo");
    }

    #[test]
    fn newlines_preserved() {
        assert_eq!(
            strip_ansi_escapes("line1\n\x1b[33mline2\x1b[0m\n"),
            "line1\nline2\n"
        );
    }
}
