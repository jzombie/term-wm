use crossterm::event::{Event, MouseEventKind};

use super::WindowManager;
use crate::actions::{EventResult, TermWmAction};
use crate::components::{ComponentAction, ComponentQuery, ComponentResponse};

impl WindowManager {
    pub fn open_command_menu(&mut self) {
        self.command_menu_visible = true;
        self.command_menu_opened_at = Some(std::time::Instant::now());
        if let Some(menu) = &mut self.command_menu_component {
            menu.process_action(&ComponentAction::Restore);
        }
    }

    pub fn open_command_menu_no_passthrough(&mut self) {
        self.command_menu_visible = true;
        self.command_menu_opened_at = None;
        if let Some(menu) = &mut self.command_menu_component {
            menu.process_action(&ComponentAction::Restore);
        }
    }

    pub fn close_command_menu(&mut self) {
        self.command_menu_visible = false;
        self.command_menu_opened_at = None;
        if let Some(menu) = &mut self.command_menu_component {
            menu.process_action(&ComponentAction::Restore);
        }
    }

    pub fn command_menu_visible(&self) -> bool {
        self.command_menu_visible
    }

    pub fn fold_menu(&mut self) {
        if let Some(menu) = &mut self.command_menu_component {
            menu.process_action(&ComponentAction::Outline);
        }
    }

    pub fn handle_wm_menu_event(&mut self, event: &Event) -> Option<TermWmAction> {
        if !self.command_menu_visible() {
            return None;
        }

        if let Event::Mouse(mouse) = event
            && matches!(mouse.kind, MouseEventKind::Down(_))
        {
            // Check if click is on the top panel's menu icon
            if let Some(p) = &self.top_component
                && let ComponentResponse::Rect(Some(rect)) = p.query(&ComponentQuery::MenuIconRect)
                && crate::layout::rect_contains(rect, mouse.column, mouse.row)
            {
                return None; // handled by chrome overlay toggle
            }
        }

        let ctx = self.component_context(false).with_overlay(true);
        let Some(menu) = &mut self.command_menu_component else {
            return None;
        };

        match menu.handle_event(event, &ctx) {
            EventResult::Action(action) => return Some(action),
            EventResult::Consumed => {}
            EventResult::Ignored => {}
        }

        if let ComponentResponse::Action(Some(action)) = menu.query(&ComponentQuery::SelectedAction)
        {
            return Some(action);
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
