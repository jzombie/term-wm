use std::any::Any;
use std::collections::VecDeque;

use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::app_context::AppContext;
use term_wm_core::component_context::ComponentContext;
use term_wm_core::components::{Component, NoopComponent, SelectionStatus};
use term_wm_core::events::Event;
use term_wm_core::hitbox_registry::HitboxId;
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;
use term_wm_render::RenderBackend;
use term_wm_ui_components::scroll_view::ScrollViewComponent;
use term_wm_ui_components::terminal::TerminalComponent;

use crate::wm_debug_log::WmDebugLogComponent;
use crate::wm_session_manager::WmSessionManagerComponent;
use crate::wm_system_panel::WmSystemPanelComponent;

pub enum CoreWmComponent {
    Terminal(ScrollViewComponent<TerminalComponent>),
    DebugLog(WmDebugLogComponent),
    SystemPanel(WmSystemPanelComponent),
    SessionManager(WmSessionManagerComponent),
    Noop(NoopComponent),
}

impl Component<TermWmAction> for CoreWmComponent {
    fn init(&mut self) {
        match self {
            Self::Terminal(c) => c.init(),
            Self::DebugLog(c) => c.init(),
            Self::SystemPanel(c) => c.init(),
            Self::SessionManager(c) => c.init(),
            Self::Noop(c) => c.init(),
        }
    }

    fn on_mount(&mut self, key: WindowKey, app: &AppContext) {
        match self {
            Self::Terminal(c) => c.on_mount(key, app),
            Self::DebugLog(c) => c.on_mount(key, app),
            Self::SystemPanel(c) => c.on_mount(key, app),
            Self::SessionManager(c) => c.on_mount(key, app),
            Self::Noop(c) => c.on_mount(key, app),
        }
    }

    fn hitbox_id(&self) -> Option<HitboxId> {
        match self {
            Self::Terminal(c) => c.hitbox_id(),
            Self::DebugLog(c) => c.hitbox_id(),
            Self::SystemPanel(c) => c.hitbox_id(),
            Self::SessionManager(c) => c.hitbox_id(),
            Self::Noop(c) => c.hitbox_id(),
        }
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match self {
            Self::Terminal(c) => c.handle_events(event, ctx),
            Self::DebugLog(c) => c.handle_events(event, ctx),
            Self::SystemPanel(c) => c.handle_events(event, ctx),
            Self::SessionManager(c) => c.handle_events(event, ctx),
            Self::Noop(c) => c.handle_events(event, ctx),
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
            Self::Terminal(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
            Self::DebugLog(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
            Self::SystemPanel(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
            Self::SessionManager(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
            Self::Noop(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
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
            Self::Terminal(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
            Self::DebugLog(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
            Self::SystemPanel(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
            Self::SessionManager(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
            Self::Noop(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
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
            Self::Terminal(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
            Self::DebugLog(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
            Self::SystemPanel(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
            Self::SessionManager(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
            Self::Noop(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
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
            Self::Terminal(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
            Self::DebugLog(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
            Self::SystemPanel(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
            Self::SessionManager(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
            Self::Noop(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
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
            Self::Terminal(c) => c.on_mouse_move(col, row, modifiers, ctx),
            Self::DebugLog(c) => c.on_mouse_move(col, row, modifiers, ctx),
            Self::SystemPanel(c) => c.on_mouse_move(col, row, modifiers, ctx),
            Self::SessionManager(c) => c.on_mouse_move(col, row, modifiers, ctx),
            Self::Noop(c) => c.on_mouse_move(col, row, modifiers, ctx),
        }
    }

    fn on_key(&mut self, event: &Event, ctx: &ComponentContext) -> EventResult<TermWmAction> {
        match self {
            Self::Terminal(c) => c.on_key(event, ctx),
            Self::DebugLog(c) => c.on_key(event, ctx),
            Self::SystemPanel(c) => c.on_key(event, ctx),
            Self::SessionManager(c) => c.on_key(event, ctx),
            Self::Noop(c) => c.on_key(event, ctx),
        }
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        match self {
            Self::Terminal(c) => c.update(action, ctx, actions),
            Self::DebugLog(c) => c.update(action, ctx, actions),
            Self::SystemPanel(c) => c.update(action, ctx, actions),
            Self::SessionManager(c) => c.update(action, ctx, actions),
            Self::Noop(c) => c.update(action, ctx, actions),
        }
    }

    fn render(
        &mut self,
        backend: &mut dyn RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        match self {
            Self::Terminal(c) => c.render(backend, area, ctx, registry),
            Self::DebugLog(c) => c.render(backend, area, ctx, registry),
            Self::SystemPanel(c) => c.render(backend, area, ctx, registry),
            Self::SessionManager(c) => c.render(backend, area, ctx, registry),
            Self::Noop(c) => c.render(backend, area, ctx, registry),
        }
    }

    fn destroy(&mut self) {
        match self {
            Self::Terminal(c) => c.destroy(),
            Self::DebugLog(c) => c.destroy(),
            Self::SystemPanel(c) => c.destroy(),
            Self::SessionManager(c) => c.destroy(),
            Self::Noop(c) => c.destroy(),
        }
    }

    fn clear_selection(&mut self) {
        match self {
            Self::Terminal(c) => c.clear_selection(),
            Self::DebugLog(c) => c.clear_selection(),
            Self::SystemPanel(c) => c.clear_selection(),
            Self::SessionManager(c) => c.clear_selection(),
            Self::Noop(c) => c.clear_selection(),
        }
    }

    fn selection_status(&self) -> SelectionStatus {
        match self {
            Self::Terminal(c) => c.selection_status(),
            Self::DebugLog(c) => c.selection_status(),
            Self::SystemPanel(c) => c.selection_status(),
            Self::SessionManager(c) => c.selection_status(),
            Self::Noop(c) => c.selection_status(),
        }
    }

    fn selection_text(&self) -> Option<String> {
        match self {
            Self::Terminal(c) => c.selection_text(),
            Self::DebugLog(c) => c.selection_text(),
            Self::SystemPanel(c) => c.selection_text(),
            Self::SessionManager(c) => c.selection_text(),
            Self::Noop(c) => c.selection_text(),
        }
    }

    fn desired_height(&self, width: u16) -> u16 {
        match self {
            Self::Terminal(c) => c.desired_height(width),
            Self::DebugLog(c) => c.desired_height(width),
            Self::SystemPanel(c) => c.desired_height(width),
            Self::SessionManager(c) => c.desired_height(width),
            Self::Noop(c) => c.desired_height(width),
        }
    }

    fn take_pending_title(&mut self) -> Option<String> {
        match self {
            Self::Terminal(c) => c.take_pending_title(),
            Self::DebugLog(c) => c.take_pending_title(),
            Self::SystemPanel(c) => c.take_pending_title(),
            Self::SessionManager(c) => c.take_pending_title(),
            Self::Noop(c) => c.take_pending_title(),
        }
    }

    fn take_teardown_parts(
        &mut self,
    ) -> Option<(Box<dyn Any + Send + Sync>, std::thread::JoinHandle<()>)> {
        match self {
            Self::Terminal(c) => c.take_teardown_parts(),
            Self::DebugLog(c) => c.take_teardown_parts(),
            Self::SystemPanel(c) => c.take_teardown_parts(),
            Self::SessionManager(c) => c.take_teardown_parts(),
            Self::Noop(c) => c.take_teardown_parts(),
        }
    }

    fn set_selection_enabled(&mut self, enabled: bool) {
        match self {
            Self::Terminal(c) => c.set_selection_enabled(enabled),
            Self::DebugLog(c) => c.set_selection_enabled(enabled),
            Self::SystemPanel(c) => c.set_selection_enabled(enabled),
            Self::SessionManager(c) => c.set_selection_enabled(enabled),
            Self::Noop(c) => c.set_selection_enabled(enabled),
        }
    }

    fn paste(&mut self, text: &str) -> bool {
        match self {
            Self::Terminal(c) => c.paste(text),
            Self::DebugLog(c) => c.paste(text),
            Self::SystemPanel(c) => c.paste(text),
            Self::SessionManager(c) => c.paste(text),
            Self::Noop(c) => c.paste(text),
        }
    }
}
