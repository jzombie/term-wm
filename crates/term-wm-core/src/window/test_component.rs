use std::any::Any;
use std::collections::VecDeque;

use crate::actions::{EventResult, TermWmAction};
use crate::app_context::AppContext;
use crate::component_context::ComponentContext;
use crate::components::{Component, NoopComponent, SelectionStatus};
use crate::events::Event;
use crate::hitbox_registry::HitboxId;
use crate::window::WindowKey;
use term_wm_layout_engine::LayoutRect;
use term_wm_render::RenderBackend;

use crate::impl_component_delegate;

pub enum TestComponent {
    Noop(NoopComponent),
    ActionRecorder(ActionRecorder),
    KeyRecorder(KeyRecorder),
    SelComponent(SelComponent),
    RenderTracker(RenderTracker),
}

#[derive(Default)]
pub struct ActionRecorder {
    pub actions: Vec<TermWmAction>,
    pub received_mouse_bytes: bool,
}

impl Component<TermWmAction> for ActionRecorder {
    fn update(
        &mut self,
        action: TermWmAction,
        _ctx: &ComponentContext,
        _actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        if matches!(action, TermWmAction::MouseToBytes(_)) {
            self.received_mouse_bytes = true;
        }
        self.actions.push(action);
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
    fn destroy(&mut self) {}
    fn clear_selection(&mut self) {}
    fn selection_status(&self) -> SelectionStatus {
        SelectionStatus::default()
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
        local_x: u16,
        local_y: u16,
        _button: crate::events::MouseButton,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Action(TermWmAction::MouseToBytes(vec![
            local_x as u8,
            local_y as u8,
        ]))
    }
    fn on_mouse_release(
        &mut self,
        local_x: u16,
        local_y: u16,
        _button: crate::events::MouseButton,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Action(TermWmAction::MouseToBytes(vec![
            local_x as u8,
            local_y as u8,
        ]))
    }
    fn on_mouse_drag(
        &mut self,
        local_x: u16,
        local_y: u16,
        _button: crate::events::MouseButton,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Action(TermWmAction::MouseToBytes(vec![
            local_x as u8,
            local_y as u8,
        ]))
    }
    fn on_mouse_move(
        &mut self,
        local_x: u16,
        local_y: u16,
        _modifiers: crate::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Action(TermWmAction::MouseToBytes(vec![
            local_x as u8,
            local_y as u8,
        ]))
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
        SelectionStatus::default()
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

#[derive(Default)]
pub struct SelComponent {
    pub enabled: bool,
    pub received_down: bool,
}

impl Component<TermWmAction> for SelComponent {
    fn selection_status(&self) -> SelectionStatus {
        SelectionStatus {
            active: self.received_down,
            dragging: false,
        }
    }
    fn selection_text(&self) -> Option<String> {
        if self.received_down {
            Some("selected text".to_string())
        } else {
            None
        }
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
    fn desired_height(&self, _width: u16) -> u16 {
        0
    }
    fn take_pending_title(&mut self) -> Option<String> {
        None
    }
    fn set_selection_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
    fn paste(&mut self, _text: &str) -> bool {
        false
    }
    fn on_mouse_press(
        &mut self,
        _col: u16,
        _row: u16,
        _button: crate::events::MouseButton,
        _modifiers: crate::events::KeyModifiers,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        if !ctx.direct_mode() && self.enabled {
            self.received_down = true;
            return EventResult::Consumed;
        }
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

#[derive(Default, Clone)]
pub struct RenderTracker {
    pub last_area: Option<crate::Rect>,
    pub render_count: usize,
}

impl Component<TermWmAction> for RenderTracker {
    fn render(
        &mut self,
        _backend: &mut dyn RenderBackend,
        area: crate::Rect,
        _ctx: &ComponentContext,
        _registry: &mut crate::hitbox_registry::HitboxRegistry,
    ) {
        self.last_area = Some(area);
        self.render_count += 1;
    }
    fn init(&mut self) {}
    fn on_mount(&mut self, _key: WindowKey, _app: &AppContext) {}
    fn hitbox_id(&self) -> Option<HitboxId> {
        None
    }
    fn handle_events(
        &mut self,
        _event: &Event,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
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
        SelectionStatus {
            active: false,
            dragging: false,
        }
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
    fn take_teardown_parts(
        &mut self,
    ) -> Option<(Box<dyn Any + Send + Sync>, std::thread::JoinHandle<()>)> {
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

impl_component_delegate!(TestComponent {
    Noop, ActionRecorder, KeyRecorder, SelComponent, RenderTracker,
});
