pub use term_wm_core::components::NoopComponent;

use term_wm_core::actions::TermWmAction;
use term_wm_core::components::Component;
use term_wm_core::impl_component_delegate;
use term_wm_ui_facade::core_component::CoreWmComponent;

#[allow(clippy::large_enum_variant)]
pub enum AppRootComponent<C = NoopComponent> {
    Core(CoreWmComponent),
    Custom(C),
}

impl_component_delegate!(AppRootComponent, param: C, bound: Component<TermWmAction>, variants: { Core, Custom });
