use std::sync::Arc;

use term_wm::config::AppBuilder;

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
    let menu: Box<dyn term_wm_core::components::WmComponent> =
        Box::new(term_wm_sys_ui_components::WmCommandPaletteOverlay::new());
    let mut wm = AppBuilder::bare()
        .app_ctx(ctx)
        .top_panel(top)
        .bottom_panel(bottom)
        .command_menu(menu)
        .build()
        .expect("test build");

    let key = wm.create_window(Box::new(term_wm::components::NoopComponent));
    wm.set_focus(key);

    assert_eq!(wm.focused_window(), key);
}
