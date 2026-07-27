use std::any::TypeId;

use crate::components::{Component, Overlay, WmComponent};
use crate::events::Event;
use term_wm_layout_engine::LayoutRect;

use super::{OverlayKey, WindowManager};
use crate::actions::{ConfirmAction, EventResult, TermWmAction};
use crate::window::window_manager::system_tags;

impl<C: Component<TermWmAction>, L: WmComponent, O: Overlay<TermWmAction>> WindowManager<C, L, O> {
    pub fn close_exit_confirm(&mut self) {
        if let Some(key) = self.system_overlays.remove(&TypeId::of::<system_tags::ExitConfirm>()) {
            self.overlays.remove(key);
        }
    }

    pub fn exit_confirm_visible(&self) -> bool {
        self.system_overlays.contains_key(&TypeId::of::<system_tags::ExitConfirm>())
    }

    pub fn help_overlay_visible(&self) -> bool {
        self.system_overlays.contains_key(&TypeId::of::<system_tags::HelpOverlay>())
    }

    pub fn help_key(&self) -> Option<OverlayKey> {
        self.get_overlay::<system_tags::HelpOverlay>()
    }

    pub fn command_palette_key(&self) -> Option<OverlayKey> {
        self.get_overlay::<system_tags::CommandPalette>()
    }

    pub fn close_help_overlay(&mut self) {
        if let Some(key) = self.system_overlays.remove(&TypeId::of::<system_tags::HelpOverlay>()) {
            self.overlays.remove(key);
        }
        self.input_mode = crate::actions::WmInputMode::Passthrough;
    }

    pub fn handle_help_event(&mut self, event: &Event) -> bool {
        if let Event::Mouse(mouse) = event {
            self.hover = Some((mouse.column, mouse.row));
        }
        let Some(key) = self.get_overlay::<system_tags::HelpOverlay>() else {
            return false;
        };
        let ctx = self
            .component_context(true)
            .with_overlay(true)
            .with_screen_area(self.managed_area());
        let Some(boxed) = self.overlays.get_mut(key) else {
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
            self.close_help_overlay();
        }
        was_handled
    }

    pub fn handle_exit_confirm_event(&mut self, event: &Event) -> Option<ConfirmAction> {
        if let Event::Mouse(mouse) = event {
            self.hover = Some((mouse.column, mouse.row));
        }
        self.overlays
            .get_mut(self.get_overlay::<system_tags::ExitConfirm>()?)?
            .handle_confirm_event(event)
    }

    pub fn command_palette_visible(&self) -> bool {
        self.system_overlays.contains_key(&TypeId::of::<system_tags::CommandPalette>())
    }

    pub fn command_palette_bounds(&self) -> Option<LayoutRect> {
        self.get_overlay::<system_tags::CommandPalette>()
            .and_then(|key| self.overlays.get(key))
            .and_then(|o| o.render_area())
    }

    pub fn help_overlay_bounds(&self) -> Option<LayoutRect> {
        self.get_overlay::<system_tags::HelpOverlay>()
            .and_then(|key| self.overlays.get(key))
            .and_then(|o| o.render_area())
    }

    pub fn close_command_palette(&mut self) {
        if let Some(key) = self.system_overlays.remove(&TypeId::of::<system_tags::CommandPalette>()) {
            self.overlays.remove(key);
        }
        self.input_mode = crate::actions::WmInputMode::Passthrough;
    }

    pub fn handle_command_palette_event(&mut self, event: &Event) -> Option<TermWmAction> {
        if let Event::Mouse(mouse) = event {
            self.hover = Some((mouse.column, mouse.row));
        }
        let ctx = self
            .component_context(false)
            .with_overlay(true)
            .with_screen_area(self.managed_area())
            .with_hover_pos(self.hover);

        let key = self.get_overlay::<system_tags::CommandPalette>()?;
        let palette = self.overlays.get_mut(key)?;
        match palette.handle_events(event, &ctx) {
            EventResult::Action(action) => {
                self.close_command_palette();
                Some(action)
            }
            _ => None,
        }
    }
}
