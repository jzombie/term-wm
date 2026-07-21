use std::collections::VecDeque;

use term_wm_core::Rect;
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::app_context::AppContext;
use term_wm_core::component_context::ComponentContext;
use term_wm_core::components::{
    Component, ComponentAction, ComponentQuery, ComponentResponse, SelectionStatus, WmComponent,
};
use term_wm_core::events::Event;
use term_wm_core::hitbox_registry::HitboxId;
use term_wm_core::window::WindowKey;
use term_wm_render::RenderBackend;
use term_wm_sys_ui_components::{
    WmBottomPanelComponent, WmCommandPaletteComponent, WmFabComponent, WmNotificationAreaComponent,
    WmTopPanelComponent,
};

#[allow(clippy::large_enum_variant)]
pub enum LayerComponent {
    TopPanel(WmTopPanelComponent),
    BottomPanel(WmBottomPanelComponent),
    Fab(WmFabComponent),
    NotificationArea(WmNotificationAreaComponent),
    CommandPalette(WmCommandPaletteComponent),
}

impl std::fmt::Debug for LayerComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TopPanel(_) => f.debug_tuple("TopPanel").finish(),
            Self::BottomPanel(_) => f.debug_tuple("BottomPanel").finish(),
            Self::Fab(_) => f.debug_tuple("Fab").finish(),
            Self::NotificationArea(_) => f.debug_tuple("NotificationArea").finish(),
            Self::CommandPalette(_) => f.debug_tuple("CommandPalette").finish(),
        }
    }
}

impl Component<TermWmAction> for LayerComponent {
    fn init(&mut self) {
        match self {
            Self::TopPanel(c) => c.init(),
            Self::BottomPanel(c) => c.init(),
            Self::Fab(c) => c.init(),
            Self::NotificationArea(c) => c.init(),
            Self::CommandPalette(c) => c.init(),
        }
    }

    fn on_mount(&mut self, key: WindowKey, app: &AppContext) {
        match self {
            Self::TopPanel(c) => c.on_mount(key, app),
            Self::BottomPanel(c) => c.on_mount(key, app),
            Self::Fab(c) => c.on_mount(key, app),
            Self::NotificationArea(c) => c.on_mount(key, app),
            Self::CommandPalette(c) => c.on_mount(key, app),
        }
    }

    fn hitbox_id(&self) -> Option<HitboxId> {
        match self {
            Self::TopPanel(c) => c.hitbox_id(),
            Self::BottomPanel(c) => c.hitbox_id(),
            Self::Fab(c) => c.hitbox_id(),
            Self::NotificationArea(c) => c.hitbox_id(),
            Self::CommandPalette(c) => c.hitbox_id(),
        }
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match self {
            Self::TopPanel(c) => c.handle_events(event, ctx),
            Self::BottomPanel(c) => c.handle_events(event, ctx),
            Self::Fab(c) => c.handle_events(event, ctx),
            Self::NotificationArea(c) => c.handle_events(event, ctx),
            Self::CommandPalette(c) => c.handle_events(event, ctx),
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
            Self::TopPanel(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
            Self::BottomPanel(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
            Self::Fab(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
            Self::NotificationArea(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
            Self::CommandPalette(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
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
            Self::TopPanel(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
            Self::BottomPanel(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
            Self::Fab(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
            Self::NotificationArea(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
            Self::CommandPalette(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
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
            Self::TopPanel(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
            Self::BottomPanel(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
            Self::Fab(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
            Self::NotificationArea(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
            Self::CommandPalette(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
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
            Self::TopPanel(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
            Self::BottomPanel(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
            Self::Fab(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
            Self::NotificationArea(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
            Self::CommandPalette(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
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
            Self::TopPanel(c) => c.on_mouse_move(col, row, modifiers, ctx),
            Self::BottomPanel(c) => c.on_mouse_move(col, row, modifiers, ctx),
            Self::Fab(c) => c.on_mouse_move(col, row, modifiers, ctx),
            Self::NotificationArea(c) => c.on_mouse_move(col, row, modifiers, ctx),
            Self::CommandPalette(c) => c.on_mouse_move(col, row, modifiers, ctx),
        }
    }

    fn on_key(&mut self, event: &Event, ctx: &ComponentContext) -> EventResult<TermWmAction> {
        match self {
            Self::TopPanel(c) => c.on_key(event, ctx),
            Self::BottomPanel(c) => c.on_key(event, ctx),
            Self::Fab(c) => c.on_key(event, ctx),
            Self::NotificationArea(c) => c.on_key(event, ctx),
            Self::CommandPalette(c) => c.on_key(event, ctx),
        }
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        match self {
            Self::TopPanel(c) => c.update(action, ctx, actions),
            Self::BottomPanel(c) => c.update(action, ctx, actions),
            Self::Fab(c) => c.update(action, ctx, actions),
            Self::NotificationArea(c) => c.update(action, ctx, actions),
            Self::CommandPalette(c) => c.update(action, ctx, actions),
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
            Self::TopPanel(c) => c.render(backend, area, ctx, registry),
            Self::BottomPanel(c) => Component::render(c, backend, area, ctx, registry),
            Self::Fab(c) => c.render(backend, area, ctx, registry),
            Self::NotificationArea(c) => c.render(backend, area, ctx, registry),
            Self::CommandPalette(c) => c.render(backend, area, ctx, registry),
        }
    }

    fn destroy(&mut self) {
        match self {
            Self::TopPanel(c) => c.destroy(),
            Self::BottomPanel(c) => c.destroy(),
            Self::Fab(c) => c.destroy(),
            Self::NotificationArea(c) => c.destroy(),
            Self::CommandPalette(c) => c.destroy(),
        }
    }

    fn clear_selection(&mut self) {
        match self {
            Self::TopPanel(c) => c.clear_selection(),
            Self::BottomPanel(c) => c.clear_selection(),
            Self::Fab(c) => c.clear_selection(),
            Self::NotificationArea(c) => c.clear_selection(),
            Self::CommandPalette(c) => c.clear_selection(),
        }
    }

    fn selection_status(&self) -> SelectionStatus {
        match self {
            Self::TopPanel(c) => c.selection_status(),
            Self::BottomPanel(c) => c.selection_status(),
            Self::Fab(c) => c.selection_status(),
            Self::NotificationArea(c) => c.selection_status(),
            Self::CommandPalette(c) => c.selection_status(),
        }
    }

    fn selection_text(&self) -> Option<String> {
        match self {
            Self::TopPanel(c) => c.selection_text(),
            Self::BottomPanel(c) => c.selection_text(),
            Self::Fab(c) => c.selection_text(),
            Self::NotificationArea(c) => c.selection_text(),
            Self::CommandPalette(c) => c.selection_text(),
        }
    }

    fn desired_height(&self, width: u16) -> u16 {
        match self {
            Self::TopPanel(c) => c.desired_height(width),
            Self::BottomPanel(c) => c.desired_height(width),
            Self::Fab(c) => c.desired_height(width),
            Self::NotificationArea(c) => c.desired_height(width),
            Self::CommandPalette(c) => c.desired_height(width),
        }
    }

    fn take_pending_title(&mut self) -> Option<String> {
        match self {
            Self::TopPanel(c) => c.take_pending_title(),
            Self::BottomPanel(c) => c.take_pending_title(),
            Self::Fab(c) => c.take_pending_title(),
            Self::NotificationArea(c) => c.take_pending_title(),
            Self::CommandPalette(c) => c.take_pending_title(),
        }
    }

    fn take_teardown_parts(
        &mut self,
    ) -> Option<(
        Box<dyn std::any::Any + Send + Sync>,
        std::thread::JoinHandle<()>,
    )> {
        match self {
            Self::TopPanel(c) => c.take_teardown_parts(),
            Self::BottomPanel(c) => c.take_teardown_parts(),
            Self::Fab(c) => c.take_teardown_parts(),
            Self::NotificationArea(c) => c.take_teardown_parts(),
            Self::CommandPalette(c) => c.take_teardown_parts(),
        }
    }

    fn set_selection_enabled(&mut self, enabled: bool) {
        match self {
            Self::TopPanel(c) => c.set_selection_enabled(enabled),
            Self::BottomPanel(c) => c.set_selection_enabled(enabled),
            Self::Fab(c) => c.set_selection_enabled(enabled),
            Self::NotificationArea(c) => c.set_selection_enabled(enabled),
            Self::CommandPalette(c) => c.set_selection_enabled(enabled),
        }
    }

    fn paste(&mut self, text: &str) -> bool {
        match self {
            Self::TopPanel(c) => c.paste(text),
            Self::BottomPanel(c) => c.paste(text),
            Self::Fab(c) => c.paste(text),
            Self::NotificationArea(c) => c.paste(text),
            Self::CommandPalette(c) => c.paste(text),
        }
    }
}

impl WmComponent for LayerComponent {
    fn consume_area(&mut self, available: Rect) -> (Rect, Rect) {
        match self {
            Self::TopPanel(c) => c.consume_area(available),
            Self::BottomPanel(c) => c.consume_area(available),
            Self::Fab(c) => c.consume_area(available),
            Self::NotificationArea(c) => c.consume_area(available),
            Self::CommandPalette(c) => c.consume_area(available),
        }
    }

    fn process_action(&mut self, action: &ComponentAction) {
        match self {
            Self::TopPanel(c) => c.process_action(action),
            Self::BottomPanel(c) => c.process_action(action),
            Self::Fab(c) => c.process_action(action),
            Self::NotificationArea(c) => c.process_action(action),
            Self::CommandPalette(c) => c.process_action(action),
        }
    }

    fn query(&self, query: &ComponentQuery) -> ComponentResponse {
        match self {
            Self::TopPanel(c) => c.query(query),
            Self::BottomPanel(c) => c.query(query),
            Self::Fab(c) => c.query(query),
            Self::NotificationArea(c) => c.query(query),
            Self::CommandPalette(c) => c.query(query),
        }
    }

    fn hit_test(&self, x: u16, y: u16) -> bool {
        match self {
            Self::TopPanel(c) => c.hit_test(x, y),
            Self::BottomPanel(c) => c.hit_test(x, y),
            Self::Fab(c) => c.hit_test(x, y),
            Self::NotificationArea(c) => c.hit_test(x, y),
            Self::CommandPalette(c) => c.hit_test(x, y),
        }
    }

    fn begin_frame(&mut self) {
        match self {
            Self::TopPanel(c) => c.begin_frame(),
            Self::BottomPanel(c) => c.begin_frame(),
            Self::Fab(c) => c.begin_frame(),
            Self::NotificationArea(c) => c.begin_frame(),
            Self::CommandPalette(c) => c.begin_frame(),
        }
    }

    fn visible(&self) -> bool {
        match self {
            Self::TopPanel(c) => c.visible(),
            Self::BottomPanel(c) => c.visible(),
            Self::Fab(c) => c.visible(),
            Self::NotificationArea(c) => c.visible(),
            Self::CommandPalette(c) => c.visible(),
        }
    }

    fn set_visible(&mut self, visible: bool) {
        match self {
            Self::TopPanel(c) => c.set_visible(visible),
            Self::BottomPanel(c) => c.set_visible(visible),
            Self::Fab(c) => c.set_visible(visible),
            Self::NotificationArea(c) => c.set_visible(visible),
            Self::CommandPalette(c) => c.set_visible(visible),
        }
    }
}
