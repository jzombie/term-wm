pub use term_wm_core::components::NoopComponent;

use term_wm_core::impl_component_delegate;
use term_wm_ui_components::svg_image::SvgImageComponent;
use term_wm_ui_facade::core_component::CoreWmComponent;

pub enum AppRootComponent {
    Core(CoreWmComponent),
    SvgImage(SvgImageComponent),
}

impl_component_delegate!(AppRootComponent { Core, SvgImage });
