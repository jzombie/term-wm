use std::sync::Arc;

use term_wm::window::{WindowKey, WindowManager};

#[test]
fn new_window_is_focused() {
    let ctx = Arc::new(term_wm::AppContext::new("test", "0.0.0"));
    let top_panel: Box<dyn term_wm_core::top_panel_trait::TopPanel<WindowKey>> = Box::new(
        term_wm_sys_ui_components::WmTopPanelComponent::new(&ctx.app_name),
    );
    let bottom_panel: Box<dyn term_wm_core::bottom_panel_trait::BottomPanel> =
        Box::new(term_wm_sys_ui_components::WmBottomPanelComponent::new(
            &ctx.app_name,
            &ctx.app_version,
            None,
        ));
    let menu: Box<dyn term_wm_core::components::MenuOverlay<term_wm_core::window::WmMenuAction>> =
        Box::new(term_wm_sys_ui_components::WmMenuOverlay::new());
    let mut wm = WindowManager::with_config(
        term_wm::wm_config::WmConfig::standalone(),
        ctx,
        Some(top_panel),
        Some(bottom_panel),
        Some(menu),
    );

    let key = wm.create_window();
    wm.set_focus(key);

    assert_eq!(wm.focused_window(), key);
}
