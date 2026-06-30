use crossterm::event::{Event, MouseEventKind};

use super::{SystemWindowId, WindowId, WindowManager};
use crate::components::Overlay;

impl<Id: Copy + Eq + Ord + std::fmt::Debug + 'static> WindowManager<Id> {
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
        if !self.config.wm_overlay_enabled {
            return;
        }
        let window_id = WindowId::system(id);
        let _ = self.window_mut(window_id);
        self.ensure_system_window_in_layout(window_id);
        self.focus_window_id(window_id);
    }

    pub(super) fn hide_system_window(&mut self, id: SystemWindowId) {
        if !self.system_window_visible(id) {
            return;
        }
        let window_id = WindowId::system(id);
        self.set_system_window_visible(id, false);
        self.remove_system_window_from_layout(window_id);
        if *self.wm_focus.current() == window_id {
            self.select_fallback_focus();
        }
    }

    pub(super) fn ensure_system_window_in_layout(&mut self, id: WindowId<Id>) {
        if !self.config.wm_overlay_enabled {
            return;
        }
        if self.layout_contains(id) {
            return;
        }
        let _ = self.window_mut(id);
        if self.managed_layout.is_none() {
            self.managed_layout = Some(crate::layout::TilingLayout::new(
                crate::layout::LayoutNode::leaf(id),
            ));
            return;
        }
        let _ = self.tile_window_id(id);
    }

    pub(super) fn remove_system_window_from_layout(&mut self, id: WindowId<Id>) {
        self.clear_floating_rect(id);
        if let Some(layout) = &mut self.managed_layout {
            if matches!(layout.root(), crate::layout::LayoutNode::Leaf(root_id) if *root_id == id) {
                self.managed_layout = None;
            } else {
                layout.root_mut().remove_leaf(id);
            }
        }
        self.z_order.retain(|window_id| *window_id != id);
        self.managed_draw_order.retain(|window_id| *window_id != id);
    }

    pub(super) fn dispatch_system_window_event(
        &mut self,
        id: SystemWindowId,
        event: &Event,
    ) -> bool {
        if let Some(localized) = self.localize_event_content(WindowId::system(id), event) {
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
        let adjusted = self.adjust_event_for_window(WindowId::system(id), event);
        self.system_window_entry_mut(id)
            .map(|entry| entry.handle_event(&adjusted))
            .unwrap_or(false)
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
        if self.z_order.iter().any(|id| id.as_app().is_some()) {
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
                if let Some(WindowId::System(system_id)) = hit {
                    if !self.system_window_visible(system_id) {
                        return false;
                    }
                    if matches!(mouse.kind, MouseEventKind::Down(_)) {
                        self.focus_window_id(WindowId::system(system_id));
                    }
                    return self.dispatch_system_window_event(system_id, event);
                }
                if matches!(mouse.kind, MouseEventKind::Down(_))
                    && let &WindowId::System(system_id) = self.wm_focus.current()
                    && self.system_window_visible(system_id)
                {
                    self.select_fallback_focus();
                }
                false
            }
            Event::Key(_) => {
                if let &WindowId::System(system_id) = self.wm_focus.current()
                    && self.system_window_visible(system_id)
                {
                    return self.dispatch_system_window_event(system_id, event);
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
