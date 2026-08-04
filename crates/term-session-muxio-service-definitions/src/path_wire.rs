//! Lossless conversion between OS paths and the raw bytes carried on the
//! muxio wire.
//!
//! The client and daemon always run on the same host (local IPC), so a
//! platform-native encoding is safe: Unix sends the raw `OsStr` bytes and
//! Windows sends UTF-16 code units packed as little-endian `u16` pairs. Both
//! representations are byte-for-byte reversible — including non-UTF-8 paths on
//! Unix and unpaired-surrogate (WTF-16) paths on Windows — which is what makes
//! the session's launch directory round-trip losslessly.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Encode a path losslessly into wire bytes (see module docs).
pub fn encode_path<P: AsRef<Path>>(path: P) -> Vec<u8> {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_ref().as_os_str().as_bytes().to_vec()
    };
    #[cfg(windows)]
    let bytes = {
        use std::os::windows::ffi::OsStrExt;
        let units: Vec<u16> = path.as_ref().as_os_str().encode_wide().collect();
        let mut out = Vec::with_capacity(units.len() * 2);
        for unit in units {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out
    };
    #[cfg(not(any(unix, windows)))]
    let bytes = path.as_ref().to_string_lossy().into_owned().into_bytes();
    bytes
}

/// Reconstruct a path from wire bytes produced by [`encode_path`].
pub fn decode_path(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(OsStr::from_bytes(bytes))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        debug_assert!(
            bytes.len() % 2 == 0,
            "windows cwd bytes must be u16 little-endian pairs"
        );
        let units = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect::<Vec<u16>>();
        PathBuf::from(OsString::from_wide(&units))
    }
    #[cfg(not(any(unix, windows)))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_path, encode_path};
    use std::path::PathBuf;

    #[test]
    fn round_trips_plain_unicode_path() {
        let dir = std::env::temp_dir().join("term-session path with spaces");
        assert_eq!(decode_path(&encode_path(&dir)), dir);
    }

    #[cfg(unix)]
    #[test]
    fn round_trips_non_utf8_path() {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::OsStr::from_bytes(b"cwd-\xff\xfe-non-utf8");
        let dir = PathBuf::from(name);
        assert_eq!(decode_path(&encode_path(&dir)), dir);
    }

    #[cfg(windows)]
    #[test]
    fn round_trips_unpaired_surrogate_path() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        // WTF-16 with an unpaired low surrogate: `0xDC00` cannot be represented
        // in UTF-8, so only a wide round-trip can preserve it.
        let units = vec![0x0058u16, 0xDC00, 0x0059]; // "X\u{DC00}Y"
        let dir = PathBuf::from(OsString::from_wide(&units));
        assert_eq!(decode_path(&encode_path(&dir)), dir);
    }
}
