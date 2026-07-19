use super::WindowManager;
use crate::components::ComponentAction;

impl WindowManager {
    pub fn open_command_menu(&mut self) {
        self.command_menu_visible = true;
        self.command_menu_opened_at = Some(std::time::Instant::now());
        if let Some(menu) =
            self.get_semantic_component_mut(super::layer_manager::ComponentTag::CommandPalette)
        {
            menu.process_action(&ComponentAction::Restore);
        }
    }

    pub fn open_command_menu_no_passthrough(&mut self) {
        self.open_command_menu();
        self.command_menu_opened_at = None;
    }

    pub fn close_command_menu(&mut self) {
        self.command_menu_visible = false;
        self.command_menu_opened_at = None;
    }

    pub fn command_menu_visible(&self) -> bool {
        self.command_menu_visible
    }
}
