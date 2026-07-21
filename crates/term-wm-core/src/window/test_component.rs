use std::any::Any;
use std::collections::VecDeque;
use std::sync::Arc;

use slotmap::DefaultKey;

use crate::actions::{EventResult, TermWmAction};
use crate::app_context::AppContext;
use crate::component_context::ComponentContext;
use crate::components::{Component, NoopComponent, SelectionStatus};
use crate::events::Event;
use crate::hitbox_registry::HitboxId;
use crate::window::WindowKey;
use term_wm_layout_engine::LayoutRect;
use term_wm_render::RenderBackend;

pub enum TestComponent {
    Noop(NoopComponent),
    ActionRecorder(ActionRecorder),
    KeyRecorder(KeyRecorder),
    SelComponent(SelComponent),
}

pub struct ActionRecorder {
    pub actions: Vec<TermWmAction>,
}

impl ActionRecorder {
    pub fn new() -> Self {
        Self {
            actions: Vec::new(),
        }
    }
}

impl Component<TermWmAction> for ActionRecorder {
    fn update(
        &mut self,
        action: TermWmAction,
        _ctx: &ComponentContext,
        _actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        self.actions.push(action);
    }
    fn on_mount(&mut self, _key: WindowKey, _app: &AppContext) {}
    fn handle_events(
        &mut self,
        _event: &Event,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
    fn render(
        &mut self,
        _backend: &mut dyn RenderBackend,
        _area: LayoutRect,
        _ctx: &ComponentContext,
        _registry: &mut crate::hitbox_registry::HitboxRegistry,
    ) {
    }
    fn take_teardown_parts(
        &mut self,
    ) -> Option<(Box<dyn Any + Send + Sync>, std::thread::JoinHandle<()>)> {
        None
    }
    fn init(&mut self) {}
    fn hitbox_id(&self) -> Option<HitboxId> {
        None
    }
    fn on_key(&mut self, _event: &Event, _ctx: &ComponentContext) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
    fn destroy(&mut self) {}
    fn clear_selection(&mut self) {}
    fn selection_status(&self) -> SelectionStatus {
        SelectionStatus::Inactive
    }
    fn selection_text(&self) -> Option<String> {
        None
    }
    fn desired_height(&self, _width: u16) -> u16 {
        0
    }
    fn take_pending_title(&mut self) -> Option<String> {
        None
    }
    fn set_selection_enabled(&mut self, _enabled: bool) {}
    fn paste(&mut self, _text: &str) -> bool {
        false
    }
    fn on_mouse_press(
        &mut self,
        _col: u16,
        _row: u16,
        _button: crate::events::MouseButton,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
    fn on_mouse_release(
        &mut self,
        _col: u16,
        _row: u16,
        _button: crate::events::MouseButton,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
    fn on_mouse_drag(
        &mut self,
        _col: u16,
        _row: u16,
        _button: crate::events::MouseButton,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
    fn on_mouse_scroll(
        &mut self,
        _col: u16,
        _row: u16,
        _kind: crate::events::MouseEventKind,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
    fn on_mouse_move(
        &mut self,
        _col: u16,
        _row: u16,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
}

pub struct KeyRecorder {
    pub received_key: Option<Event>,
}

impl Component<TermWmAction> for KeyRecorder {
    fn handle_events(
        &mut self,
        event: &Event,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        if matches!(event, Event::Key(_)) {
            self.received_key = Some(event.clone());
        }
        EventResult::Ignored
    }
    fn on_mount(&mut self, _key: WindowKey, _app: &AppContext) {}
    fn render(
        &mut self,
        _backend: &mut dyn RenderBackend,
        _area: LayoutRect,
        _ctx: &ComponentContext,
        _registry: &mut crate::hitbox_registry::HitboxRegistry,
    ) {
    }
    fn take_teardown_parts(
        &mut self,
    ) -> Option<(Box<dyn Any + Send + Sync>, std::thread::JoinHandle<()>)> {
        None
    }
    fn init(&mut self) {}
    fn hitbox_id(&self) -> Option<HitboxId> {
        None
    }
    fn on_key(&mut self, _event: &Event, _ctx: &ComponentContext) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
    fn update(
        &mut self,
        _action: TermWmAction,
        _ctx: &ComponentContext,
        _actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
    }
    fn destroy(&mut self) {}
    fn clear_selection(&mut self) {}
    fn selection_status(&self) -> SelectionStatus {
        SelectionStatus::Inactive
    }
    fn selection_text(&self) -> Option<String> {
        None
    }
    fn desired_height(&self, _width: u16) -> u16 {
        0
    }
    fn take_pending_title(&mut self) -> Option<String> {
        None
    }
    fn set_selection_enabled(&mut self, _enabled: bool) {}
    fn paste(&mut self, _text: &str) -> bool {
        false
    }
    fn on_mouse_press(
        &mut self,
        _col: u16,
        _row: u16,
        _button: crate::events::MouseButton,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
    fn on_mouse_release(
        &mut self,
        _col: u16,
        _row: u16,
        _button: crate::events::MouseButton,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
    fn on_mouse_drag(
        &mut self,
        _col: u16,
        _row: u16,
        _button: crate::events::MouseButton,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
    fn on_mouse_scroll(
        &mut self,
        _col: u16,
        _row: u16,
        _kind: crate::events::MouseEventKind,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
    fn on_mouse_move(
        &mut self,
        _col: u16,
        _row: u16,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
}

pub struct SelComponent;

impl Component<TermWmAction> for SelComponent {
    fn selection_status(&self) -> SelectionStatus {
        SelectionStatus::Active
    }
    fn selection_text(&self) -> Option<String> {
        Some("selected text".to_string())
    }
    fn on_mount(&mut self, _key: WindowKey, _app: &AppContext) {}
    fn handle_events(
        &mut self,
        _event: &Event,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
    fn render(
        &mut self,
        _backend: &mut dyn RenderBackend,
        _area: LayoutRect,
        _ctx: &ComponentContext,
        _registry: &mut crate::hitbox_registry::HitboxRegistry,
    ) {
    }
    fn take_teardown_parts(
        &mut self,
    ) -> Option<(Box<dyn Any + Send + Sync>, std::thread::JoinHandle<()>)> {
        None
    }
    fn init(&mut self) {}
    fn hitbox_id(&self) -> Option<HitboxId> {
        None
    }
    fn on_key(&mut self, _event: &Event, _ctx: &ComponentContext) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
    fn update(
        &mut self,
        _action: TermWmAction,
        _ctx: &ComponentContext,
        _actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
    }
    fn destroy(&mut self) {}
    fn clear_selection(&mut self) {}
    fn desired_height(&self, _width: u16) -> u16 {
        0
    }
    fn take_pending_title(&mut self) -> Option<String> {
        None
    }
    fn set_selection_enabled(&mut self, _enabled: bool) {}
    fn paste(&mut self, _text: &str) -> bool {
        false
    }
    fn on_mouse_press(
        &mut self,
        _col: u16,
        _row: u16,
        _button: crate::events::MouseButton,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
    fn on_mouse_release(
        &mut self,
        _col: u16,
        _row: u16,
        _button: crate::events::MouseButton,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
    fn on_mouse_drag(
        &mut self,
        _col: u16,
        _row: u16,
        _button: crate::events::MouseButton,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
    fn on_mouse_scroll(
        &mut self,
        _col: u16,
        _row: u16,
        _kind: crate::events::MouseEventKind,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
    fn on_mouse_move(
        &mut self,
        _col: u16,
        _row: u16,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }
}

impl Component<TermWmAction> for TestComponent {
    fn init(&mut self) {
        match self {
            Self::Noop(c) => c.init(),
            Self::ActionRecorder(c) => c.init(),
            Self::KeyRecorder(c) => c.init(),
            Self::SelComponent(c) => c.init(),
        }
    }
    fn on_mount(&mut self, key: WindowKey, app: &AppContext) {
        match self {
            Self::Noop(c) => c.on_mount(key, app),
            Self::ActionRecorder(c) => c.on_mount(key, app),
            Self::KeyRecorder(c) => c.on_mount(key, app),
            Self::SelComponent(c) => c.on_mount(key, app),
        }
    }
    fn hitbox_id(&self) -> Option<HitboxId> {
        match self {
            Self::Noop(c) => c.hitbox_id(),
            Self::ActionRecorder(c) => c.hitbox_id(),
            Self::KeyRecorder(c) => c.hitbox_id(),
            Self::SelComponent(c) => c.hitbox_id(),
        }
    }
    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match self {
            Self::Noop(c) => c.handle_events(event, ctx),
            Self::ActionRecorder(c) => c.handle_events(event, ctx),
            Self::KeyRecorder(c) => c.handle_events(event, ctx),
            Self::SelComponent(c) => c.handle_events(event, ctx),
        }
    }
    fn on_key(&mut self, event: &Event, ctx: &ComponentContext) -> EventResult<TermWmAction> {
        match self {
            Self::Noop(c) => c.on_key(event, ctx),
            Self::ActionRecorder(c) => c.on_key(event, ctx),
            Self::KeyRecorder(c) => c.on_key(event, ctx),
            Self::SelComponent(c) => c.on_key(event, ctx),
        }
    }
    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        match self {
            Self::Noop(c) => c.update(action, ctx, actions),
            Self::ActionRecorder(c) => c.update(action, ctx, actions),
            Self::KeyRecorder(c) => c.update(action, ctx, actions),
            Self::SelComponent(c) => c.update(action, ctx, actions),
        }
    }
    fn render(
        &mut self,
        backend: &mut dyn RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut crate::hitbox_registry::HitboxRegistry,
    ) {
        match self {
            Self::Noop(c) => c.render(backend, area, ctx, registry),
            Self::ActionRecorder(c) => c.render(backend, area, ctx, registry),
            Self::KeyRecorder(c) => c.render(backend, area, ctx, registry),
            Self::SelComponent(c) => c.render(backend, area, ctx, registry),
        }
    }
    fn destroy(&mut self) {
        match self {
            Self::Noop(c) => c.destroy(),
            Self::ActionRecorder(c) => c.destroy(),
            Self::KeyRecorder(c) => c.destroy(),
            Self::SelComponent(c) => c.destroy(),
        }
    }
    fn clear_selection(&mut self) {
        match self {
            Self::Noop(c) => c.clear_selection(),
            Self::ActionRecorder(c) => c.clear_selection(),
            Self::KeyRecorder(c) => c.clear_selection(),
            Self::SelComponent(c) => c.clear_selection(),
        }
    }
    fn selection_status(&self) -> SelectionStatus {
        match self {
            Self::Noop(c) => c.selection_status(),
            Self::ActionRecorder(c) => c.selection_status(),
            Self::KeyRecorder(c) => c.selection_status(),
            Self::SelComponent(c) => c.selection_status(),
        }
    }
    fn selection_text(&self) -> Option<String> {
        match self {
            Self::Noop(c) => c.selection_text(),
            Self::ActionRecorder(c) => c.selection_text(),
            Self::KeyRecorder(c) => c.selection_text(),
            Self::SelComponent(c) => c.selection_text(),
        }
    }
    fn desired_height(&self, width: u16) -> u16 {
        match self {
            Self::Noop(c) => c.desired_height(width),
            Self::ActionRecorder(c) => c.desired_height(width),
            Self::KeyRecorder(c) => c.desired_height(width),
            Self::SelComponent(c) => c.desired_height(width),
        }
    }
    fn take_pending_title(&mut self) -> Option<String> {
        match self {
            Self::Noop(c) => c.take_pending_title(),
            Self::ActionRecorder(c) => c.take_pending_title(),
            Self::KeyRecorder(c) => c.take_pending_title(),
            Self::SelComponent(c) => c.take_pending_title(),
        }
    }
    fn take_teardown_parts(
        &mut self,
    ) -> Option<(Box<dyn Any + Send + Sync>, std::thread::JoinHandle<()>)> {
        match self {
            Self::Noop(c) => c.take_teardown_parts(),
            Self::ActionRecorder(c) => c.take_teardown_parts(),
            Self::KeyRecorder(c) => c.take_teardown_parts(),
            Self::SelComponent(c) => c.take_teardown_parts(),
        }
    }
    fn set_selection_enabled(&mut self, enabled: bool) {
        match self {
            Self::Noop(c) => c.set_selection_enabled(enabled),
            Self::ActionRecorder(c) => c.set_selection_enabled(enabled),
            Self::KeyRecorder(c) => c.set_selection_enabled(enabled),
            Self::SelComponent(c) => c.set_selection_enabled(enabled),
        }
    }
    fn paste(&mut self, text: &str) -> bool {
        match self {
            Self::Noop(c) => c.paste(text),
            Self::ActionRecorder(c) => c.paste(text),
            Self::KeyRecorder(c) => c.paste(text),
            Self::SelComponent(c) => c.paste(text),
        }
    }
    fn on_mouse_press(
        &mut self,
        col: u16,
        row: u16,
        button: crate::events::MouseButton,
        modifiers: crate::events::KeyModifiers,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match self {
            Self::Noop(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
            Self::ActionRecorder(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
            Self::KeyRecorder(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
            Self::SelComponent(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
        }
    }
    fn on_mouse_release(
        &mut self,
        col: u16,
        row: u16,
        button: crate::events::MouseButton,
        modifiers: crate::events::KeyModifiers,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match self {
            Self::Noop(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
            Self::ActionRecorder(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
            Self::KeyRecorder(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
            Self::SelComponent(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
        }
    }
    fn on_mouse_drag(
        &mut self,
        col: u16,
        row: u16,
        button: crate::events::MouseButton,
        modifiers: crate::events::KeyModifiers,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match self {
            Self::Noop(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
            Self::ActionRecorder(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
            Self::KeyRecorder(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
            Self::SelComponent(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
        }
    }
    fn on_mouse_scroll(
        &mut self,
        col: u16,
        row: u16,
        kind: crate::events::MouseEventKind,
        modifiers: crate::events::KeyModifiers,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match self {
            Self::Noop(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
            Self::ActionRecorder(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
            Self::KeyRecorder(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
            Self::SelComponent(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
        }
    }
    fn on_mouse_move(
        &mut self,
        col: u16,
        row: u16,
        modifiers: crate::events::KeyModifiers,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match self {
            Self::Noop(c) => c.on_mouse_move(col, row, modifiers, ctx),
            Self::ActionRecorder(c) => c.on_mouse_move(col, row, modifiers, ctx),
            Self::KeyRecorder(c) => c.on_mouse_move(col, row, modifiers, ctx),
            Self::SelComponent(c) => c.on_mouse_move(col, row, modifiers, ctx),
        }
    }
}
