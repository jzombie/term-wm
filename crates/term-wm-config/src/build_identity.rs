//! Shared build-identity types and emission.
//!
//! Consumed two ways:
//!
//! - **External build scripts** (`term-session-muxio-service-definitions`):
//!   list `term-wm-config` under `[build-dependencies]` and call [`emit`]
//!   from their `build.rs`. This bakes three environment variables into
//!   their crate compilation:
//!   - `TERM_WM_WORKSPACE_ROOT`: canonicalized workspace root directory
//!   - `TERM_WM_DEV_HASH`:       FNV-1a hex8 of that root path
//!     (per-checkout stable)
//!   - `TERM_WM_BUILD_HASH`:     FNV-1a hex8 of the compile timestamp
//!     (per-compilation)
//! - **Runtime** ([`generation_hash`] / accessors): selects between dev and
//!   build hashes with a string-prefix check of the canonicalized
//!   executable path against the baked workspace root. No file contents are
//!   ever read or hashed at runtime.
//!
//! Consequence model: binaries built from one checkout share a single
//! daemon; installed/copied binaries are their own isolated generation.
//! Cross-generation endpoint collisions are structurally impossible.
//!
//! NOTE for `term-wm-config`'s own `build.rs`: it cannot depend on this
//! crate (self build-dependency), so it carries a minimal inline copy of
//! the walk+hash emission. Keep the two implementations in sync.

use std::path::{Path, PathBuf};

/// Baked workspace root (empty when no `[workspace]` manifest was found).
const WORKSPACE_ROOT: &str = env!("TERM_WM_WORKSPACE_ROOT");
/// Baked per-checkout identity.
const DEV_HASH: &str = env!("TERM_WM_DEV_HASH");
/// Baked per-compilation identity.
const BUILD_HASH: &str = env!("TERM_WM_BUILD_HASH");

/// FNV-1a 64-bit over `bytes`.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Hex-encode the low 32 bits as an 8-character identity suffix.
pub fn hex8(value: u64) -> String {
    format!("{value:08x}", value = (value & 0xFFFF_FFFF) as u32)
}

/// Walk ancestors of `start` looking for the directory whose `Cargo.toml`
/// contains the `[workspace]` table. Canonicalizes the result so every
/// workspace crate resolves the identical root regardless of nesting depth,
/// and so symlinked mount points collapse to one identity.
pub fn find_workspace_root_from(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start.to_path_buf());
    while let Some(current) = dir {
        let manifest = current.join("Cargo.toml");
        let is_workspace = manifest.is_file()
            && std::fs::read_to_string(&manifest)
                .map(|content| content.contains("[workspace]"))
                .unwrap_or(false);
        if is_workspace {
            return current.canonicalize().ok();
        }
        dir = current.parent().map(Path::to_path_buf);
    }
    None
}

/// [`find_workspace_root_from`] starting at `CARGO_MANIFEST_DIR`.
pub fn find_workspace_root() -> Option<PathBuf> {
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")?;
    find_workspace_root_from(Path::new(&manifest_dir))
}

/// Emit the three cargo environment variables from a participating crate's
/// `build.rs`.
///
/// Best-effort by design: when no workspace manifest can be located
/// (detached builds), the dev identity degrades to zeros and a warning
/// surfaces in build output. Always declares `rerun-if-changed` for the
/// resolved root manifest (absolute path) so a moved checkout re-bakes the
/// dev hash.
pub fn emit() {
    match find_workspace_root() {
        Some(root) => {
            let rendered = root.to_string_lossy().into_owned();
            println!(
                "cargo:rerun-if-changed={}",
                root.join("Cargo.toml").display()
            );
            println!("cargo:rustc-env=TERM_WM_WORKSPACE_ROOT={rendered}");
            println!(
                "cargo:rustc-env=TERM_WM_DEV_HASH={}",
                hex8(fnv1a64(rendered.as_bytes()))
            );
        }
        None => {
            println!(
                "cargo:warning=build_identity: no [workspace] manifest found above {}; \
                 dev identity disabled",
                std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default()
            );
            println!("cargo:rustc-env=TERM_WM_WORKSPACE_ROOT=");
            println!("cargo:rustc-env=TERM_WM_DEV_HASH=00000000");
        }
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let nanos64 = (nanos & 0xFFFF_FFFF_FFFF_FFFF) as u64;
    println!(
        "cargo:rustc-env=TERM_WM_BUILD_HASH={}",
        hex8(fnv1a64(nanos64.to_le_bytes().as_slice()))
    );
}

/// Which baked generation hash applies to THIS process's default endpoint.
///
/// In-tree binaries (canonicalized executable under the baked workspace
/// root) share the per-checkout dev hash, so every rebuild of a checkout
/// reuses the same running daemon. Binaries living anywhere else
/// (installed, copied out of target/) use their compile-time hash: each
/// installation is its own isolated generation.
fn generation_hash() -> &'static str {
    // macOS/BSD resolve /var -> /private/var: canonicalize both sides or
    // the prefix check silently fails for every in-tree binary. An error
    // falls back to the raw path (conservatively treated as foreign).
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok());
    let raw = std::env::current_exe().unwrap_or_default();
    let resolved = exe.unwrap_or(raw);
    let in_tree = !WORKSPACE_ROOT.is_empty() && resolved.starts_with(WORKSPACE_ROOT);
    if in_tree { DEV_HASH } else { BUILD_HASH }
}

/// Public read-only accessor: the raw generation hash applied by THIS
/// process to default-resolved gateway names.
///
/// Deliberately NOT applied to explicit `--gateway <NAME>` overrides: those
/// are power-user/test escape hatches taken verbatim.
pub fn default_generation_hash() -> &'static str {
    generation_hash()
}

/// [`default_generation_hash`] formatted as a socket-name suffix
/// (`-<hash8>`).
pub fn default_generation_suffix() -> String {
    format!("-{}", generation_hash())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a64_matches_known_vectors() {
        // Standard FNV-1a 64 test vectors.
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn hex8_pads_to_eight_lowercase_chars() {
        assert_eq!(hex8(0xdead_beef), "deadbeef");
        assert_eq!(hex8(1), "00000001");
        let s = hex8(fnv1a64(b"anything"));
        assert_eq!(s.len(), 8);
        assert!(
            s.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn walk_finds_this_workspace_root_from_nested_crate() {
        // This crate lives at <root>/crates/term-wm-config: the walk must
        // skip past the crate-local manifest (no [workspace] table) and
        // land on the repository root.
        let start = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = find_workspace_root_from(start).expect("workspace root found");
        assert!(root.join("Cargo.toml").is_file());
        assert!(
            root.join("crates").is_dir(),
            "expected the repo workspace root, got {}",
            root.display()
        );
    }

    /// Isolated synthetic tree: outer dir with a plain (non-workspace)
    /// Cargo.toml containing an inner dir with another plain manifest. The
    /// walk from inner must stop at outer only when outer carries the
    /// `[workspace]` marker.
    #[test]
    fn walk_requires_workspace_marker_and_stops_there() {
        let base = std::env::temp_dir().join(format!("twm-bi-{}", std::process::id()));
        let inner = base.join("outer").join("inner");
        std::fs::create_dir_all(&inner).expect("mkdir");
        std::fs::write(
            base.join("outer").join("Cargo.toml"),
            "[package]\nname=\"x\"\n",
        )
        .expect("write outer");
        std::fs::write(inner.join("Cargo.toml"), "[package]\nname=\"y\"\n").expect("write inner");

        // No [workspace] anywhere in the chain yet: no root found.
        assert!(find_workspace_root_from(&inner).is_none());

        // Marking OUTER as the workspace makes the walk stop there.
        std::fs::write(
            base.join("outer").join("Cargo.toml"),
            "[workspace]\nmembers=[]\n",
        )
        .expect("rewrite outer");
        let found = find_workspace_root_from(&inner).expect("outer is the workspace root");
        assert_eq!(found, base.join("outer").canonicalize().expect("canon"));

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn generation_hash_is_stable_within_process() {
        assert_eq!(default_generation_hash(), default_generation_hash());
    }

    #[test]
    fn generation_suffix_is_dash_plus_hex8() {
        let s = default_generation_suffix();
        assert!(s.starts_with('-'), "suffix must start with dash: {s}");
        assert_eq!(s[1..].len(), 8);
        assert!(
            s[1..]
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}
