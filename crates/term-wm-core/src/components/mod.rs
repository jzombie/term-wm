use std::collections::VecDeque;

use crossterm::event::Event;
use ratatui::layout::Rect;

pub use crate::actions::EventResult;
use crate::actions::TermWmAction;
pub use crate::component_context::ComponentContext;
use crate::power_profile::PowerProfile;
use crate::ui::UiFrame;
use crate::window::WindowKey;
use crate::wm_config::HintVisibility;

#[derive(Debug, Clone, Copy, Default)]
pub struct SelectionStatus {
    pub active: bool,
    pub dragging: bool,
}

/// Five-phase message-passing component lifecycle.
///
/// 1. `init` — one-time setup on mount
/// 2. `handle_events` — evaluate raw events, return `EventResult<Msg>`
/// 3. `update` — mutate state exclusively via `Msg` actions
/// 4. `render(&self)` — pure translation of state to drawing instructions
/// 5. `destroy` — teardown before unmount
///
/// The `Any` bound enables safe downcasting for scenarios where
/// the concrete component type is known at the call site
/// (e.g. extracting a PTY child handle from a terminal component).
pub trait Component<Msg>: std::any::Any {
    /// Phase 1: Called once when the component is mounted.
    fn init(&mut self) {}

    /// Called immediately after the component is registered with the WindowManager.
    /// Provides the assigned `WindowKey` and app context, resolving lifecycle
    /// circularity without `Arc<Mutex<Option<WindowKey>>>` hacks.
    fn on_mount(&mut self, _key: WindowKey, _app: &crate::app_context::AppContext) {}

    /// Phase 2: Evaluate raw events, return EventResult.
    /// Does NOT mutate state.
    fn handle_events(&mut self, event: &Event, ctx: &ComponentContext) -> EventResult<Msg> {
        if let Event::Mouse(mouse) = event {
            if let Some(screen_area) = ctx.screen_area() {
                let is_inside = mouse.column >= screen_area.x
                    && mouse.column < screen_area.x.saturating_add(screen_area.width)
                    && mouse.row >= screen_area.y
                    && mouse.row < screen_area.y.saturating_add(screen_area.height);
                if is_inside {
                    let local = crate::events::LocalMouseEvent {
                        col: mouse.column.saturating_sub(screen_area.x),
                        row: mouse.row.saturating_sub(screen_area.y),
                        kind: mouse.kind,
                        modifiers: mouse.modifiers,
                    };
                    return self.on_mouse(&local, ctx);
                }
            }
            return EventResult::Ignored;
        }
        self.on_key(event, ctx)
    }

    fn on_mouse(
        &mut self,
        _mouse: &crate::events::LocalMouseEvent,
        _ctx: &ComponentContext,
    ) -> EventResult<Msg> {
        EventResult::Ignored
    }

    fn on_key(&mut self, _event: &Event, _ctx: &ComponentContext) -> EventResult<Msg> {
        EventResult::Ignored
    }

    /// Phase 3: Mutate state in response to an Action.
    /// Passed by value (the pop_front caller already owns it).
    /// Can push new actions into the queue via the sink.
    fn update(
        &mut self,
        _action: Msg,
        _ctx: &ComponentContext,
        _actions: &mut VecDeque<(crate::window::WindowKey, Msg)>,
    ) {
    }

    /// Phase 4: Pure render. Takes &self — no state mutation.
    ///
    /// The `registry` parameter allows components to register their clickable
    /// areas for hit-testing. Scroll containers call `registry.push_clip()`
    /// before rendering children and `registry.pop_clip()` afterward, which
    /// automatically clips child registrations to the visible viewport.
    fn render(
        &self,
        frame: &mut UiFrame<'_>,
        area: Rect,
        ctx: &ComponentContext,
        registry: &mut crate::hitbox_registry::HitboxRegistry,
    );

    /// Phase 5: Teardown. Called before the component is unmounted.
    fn destroy(&mut self) {}

    // Queries
    fn selection_status(&self) -> SelectionStatus {
        SelectionStatus::default()
    }
    fn selection_text(&self) -> Option<String> {
        None
    }

    /// Read and clear the pending window title (set by OSC 0/1/2 escape sequences).
    /// Returns `None` for non-terminal components.
    fn take_pending_title(&mut self) -> Option<String> {
        None
    }

    /// Extract child process and reader handles for the Reaper during teardown.
    /// Called during close_window, before the component is dropped.
    /// Default implementation returns None.
    fn take_teardown_parts(
        &mut self,
    ) -> Option<(
        Box<dyn std::any::Any + Send + Sync>,
        std::thread::JoinHandle<()>,
    )> {
        None
    }

    // Clipboard
    fn set_selection_enabled(&mut self, _enabled: bool) {}
    fn paste(&mut self, _text: &str) -> bool {
        false
    }
}

/// Helper to downcast a `&mut dyn Component<TermWmAction>` to a concrete type.
/// This works because `Component<TermWmAction>: Any`.
pub fn component_downcast_mut<T: 'static>(
    comp: &mut dyn Component<TermWmAction>,
) -> Option<&mut T> {
    let any: &mut dyn std::any::Any = comp;
    any.downcast_mut::<T>()
}

/// A component that does nothing — used for chrome-only windows.
#[derive(Debug)]
pub struct NoopComponent;

impl Component<TermWmAction> for NoopComponent {
    fn render(
        &self,
        _frame: &mut UiFrame<'_>,
        _area: Rect,
        _ctx: &ComponentContext,
        _registry: &mut crate::hitbox_registry::HitboxRegistry,
    ) {
    }
    fn update(
        &mut self,
        _action: TermWmAction,
        _ctx: &ComponentContext,
        _actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
    }
    fn destroy(&mut self) {}
}

pub trait Overlay<Msg>: Component<Msg> + std::any::Any {
    fn visible(&self) -> bool {
        true
    }
    /// Optional terminal-area rect behind which a drop-shadow should be
    /// rendered.  The overlay is drawn on top of the shadow.
    fn shadow_rect(&self, _area: Rect) -> Option<Rect> {
        None
    }
    fn handle_confirm_event(&mut self, _event: &Event) -> Option<crate::actions::ConfirmAction> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct MenuItem<R> {
    pub icon: Option<&'static str>,
    pub label: &'static str,
    pub action: R,
}

/// Actions the engine broadcasts to components.
#[derive(Debug, Clone)]
pub enum ComponentAction {
    Restore,
    Outline,
    SetMenuItems(Vec<MenuItem<TermWmAction>>),
    SetMenuAnchor(Option<(u16, u16)>),
    SetManagedArea(Rect),
    SetKeybindingHints(Vec<(TermWmAction, Vec<String>)>),
    SetPowerProfile(PowerProfile),
    SetHintVisibility(HintVisibility),
    ToggleVisibility,
    SetPanelActive(bool),
    SetTopPanelState(Box<TopPanelState>),
    SetWindowLabels(std::collections::BTreeMap<crate::window::WindowKey, String>),
}

/// Render-time state pushed to the top panel before each frame.
#[derive(Debug, Clone)]
pub struct TopPanelState {
    pub focus_current: Option<crate::window::WindowKey>,
    pub display_order: Vec<crate::window::WindowKey>,
    pub status_line: Option<String>,
    pub mouse_capture_enabled: bool,
    pub clipboard_enabled: bool,
    pub window_selection_enabled: bool,
    pub selection_active: bool,
    pub selection_dragging: bool,
    pub selection_copy_available: bool,
    pub selection_copied: bool,
    pub menu_open: bool,
}

/// Queries the engine can ask components.
#[derive(Debug)]
pub enum ComponentQuery {
    SelectedAction,
    KeybindingHints,
    MenuIconRect,
}

/// Responses from component queries.
#[derive(Debug)]
pub enum ComponentResponse {
    None,
    Action(Option<TermWmAction>),
    Hints(Vec<(TermWmAction, Vec<String>)>),
    Rect(Option<Rect>),
}

/// Unified WM chrome component trait.
///
/// Replaces the position-specific `TopPanel`, `BottomPanel`, and `MenuOverlay`
/// traits. A component's position (top, bottom, overlay) is determined by where
/// it is injected in the layout graph, not by its trait definition.
pub trait WmComponent: std::fmt::Debug {
    /// Carve out the component's required space from the available area.
    /// Returns (claimed_rect, remaining_rect).
    /// Default: claims nothing, passes through full area.
    fn consume_area(&mut self, available: Rect) -> (Rect, Rect) {
        (Rect::default(), available)
    }

    /// Render the component into the claimed area.
    fn render(
        &mut self,
        frame: &mut UiFrame<'_>,
        area: Rect,
        ctx: &ComponentContext,
        registry: &mut crate::hitbox_registry::HitboxRegistry,
    );

    /// Handle an event before it reaches the window graph.
    fn handle_event(
        &mut self,
        _event: &Event,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
    }

    /// Process a system-level action broadcasted by the window manager.
    fn process_action(&mut self, _action: &ComponentAction) {}

    /// Query the component for a result after processing.
    fn query(&self, _query: &ComponentQuery) -> ComponentResponse {
        ComponentResponse::None
    }

    /// Mouse hit-test. Returns true if (x, y) is within this component.
    fn hit_test(&self, _x: u16, _y: u16) -> bool {
        false
    }

    /// Begin a new frame (clear per-frame state).
    fn begin_frame(&mut self) {}

    /// Visible flag (layout engine skips invisible components).
    fn visible(&self) -> bool {
        true
    }

    /// Set visible flag.
    fn set_visible(&mut self, _visible: bool) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::UiFrame;
    use crossterm::event::Event;
    use ratatui::prelude::Rect;

    struct DummyComp;
    impl Component<()> for DummyComp {
        fn update(
            &mut self,
            _action: (),
            _ctx: &ComponentContext,
            _actions: &mut VecDeque<(crate::window::WindowKey, ())>,
        ) {
        }
        fn render(
            &self,
            _frame: &mut UiFrame<'_>,
            _area: Rect,
            _ctx: &ComponentContext,
            _registry: &mut crate::hitbox_registry::HitboxRegistry,
        ) {
        }
    }

    #[test]
    fn default_handle_events_returns_ignored() {
        let mut d = DummyComp;
        assert!(
            d.handle_events(
                &Event::Key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char('a'),
                    crossterm::event::KeyModifiers::NONE
                )),
                &ComponentContext::default()
            )
            .is_ignored()
        );
    }
}
