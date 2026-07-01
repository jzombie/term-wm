use crossterm::event::{Event, MouseEventKind};

use super::{SystemWindowId, WindowManager};
use crate::components::Overlay;

impl WindowManager {
    pub(crate) fn system_window_entry(
        &self,
        id: SystemWindowId,
    ) -> Option<&super::SystemWindowEntry> {
        self.system_windows.get(&id)
    }

    pub(crate) fn system_window_entry_mut(
        &mut self,
        id: SystemWindowId,
    ) -> Option<&mut super::SystemWindowEntry> {
        self.system_windows.get_mut(&id)
    }

    pub(super) fn system_window_visible(&self, id: SystemWindowId) -> bool {
        self.system_window_entry(id)
            .map(|entry| entry.visible())
            .unwrap_or(false)
    }

    pub(super) fn set_system_window_visible(&mut self, id: SystemWindowId, visible: bool) {
        if let Some(entry) = self.system_window_entry_mut(id) {
            entry.set_visible(visible);
        }
    }

    pub(super) fn show_system_window(&mut self, id: SystemWindowId) {
        if self.system_window_visible(id) {
            return;
        }
        if self.system_window_entry(id).is_none() {
            return;
        }
        self.set_system_window_visible(id, true);
    }

    pub(super) fn hide_system_window(&mut self, id: SystemWindowId) {
        if !self.system_window_visible(id) {
            return;
        }
        self.set_system_window_visible(id, false);
    }

    pub(super) fn dispatch_system_window_event(
        &mut self,
        id: SystemWindowId,
        event: &Event,
    ) -> bool {
        if let Some(localized) = self.localize_event_system(id, event) {
            return self.dispatch_system_window_event_localized(id, &localized);
        }
        self.system_window_entry_mut(id)
            .map(|entry| entry.handle_event(event))
            .unwrap_or(false)
    }

    pub(super) fn dispatch_system_window_event_localized(
        &mut self,
        id: SystemWindowId,
        event: &Event,
    ) -> bool {
        self.system_window_entry_mut(id)
            .map(|entry| entry.handle_event(event))
            .unwrap_or(false)
    }

    pub(super) fn localize_event_system(
        &self,
        id: SystemWindowId,
        event: &Event,
    ) -> Option<Event> {
        // Find the region for this system window and localize the event
        let region = self.regions_for_system(id);
        match event {
            Event::Mouse(mouse) => {
                let local_x = mouse.column.saturating_sub(region.x);
                let local_y = mouse.row.saturating_sub(region.y);
                Some(Event::Mouse(crossterm::event::MouseEvent {
                    column: local_x,
                    row: local_y,
                    ..*mouse
                }))
            }
            _ => Some(event.clone()),
        }
    }

    fn regions_for_system(&self, id: SystemWindowId) -> ratatui::prelude::Rect {
        // Use a rectangular region for the system window if we have one
        // System windows use the full managed area for now
        self.managed_area
    }

    pub(super) fn render_system_window_entry(
        &mut self,
        frame: &mut crate::ui::UiFrame<'_>,
        draw: super::SystemWindowDraw,
    ) {
        if let Some(entry) = self.system_window_entry_mut(draw.id) {
            entry.render(frame, draw.surface, draw.focused);
        }
    }

    pub fn toggle_debug_window(&mut self) {
        if self.system_window_visible(SystemWindowId::DebugLog) {
            self.hide_system_window(SystemWindowId::DebugLog);
        } else {
            self.show_system_window(SystemWindowId::DebugLog);
        }
    }

    pub fn open_debug_window(&mut self) {
        if !self.system_window_visible(SystemWindowId::DebugLog) {
            self.show_system_window(SystemWindowId::DebugLog);
        }
    }

    pub fn debug_window_visible(&self) -> bool {
        self.system_window_visible(SystemWindowId::DebugLog)
    }

    pub fn has_active_system_windows(&self) -> bool {
        self.system_windows.values().any(|w| w.visible()) || !self.overlays.is_empty()
    }

    pub fn has_any_active_windows(&self) -> bool {
        if self.has_active_system_windows() {
            return true;
        }
        if !self.regions.ids().is_empty() {
            return true;
        }
        if !self.z_order.is_empty() {
            return true;
        }
        false
    }

    pub(super) fn handle_system_window_event(&mut self, event: &Event) -> bool {
        if !self.config.wm_overlay_enabled {
            return false;
        }
        match event {
            Event::Mouse(mouse) => {
                if self.managed_draw_order.is_empty() {
                    return false;
                }
                let hit =
                    self.hit_test_region_topmost(mouse.column, mouse.row, &self.managed_draw_order);
                if let Some(key) = hit {
                    if matches!(mouse.kind, MouseEventKind::Down(_)) {
                        self.focus_window_id(key);
                    }
                }
                false
            }
            _ => false,
        }
    }

    pub(super) fn apply_clipboard_selection_state(&mut self, enabled: bool) {
        for entry in self.system_windows.values_mut() {
            entry.set_selection_enabled(enabled);
        }
        for overlay in self.overlays.values_mut() {
            Overlay::set_selection_enabled(&mut **overlay, enabled);
        }
    }
}
