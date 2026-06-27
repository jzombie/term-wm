#[allow(ambiguous_glob_reexports)]
pub use term_wm_core::*;
#[allow(ambiguous_glob_reexports)]
pub use term_wm_ui_components::*;
pub mod tracing_sub;

pub trait WindowManagerExt<Id> {
    fn new_standalone(current: Id, app_ctx: term_wm_core::app_context::AppContext) -> Self;
    fn new_embedded(current: Id, app_ctx: term_wm_core::app_context::AppContext) -> Self;
}

impl<Id: Copy + Eq + Ord + std::fmt::Debug + 'static> WindowManagerExt<Id>
    for term_wm_core::window::WindowManager<Id>
{
    fn new_standalone(current: Id, app_ctx: term_wm_core::app_context::AppContext) -> Self {
        let hostname = app_ctx.hostname.as_deref();
        let panel: Box<dyn term_wm_core::panel_trait::Panel<term_wm_core::window::WindowId<Id>>> =
            Box::new(term_wm_ui_components::PanelComponent::new(&app_ctx.app_name, &app_ctx.app_version, hostname));
        let menu: Box<dyn term_wm_core::components::MenuOverlay<term_wm_core::window::WmMenuAction>> =
            Box::new(term_wm_sys_ui_components::WmMenuOverlay::new());
        term_wm_core::window::WindowManager::with_config(
            current,
            term_wm_core::wm_config::WmConfig::standalone(),
            std::sync::Arc::new(app_ctx),
            panel,
            menu,
        )
    }

    fn new_embedded(current: Id, app_ctx: term_wm_core::app_context::AppContext) -> Self {
        let hostname = app_ctx.hostname.as_deref();
        let panel: Box<dyn term_wm_core::panel_trait::Panel<term_wm_core::window::WindowId<Id>>> =
            Box::new(term_wm_ui_components::PanelComponent::new(&app_ctx.app_name, &app_ctx.app_version, hostname));
        term_wm_core::window::WindowManager::with_config(
            current,
            term_wm_core::wm_config::WmConfig::embedded(),
            std::sync::Arc::new(app_ctx),
            panel,
            Box::new(term_wm_core::window::NoopMenu),
        )
    }
}
