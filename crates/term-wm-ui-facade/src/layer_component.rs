use term_wm_core::impl_component_delegate;
use term_wm_core::impl_wm_component_delegate;
use term_wm_sys_ui_components::{
    WmBottomPanelComponent, WmCommandPaletteComponent, WmFabComponent, WmNotificationAreaComponent,
    WmTopPanelComponent,
};

#[allow(clippy::large_enum_variant)]
pub enum LayerComponent {
    TopPanel(WmTopPanelComponent),
    BottomPanel(WmBottomPanelComponent),
    Fab(WmFabComponent),
    NotificationArea(WmNotificationAreaComponent),
    CommandPalette(WmCommandPaletteComponent),
}

impl_component_delegate!(LayerComponent {
    TopPanel, BottomPanel, Fab, NotificationArea, CommandPalette,
});

impl_wm_component_delegate!(LayerComponent {
    TopPanel, BottomPanel, Fab, NotificationArea, CommandPalette,
});
