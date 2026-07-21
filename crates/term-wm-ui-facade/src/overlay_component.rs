use std::collections::VecDeque;

use term_wm_core::Rect;
use term_wm_core::actions::{ConfirmAction, EventResult, TermWmAction};
use term_wm_core::app_context::AppContext;
use term_wm_core::component_context::ComponentContext;
use term_wm_core::components::{Component, MenuItem, Overlay, SelectionStatus};
use term_wm_core::events::Event;
use term_wm_core::hitbox_registry::HitboxId;
use term_wm_core::window::WindowKey;
use term_wm_render::RenderBackend;
use term_wm_sys_ui_components::WmCommandPaletteComponent;
use term_wm_sys_ui_components::wm_help_overlay::WmHelpOverlayComponent;
use term_wm_ui_components::confirm_overlay::ConfirmOverlayComponent;

pub enum OverlayComponent {
    Help(WmHelpOverlayComponent),
    CommandPalette(WmCommandPaletteComponent),
    ExitConfirm(ConfirmOverlayComponent),
}

impl std::fmt::Debug for OverlayComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Help(_) => f.debug_tuple("Help").finish(),
            Self::CommandPalette(_) => f.debug_tuple("CommandPalette").finish(),
            Self::ExitConfirm(_) => f.debug_tuple("ExitConfirm").finish(),
        }
    }
}

impl Component<TermWmAction> for OverlayComponent {
    fn init(&mut self) {
        match self {
            Self::Help(c) => c.init(),
            Self::CommandPalette(c) => c.init(),
            Self::ExitConfirm(c) => c.init(),
        }
    }

    fn on_mount(&mut self, key: WindowKey, app: &AppContext) {
        match self {
            Self::Help(c) => c.on_mount(key, app),
            Self::CommandPalette(c) => c.on_mount(key, app),
            Self::ExitConfirm(c) => c.on_mount(key, app),
        }
    }

    fn hitbox_id(&self) -> Option<HitboxId> {
        match self {
            Self::Help(c) => c.hitbox_id(),
            Self::CommandPalette(c) => c.hitbox_id(),
            Self::ExitConfirm(c) => c.hitbox_id(),
        }
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match self {
            Self::Help(c) => c.handle_events(event, ctx),
            Self::CommandPalette(c) => c.handle_events(event, ctx),
            Self::ExitConfirm(c) => c.handle_events(event, ctx),
        }
    }

    fn on_mouse_press(
        &mut self,
        col: u16,
        row: u16,
        button: term_wm_core::events::MouseButton,
        modifiers: term_wm_core::events::KeyModifiers,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match self {
            Self::Help(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
            Self::CommandPalette(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
            Self::ExitConfirm(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
        }
    }

    fn on_mouse_release(
        &mut self,
        col: u16,
        row: u16,
        button: term_wm_core::events::MouseButton,
        modifiers: term_wm_core::events::KeyModifiers,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match self {
            Self::Help(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
            Self::CommandPalette(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
            Self::ExitConfirm(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
        }
    }

    fn on_mouse_drag(
        &mut self,
        col: u16,
        row: u16,
        button: term_wm_core::events::MouseButton,
        modifiers: term_wm_core::events::KeyModifiers,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match self {
            Self::Help(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
            Self::CommandPalette(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
            Self::ExitConfirm(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
        }
    }

    fn on_mouse_scroll(
        &mut self,
        col: u16,
        row: u16,
        kind: term_wm_core::events::MouseEventKind,
        modifiers: term_wm_core::events::KeyModifiers,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match self {
            Self::Help(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
            Self::CommandPalette(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
            Self::ExitConfirm(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
        }
    }

    fn on_mouse_move(
        &mut self,
        col: u16,
        row: u16,
        modifiers: term_wm_core::events::KeyModifiers,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match self {
            Self::Help(c) => c.on_mouse_move(col, row, modifiers, ctx),
            Self::CommandPalette(c) => c.on_mouse_move(col, row, modifiers, ctx),
            Self::ExitConfirm(c) => c.on_mouse_move(col, row, modifiers, ctx),
        }
    }

    fn on_key(&mut self, event: &Event, ctx: &ComponentContext) -> EventResult<TermWmAction> {
        match self {
            Self::Help(c) => c.on_key(event, ctx),
            Self::CommandPalette(c) => c.on_key(event, ctx),
            Self::ExitConfirm(c) => c.on_key(event, ctx),
        }
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        match self {
            Self::Help(c) => c.update(action, ctx, actions),
            Self::CommandPalette(c) => c.update(action, ctx, actions),
            Self::ExitConfirm(c) => c.update(action, ctx, actions),
        }
    }

    fn render(
        &mut self,
        backend: &mut dyn RenderBackend,
        area: Rect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        match self {
            Self::Help(c) => c.render(backend, area, ctx, registry),
            Self::CommandPalette(c) => c.render(backend, area, ctx, registry),
            Self::ExitConfirm(c) => c.render(backend, area, ctx, registry),
        }
    }

    fn destroy(&mut self) {
        match self {
            Self::Help(c) => c.destroy(),
            Self::CommandPalette(c) => c.destroy(),
            Self::ExitConfirm(c) => c.destroy(),
        }
    }

    fn clear_selection(&mut self) {
        match self {
            Self::Help(c) => c.clear_selection(),
            Self::CommandPalette(c) => c.clear_selection(),
            Self::ExitConfirm(c) => c.clear_selection(),
        }
    }

    fn selection_status(&self) -> SelectionStatus {
        match self {
            Self::Help(c) => c.selection_status(),
            Self::CommandPalette(c) => c.selection_status(),
            Self::ExitConfirm(c) => c.selection_status(),
        }
    }

    fn selection_text(&self) -> Option<String> {
        match self {
            Self::Help(c) => c.selection_text(),
            Self::CommandPalette(c) => c.selection_text(),
            Self::ExitConfirm(c) => c.selection_text(),
        }
    }

    fn desired_height(&self, width: u16) -> u16 {
        match self {
            Self::Help(c) => c.desired_height(width),
            Self::CommandPalette(c) => c.desired_height(width),
            Self::ExitConfirm(c) => c.desired_height(width),
        }
    }

    fn take_pending_title(&mut self) -> Option<String> {
        match self {
            Self::Help(c) => c.take_pending_title(),
            Self::CommandPalette(c) => c.take_pending_title(),
            Self::ExitConfirm(c) => c.take_pending_title(),
        }
    }

    fn take_teardown_parts(
        &mut self,
    ) -> Option<(
        Box<dyn std::any::Any + Send + Sync>,
        std::thread::JoinHandle<()>,
    )> {
        match self {
            Self::Help(c) => c.take_teardown_parts(),
            Self::CommandPalette(c) => c.take_teardown_parts(),
            Self::ExitConfirm(c) => c.take_teardown_parts(),
        }
    }

    fn set_selection_enabled(&mut self, enabled: bool) {
        match self {
            Self::Help(c) => c.set_selection_enabled(enabled),
            Self::CommandPalette(c) => c.set_selection_enabled(enabled),
            Self::ExitConfirm(c) => c.set_selection_enabled(enabled),
        }
    }

    fn paste(&mut self, text: &str) -> bool {
        match self {
            Self::Help(c) => c.paste(text),
            Self::CommandPalette(c) => c.paste(text),
            Self::ExitConfirm(c) => c.paste(text),
        }
    }
}

impl Overlay<TermWmAction> for OverlayComponent {
    fn visible(&self) -> bool {
        match self {
            Self::Help(c) => c.visible(),
            Self::CommandPalette(c) => c.visible(),
            Self::ExitConfirm(c) => c.visible(),
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        match self {
            Self::Help(c) => c.as_any_mut(),
            Self::CommandPalette(c) => c.as_any_mut(),
            Self::ExitConfirm(c) => c.as_any_mut(),
        }
    }

    fn shadow_rect(&self, area: Rect) -> Option<Rect> {
        match self {
            Self::Help(c) => c.shadow_rect(area),
            Self::CommandPalette(c) => c.shadow_rect(area),
            Self::ExitConfirm(c) => c.shadow_rect(area),
        }
    }

    fn handle_confirm_event(&mut self, event: &Event) -> Option<ConfirmAction> {
        match self {
            Self::Help(c) => c.handle_confirm_event(event),
            Self::CommandPalette(c) => c.handle_confirm_event(event),
            Self::ExitConfirm(c) => c.handle_confirm_event(event),
        }
    }

    fn mark_dirty(&mut self) {
        match self {
            Self::Help(c) => c.mark_dirty(),
            Self::CommandPalette(c) => c.mark_dirty(),
            Self::ExitConfirm(c) => c.mark_dirty(),
        }
    }

    fn set_menu_items(&mut self, items: Vec<MenuItem<TermWmAction>>) {
        match self {
            Self::Help(c) => c.set_menu_items(items),
            Self::CommandPalette(c) => c.set_menu_items(items),
            Self::ExitConfirm(c) => c.set_menu_items(items),
        }
    }
}
