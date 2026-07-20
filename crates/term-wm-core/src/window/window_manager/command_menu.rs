use super::WindowManager;

impl WindowManager {
    pub fn command_menu_visible(&self) -> bool {
        self.command_palette_visible()
    }

    pub fn close_command_menu(&mut self) {
        self.close_command_palette();
    }
}
