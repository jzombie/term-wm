/// Scan `data` for a complete OSC 0 or OSC 2 window-title sequence
/// (`OSC 0 ; <title> ST` or `OSC 2 ; <title> ST`) and return the title.
///
/// Only the **first** complete sequence is extracted.  Terminators:
/// - `BEL` (`\x07`)
/// - `ST`  (`\x1b\\`)
pub fn extract_osc_title(data: &[u8]) -> Option<String> {
    let mut i = 0;
    while i < data.len() {
        if i + 5 > data.len() || data[i] != 0x1b || data[i + 1] != b']' {
            i += 1;
            continue;
        }
        // Check for "0;" or "2;" after the ESC ] introducer
        let ps = data[i + 2];
        if (ps != b'0' && ps != b'2') || i + 4 > data.len() || data[i + 3] != b';' {
            i += 1;
            continue;
        }
        let payload_start = i + 4;
        let mut end = None;
        let mut j = payload_start;
        while j < data.len() {
            if data[j] == 0x07 {
                end = Some(j);
                break;
            }
            if data[j] == 0x1b && j + 1 < data.len() && data[j + 1] == b'\\' {
                end = Some(j);
                break;
            }
            j += 1;
        }
        if let Some(end_pos) = end {
            let raw = &data[payload_start..end_pos];
            if let Ok(text) = String::from_utf8(raw.to_vec()) {
                return Some(text);
            }
            return Some(String::from_utf8_lossy(raw).into_owned());
        }
        break;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_osc0_bel_terminated() {
        let data = b"\x1b]0;hello\x07";
        assert_eq!(extract_osc_title(data), Some("hello".to_string()));
    }

    #[test]
    fn extract_osc2_st_terminated() {
        let data = b"\x1b]2;world\x1b\\";
        assert_eq!(extract_osc_title(data), Some("world".to_string()));
    }

    #[test]
    fn extract_no_osc() {
        assert_eq!(extract_osc_title(b"no osc here"), None);
    }

    #[test]
    fn extract_from_partial_buffer() {
        let data = b"\x1b]0;hello\x07\x1b]2;world\x07";
        assert_eq!(extract_osc_title(data), Some("hello".to_string()));
    }

    #[test]
    fn extract_from_merged_output() {
        let mut buf = b"some output ".to_vec();
        buf.extend_from_slice(b"\x1b]0;vim /tmp/test\x07");
        buf.extend_from_slice(b"\x1b[32mgreen text\x1b[0m");
        assert_eq!(extract_osc_title(&buf), Some("vim /tmp/test".to_string()));
    }
}
