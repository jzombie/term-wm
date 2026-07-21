use term_wm_core::impl_component_delegate;
use term_wm_core::impl_overlay_delegate;
use term_wm_sys_ui_components::wm_help_overlay::WmHelpOverlayComponent;
use term_wm_sys_ui_components::wm_command_palette::WmCommandPaletteComponent;
use term_wm_ui_components::confirm_overlay::ConfirmOverlayComponent;

pub enum OverlayComponent {
    Help(WmHelpOverlayComponent),
    CommandPalette(WmCommandPaletteComponent),
    ExitConfirm(ConfirmOverlayComponent),
}

impl_component_delegate!(OverlayComponent {
    Help, CommandPalette, ExitConfirm,
});

impl_overlay_delegate!(OverlayComponent {
    Help, CommandPalette, ExitConfirm,
});
