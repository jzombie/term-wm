use crossterm::event::{Event, MouseEventKind};

use super::WindowManager;
use crate::actions::{EventResult, TermWmAction};
use crate::components::Component;

impl WindowManager {
    pub fn open_command_menu(&mut self) {
        self.command_menu_visible = true;
        self.command_menu_opened_at = Some(std::time::Instant::now());
        if let Some(menu) = &mut self.command_menu {
            menu.restore();
        }
    }

    pub fn open_command_menu_no_passthrough(&mut self) {
        self.command_menu_visible = true;
        self.command_menu_opened_at = None;
        if let Some(menu) = &mut self.command_menu {
            menu.restore();
        }
    }

    pub fn close_command_menu(&mut self) {
        self.command_menu_visible = false;
        self.command_menu_opened_at = None;
        if let Some(menu) = &mut self.command_menu {
            menu.restore();
        }
    }

    pub fn command_menu_visible(&self) -> bool {
        self.command_menu_visible
    }

    pub fn fold_menu(&mut self) {
        if let Some(menu) = &mut self.command_menu {
            menu.outline();
        }
    }

    pub fn handle_wm_menu_event(&mut self, event: &Event) -> Option<TermWmAction> {
        if !self.command_menu_visible() {
            return None;
        }

        if let Event::Mouse(mouse) = event
            && matches!(mouse.kind, MouseEventKind::Down(_))
            && self
                .top_panel
                .as_ref()
                .is_some_and(|p| p.menu_icon_contains_point(mouse.column, mouse.row))
        {
            return None; // handled by chrome overlay toggle
        }

        let ctx = self.component_context(false).with_overlay(true);
        let Some(menu) = &mut self.command_menu else {
            return None;
        };
        let comp: &mut dyn Component<TermWmAction> = &mut **menu;
        if let EventResult::Action(action) = comp.handle_events(event, &ctx) {
            // Process the action through update
            let mut queue = std::collections::VecDeque::new();
            comp.update(action, &ctx, &mut queue);
            // Drain any cascading actions
            while let Some((_key, _action)) = queue.pop_front() {}
        }

        if let Some(action) = menu.selected_action() {
            return Some(action.clone());
        }

        if let Event::Mouse(mouse) = event
            && matches!(mouse.kind, MouseEventKind::Down(_))
        {
            return None; // handled by chrome layer
        }

        None
    }

    pub fn wm_menu_consumes_event(&self, event: &Event) -> bool {
        if !self.command_menu_visible() {
            return false;
        }
        let Event::Key(key) = event else {
            return false;
        };
        let kb = self.keybindings();
        kb.matches(TermWmAction::MenuUp, key)
            || kb.matches(TermWmAction::MenuDown, key)
            || kb.matches(TermWmAction::MenuSelect, key)
            || kb.matches(TermWmAction::MenuNext, key)
            || kb.matches(TermWmAction::MenuPrev, key)
    }
}
