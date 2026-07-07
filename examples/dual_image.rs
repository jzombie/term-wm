use std::io;

use term_wm::AppContext;
use term_wm::SvgImageComponent;
use term_wm::term_wm_app::TermWmApp;

fn main() -> io::Result<()> {
    let mut paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        paths.push("assets/zenOSmosis-logo.svg".to_string());
    }
    if paths.len() == 1 {
        paths.push(paths[0].clone());
    }

    let mut app = TermWmApp::new(AppContext::new("example", "0.0.0"));

    let mut left = SvgImageComponent::new();
    left.set_keep_aspect(true);
    left.set_colorize(true);
    let left_key = app.register(left);

    let mut right = SvgImageComponent::new();
    right.set_keep_aspect(true);
    right.set_colorize(true);
    let right_key = app.register(right);

    if let Some(img) = app.component_mut::<SvgImageComponent>(left_key) {
        img.load_from_path(&paths[0])?;
    }
    if let Some(img) = app.component_mut::<SvgImageComponent>(right_key) {
        img.load_from_path(&paths[1])?;
    }

    app.run()
}
