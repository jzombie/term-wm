use indoc::indoc;
use std::env;
use std::fs;
use std::path::Path;

const HELP_REL: &str = "assets/help.md";

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let help_basename = Path::new(HELP_REL)
        .file_name()
        .and_then(|s| s.to_str())
        .expect("invalid help asset filename");
    let help_path = Path::new(&manifest).join(HELP_REL);
    // Re-run build if the help file changes
    println!("cargo:rerun-if-changed={}", help_path.display());

    let modified_rfc3339 = match fs::metadata(&help_path).and_then(|m| m.modified()) {
        Ok(t) => {
            let dt: chrono::DateTime<chrono::Local> = chrono::DateTime::from(t);
            // Use RFC3339 here so runtime code can parse it into any
            // localised/pretty format it prefers.
            dt.to_rfc3339()
        }
        Err(_) => String::new(),
    };

    // Write a generated Rust source file into OUT_DIR which defines a
    // small struct with the embedded help content and the modification
    // timestamp. Placing the file in `OUT_DIR` avoids mutating tracked
    // source files during builds and keeps generated artifacts isolated.
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let gen_path = Path::new(&out_dir).join("generated_help.rs");

    // The generated file will `include_bytes!` the markdown from the
    // crate source tree (`CARGO_MANIFEST_DIR`) so we don't need to copy
    // the file around. This ensures the generated file is self
    // contained while avoiding separate runtime file dependencies.
    // Copy the help markdown into OUT_DIR so the compiled crate can
    // include it with `include_bytes!(concat!(env!("OUT_DIR"), "/<basename>"))`.
    let help_dest = Path::new(&out_dir).join(help_basename);
    fs::copy(&help_path, &help_dest).expect("failed to copy help.md to OUT_DIR");

    // Generate a tiny source file containing the embedded struct that
    // references the copied markdown basename and the RFC3339 timestamp.
    let escaped = modified_rfc3339.replace('"', "\\\"");
    let gen_src = format!(
        indoc!(
            r#"
                pub struct EmbeddedHelp {{ pub content: &'static [u8], pub modified_rfc3339: &'static str }}

                pub const EMBEDDED_HELP: EmbeddedHelp = EmbeddedHelp {{
                    content: include_bytes!(concat!(env!("OUT_DIR"), "/{basename}")),
                    modified_rfc3339: "{rfc}",
                }};
            "#
        ),
        basename = help_basename,
        rfc = escaped,
    );
    fs::write(&gen_path, gen_src).expect("failed to write generated_help.rs to OUT_DIR");
}
