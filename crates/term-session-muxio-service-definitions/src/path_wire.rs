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

use bitcode::{Decode, Encode};

/// A byte buffer carrying a path's lossless wire encoding.
///
/// The encoding is platform-native (safe because the client and daemon always
/// run on the same host): Unix stores the raw `OsStr` bytes; Windows stores
/// UTF-16 code units packed as little-endian `u16` pairs. Both are byte-for-byte
/// reversible, so even non-UTF-8 / WTF-16 paths round-trip intact.
///
/// Distinct from a raw `Vec<u8>` (e.g. PTY input/output), so a path payload
/// cannot be accidentally substituted for, or mixed with, arbitrary buffers.
/// Dereferences to `[u8]` for reading; construct via [`encode_path`] or
/// `PathWire(bytes)`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct PathWire(pub Vec<u8>);

impl std::ops::Deref for PathWire {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for PathWire {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for PathWire {
    fn from(bytes: Vec<u8>) -> Self {
        PathWire(bytes)
    }
}

/// Encode a path losslessly into wire bytes (see module docs).
pub fn encode_path<P: AsRef<Path>>(path: P) -> PathWire {
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
    PathWire(bytes)
}

/// Reconstruct a path from wire bytes produced by [`encode_path`].
pub fn decode_path(pw: &PathWire) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(OsStr::from_bytes(&pw.0))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        let bytes = &pw.0;
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
        PathBuf::from(String::from_utf8_lossy(&pw.0).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_path, encode_path, PathWire};
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

    #[test]
    fn from_vec_wraps_raw_bytes() {
        let raw = vec![b'/', b't', b'm', b'p', b'/', b'c', b'w', b'd'];
        let pw = PathWire::from(raw.clone());
        assert_eq!(pw, PathWire(raw));
        assert_eq!(pw.as_ref(), b"/tmp/cwd");
        assert!(!pw.is_empty());
    }

    #[test]
    fn default_is_empty_sentinel() {
        assert!(PathWire::default().is_empty());
    }
}
