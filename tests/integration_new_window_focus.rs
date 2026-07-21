use std::sync::Arc;

use term_wm::config::AppBuilder;
use term_wm_core::components::{NoopComponent, NoopOverlay};
use term_wm_ui_facade::layer_component::LayerComponent;

#[test]
fn new_window_is_focused() {
    let ctx = Arc::new(term_wm::AppContext::new("test", "0.0.0"));
    let mut wm = AppBuilder::<LayerComponent>::bare()
        .app_ctx(ctx)
        .top_panel(LayerComponent::TopPanel(
            term_wm_sys_ui_components::WmTopPanelComponent::new("test"),
        ))
        .bottom_panel(LayerComponent::BottomPanel(
            term_wm_sys_ui_components::WmBottomPanelComponent::new(
                "test",
                "0.0.0",
                None,
            ),
        ))
        .build::<NoopComponent, NoopOverlay>()
        .expect("test build");

    let key = wm.create_window(NoopComponent);
    wm.set_focus(key);

    assert_eq!(wm.focused_window(), key);
}
