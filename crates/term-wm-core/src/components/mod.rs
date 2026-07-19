// TODO: Refactor; it looks like "components" are in the core

use std::borrow::Cow;
use std::collections::VecDeque;

use term_wm_layout_engine::LayoutRect;

pub use crate::actions::EventResult;
use crate::actions::TermWmAction;
pub use crate::component_context::ComponentContext;
use crate::events::{Event, KeyModifiers, MouseButton, MouseEventKind};
use crate::hitbox_registry::HitboxId;
use crate::power_profile::PowerProfile;
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

    /// Returns the component's hitbox ID, if any.
    /// Components that register their clickable area in `render()` should
    /// override this to return their persistent `HitboxId`. The default
    /// `handle_events` uses this for leaf-component occlusion convenience.
    fn hitbox_id(&self) -> Option<HitboxId> {
        None
    }

    /// Phase 2: Evaluate raw events, return EventResult.
    /// Does NOT mutate state.
    /// Dispatches mouse events to semantic handlers by MouseEventKind.
    fn handle_events(&mut self, event: &Event, ctx: &ComponentContext) -> EventResult<Msg> {
        if let Event::Mouse(mouse) = event {
            // Leaf-component convenience: skip if our ID doesn't match.
            // Container components override handle_events entirely and
            // implement "delegate first, self-identify second" manually.
            if let Some(my_id) = self.hitbox_id()
                && ctx.active_hitbox() != Some(my_id)
            {
                return EventResult::Ignored;
            }
            if let Some(screen_area) = ctx.screen_area() {
                let local_x = (i32::from(mouse.column) - screen_area.x).max(0) as u16;
                let local_y = (i32::from(mouse.row) - screen_area.y).max(0) as u16;
                return match mouse.kind {
                    MouseEventKind::Press(btn) => {
                        self.on_mouse_press(local_x, local_y, btn, mouse.modifiers, ctx)
                    }
                    MouseEventKind::Release(btn) => {
                        self.on_mouse_release(local_x, local_y, btn, mouse.modifiers, ctx)
                    }
                    MouseEventKind::Drag(btn) => {
                        self.on_mouse_drag(local_x, local_y, btn, mouse.modifiers, ctx)
                    }
                    MouseEventKind::ScrollUp
                    | MouseEventKind::ScrollDown
                    | MouseEventKind::ScrollLeft
                    | MouseEventKind::ScrollRight => {
                        self.on_mouse_scroll(local_x, local_y, mouse.kind, mouse.modifiers, ctx)
                    }
                    MouseEventKind::Moved => {
                        self.on_mouse_move(local_x, local_y, mouse.modifiers, ctx)
                    }
                };
            }
            return EventResult::Ignored;
        }
        // Keyboard events: route to on_key if this component owns keyboard focus.
        // Components without a hitbox_id forward unconditionally (backward compat).
        // Components with a hitbox_id only receive key events when focused.
        if let Event::Key(_) = event
            && let Some(my_id) = self.hitbox_id()
        {
            if ctx.keyboard_focus_id() == Some(my_id) {
                return self.on_key(event, ctx);
            }
            return EventResult::Ignored;
        }
        self.on_key(event, ctx)
    }

    fn on_mouse_press(
        &mut self,
        _local_x: u16,
        _local_y: u16,
        _button: MouseButton,
        _modifiers: KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<Msg> {
        EventResult::Ignored
    }

    fn on_mouse_release(
        &mut self,
        _local_x: u16,
        _local_y: u16,
        _button: MouseButton,
        _modifiers: KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<Msg> {
        EventResult::Ignored
    }

    fn on_mouse_drag(
        &mut self,
        _local_x: u16,
        _local_y: u16,
        _button: MouseButton,
        _modifiers: KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<Msg> {
        EventResult::Ignored
    }

    fn on_mouse_scroll(
        &mut self,
        _local_x: u16,
        _local_y: u16,
        _kind: MouseEventKind,
        _modifiers: KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<Msg> {
        EventResult::Ignored
    }

    fn on_mouse_move(
        &mut self,
        _local_x: u16,
        _local_y: u16,
        _modifiers: KeyModifiers,
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

    /// Phase 4: Render the component.
    ///
    /// The `registry` parameter allows components to register their clickable
    /// areas for hit-testing. Scroll containers call `registry.push_clip()`
    /// before rendering children and `pop_clip()` afterward, which
    /// automatically clips child registrations to the visible viewport.
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut crate::hitbox_registry::HitboxRegistry,
    );

    /// Phase 5: Teardown. Called before the component is unmounted.
    fn destroy(&mut self) {}

    /// Clear any active selection. Default no-op.
    fn clear_selection(&mut self) {}

    // Queries
    fn selection_status(&self) -> SelectionStatus {
        SelectionStatus::default()
    }
    fn selection_text(&self) -> Option<String> {
        None
    }

    /// The preferred height in terminal rows given the available width.
    /// Return 0 to stretch and fill remaining space (default).
    fn desired_height(&self, _width: u16) -> u16 {
        0
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
        &mut self,
        _backend: &mut dyn term_wm_render::RenderBackend,
        _area: LayoutRect,
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
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
    /// Optional terminal-area rect behind which a drop-shadow should be
    /// rendered.  The overlay is drawn on top of the shadow.
    fn shadow_rect(&self, _area: LayoutRect) -> Option<LayoutRect> {
        None
    }
    fn handle_confirm_event(&mut self, _event: &Event) -> Option<crate::actions::ConfirmAction> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct MenuItem<R> {
    pub icon: Option<&'static str>,
    pub label: Cow<'static, str>,
    pub action: R,
}

/// Actions the engine broadcasts to components.
#[derive(Debug, Clone)]
pub enum ComponentAction {
    Restore,
    Outline,
    SetMenuItems(Vec<MenuItem<TermWmAction>>),
    SetManagedArea(LayoutRect),
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
    Rect(Option<LayoutRect>),
}

/// Unified WM chrome component trait.
///
/// Replaces the position-specific `TopPanel`, `BottomPanel`, and `MenuOverlay`
/// traits. A component's position (top, bottom, overlay) is determined by where
/// it is injected in the layout graph, not by its trait definition.
pub trait WmComponent: std::fmt::Debug + Component<TermWmAction> {
    /// Carve out the component's required space from the available area.
    /// Returns (claimed_rect, remaining_rect).
    /// Default: claims nothing, passes through full area.
    fn consume_area(&mut self, available: LayoutRect) -> (LayoutRect, LayoutRect) {
        (
            LayoutRect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            available,
        )
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
    use crate::events::Event;
    use term_wm_layout_engine::LayoutRect;

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
            &mut self,
            _backend: &mut dyn term_wm_render::RenderBackend,
            _area: LayoutRect,
            _ctx: &ComponentContext,
            _registry: &mut crate::hitbox_registry::HitboxRegistry,
        ) {
        }
    }

    #[test]
    fn default_handle_events_returns_ignored() {
        use crate::events::{KeyCode, KeyKind, KeyModifiers};

        let mut d = DummyComp;
        assert!(
            d.handle_events(
                &Event::Key(crate::events::KeyEvent {
                    code: KeyCode::Char('a'),
                    modifiers: KeyModifiers::NONE,
                    kind: KeyKind::Press,
                }),
                &ComponentContext::default()
            )
            .is_ignored()
        );
    }

    #[test]
    fn mouse_outside_screen_area_returns_ignored() {
        use crate::events::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        let mut d = DummyComp;
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 10,
            y: 10,
            width: 20,
            height: 10,
        });
        let event = Event::Mouse(MouseEvent {
            column: 5,
            row: 5,
            kind: MouseEventKind::Press(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
        });
        assert!(d.handle_events(&event, &ctx).is_ignored());
    }

    #[test]
    fn mouse_no_screen_area_returns_ignored() {
        use crate::events::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        let mut d = DummyComp;
        let ctx = ComponentContext::new(true);
        let event = Event::Mouse(MouseEvent {
            column: 0,
            row: 0,
            kind: MouseEventKind::Press(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
        });
        assert!(d.handle_events(&event, &ctx).is_ignored());
    }

    #[test]
    fn dispatch_press_calls_on_mouse_press() {
        use crate::events::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        struct PressRecorder {
            received: std::cell::Cell<bool>,
        }
        impl Component<()> for PressRecorder {
            fn on_mouse_press(
                &mut self,
                local_x: u16,
                local_y: u16,
                button: MouseButton,
                _modifiers: KeyModifiers,
                _ctx: &ComponentContext,
            ) -> EventResult<()> {
                self.received.set(true);
                assert_eq!(local_x, 5);
                assert_eq!(local_y, 3);
                assert_eq!(button, MouseButton::Left);
                EventResult::Consumed
            }
            fn update(
                &mut self,
                _: (),
                _: &ComponentContext,
                _: &mut VecDeque<(crate::window::WindowKey, ())>,
            ) {
            }
            fn render(
                &mut self,
                _: &mut dyn term_wm_render::RenderBackend,
                _: LayoutRect,
                _: &ComponentContext,
                _: &mut crate::hitbox_registry::HitboxRegistry,
            ) {
            }
        }

        let mut comp = PressRecorder {
            received: std::cell::Cell::new(false),
        };
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 10,
            y: 10,
            width: 20,
            height: 10,
        });
        let event = Event::Mouse(MouseEvent {
            column: 15,
            row: 13,
            kind: MouseEventKind::Press(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
        });
        let result = comp.handle_events(&event, &ctx);
        assert!(matches!(result, EventResult::Consumed));
        assert!(comp.received.get());
    }

    #[test]
    fn dispatch_release_calls_on_mouse_release() {
        use crate::events::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        struct ReleaseRecorder {
            received: std::cell::Cell<bool>,
        }
        impl Component<()> for ReleaseRecorder {
            fn on_mouse_release(
                &mut self,
                local_x: u16,
                _local_y: u16,
                _button: MouseButton,
                _modifiers: KeyModifiers,
                _ctx: &ComponentContext,
            ) -> EventResult<()> {
                self.received.set(true);
                assert_eq!(local_x, 5);
                EventResult::Consumed
            }
            fn update(
                &mut self,
                _: (),
                _: &ComponentContext,
                _: &mut VecDeque<(crate::window::WindowKey, ())>,
            ) {
            }
            fn render(
                &mut self,
                _: &mut dyn term_wm_render::RenderBackend,
                _: LayoutRect,
                _: &ComponentContext,
                _: &mut crate::hitbox_registry::HitboxRegistry,
            ) {
            }
        }

        let mut comp = ReleaseRecorder {
            received: std::cell::Cell::new(false),
        };
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 10,
            y: 10,
            width: 20,
            height: 10,
        });
        let event = Event::Mouse(MouseEvent {
            column: 15,
            row: 13,
            kind: MouseEventKind::Release(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
        });
        comp.handle_events(&event, &ctx);
        assert!(comp.received.get());
    }

    #[test]
    fn dispatch_drag_calls_on_mouse_drag() {
        use crate::events::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        struct DragRecorder {
            received: std::cell::Cell<bool>,
        }
        impl Component<()> for DragRecorder {
            fn on_mouse_drag(
                &mut self,
                local_x: u16,
                _local_y: u16,
                _button: MouseButton,
                _modifiers: KeyModifiers,
                _ctx: &ComponentContext,
            ) -> EventResult<()> {
                self.received.set(true);
                assert_eq!(local_x, 0);
                EventResult::Consumed
            }
            fn update(
                &mut self,
                _: (),
                _: &ComponentContext,
                _: &mut VecDeque<(crate::window::WindowKey, ())>,
            ) {
            }
            fn render(
                &mut self,
                _: &mut dyn term_wm_render::RenderBackend,
                _: LayoutRect,
                _: &ComponentContext,
                _: &mut crate::hitbox_registry::HitboxRegistry,
            ) {
            }
        }

        let mut comp = DragRecorder {
            received: std::cell::Cell::new(false),
        };
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 10,
            y: 10,
            width: 20,
            height: 10,
        });
        let event = Event::Mouse(MouseEvent {
            column: 8,
            row: 15,
            kind: MouseEventKind::Drag(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
        });
        comp.handle_events(&event, &ctx);
        assert!(comp.received.get());
    }

    #[test]
    fn dispatch_scroll_variants_calls_on_mouse_scroll() {
        use crate::events::{KeyModifiers, MouseEvent, MouseEventKind};

        struct ScrollRecorder {
            received: std::cell::Cell<bool>,
        }
        impl Component<()> for ScrollRecorder {
            fn on_mouse_scroll(
                &mut self,
                _local_x: u16,
                _local_y: u16,
                kind: MouseEventKind,
                _modifiers: KeyModifiers,
                _ctx: &ComponentContext,
            ) -> EventResult<()> {
                self.received.set(true);
                assert!(matches!(
                    kind,
                    MouseEventKind::ScrollUp
                        | MouseEventKind::ScrollDown
                        | MouseEventKind::ScrollLeft
                        | MouseEventKind::ScrollRight
                ));
                EventResult::Consumed
            }
            fn update(
                &mut self,
                _: (),
                _: &ComponentContext,
                _: &mut VecDeque<(crate::window::WindowKey, ())>,
            ) {
            }
            fn render(
                &mut self,
                _: &mut dyn term_wm_render::RenderBackend,
                _: LayoutRect,
                _: &ComponentContext,
                _: &mut crate::hitbox_registry::HitboxRegistry,
            ) {
            }
        }

        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        for kind in [
            MouseEventKind::ScrollUp,
            MouseEventKind::ScrollDown,
            MouseEventKind::ScrollLeft,
            MouseEventKind::ScrollRight,
        ] {
            let mut comp = ScrollRecorder {
                received: std::cell::Cell::new(false),
            };
            let event = Event::Mouse(MouseEvent {
                column: 40,
                row: 12,
                kind,
                modifiers: KeyModifiers::NONE,
            });
            comp.handle_events(&event, &ctx);
            assert!(
                comp.received.get(),
                "scroll variant {:?} not dispatched",
                kind
            );
        }
    }

    #[test]
    fn dispatch_move_calls_on_mouse_move() {
        use crate::events::{KeyModifiers, MouseEvent, MouseEventKind};

        struct MoveRecorder {
            received: std::cell::Cell<bool>,
        }
        impl Component<()> for MoveRecorder {
            fn on_mouse_move(
                &mut self,
                local_x: u16,
                local_y: u16,
                _modifiers: KeyModifiers,
                _ctx: &ComponentContext,
            ) -> EventResult<()> {
                self.received.set(true);
                assert_eq!(local_x, 40);
                assert_eq!(local_y, 12);
                EventResult::Consumed
            }
            fn update(
                &mut self,
                _: (),
                _: &ComponentContext,
                _: &mut VecDeque<(crate::window::WindowKey, ())>,
            ) {
            }
            fn render(
                &mut self,
                _: &mut dyn term_wm_render::RenderBackend,
                _: LayoutRect,
                _: &ComponentContext,
                _: &mut crate::hitbox_registry::HitboxRegistry,
            ) {
            }
        }

        let mut comp = MoveRecorder {
            received: std::cell::Cell::new(false),
        };
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        let event = Event::Mouse(MouseEvent {
            column: 40,
            row: 12,
            kind: MouseEventKind::Moved,
            modifiers: KeyModifiers::NONE,
        });
        comp.handle_events(&event, &ctx);
        assert!(comp.received.get());
    }

    #[test]
    fn mouse_coordinate_clamping_at_negative_area_origin() {
        use crate::events::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        struct CoordRecorder {
            coords: std::cell::Cell<(u16, u16)>,
        }
        impl Component<()> for CoordRecorder {
            fn on_mouse_press(
                &mut self,
                local_x: u16,
                local_y: u16,
                _button: MouseButton,
                _modifiers: KeyModifiers,
                _ctx: &ComponentContext,
            ) -> EventResult<()> {
                self.coords.set((local_x, local_y));
                EventResult::Consumed
            }
            fn update(
                &mut self,
                _: (),
                _: &ComponentContext,
                _: &mut VecDeque<(crate::window::WindowKey, ())>,
            ) {
            }
            fn render(
                &mut self,
                _: &mut dyn term_wm_render::RenderBackend,
                _: LayoutRect,
                _: &ComponentContext,
                _: &mut crate::hitbox_registry::HitboxRegistry,
            ) {
            }
        }

        let mut comp = CoordRecorder {
            coords: std::cell::Cell::new((0, 0)),
        };
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 10,
            y: 10,
            width: 20,
            height: 10,
        });
        let event = Event::Mouse(MouseEvent {
            column: 5,
            row: 5,
            kind: MouseEventKind::Press(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
        });
        comp.handle_events(&event, &ctx);
        assert_eq!(comp.coords.get(), (0, 0));
    }
}
