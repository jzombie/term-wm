pub use term_wm_core::components::NoopComponent;

use std::any::Any;
use std::collections::VecDeque;

use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::app_context::AppContext;
use term_wm_core::component_context::ComponentContext;
use term_wm_core::components::{Component, SelectionStatus};
use term_wm_core::events::Event;
use term_wm_core::hitbox_registry::HitboxId;
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;
use term_wm_render::RenderBackend;
use term_wm_ui_components::svg_image::SvgImageComponent;
use term_wm_ui_facade::core_component::CoreWmComponent;

pub enum AppRootComponent {
    Core(CoreWmComponent),
    SvgImage(SvgImageComponent),
}

impl Component<TermWmAction> for AppRootComponent {
    fn init(&mut self) {
        match self {
            Self::Core(c) => c.init(),
            Self::SvgImage(c) => c.init(),
        }
    }

    fn on_mount(&mut self, key: WindowKey, app: &AppContext) {
        match self {
            Self::Core(c) => c.on_mount(key, app),
            Self::SvgImage(c) => c.on_mount(key, app),
        }
    }

    fn hitbox_id(&self) -> Option<HitboxId> {
        match self {
            Self::Core(c) => c.hitbox_id(),
            Self::SvgImage(c) => c.hitbox_id(),
        }
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match self {
            Self::Core(c) => c.handle_events(event, ctx),
            Self::SvgImage(c) => c.handle_events(event, ctx),
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
            Self::Core(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
            Self::SvgImage(c) => c.on_mouse_press(col, row, button, modifiers, ctx),
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
            Self::Core(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
            Self::SvgImage(c) => c.on_mouse_release(col, row, button, modifiers, ctx),
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
            Self::Core(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
            Self::SvgImage(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),
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
            Self::Core(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
            Self::SvgImage(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),
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
            Self::Core(c) => c.on_mouse_move(col, row, modifiers, ctx),
            Self::SvgImage(c) => c.on_mouse_move(col, row, modifiers, ctx),
        }
    }

    fn on_key(&mut self, event: &Event, ctx: &ComponentContext) -> EventResult<TermWmAction> {
        match self {
            Self::Core(c) => c.on_key(event, ctx),
            Self::SvgImage(c) => c.on_key(event, ctx),
        }
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        match self {
            Self::Core(c) => c.update(action, ctx, actions),
            Self::SvgImage(c) => c.update(action, ctx, actions),
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
            Self::Core(c) => c.render(backend, area, ctx, registry),
            Self::SvgImage(c) => c.render(backend, area, ctx, registry),
        }
    }

    fn destroy(&mut self) {
        match self {
            Self::Core(c) => c.destroy(),
            Self::SvgImage(c) => c.destroy(),
        }
    }

    fn clear_selection(&mut self) {
        match self {
            Self::Core(c) => c.clear_selection(),
            Self::SvgImage(c) => c.clear_selection(),
        }
    }

    fn selection_status(&self) -> SelectionStatus {
        match self {
            Self::Core(c) => c.selection_status(),
            Self::SvgImage(c) => c.selection_status(),
        }
    }

    fn selection_text(&self) -> Option<String> {
        match self {
            Self::Core(c) => c.selection_text(),
            Self::SvgImage(c) => c.selection_text(),
        }
    }

    fn desired_height(&self, width: u16) -> u16 {
        match self {
            Self::Core(c) => c.desired_height(width),
            Self::SvgImage(c) => c.desired_height(width),
        }
    }

    fn take_pending_title(&mut self) -> Option<String> {
        match self {
            Self::Core(c) => c.take_pending_title(),
            Self::SvgImage(c) => c.take_pending_title(),
        }
    }

    fn take_teardown_parts(
        &mut self,
    ) -> Option<(Box<dyn Any + Send + Sync>, std::thread::JoinHandle<()>)> {
        match self {
            Self::Core(c) => c.take_teardown_parts(),
            Self::SvgImage(c) => c.take_teardown_parts(),
        }
    }

    fn set_selection_enabled(&mut self, enabled: bool) {
        match self {
            Self::Core(c) => c.set_selection_enabled(enabled),
            Self::SvgImage(c) => c.set_selection_enabled(enabled),
        }
    }

    fn paste(&mut self, text: &str) -> bool {
        match self {
            Self::Core(c) => c.paste(text),
            Self::SvgImage(c) => c.paste(text),
        }
    }
}
