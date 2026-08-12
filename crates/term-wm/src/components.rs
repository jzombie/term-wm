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

impl<C> AppRootComponent<C> {
    /// Returns true for app-owned (`Custom`) windows, false for core/system
    /// windows.
    pub fn is_custom(&self) -> bool {
        matches!(self, AppRootComponent::Custom(_))
    }

    /// Returns true for core/system windows, false for app-owned (`Custom`)
    /// windows.
    pub fn is_core(&self) -> bool {
        matches!(self, AppRootComponent::Core(_))
    }
}

impl_component_delegate!(AppRootComponent, param: C, bound: Component<TermWmAction>, variants: { Core, Custom });

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_and_core_predicates() {
        let custom = AppRootComponent::Custom(NoopComponent);
        assert!(custom.is_custom());
        assert!(!custom.is_core());

        let core = AppRootComponent::<NoopComponent>::Core(CoreWmComponent::Noop(NoopComponent));
        assert!(core.is_core());
        assert!(!core.is_custom());
    }
}
