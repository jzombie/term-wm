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

    // ARCHITECTURE CONTEXT:
    // `app.register` executes two atomic engine mutations:
    // 1. wm.spawn(): Allocates the component to the engine's SlotMap, assigning it
    //    a WindowKey. At this exact moment, the window is in `WindowState::Realized` (hidden).
    // 2. wm.transition_window(key, WindowState::Mapped): Instantly promotes the window
    //    to the active render pipeline by pushing it into `z_order` and `managed_draw_order`.
    let left_key = app.register(left);
    app.set_window_title(left_key, "Left Image");

    let mut right = SvgImageComponent::new();
    right.set_keep_aspect(true);
    right.set_colorize(true);
    let right_key = app.register(right);
    app.set_window_title(right_key, "Right Image");

    if let Some(img) = app.component_mut::<SvgImageComponent>(left_key) {
        img.load_from_path(&paths[0])?;
    }
    if let Some(img) = app.component_mut::<SvgImageComponent>(right_key) {
        img.load_from_path(&paths[1])?;
    }

    // ARCHITECTURE CONTEXT:
    // `app.run()` hands control to the event loop.
    // During the render pipeline's layout phase, the engine discovers these Mapped
    // windows currently lack explicit geometric coordinates.
    // It polls the orchestrator via `WindowManagerHost::layout_for_windows()`.
    // The default trait implementation executes `auto_layout_for_windows()`, which intercepts
    // all un-positioned active windows and automatically subdivides the terminal screen
    // space into a binary partitioned grid.
    app.run()
}
