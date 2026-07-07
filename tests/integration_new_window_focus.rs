use std::sync::Arc;

use term_wm::window::WindowManager;

#[test]
fn new_window_is_focused() {
    let ctx = Arc::new(term_wm::AppContext::new("test", "0.0.0"));
    let top: Box<dyn term_wm_core::components::WmComponent> = Box::new(
        term_wm_sys_ui_components::WmTopPanelComponent::new(&ctx.app_name),
    );
    let bottom: Box<dyn term_wm_core::components::WmComponent> = Box::new(
        term_wm_sys_ui_components::WmBottomPanelComponent::new(
            &ctx.app_name,
            &ctx.app_version,
            None,
        ),
    );
    let menu: Box<dyn term_wm_core::components::WmComponent> =
        Box::new(term_wm_sys_ui_components::WmMenuOverlay::new());
    let mut wm = WindowManager::with_config(
        term_wm::wm_config::WmConfig::standalone(),
        ctx,
        Some(top),
        Some(bottom),
        Some(menu),
    );

    let key = wm.create_window(Box::new(term_wm::components::NoopComponent));
    wm.set_focus(key);

    assert_eq!(wm.focused_window(), key);
}
