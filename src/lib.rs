pub use term_wm_core::*;
pub use term_wm_ui_components::*;
pub mod prelude;
pub mod term_wm_app;
pub mod tracing_sub;
pub mod widget_adapter;
pub use widget_adapter::{WidgetAdapter, StatefulWidgetAdapter};

use std::sync::Arc;

use term_wm_core::config::{AppBuilder, Standalone};
use term_wm_core::window::WindowManager;

/// Create a standalone `AppBuilder` pre-loaded with default system chrome.
///
/// Feature-gated on `sys-ui`: when disabled, use `AppBuilder::bare_standalone()`
/// and inject components via IoC.
#[cfg(feature = "sys-ui")]
pub fn standalone_builder() -> AppBuilder<Standalone> {
    use term_wm_sys_ui_components::{
        WmBottomPanelComponent, WmMenuOverlay, WmTopPanelComponent,
    };

    AppBuilder::bare_standalone()
        .top_panel(Box::new(WmTopPanelComponent::new("")))
        .bottom_panel(Box::new(WmBottomPanelComponent::new("", "", None)))
        .command_menu(Box::new(WmMenuOverlay::new()))
}

pub trait WindowManagerExt: Sized {
    fn new_standalone(app_ctx: AppContext) -> Self;
    fn new_bare_standalone(app_ctx: AppContext) -> Self;
    fn new_embedded(app_ctx: AppContext) -> Self;
}

impl WindowManagerExt for WindowManager {
    #[cfg(feature = "sys-ui")]
    fn new_standalone(app_ctx: AppContext) -> Self {
        let app_name = app_ctx.app_name.clone();
        let app_version = app_ctx.app_version.clone();
        let hostname = app_ctx.hostname.clone();

        use term_wm_sys_ui_components::{
            WmBottomPanelComponent, WmMenuOverlay, WmTopPanelComponent,
        };

        AppBuilder::bare_standalone()
            .app_ctx(Arc::new(app_ctx))
            .top_panel(Box::new(WmTopPanelComponent::new(&app_name)))
            .bottom_panel(Box::new(WmBottomPanelComponent::new(
                &app_name,
                &app_version,
                hostname.as_deref(),
            )))
            .command_menu(Box::new(WmMenuOverlay::new()))
            .build()
            .expect("standalone build")
    }

    #[cfg(not(feature = "sys-ui"))]
    fn new_standalone(app_ctx: AppContext) -> Self {
        Self::new_bare_standalone(app_ctx)
    }

    fn new_bare_standalone(app_ctx: AppContext) -> Self {
        AppBuilder::bare_standalone()
            .app_ctx(Arc::new(app_ctx))
            .build()
            .expect("bare standalone build")
    }

    fn new_embedded(app_ctx: AppContext) -> Self {
        AppBuilder::embedded()
            .app_ctx(Arc::new(app_ctx))
            .build()
            .expect("embedded build")
    }
}
