#![allow(dead_code)]

pub const EXPECTED_OSC52_PAYLOAD: &[u8] = b"c;dGVzdA==";

pub fn get_mock_bin() -> String {
    let mut path = std::env::current_exe().expect("test exe path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(format!("term-session-mock{}", std::env::consts::EXE_SUFFIX));
    path.to_string_lossy().to_string()
}

/// Scan for a complete SGR 1006 mouse sequence: `\x1b[<...M` or `\x1b[<...m`.
/// Returns the complete token slice if found, accounting for ConPTY noise
/// injected between the start and end delimiters.
pub fn find_sgr_mouse_token(stream: &[u8]) -> Option<&[u8]> {
    let start = b"\x1b[<";
    let mut i = 0;
    while i + start.len() <= stream.len() {
        if &stream[i..i + start.len()] == start {
            let mut j = i + start.len();
            while j < stream.len() {
                match stream[j] {
                    b'M' | b'm' => return Some(&stream[i..=j]),
                    _ => j += 1,
                }
            }
            return None;
        }
        i += 1;
    }
    None
}

/// Extract OSC 52 payload from a byte stream, handling both standard
/// `\x1b]52;` and Windows-translated `←]52;` prefixes (where ESC is
/// rendered as the Unicode left arrow glyph by the Win32 console).
pub fn find_osc52_payload(stream: &[u8]) -> Option<&[u8]> {
    let standard_prefix = b"\x1b]52;";
    let windows_prefix = "←]52;".as_bytes();

    let (start_idx, prefix_len) = stream
        .windows(standard_prefix.len())
        .position(|w| w == standard_prefix)
        .map(|idx| (idx, standard_prefix.len()))
        .or_else(|| {
            stream
                .windows(windows_prefix.len())
                .position(|w| w == windows_prefix)
                .map(|idx| (idx, windows_prefix.len()))
        })?;

    let body_start = start_idx + prefix_len;
    let mut body_end = body_start;

    while body_end < stream.len() {
        let b = stream[body_end];
        if b == 0x07 {
            break;
        }
        if b == 0x1b && body_end + 1 < stream.len() && stream[body_end + 1] == b'\\' {
            break;
        }
        let is_valid_body =
            b.is_ascii_alphanumeric() || b == b';' || b == b'+' || b == b'/' || b == b'=';
        if !is_valid_body {
            break;
        }
        body_end += 1;
    }

    if body_end > body_start {
        Some(&stream[body_start..body_end])
    } else {
        None
    }
}
