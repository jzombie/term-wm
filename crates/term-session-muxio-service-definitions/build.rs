// Baked gateway generation identity. Logic lives in
// term-wm-config::build_identity; this crate consumes it as a
// build-dependency so the packaged .crate stays self-contained.
fn main() {
    term_wm_config::build_identity::emit();
}
