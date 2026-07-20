use super::WindowManager;

impl WindowManager {
    pub fn command_menu_visible(&self) -> bool {
        self.overlays.contains_key(&super::OverlayId::CommandPalette)
    }

    pub fn close_command_menu(&mut self) {
        self.overlays.remove(&super::OverlayId::CommandPalette);
    }
}
