use std::sync::Arc;

use term_wm::config::AppBuilder;
use term_wm_core::components::{NoopComponent, NoopOverlay, WmComponent};

#[test]
fn new_window_is_focused() {
    let ctx = Arc::new(term_wm::AppContext::new("test", "0.0.0"));
    let top: Box<dyn term_wm_core::components::WmComponent> = Box::new(
        term_wm_sys_ui_components::WmTopPanelComponent::new(&ctx.app_name),
    );
    let bottom: Box<dyn term_wm_core::components::WmComponent> =
        Box::new(term_wm_sys_ui_components::WmBottomPanelComponent::new(
            &ctx.app_name,
            &ctx.app_version,
            None,
        ));
    let mut wm = AppBuilder::<Box<dyn WmComponent>>::bare()
        .app_ctx(ctx)
        .top_panel(top)
        .bottom_panel(bottom)
        .build::<NoopComponent, NoopOverlay>()
        .expect("test build");

    let key = wm.create_window(NoopComponent);
    wm.set_focus(key);

    assert_eq!(wm.focused_window(), key);
}
