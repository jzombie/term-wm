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
/// Dereferences to `[u8]` for reading.
///
/// Construct via [`PathWire::encode`] or the `From<&Path>` / `From<PathBuf>` /
/// `From<&str>` / `From<String>` conversions to encode an OS path, or via
/// `From<Vec<u8>>` / `PathWire(bytes)` to wrap already-encoded wire bytes.
/// Reconstruct the path with [`PathWire::decode`] (or the `to_path_buf` alias).
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

impl PathWire {
    /// Encode a platform-native OS path into lossless wire bytes.
    pub fn encode<P: AsRef<Path>>(path: P) -> Self {
        encode_path(path)
    }

    /// Decode the wire bytes back into a platform-native `PathBuf`.
    pub fn decode(&self) -> PathBuf {
        decode_path(self)
    }

    /// Convenience alias following Rust `Path::to_path_buf` naming.
    pub fn to_path_buf(&self) -> PathBuf {
        self.decode()
    }
}

impl From<&Path> for PathWire {
    fn from(path: &Path) -> Self {
        encode_path(path)
    }
}

impl From<PathBuf> for PathWire {
    fn from(path: PathBuf) -> Self {
        encode_path(&path)
    }
}

impl From<&str> for PathWire {
    fn from(s: &str) -> Self {
        encode_path(Path::new(s))
    }
}

impl From<String> for PathWire {
    fn from(s: String) -> Self {
        encode_path(Path::new(&s))
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
    use std::path::{Path, PathBuf};

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

    #[test]
    fn inherent_encode_matches_encode_path() {
        let path = std::env::temp_dir().join("inherent-encode");
        assert_eq!(PathWire::encode(&path), encode_path(&path));
    }

    #[test]
    fn inherent_decode_round_trips() {
        let path = std::env::temp_dir().join("inherent-decode");
        assert_eq!(PathWire::encode(&path).decode(), path);
    }

    #[test]
    fn to_path_buf_alias() {
        let path = std::env::temp_dir().join("to-path-buf");
        let pw = PathWire::encode(&path);
        assert_eq!(pw.to_path_buf(), pw.decode());
        assert_eq!(pw.to_path_buf(), path);
    }

    #[test]
    fn option_map_decode_combinator() {
        let path = std::env::temp_dir().join("option-map-decode");
        let pw = PathWire::encode(&path);
        let opt: Option<&PathWire> = Some(&pw);
        assert_eq!(opt.map(PathWire::decode), Some(path));
        let none: Option<&PathWire> = None;
        assert_eq!(none.map(PathWire::decode), None);
    }

    #[test]
    fn from_path_family_encodes_like_encode_path() {
        let path = std::env::temp_dir().join("from-path-family");
        let encoded = encode_path(&path);
        assert_eq!(PathWire::from(path.clone()), encoded);
        assert_eq!(PathWire::from(path.as_path()), encoded);
        let s = path.to_string_lossy().into_owned();
        assert_eq!(PathWire::from(s.as_str()), encoded);
        assert_eq!(PathWire::from(s), encoded);
    }

    #[test]
    fn from_str_treats_string_as_path() {
        let pw = PathWire::from("/tmp/from-str");
        assert_eq!(pw, encode_path(Path::new("/tmp/from-str")));
        assert_eq!(pw.decode(), PathBuf::from("/tmp/from-str"));
    }

    #[cfg(unix)]
    #[test]
    fn from_pathbuf_round_trips_non_utf8() {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::OsStr::from_bytes(b"cwd-\xff\xfe-non-utf8");
        let dir = PathBuf::from(name);
        let pw = PathWire::from(dir.clone());
        assert_eq!(decode_path(&pw), dir);
    }
}
