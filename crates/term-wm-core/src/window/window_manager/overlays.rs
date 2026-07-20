use crate::events::Event;

use super::WindowManager;
use crate::actions::{ConfirmAction, EventResult, TermWmAction};
use crate::components::Overlay;

impl WindowManager {
    pub fn close_exit_confirm(&mut self) {
        self.overlays.remove(&super::OverlayId::ExitConfirm);
    }

    pub fn exit_confirm_visible(&self) -> bool {
        self.overlays.contains_key(&super::OverlayId::ExitConfirm)
    }

    pub fn help_overlay_visible(&self) -> bool {
        self.overlays.contains_key(&super::OverlayId::Help)
    }

    pub fn close_help_overlay(&mut self) {
        self.overlays.remove(&super::OverlayId::Help);
    }

    // TODO: Drag handling/clipboard selection, etc. should be moved into the component
    pub fn handle_help_event(&mut self, event: &Event) -> bool {
        if let Event::Mouse(mouse) = event {
            self.hover = Some((mouse.column, mouse.row));
        }
        let ctx = self
            .component_context(true)
            .with_overlay(true)
            .with_screen_area(self.managed_area());
        let Some(boxed) = self.overlays.get_mut(&super::OverlayId::Help) else {
            return false;
        };

        let was_dragging = boxed.selection_status().dragging;
        let result = boxed.handle_events(event, &ctx);
        let was_handled = !result.is_ignored();

        if let EventResult::Action(action) = result {
            let mut queue = std::collections::VecDeque::new();
            boxed.update(action, &ctx, &mut queue);
            while let Some((_key, _action)) = queue.pop_front() {}
        }

        let status = boxed.selection_status();
        let still_visible = boxed.visible();
        let text = if status.active || status.dragging {
            boxed.selection_text()
        } else {
            None
        };

        self.set_selection_snapshot(status.active, status.dragging, text);
        if was_dragging && !status.dragging && status.active {
            self.copy_selection_to_clipboard();
        }

        if !still_visible {
            self.overlays.remove(&super::OverlayId::Help);
        }
        was_handled
    }

    pub fn handle_exit_confirm_event(&mut self, event: &Event) -> Option<ConfirmAction> {
        if let Event::Mouse(mouse) = event {
            self.hover = Some((mouse.column, mouse.row));
        }
        let comp = self.overlays.get_mut(&super::OverlayId::ExitConfirm)?;
        let overlay: &mut dyn Overlay<TermWmAction> = &mut **comp;
        overlay.handle_confirm_event(event)
    }

    pub fn command_palette_visible(&self) -> bool {
        self.overlays
            .contains_key(&super::OverlayId::CommandPalette)
    }

    pub fn close_command_palette(&mut self) {
        self.overlays.remove(&super::OverlayId::CommandPalette);
    }

    pub fn handle_command_palette_event(&mut self, event: &Event) -> Option<TermWmAction> {
        // Build context BEFORE mutable borrow of overlays
        if let Event::Mouse(mouse) = event {
            self.hover = Some((mouse.column, mouse.row));
        }
        let ctx = self
            .component_context(false)
            .with_overlay(true)
            .with_screen_area(self.managed_area())
            .with_hover_pos(self.hover);

        let palette = self.overlays.get_mut(&super::OverlayId::CommandPalette)?;
        // handle_events is on Component (supertrait of Overlay)
        match palette.handle_events(event, &ctx) {
            EventResult::Action(action) => {
                self.close_command_palette();
                Some(action)
            }
            _ => None,
        }
    }
}
