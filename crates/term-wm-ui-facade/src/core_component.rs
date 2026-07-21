use term_wm_core::components::NoopComponent;
use term_wm_core::impl_component_delegate;
use term_wm_ui_components::scroll_view::ScrollViewComponent;
use term_wm_ui_components::terminal::TerminalComponent;

use term_wm_sys_ui_components::wm_debug_log::WmDebugLogComponent;
use term_wm_sys_ui_components::wm_session_manager::WmSessionManagerComponent;
use term_wm_sys_ui_components::wm_system_panel::WmSystemPanelComponent;

pub enum CoreWmComponent {
    Terminal(ScrollViewComponent<TerminalComponent>),
    DebugLog(WmDebugLogComponent),
    SystemPanel(WmSystemPanelComponent),
    SessionManager(WmSessionManagerComponent),
    Noop(NoopComponent),
}

impl_component_delegate!(CoreWmComponent {
    Terminal, DebugLog, SystemPanel, SessionManager, Noop,
});
