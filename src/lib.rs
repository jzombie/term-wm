pub use term_wm_core::*;
pub use term_wm_ui_components::*;
pub mod tracing_sub;

use term_wm_core::config::WmBuilder;
use term_wm_core::window::{WindowKey, WindowManager};

pub trait WindowManagerExt: Sized {
    fn new_standalone(app_ctx: term_wm_core::app_context::AppContext) -> Self;
    fn new_embedded(app_ctx: term_wm_core::app_context::AppContext) -> Self;
}

impl WindowManagerExt for WindowManager {
    fn new_standalone(app_ctx: term_wm_core::app_context::AppContext) -> Self {
        let hostname = app_ctx.hostname.as_deref();
        let top_panel: Box<dyn term_wm_core::top_panel_trait::TopPanel<WindowKey>> = Box::new(
            term_wm_sys_ui_components::WmTopPanelComponent::new(&app_ctx.app_name),
        );
        let bottom_panel: Box<dyn term_wm_core::bottom_panel_trait::BottomPanel> =
            Box::new(term_wm_sys_ui_components::WmBottomPanelComponent::new(
                &app_ctx.app_name,
                &app_ctx.app_version,
                hostname,
            ));
        let menu: Box<
            dyn term_wm_core::components::MenuOverlay<term_wm_core::actions::TermWmAction>,
        > = Box::new(term_wm_sys_ui_components::WmMenuOverlay::new());
        WmBuilder::standalone()
            .app_ctx(std::sync::Arc::new(app_ctx))
            .build(Some(top_panel), Some(bottom_panel), Some(menu))
    }

    fn new_embedded(app_ctx: term_wm_core::app_context::AppContext) -> Self {
        let hostname = app_ctx.hostname.as_deref();
        let top_panel: Box<dyn term_wm_core::top_panel_trait::TopPanel<WindowKey>> = Box::new(
            term_wm_sys_ui_components::WmTopPanelComponent::new(&app_ctx.app_name),
        );
        let bottom_panel: Box<dyn term_wm_core::bottom_panel_trait::BottomPanel> =
            Box::new(term_wm_sys_ui_components::WmBottomPanelComponent::new(
                &app_ctx.app_name,
                &app_ctx.app_version,
                hostname,
            ));
        WmBuilder::embedded()
            .app_ctx(std::sync::Arc::new(app_ctx))
            .build(Some(top_panel), Some(bottom_panel), None)
    }
}
