use std::io;

use term_wm::AppContext;
use term_wm::SvgImageComponent;
use term_wm::components::AppRootComponent;
use term_wm::term_wm_app::TermWmApp;

/// Default demo image, resolved against the crate root so it loads regardless
/// of the current working directory.
const DEFAULT_SVG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/zenOSmosis-logo.svg"
);

fn main() -> io::Result<()> {
    let mut paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        paths.push(DEFAULT_SVG.to_string());
    }
    if paths.len() == 1 {
        paths.push(paths[0].clone());
    }

    let mut app: TermWmApp<SvgImageComponent> =
        TermWmApp::new_custom(AppContext::new("example", "0.0.0"));

    let mut left = SvgImageComponent::new();
    left.set_keep_aspect(true);
    left.set_colorize(true);
    left.load_from_path(&paths[0])?;
    let left_key = app.open_window(AppRootComponent::Custom(left));
    app.set_window_title(left_key, "Left Image");

    let mut right = SvgImageComponent::new();
    right.set_keep_aspect(true);
    right.set_colorize(true);
    right.load_from_path(&paths[1])?;
    let right_key = app.open_window(AppRootComponent::Custom(right));
    app.set_window_title(right_key, "Right Image");

    app.run()
}
