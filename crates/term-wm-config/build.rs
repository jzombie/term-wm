// Bootstrap emission for THIS crate's own compilation (its lib reads the
// baked values via env!). term-wm-config cannot depend on itself, so this
// carries a minimal inline copy of the walk+hash logic — keep it in sync
// with src/build_identity.rs::emit.
use std::path::Path;

fn find_workspace_root_from(start: &Path) -> Option<std::path::PathBuf> {
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

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");
    match std::env::var("CARGO_MANIFEST_DIR")
        .ok()
        .and_then(|md| find_workspace_root_from(Path::new(&md)))
    {
        Some(root) => {
            let rendered = root.to_string_lossy().into_owned();
            let manifest = root.join("Cargo.toml");
            let dev_hash = format!(
                "{:08x}",
                (fnv1a64(rendered.as_bytes()) & 0xFFFF_FFFF) as u32
            );
            println!("cargo:rerun-if-changed={}", manifest.display());
            println!("cargo:rustc-env=TERM_WM_WORKSPACE_ROOT={rendered}");
            println!("cargo:rustc-env=TERM_WM_DEV_HASH={dev_hash}");
        }
        None => {
            println!("cargo:rustc-env=TERM_WM_WORKSPACE_ROOT=");
            println!("cargo:rustc-env=TERM_WM_DEV_HASH=00000000");
        }
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let nanos64 = (nanos & 0xFFFF_FFFF_FFFF_FFFF) as u64;
    let build_hash = format!(
        "{:08x}",
        (fnv1a64(nanos64.to_le_bytes().as_slice()) & 0xFFFF_FFFF) as u32
    );
    println!("cargo:rustc-env=TERM_WM_BUILD_HASH={build_hash}");
}
