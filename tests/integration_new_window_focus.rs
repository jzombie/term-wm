use std::sync::Arc;

use term_wm::window::WindowManager;

#[test]
fn new_window_is_focused() {
    let ctx = Arc::new(term_wm::AppContext::new("test", "0.0.0"));
    let top_panel: Box<
        dyn term_wm_core::top_panel_trait::TopPanel<term_wm_core::window::WindowId<usize>>,
    > = Box::new(term_wm_sys_ui_components::WmTopPanelComponent::new(&ctx.app_name));
    let bottom_panel: Box<dyn term_wm_core::bottom_panel_trait::BottomPanel> =
        Box::new(term_wm_sys_ui_components::WmBottomPanelComponent::new(
            &ctx.app_name,
            &ctx.app_version,
            None,
        ));
    let menu: Box<dyn term_wm_core::components::MenuOverlay<term_wm_core::window::WmMenuAction>> =
        Box::new(term_wm_sys_ui_components::WmMenuOverlay::new());
    let mut wm = WindowManager::<usize>::with_config(
        0,
        term_wm::wm_config::WmConfig::standalone(),
        ctx,
        top_panel,
        bottom_panel,
        menu,
    );

    // Initially there should be no app-level focused window.
    assert_eq!(wm.wm_focus_app(), None);

    // Tile (create) a new app window with id `1` and ensure it succeeds.
    let created = wm.tile_window(1);
    assert!(created, "tile_window should return true for new windows");

    // The new window should now be the focused app window.
    assert_eq!(wm.wm_focus_app(), Some(1));

    // WM-level focus should report the focused app via `wm_focus_app()` above.
}
