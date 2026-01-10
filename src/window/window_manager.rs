use super::Window;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, MouseEvent, MouseEventKind};
use ratatui::prelude::Rect;
use ratatui::widgets::Clear;

use super::decorator::{DefaultDecorator, HeaderAction, WindowDecorator};
use crate::clipboard;
use crate::components::{
    Component, ComponentContext, ConfirmAction, ConfirmOverlayComponent, Overlay,
    sys::debug_log::{DebugLogComponent, install_panic_hook, set_global_debug_log},
    sys::help_overlay::HelpOverlayComponent,
};
use crate::constants::MIN_FLOATING_VISIBLE_MARGIN;
use crate::layout::floating::*;
use crate::layout::{
    FloatingPane, InsertPosition, LayoutNode, LayoutPlan, RegionMap, SplitHandle, TilingLayout,
    rect_contains, render_handles_masked,
};
use crate::panel::Panel;
use crate::state::AppState;
use crate::ui::UiFrame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Describes who owns layout placement and how WM-level input is handled.
///
/// - AppManaged: the app owns regions; `Esc` passes through.
/// - WindowManaged: the WM owns layout; `Esc` enters WM mode/overlay.
pub enum LayoutContract {
    AppManaged,
    WindowManaged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SystemWindowId {
    DebugLog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OverlayId {
    Help,
    ExitConfirm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WindowId<R: Copy + Eq + Ord> {
    App(R),
    System(SystemWindowId),
}

impl<R: Copy + Eq + Ord> WindowId<R> {
    fn app(id: R) -> Self {
        Self::App(id)
    }

    fn system(id: SystemWindowId) -> Self {
        Self::System(id)
    }

    fn as_app(self) -> Option<R> {
        match self {
            Self::App(id) => Some(id),
            Self::System(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ScrollState {
    pub offset: usize,
    pending: isize,
}

impl ScrollState {
    pub fn reset(&mut self) {
        self.offset = 0;
        self.pending = 0;
    }

    pub fn bump(&mut self, delta: isize) {
        self.pending = self.pending.saturating_add(delta);
    }

    pub fn apply(&mut self, total: usize, view: usize) {
        let max_offset = total.saturating_sub(view);
        if self.pending != 0 {
            let delta = self.pending;
            self.pending = 0;
            let next = if delta.is_negative() {
                self.offset.saturating_sub(delta.unsigned_abs())
            } else {
                self.offset.saturating_add(delta as usize)
            };
            self.offset = next.min(max_offset);
        } else if self.offset > max_offset {
            self.offset = max_offset;
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WindowSurface {
    pub full: Rect,
    pub inner: Rect,
    pub dest: crate::window::FloatRect,
}

#[derive(Debug, Clone, Copy)]
pub struct AppWindowDraw<R: Copy + Eq + Ord> {
    pub id: R,
    pub surface: WindowSurface,
    pub focused: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum WindowDrawTask<R: Copy + Eq + Ord> {
    App(AppWindowDraw<R>),
    System(SystemWindowDraw),
}

#[derive(Debug, Clone, Copy)]
pub struct SystemWindowDraw {
    pub id: SystemWindowId,
    pub surface: WindowSurface,
    pub focused: bool,
}

trait SystemWindowView {
    fn render(&mut self, frame: &mut UiFrame<'_>, surface: WindowSurface, focused: bool);
    fn handle_event(&mut self, event: &Event) -> bool;
    fn set_selection_enabled(&mut self, _enabled: bool) {}
}

impl SystemWindowView for DebugLogComponent {
    fn render(&mut self, frame: &mut UiFrame<'_>, surface: WindowSurface, focused: bool) {
        let ctx = ComponentContext::new(focused);
        <DebugLogComponent as Component>::render(self, frame, surface.inner, &ctx);
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        Component::handle_event(self, event, &ComponentContext::default())
    }

    fn set_selection_enabled(&mut self, enabled: bool) {
        DebugLogComponent::set_selection_enabled(self, enabled);
    }
}

struct SystemWindowEntry {
    component: Box<dyn SystemWindowView>,
    visible: bool,
}

impl SystemWindowEntry {
    fn new(component: Box<dyn SystemWindowView>) -> Self {
        Self {
            component,
            visible: false,
        }
    }

    fn visible(&self) -> bool {
        self.visible
    }

    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    fn render(&mut self, frame: &mut UiFrame<'_>, surface: WindowSurface, focused: bool) {
        self.component.render(frame, surface, focused);
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        self.component.handle_event(event)
    }

    fn set_selection_enabled(&mut self, enabled: bool) {
        self.component.set_selection_enabled(enabled);
    }
}

#[derive(Debug, Clone)]
pub struct FocusRing<T: Copy + Eq> {
    order: Vec<T>,
    current: T,
}

impl<T: Copy + Eq> FocusRing<T> {
    pub fn new(current: T) -> Self {
        Self {
            order: Vec::new(),
            current,
        }
    }

    pub fn set_order(&mut self, order: Vec<T>) {
        self.order = order;
    }

    pub fn current(&self) -> T {
        self.current
    }

    pub fn set_current(&mut self, current: T) {
        self.current = current;
    }

    pub fn advance(&mut self, forward: bool) {
        if self.order.is_empty() {
            return;
        }
        let idx = self
            .order
            .iter()
            .position(|item| *item == self.current)
            .unwrap_or(0);
        let step = if forward { 1isize } else { -1isize };
        let next = ((idx as isize + step).rem_euclid(self.order.len() as isize)) as usize;
        self.current = self.order[next];
    }
}

pub struct WindowManager<W: Copy + Eq + Ord, R: Copy + Eq + Ord> {
    app_focus: FocusRing<W>,
    wm_focus: FocusRing<WindowId<R>>,
    windows: BTreeMap<WindowId<R>, Window>,
    regions: RegionMap<WindowId<R>>,
    scroll: BTreeMap<W, ScrollState>,
    handles: Vec<SplitHandle>,
    resize_handles: Vec<ResizeHandle<WindowId<R>>>,
    floating_headers: Vec<DragHandle<WindowId<R>>>,
    managed_draw_order: Vec<WindowId<R>>,
    managed_draw_order_app: Vec<R>,
    managed_layout: Option<TilingLayout<WindowId<R>>>,
    // queue of app ids removed this frame; runner drains via `take_closed_app_windows`
    closed_app_windows: Vec<R>,
    managed_area: Rect,
    panel: Panel<WindowId<R>>,
    drag_header: Option<HeaderDrag<WindowId<R>>>,
    last_header_click: Option<(WindowId<R>, Instant)>,
    drag_resize: Option<ResizeDrag<WindowId<R>>>,
    hover: Option<(u16, u16)>,
    capture_deadline: Option<Instant>,
    pending_deadline: Option<Instant>,
    state: AppState,
    clipboard_available: bool,
    layout_contract: LayoutContract,
    wm_overlay_opened_at: Option<Instant>,
    last_frame_area: ratatui::prelude::Rect,
    esc_passthrough_window: Duration,
    overlays: BTreeMap<OverlayId, Box<dyn Overlay>>,
    // Central default for whether ScrollViewComponent keyboard handling should be enabled
    // for UI components that opt into it. Individual components can override.
    scroll_keyboard_enabled_default: bool,
    decorator: Arc<dyn WindowDecorator>,
    floating_resize_offscreen: bool,
    z_order: Vec<WindowId<R>>,
    drag_snap: Option<(Option<WindowId<R>>, InsertPosition, Rect)>,
    system_windows: BTreeMap<SystemWindowId, SystemWindowEntry>,
    next_window_seq: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WmMenuAction {
    CloseMenu,
    Help,
    NewWindow,
    ToggleDebugWindow,
    ExitUi,
    BringFloatingFront,
    MinimizeWindow,
    MaximizeWindow,
    CloseWindow,
    ToggleMouseCapture,
    ToggleClipboardMode,
}

impl<W: Copy + Eq + Ord, R: Copy + Eq + Ord + std::fmt::Debug> WindowManager<W, R>
where
    R: PartialEq<W>,
{
    fn window_mut(&mut self, id: WindowId<R>) -> &mut Window {
        let seq = &mut self.next_window_seq;
        self.windows.entry(id).or_insert_with(|| {
            let order = *seq;
            *seq = order.saturating_add(1);
            tracing::debug!(window_id = ?id, seq = order, "opened window");
            Window::new(order)
        })
    }

    fn window(&self, id: WindowId<R>) -> Option<&Window> {
        self.windows.get(&id)
    }

    fn is_minimized(&self, id: WindowId<R>) -> bool {
        self.window(id).is_some_and(|window| window.minimized)
    }

    fn set_minimized(&mut self, id: WindowId<R>, value: bool) {
        self.window_mut(id).minimized = value;
    }

    fn floating_rect(&self, id: WindowId<R>) -> Option<crate::window::FloatRectSpec> {
        self.window(id).and_then(|window| window.floating_rect)
    }

    fn set_floating_rect(&mut self, id: WindowId<R>, rect: Option<crate::window::FloatRectSpec>) {
        self.window_mut(id).floating_rect = rect;
    }

    fn clear_floating_rect(&mut self, id: WindowId<R>) {
        self.window_mut(id).floating_rect = None;
    }

    fn set_prev_floating_rect(
        &mut self,
        id: WindowId<R>,
        rect: Option<crate::window::FloatRectSpec>,
    ) {
        self.window_mut(id).prev_floating_rect = rect;
    }

    fn take_prev_floating_rect(&mut self, id: WindowId<R>) -> Option<crate::window::FloatRectSpec> {
        self.window_mut(id).prev_floating_rect.take()
    }
    fn is_window_floating(&self, id: WindowId<R>) -> bool {
        self.window(id).is_some_and(|window| window.is_floating())
    }

    pub fn window_title(&self, id: WindowId<R>) -> String {
        self.window(id)
            .map(|window| window.title_or_default(id))
            .unwrap_or_else(|| match id {
                WindowId::App(app_id) => format!("{:?}", app_id),
                WindowId::System(SystemWindowId::DebugLog) => "Debug Log".to_string(),
            })
    }

    fn clear_all_floating(&mut self) {
        for window in self.windows.values_mut() {
            window.floating_rect = None;
            window.prev_floating_rect = None;
        }
    }

    fn system_window_entry(&self, id: SystemWindowId) -> Option<&SystemWindowEntry> {
        self.system_windows.get(&id)
    }

    fn system_window_entry_mut(&mut self, id: SystemWindowId) -> Option<&mut SystemWindowEntry> {
        self.system_windows.get_mut(&id)
    }

    fn system_window_visible(&self, id: SystemWindowId) -> bool {
        self.system_window_entry(id)
            .map(|entry| entry.visible())
            .unwrap_or(false)
    }

    fn set_system_window_visible(&mut self, id: SystemWindowId, visible: bool) {
        if let Some(entry) = self.system_window_entry_mut(id) {
            entry.set_visible(visible);
        }
    }

    fn show_system_window(&mut self, id: SystemWindowId) {
        if self.system_window_visible(id) {
            return;
        }
        if self.system_window_entry(id).is_none() {
            return;
        }
        self.set_system_window_visible(id, true);
        if self.layout_contract != LayoutContract::WindowManaged {
            return;
        }
        let window_id = WindowId::system(id);
        let _ = self.window_mut(window_id);
        self.ensure_system_window_in_layout(window_id);
        self.focus_window_id(window_id);
    }

    fn hide_system_window(&mut self, id: SystemWindowId) {
        if !self.system_window_visible(id) {
            return;
        }
        let window_id = WindowId::system(id);
        self.set_system_window_visible(id, false);
        self.remove_system_window_from_layout(window_id);
        if self.wm_focus.current() == window_id {
            self.select_fallback_focus();
        }
    }

    fn ensure_system_window_in_layout(&mut self, id: WindowId<R>) {
        if self.layout_contract != LayoutContract::WindowManaged {
            return;
        }
        if self.layout_contains(id) {
            return;
        }
        let _ = self.window_mut(id);
        if self.managed_layout.is_none() {
            self.managed_layout = Some(TilingLayout::new(LayoutNode::leaf(id)));
            return;
        }
        let _ = self.tile_window_id(id);
    }

    fn remove_system_window_from_layout(&mut self, id: WindowId<R>) {
        self.clear_floating_rect(id);
        if let Some(layout) = &mut self.managed_layout {
            if matches!(layout.root(), LayoutNode::Leaf(root_id) if *root_id == id) {
                self.managed_layout = None;
            } else {
                layout.root_mut().remove_leaf(id);
            }
        }
        self.z_order.retain(|window_id| *window_id != id);
        self.managed_draw_order.retain(|window_id| *window_id != id);
    }

    fn dispatch_system_window_event(&mut self, id: SystemWindowId, event: &Event) -> bool {
        if let Some(localized) = self.localize_event_content(WindowId::system(id), event) {
            return self.dispatch_system_window_event_localized(id, &localized);
        }
        self.system_window_entry_mut(id)
            .map(|entry| entry.handle_event(event))
            .unwrap_or(false)
    }

    fn dispatch_system_window_event_localized(
        &mut self,
        id: SystemWindowId,
        event: &Event,
    ) -> bool {
        let adjusted = self.adjust_event_for_window(WindowId::system(id), event);
        self.system_window_entry_mut(id)
            .map(|entry| entry.handle_event(&adjusted))
            .unwrap_or(false)
    }

    fn render_system_window_entry(&mut self, frame: &mut UiFrame<'_>, draw: SystemWindowDraw) {
        if let Some(entry) = self.system_window_entry_mut(draw.id) {
            entry.render(frame, draw.surface, draw.focused);
        }
    }

    pub fn new(current: W) -> Self {
        let clipboard_available = clipboard::available();
        let mut state = AppState::new();
        if !clipboard_available {
            state.set_clipboard_enabled(false);
        }
        let selection_enabled = state.clipboard_enabled();
        Self {
            app_focus: FocusRing::new(current),
            wm_focus: FocusRing::new(WindowId::system(SystemWindowId::DebugLog)),
            windows: BTreeMap::new(),
            regions: RegionMap::default(),
            scroll: BTreeMap::new(),
            handles: Vec::new(),
            resize_handles: Vec::new(),
            floating_headers: Vec::new(),
            managed_draw_order: Vec::new(),
            managed_draw_order_app: Vec::new(),
            managed_layout: None,
            closed_app_windows: Vec::new(),
            managed_area: Rect::default(),
            panel: Panel::new(),
            drag_header: None,
            last_header_click: None,
            drag_resize: None,
            hover: None,
            capture_deadline: None,
            pending_deadline: None,
            state,
            clipboard_available,
            layout_contract: LayoutContract::AppManaged,
            wm_overlay_opened_at: None,
            last_frame_area: Rect::default(),
            esc_passthrough_window: esc_passthrough_window_default(),
            overlays: BTreeMap::new(),
            scroll_keyboard_enabled_default: true,
            decorator: Arc::new(DefaultDecorator),
            floating_resize_offscreen: true,
            z_order: Vec::new(),
            drag_snap: None,
            system_windows: {
                let (mut component, handle) = DebugLogComponent::new_default();
                component.set_selection_enabled(selection_enabled);
                set_global_debug_log(handle);
                // Initialize tracing now that the global debug log handle exists
                // so tracing will write into the in-memory debug buffer by default.
                crate::tracing_sub::init_default();
                install_panic_hook();
                let mut map = BTreeMap::new();
                map.insert(
                    SystemWindowId::DebugLog,
                    SystemWindowEntry::new(Box::new(component)),
                );
                map
            },
            next_window_seq: 0,
        }
    }

    pub fn new_managed(current: W) -> Self {
        let mut manager = Self::new(current);
        manager.layout_contract = LayoutContract::WindowManaged;
        manager
    }

    pub fn set_layout_contract(&mut self, contract: LayoutContract) {
        self.layout_contract = contract;
    }

    /// Drain and return any app ids whose windows were closed since the last call.
    pub fn take_closed_app_windows(&mut self) -> Vec<R> {
        std::mem::take(&mut self.closed_app_windows)
    }

    pub fn layout_contract(&self) -> LayoutContract {
        self.layout_contract
    }

    pub fn set_floating_resize_offscreen(&mut self, enabled: bool) {
        self.floating_resize_offscreen = enabled;
    }

    pub fn floating_resize_offscreen(&self) -> bool {
        self.floating_resize_offscreen
    }

    pub fn begin_frame(&mut self) {
        self.regions = RegionMap::default();
        self.handles.clear();
        self.resize_handles.clear();
        self.floating_headers.clear();
        self.managed_draw_order.clear();
        self.managed_draw_order_app.clear();
        self.panel.begin_frame();
        // If a panic occurred earlier, ensure the debug log is shown and focused.
        if crate::components::sys::debug_log::take_panic_pending() {
            self.show_system_window(SystemWindowId::DebugLog);
        }
        if self.layout_contract == LayoutContract::AppManaged {
            self.clear_capture();
        } else {
            // Refresh deadlines so overlay badges can expire without events.
            self.refresh_capture();
        }
    }

    pub fn arm_capture(&mut self, timeout: Duration) {
        self.capture_deadline = Some(Instant::now() + timeout);
        self.pending_deadline = None;
    }

    pub fn arm_pending(&mut self, timeout: Duration) {
        // Shows an "Esc pending" badge while waiting for the chord.
        self.pending_deadline = Some(Instant::now() + timeout);
    }

    pub fn clear_capture(&mut self) {
        self.capture_deadline = None;
        self.pending_deadline = None;
        self.state.set_overlay_visible(false);
        self.wm_overlay_opened_at = None;
        self.state.set_wm_menu_selected(0);
    }

    pub fn capture_active(&mut self) -> bool {
        if !self.state.mouse_capture_enabled() {
            return false;
        }
        if self.layout_contract == LayoutContract::WindowManaged && self.state.overlay_visible() {
            return true;
        }
        self.refresh_capture();
        self.capture_deadline.is_some()
    }

    pub fn mouse_capture_enabled(&self) -> bool {
        self.state.mouse_capture_enabled()
    }

    pub fn set_mouse_capture_enabled(&mut self, enabled: bool) {
        self.state.set_mouse_capture_enabled(enabled);
        if !self.state.mouse_capture_enabled() {
            self.clear_capture();
        }
    }

    pub fn toggle_mouse_capture(&mut self) {
        self.state.toggle_mouse_capture();
        if !self.state.mouse_capture_enabled() {
            self.clear_capture();
        }
    }

    pub fn take_mouse_capture_change(&mut self) -> Option<bool> {
        self.state.take_mouse_capture_change()
    }

    pub fn take_clipboard_change(&mut self) -> Option<bool> {
        self.state.take_clipboard_change()
    }

    pub fn clipboard_available(&self) -> bool {
        self.clipboard_available
    }

    pub fn clipboard_enabled(&self) -> bool {
        self.state.clipboard_enabled()
    }

    pub fn set_clipboard_enabled(&mut self, enabled: bool) {
        if !self.clipboard_available {
            return;
        }
        if self.state.clipboard_enabled() == enabled {
            return;
        }
        self.state.set_clipboard_enabled(enabled);
        self.apply_clipboard_selection_state(enabled);
    }

    pub fn toggle_clipboard_enabled(&mut self) {
        if !self.clipboard_available {
            return;
        }
        let next = !self.state.clipboard_enabled();
        self.set_clipboard_enabled(next);
    }

    fn apply_clipboard_selection_state(&mut self, enabled: bool) {
        for entry in self.system_windows.values_mut() {
            entry.set_selection_enabled(enabled);
        }
        for overlay in self.overlays.values_mut() {
            if let Some(help) = overlay.as_any_mut().downcast_mut::<HelpOverlayComponent>() {
                help.set_selection_enabled(enabled);
            }
        }
    }

    fn refresh_capture(&mut self) {
        if let Some(deadline) = self.capture_deadline
            && Instant::now() > deadline
        {
            self.capture_deadline = None;
        }
        if let Some(deadline) = self.pending_deadline
            && Instant::now() > deadline
        {
            self.pending_deadline = None;
        }
    }

    pub fn open_wm_overlay(&mut self) {
        self.state.set_overlay_visible(true);
        self.wm_overlay_opened_at = Some(Instant::now());
        self.state.set_wm_menu_selected(0);
    }

    pub fn close_wm_overlay(&mut self) {
        self.state.set_overlay_visible(false);
        self.wm_overlay_opened_at = None;
        self.state.set_wm_menu_selected(0);
    }

    pub fn open_exit_confirm(&mut self) {
        let mut confirm = ConfirmOverlayComponent::new();
        // Area is likely set during render/event handling, but ConfirmOverlayComponent uses default.
        confirm.open(
            "Exit App",
            "Exit the application?\nUnsaved changes will be lost.",
        );
        self.overlays
            .insert(OverlayId::ExitConfirm, Box::new(confirm));
    }

    pub fn close_exit_confirm(&mut self) {
        self.overlays.remove(&OverlayId::ExitConfirm);
    }

    pub fn exit_confirm_visible(&self) -> bool {
        self.overlays.contains_key(&OverlayId::ExitConfirm)
    }

    pub fn help_overlay_visible(&self) -> bool {
        self.overlays.contains_key(&OverlayId::Help)
    }

    pub fn open_help_overlay(&mut self) {
        let mut h = HelpOverlayComponent::new();
        h.show();
        h.set_selection_enabled(self.clipboard_enabled());
        // respect central default: if globally disabled, ensure the overlay doesn't enable keys
        if !self.scroll_keyboard_enabled_default {
            h.set_keyboard_enabled(false);
        }
        self.overlays.insert(OverlayId::Help, Box::new(h));
    }

    pub fn close_help_overlay(&mut self) {
        self.overlays.remove(&OverlayId::Help);
    }

    /// Set the central default for enabling scroll-keyboard handling.
    pub fn set_scroll_keyboard_enabled(&mut self, enabled: bool) {
        self.scroll_keyboard_enabled_default = enabled;
    }

    pub fn handle_help_event(&mut self, event: &Event) -> bool {
        // Retrieve component as &mut Box<dyn Overlay>
        let Some(boxed) = self.overlays.get_mut(&OverlayId::Help) else {
            return false;
        };

        // We need to invoke handle_event on the HelpOverlayComponent.
        // Since we refactored it to use internal area and Component::handle_event,
        // we can just use the trait method.
        // BUT, HelpOverlayComponent::handle_event now calls handle_help_event_in_area with stored area.
        // Update area to ensure correct hit-testing (Component::resize)
        boxed.resize(
            self.last_frame_area,
            &ComponentContext::new(true).with_overlay(true),
        );

        let handled = boxed.handle_event(event, &ComponentContext::new(true).with_overlay(true));

        // Remove the overlay if it has closed itself
        let should_close = if let Some(help) = boxed.as_any().downcast_ref::<HelpOverlayComponent>()
        {
            !help.visible()
        } else {
            false
        };

        if should_close {
            self.overlays.remove(&OverlayId::Help);
        }

        handled
    }

    pub fn wm_overlay_visible(&self) -> bool {
        self.state.overlay_visible()
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

    /// Returns true when any windows are still active (app or system).
    ///
    /// This is intended as a simple, conservative check for callers that
    /// want to know whether the window manager currently has any visible
    /// or managed windows remaining.
    pub fn has_any_active_windows(&self) -> bool {
        // Active system windows or overlays
        if self.has_active_system_windows() {
            return true;
        }
        // Any regions (tiled or floating) indicate active app windows
        if !self.regions.ids().is_empty() {
            return true;
        }
        // Z-order may contain app windows (including floating ones)
        if self.z_order.iter().any(|id| id.as_app().is_some()) {
            return true;
        }
        false
    }

    pub fn esc_passthrough_active(&self) -> bool {
        self.esc_passthrough_remaining().is_some()
    }

    pub fn esc_passthrough_remaining(&self) -> Option<Duration> {
        if !self.wm_overlay_visible() {
            return None;
        }
        let opened_at = self.wm_overlay_opened_at?;
        let elapsed = opened_at.elapsed();
        if elapsed >= self.esc_passthrough_window {
            return None;
        }
        Some(self.esc_passthrough_window.saturating_sub(elapsed))
    }

    pub fn focus(&self) -> W {
        self.app_focus.current()
    }

    pub fn focused_window(&self) -> WindowId<R> {
        self.wm_focus.current()
    }

    /// Returns the currently focused window along with an event localized to its content area.
    pub fn focused_window_event(&self, event: &Event) -> Option<(WindowId<R>, Event)> {
        let window_id = self.focused_window();
        let localized = self
            .localize_event_content(window_id, event)
            .unwrap_or_else(|| event.clone());
        Some((window_id, localized))
    }

    /// Handle managed chrome interactions first, then dispatch the localized event to either the
    /// focused system window or the provided app handler. Returns true when the event was consumed.
    pub fn dispatch_focused_event<F>(&mut self, event: &Event, mut on_app: F) -> bool
    where
        F: FnMut(R, &Event) -> bool,
    {
        if matches!(event, Event::Mouse(_)) && self.handle_managed_event(event) {
            return true;
        }
        let Some((window_id, localized)) = self.focused_window_event(event) else {
            return false;
        };
        let adjusted = self.adjust_event_for_window(window_id, &localized);
        match window_id {
            WindowId::App(id) => on_app(id, &adjusted),
            WindowId::System(system_id) => {
                self.dispatch_system_window_event_localized(system_id, &adjusted)
            }
        }
    }

    pub fn set_focus(&mut self, focus: W) {
        self.app_focus.set_current(focus);
    }

    pub fn set_focus_order(&mut self, order: Vec<W>) {
        self.app_focus.set_order(order);
        if !self.app_focus.order.is_empty()
            && !self.app_focus.order.contains(&self.app_focus.current)
        {
            self.app_focus.current = self.app_focus.order[0];
        }
    }

    pub fn advance_focus(&mut self, forward: bool) {
        self.app_focus.advance(forward);
    }

    pub fn wm_focus(&self) -> WindowId<R> {
        self.wm_focus.current()
    }

    pub fn wm_focus_app(&self) -> Option<R> {
        self.wm_focus.current().as_app()
    }

    fn set_wm_focus(&mut self, focus: WindowId<R>) {
        self.wm_focus.set_current(focus);
        if let Some(app_id) = focus.as_app()
            && let Some(app_focus) = self.focus_for_region(app_id)
        {
            self.app_focus.set_current(app_focus);
        }
    }

    /// Unified focus API: set WM focus, bring the window to front, update draw order,
    /// and sync app-level focus if applicable.
    fn focus_window_id(&mut self, id: WindowId<R>) {
        self.set_wm_focus(id);
        self.bring_to_front_id(id);
        self.managed_draw_order = self.z_order.clone();
        if let Some(app_id) = id.as_app()
            && let Some(app_focus) = self.focus_for_region(app_id)
        {
            self.app_focus.set_current(app_focus);
        }
    }

    fn set_wm_focus_order(&mut self, order: Vec<WindowId<R>>) {
        self.wm_focus.set_order(order);
        if !self.wm_focus.order.is_empty() && !self.wm_focus.order.contains(&self.wm_focus.current)
        {
            self.wm_focus.current = self.wm_focus.order[0];
        }
    }

    fn rebuild_wm_focus_ring(&mut self, active_ids: &[WindowId<R>]) {
        if active_ids.is_empty() {
            self.set_wm_focus_order(Vec::new());
            return;
        }
        let active: BTreeSet<_> = active_ids.iter().copied().collect();
        let mut next_order: Vec<WindowId<R>> = Vec::with_capacity(active.len());
        let mut seen: BTreeSet<WindowId<R>> = BTreeSet::new();

        for &id in &self.wm_focus.order {
            if active.contains(&id) && seen.insert(id) {
                next_order.push(id);
            }
        }
        for &id in active_ids {
            if seen.insert(id) {
                next_order.push(id);
            }
        }
        self.set_wm_focus_order(next_order);
    }

    fn advance_wm_focus(&mut self, forward: bool) {
        if self.wm_focus.order.is_empty() {
            return;
        }
        self.wm_focus.advance(forward);
        let focused = self.wm_focus.current();
        self.focus_window_id(focused);
    }

    fn select_fallback_focus(&mut self) {
        if let Some(fallback) = self.wm_focus.order.first().copied() {
            self.set_wm_focus(fallback);
        }
    }

    pub fn scroll(&self, id: W) -> ScrollState {
        self.scroll.get(&id).copied().unwrap_or_default()
    }

    pub fn scroll_mut(&mut self, id: W) -> &mut ScrollState {
        self.scroll.entry(id).or_default()
    }

    pub fn scroll_offset(&self, id: W) -> usize {
        self.scroll(id).offset
    }

    pub fn reset_scroll(&mut self, id: W) {
        self.scroll_mut(id).reset();
    }

    pub fn apply_scroll(&mut self, id: W, total: usize, view: usize) {
        self.scroll_mut(id).apply(total, view);
    }

    pub fn set_region(&mut self, id: R, rect: Rect) {
        self.regions.set(WindowId::app(id), rect);
    }

    pub fn full_region(&self, id: R) -> Rect {
        self.full_region_for_id(WindowId::app(id))
    }

    pub fn region(&self, id: R) -> Rect {
        self.region_for_id(WindowId::app(id))
    }

    fn window_content_offset(&self, id: WindowId<R>) -> (u16, u16) {
        let full = self.full_region_for_id(id);
        let content = self.region_for_id(id);
        (
            content.x.saturating_sub(full.x),
            content.y.saturating_sub(full.y),
        )
    }

    fn adjust_event_for_window(&self, id: WindowId<R>, event: &Event) -> Event {
        if let Event::Mouse(mut mouse) = event.clone() {
            let (offset_x, offset_y) = self.window_content_offset(id);
            mouse.column = mouse.column.saturating_add(offset_x);
            mouse.row = mouse.row.saturating_add(offset_y);
            Event::Mouse(mouse)
        } else {
            event.clone()
        }
    }

    /// Translate a mouse event into the content coordinate space for the given app window.
    /// Returns a new `Event` when translation occurs; otherwise returns `None`.
    pub fn localize_event_to_app(&self, id: R, event: &Event) -> Option<Event> {
        self.localize_event_content(WindowId::app(id), event)
    }

    /// Translate mouse coordinates into the window-local coordinate space, including chrome.
    pub fn localize_event(&self, id: WindowId<R>, event: &Event) -> Option<Event> {
        match event {
            Event::Mouse(mouse) => {
                let dest = self.window_dest(id, self.full_region_for_id(id));
                let column =
                    (i32::from(mouse.column) - dest.x).clamp(0, i32::from(u16::MAX)) as u16;
                let row = (i32::from(mouse.row) - dest.y).clamp(0, i32::from(u16::MAX)) as u16;
                Some(Event::Mouse(MouseEvent {
                    column,
                    row,
                    kind: mouse.kind,
                    modifiers: mouse.modifiers,
                }))
            }
            _ => None,
        }
    }

    /// Translate mouse coordinates into the content-area coordinate space for the provided window id.
    fn localize_event_content(&self, id: WindowId<R>, event: &Event) -> Option<Event> {
        match event {
            Event::Mouse(mouse) => {
                let dest = self.window_dest(id, self.full_region_for_id(id));
                let (offset_x, offset_y) = self.window_content_offset(id);
                let content_x = dest.x + i32::from(offset_x);
                let content_y = dest.y + i32::from(offset_y);
                let column =
                    (i32::from(mouse.column) - content_x).clamp(0, i32::from(u16::MAX)) as u16;
                let row = (i32::from(mouse.row) - content_y).clamp(0, i32::from(u16::MAX)) as u16;
                Some(Event::Mouse(MouseEvent {
                    column,
                    row,
                    kind: mouse.kind,
                    modifiers: mouse.modifiers,
                }))
            }
            _ => None,
        }
    }

    fn full_region_for_id(&self, id: WindowId<R>) -> Rect {
        self.regions.get(id).unwrap_or_default()
    }

    fn region_for_id(&self, id: WindowId<R>) -> Rect {
        let rect = self.regions.get(id).unwrap_or_default();
        if self.layout_contract == LayoutContract::WindowManaged {
            let area = if self.floating_resize_offscreen {
                // If we allow off-screen resizing/dragging, we shouldn't clamp the
                // logical region to the bounds, otherwise the PTY will be resized
                // (shrinking the content) instead of just being clipped during render.
                rect
            } else {
                clamp_rect(rect, self.managed_area)
            };
            if area.width < 3 || area.height < 4 {
                return Rect::default();
            }
            Rect {
                x: area.x + 1,
                y: area.y + 2,
                width: area.width.saturating_sub(2),
                height: area.height.saturating_sub(3),
            }
        } else {
            rect
        }
    }

    pub fn set_regions_from_layout(&mut self, layout: &LayoutNode<R>, area: Rect) {
        self.regions = RegionMap::default();
        for (id, rect) in layout.layout(area) {
            self.regions.set(WindowId::app(id), rect);
        }
    }

    pub fn register_tiling_layout(&mut self, layout: &TilingLayout<R>, area: Rect) {
        let (regions, handles) = layout.root().layout_with_handles(area);
        for (id, rect) in regions {
            self.regions.set(WindowId::app(id), rect);
        }
        self.handles.extend(handles);
    }

    pub fn set_managed_layout(&mut self, layout: TilingLayout<R>) {
        self.managed_layout = Some(TilingLayout::new(map_layout_node(layout.root())));
        self.clear_all_floating();
        if self.system_window_visible(SystemWindowId::DebugLog) {
            self.ensure_system_window_in_layout(WindowId::system(SystemWindowId::DebugLog));
        }
    }

    pub fn set_managed_layout_none(&mut self) {
        if self.managed_layout.is_none() {
            return;
        }
        self.managed_layout = None;
        if self.system_window_visible(SystemWindowId::DebugLog) {
            self.ensure_system_window_in_layout(WindowId::system(SystemWindowId::DebugLog));
        }
    }

    pub fn set_panel_visible(&mut self, visible: bool) {
        self.panel.set_visible(visible);
    }

    pub fn set_panel_height(&mut self, height: u16) {
        self.panel.set_height(height);
    }

    pub fn decorator(&self) -> Arc<dyn WindowDecorator> {
        Arc::clone(&self.decorator)
    }

    pub fn register_managed_layout(&mut self, area: Rect) {
        self.last_frame_area = area;
        let (_, _, managed_area) = self.panel.split_area(self.panel_active(), area);
        // Preserve the previous managed area so we can update any windows that
        // were maximized to it; this ensures a maximized window remains
        // maximized when the terminal (and thus managed area) resizes.
        let prev_managed = self.managed_area;
        self.managed_area = managed_area;
        // If any window's floating rect exactly matched the previous managed
        // area (i.e. it was maximized), update it to the new managed area so
        // maximize persists across resizes.
        if prev_managed.width > 0 && prev_managed.height > 0 {
            let prev_full = crate::window::FloatRectSpec::Absolute(crate::window::FloatRect {
                x: prev_managed.x as i32,
                y: prev_managed.y as i32,
                width: prev_managed.width,
                height: prev_managed.height,
            });
            let new_full = crate::window::FloatRectSpec::Absolute(crate::window::FloatRect {
                x: self.managed_area.x as i32,
                y: self.managed_area.y as i32,
                width: self.managed_area.width,
                height: self.managed_area.height,
            });
            for (_id, window) in self.windows.iter_mut() {
                if window.floating_rect == Some(prev_full) {
                    window.floating_rect = Some(new_full);
                }
            }
        }
        self.clamp_floating_to_bounds();
        if self.system_window_visible(SystemWindowId::DebugLog) {
            self.ensure_system_window_in_layout(WindowId::system(SystemWindowId::DebugLog));
        }
        let z_snapshot = self.z_order.clone();
        let mut active_ids: Vec<WindowId<R>> = Vec::new();

        if let Some(layout) = self.managed_layout.as_ref() {
            let (regions, handles) = layout.root().layout_with_handles(self.managed_area);
            for (id, rect) in &regions {
                if self.is_window_floating(*id) {
                    continue;
                }
                // skip minimized windows
                if self.is_minimized(*id) {
                    continue;
                }
                self.regions.set(*id, *rect);
                if let Some(header) = floating_header_for_region(*id, *rect, self.managed_area) {
                    self.floating_headers.push(header);
                }
                active_ids.push(*id);
            }
            let filtered_handles: Vec<SplitHandle> = handles
                .into_iter()
                .filter(|handle| {
                    let Some(LayoutNode::Split { children, .. }) =
                        layout.root().node_at_path(&handle.path)
                    else {
                        return false;
                    };
                    let left = children.get(handle.index);
                    let right = children.get(handle.index + 1);
                    left.is_some_and(|node| node.subtree_any(|id| !self.is_window_floating(id)))
                        || right
                            .is_some_and(|node| node.subtree_any(|id| !self.is_window_floating(id)))
                })
                .collect();
            self.handles.extend(filtered_handles);
        }
        let mut floating_ids: Vec<WindowId<R>> = self
            .windows
            .iter()
            .filter_map(|(&id, window)| {
                if window.is_floating() && !window.minimized {
                    Some(id)
                } else {
                    None
                }
            })
            .collect();
        floating_ids.sort_by_key(|id| {
            z_snapshot
                .iter()
                .position(|existing| existing == id)
                .unwrap_or(usize::MAX)
        });
        for floating_id in floating_ids {
            let Some(spec) = self.floating_rect(floating_id) else {
                continue;
            };
            let rect = spec.resolve(self.managed_area);
            self.regions.set(floating_id, rect);
            let visible = self.visible_rect_from_spec(spec);
            if visible.width > 0 && visible.height > 0 {
                self.resize_handles.extend(resize_handles_for_region(
                    floating_id,
                    visible,
                    self.managed_area,
                ));
                if let Some(header) =
                    floating_header_for_region(floating_id, visible, self.managed_area)
                {
                    self.floating_headers.push(header);
                }
            }
            active_ids.push(floating_id);
        }

        self.z_order.retain(|id| active_ids.contains(id));
        for &id in &active_ids {
            if !self.z_order.contains(&id) {
                self.z_order.push(id);
            }
        }
        self.managed_draw_order = self.z_order.clone();
        self.managed_draw_order_app = self
            .managed_draw_order
            .iter()
            .filter_map(|id| id.as_app())
            .collect();
        self.rebuild_wm_focus_ring(&active_ids);
        // Ensure the current focus is actually on top and synced after layout registration.
        // Only bring the focused window to front if it's not already the topmost window
        // to avoid repeatedly forcing focus every frame.
        let focused = self.wm_focus.current();
        if self.z_order.last().copied() != Some(focused) {
            self.focus_window_id(focused);
        }
    }

    pub fn managed_draw_order(&self) -> &[R] {
        &self.managed_draw_order_app
    }

    /// Build a stable display order for UI components.
    /// By default this returns the canonical creation order filtered to active managed windows,
    /// appending any windows that are active but not yet present in the canonical ordering.
    pub fn build_display_order(&self) -> Vec<WindowId<R>> {
        let mut ordered: Vec<(WindowId<R>, &Window)> = self
            .windows
            .iter()
            .map(|(id, window)| (*id, window))
            .collect();
        ordered.sort_by_key(|(_, window)| window.creation_order);

        let mut out: Vec<WindowId<R>> = Vec::new();
        for (id, window) in ordered {
            if self.managed_draw_order.contains(&id) || window.minimized {
                out.push(id);
            }
        }
        for id in &self.managed_draw_order {
            if !out.contains(id) {
                out.push(*id);
            }
        }
        out
    }

    /// Set a user-visible title for an app window. This overrides the default
    /// Debug-derived title displayed for the given `id`.
    pub fn set_window_title(&mut self, id: R, title: impl Into<String>) {
        self.window_mut(WindowId::app(id)).title = Some(title.into());
    }

    pub fn handle_managed_event(&mut self, event: &Event) -> bool {
        if self.layout_contract != LayoutContract::WindowManaged {
            return false;
        }
        if let Event::Mouse(mouse) = event
            && self.panel_active()
            && rect_contains(self.panel.area(), mouse.column, mouse.row)
        {
            if self.panel.hit_test_menu(event) {
                if self.wm_overlay_visible() {
                    self.close_wm_overlay();
                } else {
                    self.open_wm_overlay();
                }
            } else if self.panel.hit_test_mouse_capture(event) {
                self.toggle_mouse_capture();
            } else if self.panel.hit_test_clipboard(event) {
                self.toggle_clipboard_enabled();
            } else if let Some(id) = self.panel.hit_test_window(event) {
                // If the clicked window is minimized, restore it first so it appears
                // in the layout; otherwise just focus and bring to front.
                if self.is_minimized(id) {
                    self.restore_minimized(id);
                }
                self.focus_window_id(id);
            }
            return true;
        }
        if let Event::Mouse(mouse) = event {
            self.hover = Some((mouse.column, mouse.row));
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.focus_window_at(mouse.column, mouse.row);
            }
        }
        if self.handle_resize_event(event) {
            return true;
        }
        if self.handle_header_drag_event(event) {
            return true;
        }
        if self.handle_system_window_event(event) {
            return true;
        }
        if let Some(layout) = self.managed_layout.as_mut() {
            return layout.handle_event(event, self.managed_area);
        }
        false
    }

    pub fn minimize_window(&mut self, id: WindowId<R>) {
        if self.is_minimized(id) {
            return;
        }
        // remove from floating and regions; keep canonical order so it can be restored
        self.clear_floating_rect(id);
        self.z_order.retain(|x| *x != id);
        self.managed_draw_order.retain(|x| *x != id);
        self.set_minimized(id, true);
        // ensure focus moves if needed
        if self.wm_focus.current() == id {
            self.select_fallback_focus();
        }
    }

    pub fn restore_minimized(&mut self, id: WindowId<R>) {
        if !self.is_minimized(id) {
            return;
        }
        self.set_minimized(id, false);
        // reinstall into z_order and draw order
        if !self.z_order.contains(&id) {
            self.z_order.push(id);
        }
        if !self.managed_draw_order.contains(&id) {
            self.managed_draw_order.push(id);
        }
    }

    pub fn toggle_maximize(&mut self, id: WindowId<R>) {
        // maximize toggles the floating rect to full managed_area
        let full = crate::window::FloatRectSpec::Absolute(crate::window::FloatRect {
            x: self.managed_area.x as i32,
            y: self.managed_area.y as i32,
            width: self.managed_area.width,
            height: self.managed_area.height,
        });
        if let Some(current) = self.floating_rect(id) {
            if current == full {
                if let Some(prev) = self.take_prev_floating_rect(id) {
                    self.set_floating_rect(id, Some(prev));
                }
            } else {
                self.set_prev_floating_rect(id, Some(current));
                self.set_floating_rect(id, Some(full));
            }
            self.bring_floating_to_front_id(id);
            return;
        }
        // not floating: add floating pane covering full area
        // Save the current region (if available) so we can restore later.
        let prev_rect = if let Some(rect) = self.regions.get(id) {
            crate::window::FloatRectSpec::Absolute(crate::window::FloatRect {
                x: rect.x as i32,
                y: rect.y as i32,
                width: rect.width,
                height: rect.height,
            })
        } else {
            crate::window::FloatRectSpec::Percent {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            }
        };
        self.set_prev_floating_rect(id, Some(prev_rect));
        self.set_floating_rect(id, Some(full));
        self.bring_floating_to_front_id(id);
    }

    pub fn close_window(&mut self, id: WindowId<R>) {
        tracing::debug!(window_id = ?id, "closing window");
        if let WindowId::System(system_id) = id {
            self.hide_system_window(system_id);
            return;
        }

        // Remove references to this window
        self.clear_floating_rect(id);
        self.z_order.retain(|x| *x != id);
        self.managed_draw_order.retain(|x| *x != id);
        self.set_minimized(id, false);
        self.regions.remove(id);
        // update focus
        if self.wm_focus.current() == id {
            self.select_fallback_focus();
        }
        // If this window corresponded to an app id, enqueue it for the runner to drain.
        if let Some(app_id) = id.as_app() {
            self.closed_app_windows.push(app_id);
        }
    }

    fn handle_header_drag_event(&mut self, event: &Event) -> bool {
        use crossterm::event::MouseEventKind;
        let Event::Mouse(mouse) = event else {
            return false;
        };
        match mouse.kind {
            MouseEventKind::Down(_) => {
                // Check if the mouse is blocked by a window above
                let topmost_hit = if self.layout_contract == LayoutContract::WindowManaged
                    && !self.managed_draw_order.is_empty()
                {
                    self.hit_test_region_topmost(mouse.column, mouse.row, &self.managed_draw_order)
                } else {
                    None
                };

                if let Some(header) = self
                    .floating_headers
                    .iter()
                    .rev()
                    .find(|handle| rect_contains(handle.rect, mouse.column, mouse.row))
                    .copied()
                {
                    // If we hit a window body that is NOT the owner of this header,
                    // then the header is obscured.
                    if let Some(hit_id) = topmost_hit
                        && hit_id != header.id
                    {
                        return false;
                    }

                    let rect = self.full_region_for_id(header.id);
                    match self.decorator.hit_test(rect, mouse.column, mouse.row) {
                        HeaderAction::Minimize => {
                            self.minimize_window(header.id);
                            self.last_header_click = None;
                            return true;
                        }
                        HeaderAction::Maximize => {
                            self.toggle_maximize(header.id);
                            self.last_header_click = None;
                            return true;
                        }
                        HeaderAction::Close => {
                            self.close_window(header.id);
                            self.last_header_click = None;
                            return true;
                        }
                        HeaderAction::Drag => {
                            // Double-click on header toggles maximize/restore.
                            let now = Instant::now();
                            if let Some((prev_id, prev)) = self.last_header_click
                                && prev_id == header.id
                                && now.duration_since(prev) <= Duration::from_millis(500)
                            {
                                self.toggle_maximize(header.id);
                                self.last_header_click = None;
                                return true;
                            }
                            // Record this click time and proceed to drag below.
                            self.last_header_click = Some((header.id, now));
                        }
                        HeaderAction::None => {
                            // Should not happen as we already checked rect contains
                        }
                    }

                    // Standard floating drag start
                    if self.is_window_floating(header.id) {
                        self.bring_floating_to_front_id(header.id);
                    } else {
                        // If Tiled: We detach immediately to floating (responsive drag).
                        // Keep the tiling slot reserved so the sibling doesn't expand to full screen.
                        let _ = self.detach_to_floating(header.id, rect);
                    }

                    let (initial_x, initial_y) =
                        if let Some(crate::window::FloatRectSpec::Absolute(fr)) =
                            self.floating_rect(header.id)
                        {
                            (fr.x, fr.y)
                        } else {
                            (rect.x as i32, rect.y as i32)
                        };
                    self.drag_header = Some(HeaderDrag {
                        id: header.id,
                        initial_x,
                        initial_y,
                        start_x: mouse.column,
                        start_y: mouse.row,
                    });
                    return true;
                }
            }
            MouseEventKind::Drag(_) => {
                if let Some(drag) = self.drag_header {
                    if self.is_window_floating(drag.id) {
                        self.move_floating(
                            drag.id,
                            mouse.column,
                            mouse.row,
                            drag.start_x,
                            drag.start_y,
                            drag.initial_x,
                            drag.initial_y,
                        );
                        // Only show snap preview if dragged a bit
                        let dx = mouse.column.abs_diff(drag.start_x);
                        let dy = mouse.row.abs_diff(drag.start_y);
                        if dx + dy > 2 {
                            self.update_snap_preview(drag.id, mouse.column, mouse.row);
                        } else {
                            self.drag_snap = None;
                        }
                    }
                    return true;
                }
            }
            MouseEventKind::Up(_) => {
                if let Some(drag) = self.drag_header.take() {
                    if self.drag_snap.is_some() {
                        self.apply_snap(drag.id);
                    }
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn focus_window_at(&mut self, column: u16, row: u16) -> bool {
        if self.layout_contract != LayoutContract::WindowManaged
            || self.managed_draw_order.is_empty()
        {
            return false;
        }
        let Some(hit) = self.hit_test_region_topmost(column, row, &self.managed_draw_order) else {
            return false;
        };
        if !matches!(hit, WindowId::App(_)) {
            return false;
        }
        self.focus_window_id(hit);
        true
    }

    fn handle_resize_event(&mut self, event: &Event) -> bool {
        use crossterm::event::MouseEventKind;
        let Event::Mouse(mouse) = event else {
            return false;
        };
        match mouse.kind {
            MouseEventKind::Down(_) => {
                // Check if the mouse is blocked by a window above
                let topmost_hit = if self.layout_contract == LayoutContract::WindowManaged
                    && !self.managed_draw_order.is_empty()
                {
                    self.hit_test_region_topmost(mouse.column, mouse.row, &self.managed_draw_order)
                } else {
                    None
                };

                let hit = self
                    .resize_handles
                    .iter()
                    .rev()
                    .find(|handle| rect_contains(handle.rect, mouse.column, mouse.row))
                    .copied();
                if let Some(handle) = hit {
                    // If we hit a window body that is NOT the owner of this handle,
                    // then the handle is obscured.
                    if let Some(hit_id) = topmost_hit
                        && hit_id != handle.id
                    {
                        return false;
                    }

                    let rect = self.full_region_for_id(handle.id);
                    if !self.is_window_floating(handle.id) {
                        return false;
                    }
                    self.bring_floating_to_front_id(handle.id);
                    let (start_x, start_y, start_width, start_height) =
                        if let Some(crate::window::FloatRectSpec::Absolute(fr)) =
                            self.floating_rect(handle.id)
                        {
                            (fr.x, fr.y, fr.width, fr.height)
                        } else {
                            (rect.x as i32, rect.y as i32, rect.width, rect.height)
                        };
                    self.drag_resize = Some(ResizeDrag {
                        id: handle.id,
                        edge: handle.edge,
                        start_rect: rect,
                        start_col: mouse.column,
                        start_row: mouse.row,
                        start_x,
                        start_y,
                        start_width,
                        start_height,
                    });
                    return true;
                }
            }
            MouseEventKind::Drag(_) => {
                if let Some(drag) = self.drag_resize.as_ref()
                    && self.is_window_floating(drag.id)
                {
                    let resized = apply_resize_drag_signed(
                        drag.start_x,
                        drag.start_y,
                        drag.start_width,
                        drag.start_height,
                        drag.edge,
                        mouse.column,
                        mouse.row,
                        drag.start_col,
                        drag.start_row,
                        self.managed_area,
                        self.floating_resize_offscreen,
                    );
                    self.set_floating_rect(
                        drag.id,
                        Some(crate::window::FloatRectSpec::Absolute(resized)),
                    );
                    return true;
                }
            }
            MouseEventKind::Up(_) => {
                if self.drag_resize.take().is_some() {
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn detach_to_floating(&mut self, id: WindowId<R>, rect: Rect) -> bool {
        if self.is_window_floating(id) {
            return true;
        }
        if self.managed_layout.is_none() {
            return false;
        }

        let width = rect.width.max(1);
        let height = rect.height.max(1);
        let x = rect.x;
        let y = rect.y;
        self.set_floating_rect(
            id,
            Some(crate::window::FloatRectSpec::Absolute(
                crate::window::FloatRect {
                    x: x as i32,
                    y: y as i32,
                    width,
                    height,
                },
            )),
        );
        self.bring_to_front_id(id);
        true
    }

    fn layout_contains(&self, id: WindowId<R>) -> bool {
        self.managed_layout
            .as_ref()
            .is_some_and(|layout| layout.root().subtree_any(|node_id| node_id == id))
    }

    // narrow allow: refactor into a small struct if/when argument list needs reduction
    #[allow(clippy::too_many_arguments)]
    fn move_floating(
        &mut self,
        id: WindowId<R>,
        column: u16,
        row: u16,
        start_mouse_x: u16,
        start_mouse_y: u16,
        initial_x: i32,
        initial_y: i32,
    ) {
        let panel_active = self.panel_active();
        let bounds = self.managed_area;
        let Some(crate::window::FloatRectSpec::Absolute(fr)) = self.floating_rect(id) else {
            return;
        };
        let width = fr.width.max(1);
        let height = fr.height.max(1);
        let dx = column as i32 - start_mouse_x as i32;
        let dy = row as i32 - start_mouse_y as i32;
        let x = initial_x + dx;
        let mut y = initial_y + dy;
        let bounds_y = bounds.y as i32;
        if panel_active && y < bounds_y {
            y = bounds_y;
        }
        self.set_floating_rect(
            id,
            Some(crate::window::FloatRectSpec::Absolute(
                crate::window::FloatRect {
                    x,
                    y,
                    width,
                    height,
                },
            )),
        );
    }

    fn update_snap_preview(&mut self, dragging_id: WindowId<R>, mouse_x: u16, mouse_y: u16) {
        self.drag_snap = None;
        let area = self.managed_area;

        // 1. Check Window Snap first (more specific)
        // We iterate z-order (top-to-bottom) to find the first valid target under mouse.
        // We only allow snapping to windows that are already tiled, unless the layout is empty.
        let target = self.z_order.iter().rev().find_map(|&id| {
            if id == dragging_id {
                return None;
            }
            // If we have a layout, ignore floating windows as snap targets
            // to prevent "bait and switch" (offering to split a float, then splitting root).
            if self.managed_layout.is_some() && self.is_window_floating(id) {
                return None;
            }

            let rect = self.regions.get(id)?;
            if rect_contains(rect, mouse_x, mouse_y) {
                Some((id, rect))
            } else {
                None
            }
        });

        if let Some((target_id, rect)) = target {
            let h = rect.height;

            // Distance to edges
            let d_top = mouse_y.saturating_sub(rect.y);
            let d_bottom = (rect.y + h).saturating_sub(1).saturating_sub(mouse_y);

            // Sensitivity: Allow a reasonable localized zone.
            // Reduced sensitivity to prevent accidental snaps when crossing windows.
            // w/10 is 10%. Clamped to [2, 6] means you must be quite close to the edge.
            let sens_y = (h / 10).clamp(1, 4);

            // Check if the closest edge is within its sensitivity limit
            // Only allow snapping on horizontal seams (top/bottom of tiled panes).
            let snap = if d_top < sens_y && d_top <= d_bottom {
                Some((
                    InsertPosition::Top,
                    Rect {
                        height: h / 2,
                        ..rect
                    },
                ))
            } else if d_bottom < sens_y {
                Some((
                    InsertPosition::Bottom,
                    Rect {
                        y: rect.y + h / 2,
                        height: h / 2,
                        ..rect
                    },
                ))
            } else {
                None
            };

            if let Some((pos, preview)) = snap {
                self.drag_snap = Some((Some(target_id), pos, preview));
                return;
            }
        }

        // 2. Check Screen Edge Snap (fallback, less specific)
        let sensitivity = 2; // Strict sensitivity for screen edge

        let d_left = mouse_x.saturating_sub(area.x);
        let d_right = (area.x + area.width)
            .saturating_sub(1)
            .saturating_sub(mouse_x);
        let d_top = mouse_y.saturating_sub(area.y);
        let d_bottom = (area.y + area.height)
            .saturating_sub(1)
            .saturating_sub(mouse_y);

        let min_screen_dist = d_left.min(d_right).min(d_top).min(d_bottom);

        let position = if min_screen_dist < sensitivity {
            if d_left == min_screen_dist {
                Some(InsertPosition::Left)
            } else if d_right == min_screen_dist {
                Some(InsertPosition::Right)
            } else if d_top == min_screen_dist {
                Some(InsertPosition::Top)
            } else if d_bottom == min_screen_dist {
                Some(InsertPosition::Bottom)
            } else {
                None
            }
        } else {
            None
        };

        if let Some(pos) = position {
            let mut preview = match pos {
                InsertPosition::Left => Rect {
                    width: area.width / 2,
                    ..area
                },
                InsertPosition::Right => Rect {
                    x: area.x + area.width / 2,
                    width: area.width / 2,
                    ..area
                },
                InsertPosition::Top => Rect {
                    height: area.height / 2,
                    ..area
                },
                InsertPosition::Bottom => Rect {
                    y: area.y + area.height / 2,
                    height: area.height / 2,
                    ..area
                },
            };

            // If there's no layout to split, dragging to edge just re-tiles to full screen.
            if self.managed_layout.is_none() {
                preview = area;
            }

            self.drag_snap = Some((None, pos, preview));
        }
    }

    fn apply_snap(&mut self, id: WindowId<R>) {
        if let Some((target, position, preview)) = self.drag_snap.take() {
            // Check if we should tile or float-snap
            // We float-snap if we are snapping to a screen edge (target is None)
            // AND the layout is empty (no other tiled windows).
            let other_windows_exist = if let Some(layout) = &self.managed_layout {
                !layout.regions(self.managed_area).is_empty()
            } else {
                false
            };

            if target.is_none() && !other_windows_exist {
                // Single window edge snap -> Floating Resize
                if self.is_window_floating(id) {
                    self.set_floating_rect(
                        id,
                        Some(crate::window::FloatRectSpec::Absolute(
                            crate::window::FloatRect {
                                x: preview.x as i32,
                                y: preview.y as i32,
                                width: preview.width,
                                height: preview.height,
                            },
                        )),
                    );
                }
                return;
            }

            if self.is_window_floating(id) {
                self.clear_floating_rect(id);
            }

            if self.layout_contains(id)
                && let Some(layout) = &mut self.managed_layout
            {
                let should_retile = match target {
                    Some(target_id) => target_id != id,
                    None => true,
                };
                if should_retile {
                    layout.root_mut().remove_leaf(id);
                } else {
                    self.bring_to_front_id(id);
                    return;
                }
            }

            // Handle case where target is floating (and thus not in layout yet)
            if let Some(target_id) = target
                && self.is_window_floating(target_id)
            {
                // Target is floating. We must initialize layout with it.
                self.clear_floating_rect(target_id);
                if self.managed_layout.is_none() {
                    self.managed_layout = Some(TilingLayout::new(LayoutNode::leaf(target_id)));
                } else {
                    // This case is tricky: managed_layout exists (implied other windows), but target is floating.
                    // We need to tile 'id' based on 'target'.
                    // However, 'target' itself isn't in the tree.
                    // This implies we want to perform a "Merge" of two floating windows into a new tiled group?
                    // But we only support one root.
                    // If managed_layout exists, we probably shouldn't be here if target is floating?
                    // Actually, if we have {C} tiled, and {B} floating. Snap A to B.
                    // We want {A, B} tiled?
                    // Current logic: If managed_layout is Some, we try insert_leaf.
                    // If insert_leaf fails, we fallback to split_root.
                    // If we fall back to split_root, A is added to root. B remains floating.
                    // This is acceptable/safe.
                    // The critical case is when managed_layout is None (the 2-window case).
                }
            }

            if let Some(layout) = &mut self.managed_layout {
                let success = if let Some(target_id) = target {
                    layout.root_mut().insert_leaf(target_id, id, position)
                } else {
                    false
                };

                if !success {
                    // If insert failed (e.g. target was missing or we are splitting root),
                    // If target was the one we just initialized (in the floating case above), it should be at root.
                    // insert_leaf should have worked if target is root.
                    layout.split_root(id, position);
                }

                // Ensure the snapped window is brought to front/focused
                if let Some(pos) = self.z_order.iter().position(|&z_id| z_id == id) {
                    self.z_order.remove(pos);
                }
                self.z_order.push(id);
                self.managed_draw_order = self.z_order.clone();
            } else {
                self.managed_layout = Some(TilingLayout::new(LayoutNode::leaf(id)));
            }

            // If we snapped a window into place, any other floating windows should snap as well.
            let mut pending_snap = Vec::new();
            for r_id in self.regions.ids() {
                if r_id != id && self.is_window_floating(r_id) {
                    pending_snap.push(r_id);
                }
            }
            for float_id in pending_snap {
                self.tile_window_id(float_id);
            }
        }
    }

    /// Smartly insert a window into the tiling layout.
    /// If there is a focused tiled window, split it.
    /// Otherwise, split the root.
    pub fn tile_window(&mut self, id: R) -> bool {
        self.tile_window_id(WindowId::app(id))
    }

    fn tile_window_id(&mut self, id: WindowId<R>) -> bool {
        // If already in layout or floating, do nothing (or move it?)
        // For now, assume this is for new windows.
        if self.layout_contains(id) {
            if self.is_window_floating(id) {
                self.clear_floating_rect(id);
            }
            self.focus_window_id(id);
            return true;
        }
        if self.managed_layout.is_none() {
            self.managed_layout = Some(TilingLayout::new(LayoutNode::leaf(id)));
            self.focus_window_id(id);
            return true;
        }

        // Try to find a focused node that is in the layout
        let current_focus = self.wm_focus.current();

        let mut target_r = None;
        for r_id in self.regions.ids() {
            if r_id == current_focus {
                target_r = Some(r_id);
                break;
            }
        }

        let Some(layout) = self.managed_layout.as_mut() else {
            return false;
        };

        // If we found a focused region, split it
        if let Some(target) = target_r {
            // Prefer splitting horizontally (side-by-side) for wide windows, vertically for tall?
            // Or just default to Right/Bottom.
            // Let's default to Right for now as it's common.
            if layout
                .root_mut()
                .insert_leaf(target, id, InsertPosition::Right)
            {
                self.focus_window_id(id);
                return true;
            }
        }

        // Fallback: split root
        layout.split_root(id, InsertPosition::Right);
        self.focus_window_id(id);
        true
    }

    pub fn bring_to_front(&mut self, id: R) {
        self.bring_to_front_id(WindowId::app(id));
    }

    fn bring_to_front_id(&mut self, id: WindowId<R>) {
        if let Some(pos) = self.z_order.iter().position(|&x| x == id) {
            let item = self.z_order.remove(pos);
            self.z_order.push(item);
        }
    }

    fn window_dest(&self, id: WindowId<R>, fallback: Rect) -> crate::window::FloatRect {
        if let Some(spec) = self.floating_rect(id) {
            spec.resolve_signed(self.managed_area)
        } else {
            crate::window::FloatRect {
                x: fallback.x as i32,
                y: fallback.y as i32,
                width: fallback.width,
                height: fallback.height,
            }
        }
    }

    fn visible_rect_from_spec(&self, spec: crate::window::FloatRectSpec) -> Rect {
        float_rect_visible(spec.resolve_signed(self.managed_area), self.managed_area)
    }

    fn visible_region_for_id(&self, id: WindowId<R>) -> Rect {
        if let Some(spec) = self.floating_rect(id) {
            self.visible_rect_from_spec(spec)
        } else {
            self.full_region_for_id(id)
        }
    }

    pub fn bring_all_floating_to_front(&mut self) {
        let ids: Vec<WindowId<R>> = self
            .z_order
            .iter()
            .copied()
            .filter(|id| self.is_window_floating(*id))
            .collect();
        for id in ids {
            self.bring_to_front_id(id);
        }
    }

    fn bring_floating_to_front_id(&mut self, id: WindowId<R>) {
        self.bring_to_front_id(id);
    }

    fn bring_floating_to_front(&mut self, id: R) {
        self.bring_floating_to_front_id(WindowId::app(id));
    }

    fn clamp_floating_to_bounds(&mut self) {
        let bounds = self.managed_area;
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        // Collect updates first to avoid borrowing `self` mutably while iterating
        let mut updates: Vec<(WindowId<R>, crate::window::FloatRectSpec)> = Vec::new();
        let floating_ids: Vec<WindowId<R>> = self
            .windows
            .iter()
            .filter_map(|(&id, window)| window.floating_rect.as_ref().map(|_| id))
            .collect();
        for id in floating_ids {
            let Some(crate::window::FloatRectSpec::Absolute(fr)) = self.floating_rect(id) else {
                continue;
            };

            // Only recover panes that are fully off-screen; keep normal dragging untouched.
            // Use signed arithmetic for off-screen detection.
            let rect_left = fr.x;
            let rect_top = fr.y;
            let rect_right = fr.x.saturating_add(fr.width as i32);
            let rect_bottom = fr.y.saturating_add(fr.height as i32);
            let bounds_left = bounds.x as i32;
            let bounds_top = bounds.y as i32;
            let bounds_right = bounds_left.saturating_add(bounds.width as i32);
            let bounds_bottom = bounds_top.saturating_add(bounds.height as i32);

            let min_w = FLOATING_MIN_WIDTH.min(bounds.width.max(1));
            let min_h = FLOATING_MIN_HEIGHT.min(bounds.height.max(1));

            // Ensure at least a small portion of the window (e.g. handle) is always visible
            // so the user can grab it back.
            let min_visible_margin = MIN_FLOATING_VISIBLE_MARGIN;

            let width = if self.floating_resize_offscreen {
                fr.width.max(min_w)
            } else {
                fr.width.max(min_w).min(bounds.width)
            };
            let height = if self.floating_resize_offscreen {
                fr.height.max(min_h)
            } else {
                fr.height.max(min_h).min(bounds.height)
            };

            let max_x = if self.floating_resize_offscreen {
                (bounds
                    .x
                    .saturating_add(bounds.width)
                    .saturating_sub(min_visible_margin.min(width))) as i32
            } else {
                bounds.x.saturating_add(bounds.width.saturating_sub(width)) as i32
            };

            let max_y = if self.floating_resize_offscreen {
                (bounds
                    .y
                    .saturating_add(bounds.height)
                    .saturating_sub(min_visible_margin.min(height))) as i32
            } else {
                bounds
                    .y
                    .saturating_add(bounds.height.saturating_sub(height)) as i32
            };

            // Clamp an axis if the rect is fully outside it, or if the
            // visible portion is smaller than the minimum visible margin.
            let out_x = rect_right <= bounds_left || rect_left >= bounds_right;
            let out_y = rect_bottom <= bounds_top || rect_top >= bounds_bottom;

            // When `floating_resize_offscreen` is enabled we allow dragging a
            // floating pane partially off the edges while ensuring a small
            // visible margin remains. If the pane is fully off-screen on an
            // axis (`out_x`/`out_y`) or offscreen handling is disabled, recover
            // it into the visible bounds as before.
            let x = if out_x || !self.floating_resize_offscreen {
                fr.x.clamp(bounds_left, max_x)
            } else {
                // Compute left-most allowed x such that at least
                // `min_visible_margin` columns remain visible inside `bounds`.
                let left_allowed =
                    bounds_left.saturating_sub(width as i32 - min_visible_margin.min(width) as i32);
                let left_allowed = left_allowed.min(max_x);
                fr.x.clamp(left_allowed, max_x)
            };

            let y = if out_y || !self.floating_resize_offscreen {
                fr.y.clamp(bounds_top, max_y)
            } else {
                let visible_height = min_visible_margin.min(height) as i32;
                let top_allowed = bounds_top.saturating_sub(height as i32 - visible_height);
                let top_allowed = top_allowed.min(max_y);
                fr.y.clamp(top_allowed, max_y)
            };

            updates.push((
                id,
                crate::window::FloatRectSpec::Absolute(crate::window::FloatRect {
                    x,
                    y,
                    width,
                    height,
                }),
            ));
        }
        for (id, spec) in updates {
            self.set_floating_rect(id, Some(spec));
        }
    }

    pub fn window_draw_plan(&mut self, frame: &mut UiFrame<'_>) -> Vec<WindowDrawTask<R>> {
        let mut plan = Vec::new();
        let focused_window = self.wm_focus.current();
        for &id in &self.managed_draw_order {
            let full = self.full_region_for_id(id);
            if full.width == 0 || full.height == 0 {
                continue;
            }
            let visible_full = self.visible_region_for_id(id);
            if visible_full.width == 0 || visible_full.height == 0 {
                continue;
            }
            frame.render_widget(Clear, visible_full);
            let dest = self.window_dest(id, full);
            match id {
                WindowId::System(system_id) => {
                    if !self.system_window_visible(system_id) {
                        continue;
                    }
                    let inner_abs = self.region_for_id(id);
                    let inner = Rect {
                        x: inner_abs.x.saturating_sub(full.x),
                        y: inner_abs.y.saturating_sub(full.y),
                        width: inner_abs.width,
                        height: inner_abs.height,
                    };
                    if inner.width == 0 || inner.height == 0 {
                        continue;
                    }
                    plan.push(WindowDrawTask::System(SystemWindowDraw {
                        id: system_id,
                        surface: WindowSurface { full, inner, dest },
                        focused: focused_window == id,
                    }));
                }
                WindowId::App(app_id) => {
                    let inner_abs = self.region(app_id);
                    let inner = Rect {
                        x: inner_abs.x.saturating_sub(full.x),
                        y: inner_abs.y.saturating_sub(full.y),
                        width: inner_abs.width,
                        height: inner_abs.height,
                    };
                    if inner.width == 0 || inner.height == 0 {
                        continue;
                    }
                    plan.push(WindowDrawTask::App(AppWindowDraw {
                        id: app_id,
                        surface: WindowSurface { full, inner, dest },
                        focused: focused_window == WindowId::app(app_id),
                    }));
                }
            }
        }
        plan
    }

    pub fn render_system_window(&mut self, frame: &mut UiFrame<'_>, window: SystemWindowDraw) {
        if window.surface.inner.width == 0 || window.surface.inner.height == 0 {
            return;
        }
        self.render_system_window_entry(frame, window);
    }

    fn hover_targets(&self) -> (Option<&SplitHandle>, Option<&ResizeHandle<WindowId<R>>>) {
        let Some((column, row)) = self.hover else {
            return (None, None);
        };
        let topmost = self.hit_test_region_topmost(column, row, &self.managed_draw_order);
        let hovered = if topmost.is_none() {
            self.handles
                .iter()
                .find(|handle| rect_contains(handle.rect, column, row))
        } else {
            None
        };
        let hovered_resize = self
            .resize_handles
            .iter()
            .find(|handle| rect_contains(handle.rect, column, row) && topmost == Some(handle.id));
        (hovered, hovered_resize)
    }

    pub fn render_overlays(&mut self, frame: &mut UiFrame<'_>) {
        let (hovered, hovered_resize) = self.hover_targets();
        let obscuring: Vec<Rect> = self
            .managed_draw_order
            .iter()
            .filter_map(|&id| self.regions.get(id))
            .collect();
        let is_obscured =
            |x: u16, y: u16| -> bool { obscuring.iter().any(|r| rect_contains(*r, x, y)) };
        render_handles_masked(frame, &self.handles, hovered, is_obscured);
        // Build floating panes list from per-window entries for resize outline rendering
        let floating_panes: Vec<FloatingPane<WindowId<R>>> = self
            .windows
            .iter()
            .filter_map(|(&id, window)| {
                window.floating_rect.map(|rect| match rect {
                    crate::window::FloatRectSpec::Absolute(fr) => FloatingPane {
                        id,
                        rect: crate::layout::RectSpec::Absolute(ratatui::prelude::Rect {
                            x: fr.x.max(0) as u16,
                            y: fr.y.max(0) as u16,
                            width: fr.width,
                            height: fr.height,
                        }),
                    },
                    crate::window::FloatRectSpec::Percent {
                        x,
                        y,
                        width,
                        height,
                    } => FloatingPane {
                        id,
                        rect: crate::layout::RectSpec::Percent {
                            x,
                            y,
                            width,
                            height,
                        },
                    },
                })
            })
            .collect();

        let mut visible_regions = RegionMap::default();
        for id in self.regions.ids() {
            visible_regions.set(id, self.visible_region_for_id(id));
        }

        render_resize_outline(
            frame,
            hovered_resize.copied(),
            self.drag_resize,
            &visible_regions,
            self.managed_area,
            &floating_panes,
            &self.managed_draw_order,
        );

        if let Some((_, _, rect)) = self.drag_snap {
            let buffer = frame.buffer_mut();
            let color = crate::theme::accent();
            let clip = rect.intersection(buffer.area);
            if clip.width > 0 && clip.height > 0 {
                for y in clip.y..clip.y.saturating_add(clip.height) {
                    for x in clip.x..clip.x.saturating_add(clip.width) {
                        if let Some(cell) = buffer.cell_mut((x, y)) {
                            let mut style = cell.style();
                            style.bg = Some(color);
                            cell.set_style(style);
                        }
                    }
                }
            }
        }

        let status_line = if self.wm_overlay_visible() {
            let esc_state = if let Some(remaining) = self.esc_passthrough_remaining() {
                format!("Esc passthrough: active ({}ms)", remaining.as_millis())
            } else {
                "Esc passthrough: inactive".to_string()
            };
            Some(format!("{esc_state} · Tab/Shift-Tab: cycle windows"))
        } else {
            None
        };
        let display = self.build_display_order();
        // Build a small title map to avoid borrowing `self` inside the panel closure
        let titles_map: BTreeMap<WindowId<R>, String> = self
            .windows
            .keys()
            .map(|id| (*id, self.window_title(*id)))
            .collect();

        self.panel.render(
            frame,
            self.panel_active(),
            self.wm_focus.current(),
            &display,
            status_line.as_deref(),
            self.mouse_capture_enabled(),
            self.clipboard_enabled(),
            self.clipboard_available(),
            self.wm_overlay_visible(),
            move |id| {
                titles_map.get(&id).cloned().unwrap_or_else(|| match id {
                    WindowId::App(app_id) => format!("{:?}", app_id),
                    WindowId::System(SystemWindowId::DebugLog) => "Debug Log".to_string(),
                })
            },
        );
        let menu_items = wm_menu_items(
            self.mouse_capture_enabled(),
            self.clipboard_enabled(),
            self.clipboard_available(),
        );
        let menu_labels = menu_items
            .iter()
            .map(|item| (item.icon, item.label))
            .collect::<Vec<_>>();
        let bounds = frame.area();
        self.panel.render_menu(
            frame,
            self.wm_overlay_visible(),
            bounds,
            &menu_labels,
            self.state.wm_menu_selected(),
        );
        self.panel.render_menu_backdrop(
            frame,
            self.wm_overlay_visible(),
            self.managed_area,
            self.panel.area(),
        );

        // Render overlays in fixed order if they exist
        if let Some(confirm) = self.overlays.get_mut(&OverlayId::ExitConfirm) {
            confirm.render(
                frame,
                frame.area(),
                &ComponentContext::new(false).with_overlay(true),
            );
        }
        if let Some(help) = self.overlays.get_mut(&OverlayId::Help) {
            help.render(
                frame,
                frame.area(),
                &ComponentContext::new(false).with_overlay(true),
            );
        }
    }

    pub fn clear_window_backgrounds(&self, frame: &mut UiFrame<'_>) {
        for id in self.regions.ids() {
            let rect = self.full_region_for_id(id);
            frame.render_widget(Clear, rect);
        }
    }

    pub fn set_regions_from_plan(&mut self, plan: &LayoutPlan<R>, area: Rect) {
        let plan_regions = plan.regions(area);
        self.regions = RegionMap::default();
        for id in plan_regions.ids() {
            if let Some(rect) = plan_regions.get(id) {
                self.regions.set(WindowId::app(id), rect);
            }
        }
    }

    pub fn hit_test_region(&self, column: u16, row: u16, ids: &[R]) -> Option<R> {
        for id in ids {
            let rect = self.visible_region_for_id(WindowId::app(*id));
            if rect.width > 0 && rect.height > 0 && rect_contains(rect, column, row) {
                return Some(*id);
            }
        }
        None
    }

    /// Hit-test regions by draw order so overlapping panes pick the topmost one.
    /// This avoids clicks "falling through" floating panes to windows behind them.
    fn hit_test_region_topmost(
        &self,
        column: u16,
        row: u16,
        ids: &[WindowId<R>],
    ) -> Option<WindowId<R>> {
        for id in ids.iter().rev() {
            let rect = self.visible_region_for_id(*id);
            if rect.width > 0 && rect.height > 0 && rect_contains(rect, column, row) {
                return Some(*id);
            }
        }
        None
    }

    pub fn handle_focus_event<F, G>(
        &mut self,
        event: &Event,
        hit_targets: &[R],
        map: F,
        map_focus: G,
    ) -> bool
    where
        F: Fn(R) -> W,
        G: Fn(W) -> Option<R>,
    {
        match event {
            Event::Key(key) => {
                let kb = crate::keybindings::KeyBindings::default();
                if kb.matches(crate::keybindings::Action::FocusNext, key) {
                    if self.layout_contract == LayoutContract::WindowManaged {
                        self.advance_wm_focus(true);
                    } else {
                        self.app_focus.advance(true);
                        let focused_app = self.app_focus.current();
                        if let Some(region) = map_focus(focused_app) {
                            self.set_wm_focus(WindowId::app(region));
                            self.bring_to_front_id(WindowId::app(region));
                            self.managed_draw_order = self.z_order.clone();
                        }
                    }
                    true
                } else if kb.matches(crate::keybindings::Action::FocusPrev, key) {
                    if self.layout_contract == LayoutContract::WindowManaged {
                        self.advance_wm_focus(false);
                    } else {
                        self.app_focus.advance(false);
                        let focused_app = self.app_focus.current();
                        if let Some(region) = map_focus(focused_app) {
                            self.set_wm_focus(WindowId::app(region));
                            self.bring_to_front_id(WindowId::app(region));
                            self.managed_draw_order = self.z_order.clone();
                        }
                    }
                    true
                } else {
                    false
                }
            }
            Event::Mouse(mouse) => {
                self.hover = Some((mouse.column, mouse.row));
                match mouse.kind {
                    MouseEventKind::Down(_) => {
                        if self.layout_contract == LayoutContract::WindowManaged
                            && !self.managed_draw_order.is_empty()
                        {
                            let hit = self.hit_test_region_topmost(
                                mouse.column,
                                mouse.row,
                                &self.managed_draw_order,
                            );
                            if let Some(id) = hit {
                                self.set_wm_focus(id);
                                self.bring_floating_to_front_id(id);
                                return true;
                            }
                            return false;
                        }
                        let hit = self.hit_test_region(mouse.column, mouse.row, hit_targets);
                        if let Some(hit) = hit {
                            self.app_focus.set_current(map(hit));
                            if self.layout_contract == LayoutContract::WindowManaged {
                                self.set_wm_focus(WindowId::app(hit));
                                self.bring_floating_to_front(hit);
                            }
                            true
                        } else {
                            false
                        }
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn handle_system_window_event(&mut self, event: &Event) -> bool {
        if self.layout_contract != LayoutContract::WindowManaged {
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
                    && let WindowId::System(system_id) = self.wm_focus.current()
                    && self.system_window_visible(system_id)
                {
                    self.select_fallback_focus();
                }
                false
            }
            Event::Key(_) => {
                if let WindowId::System(system_id) = self.wm_focus.current()
                    && self.system_window_visible(system_id)
                {
                    return self.dispatch_system_window_event(system_id, event);
                }
                false
            }
            _ => false,
        }
    }

    fn panel_active(&self) -> bool {
        self.layout_contract == LayoutContract::WindowManaged
            && self.panel.visible()
            && self.panel.height() > 0
    }

    fn focus_for_region(&self, id: R) -> Option<W> {
        if self.app_focus.order.is_empty() {
            if id == self.app_focus.current {
                Some(self.app_focus.current)
            } else {
                None
            }
        } else {
            self.app_focus
                .order
                .iter()
                .copied()
                .find(|focus| id == *focus)
        }
    }

    pub fn handle_wm_menu_event(&mut self, event: &Event) -> Option<WmMenuAction> {
        if !self.wm_overlay_visible() {
            return None;
        }
        let items = wm_menu_items(
            self.mouse_capture_enabled(),
            self.clipboard_enabled(),
            self.clipboard_available(),
        );
        if let Event::Mouse(mouse) = event
            && matches!(mouse.kind, MouseEventKind::Down(_))
        {
            if let Some(index) = self.panel.hit_test_menu_item(event) {
                let selected = index.min(items.len().saturating_sub(1));
                self.state.set_wm_menu_selected(selected);
                return items.get(selected).map(|item| item.action);
            }
            if self.panel.menu_icon_contains_point(mouse.column, mouse.row) {
                return Some(WmMenuAction::CloseMenu);
            }
            if !self.panel.menu_contains_point(mouse.column, mouse.row) {
                return Some(WmMenuAction::CloseMenu);
            }
        }
        let Event::Key(key) = event else {
            return None;
        };
        let kb = crate::keybindings::KeyBindings::default();
        if kb.matches(crate::keybindings::Action::MenuUp, key)
            || kb.matches(crate::keybindings::Action::MenuPrev, key)
        {
            let total = items.len();
            if total > 0 {
                let current = self.state.wm_menu_selected();
                if current == 0 {
                    self.state.set_wm_menu_selected(total - 1);
                } else {
                    self.state.set_wm_menu_selected(current - 1);
                }
            }
            None
        } else if kb.matches(crate::keybindings::Action::MenuDown, key)
            || kb.matches(crate::keybindings::Action::MenuNext, key)
        {
            let total = items.len();
            if total > 0 {
                let current = self.state.wm_menu_selected();
                self.state.set_wm_menu_selected((current + 1) % total);
            }
            None
        } else if kb.matches(crate::keybindings::Action::MenuSelect, key) {
            items
                .get(self.state.wm_menu_selected())
                .map(|item| item.action)
        } else {
            None
        }
    }

    pub fn handle_exit_confirm_event(&mut self, event: &Event) -> Option<ConfirmAction> {
        let comp = self.overlays.get_mut(&OverlayId::ExitConfirm)?;
        // Downcast to ConfirmOverlayComponent to access specific method
        if let Some(confirm) = comp.as_any_mut().downcast_mut::<ConfirmOverlayComponent>() {
            return confirm.handle_confirm_event(event);
        }
        None
    }

    pub fn wm_menu_consumes_event(&self, event: &Event) -> bool {
        if !self.wm_overlay_visible() {
            return false;
        }
        let Event::Key(key) = event else {
            return false;
        };
        let kb = crate::keybindings::KeyBindings::default();
        kb.matches(crate::keybindings::Action::MenuUp, key)
            || kb.matches(crate::keybindings::Action::MenuDown, key)
            || kb.matches(crate::keybindings::Action::MenuSelect, key)
            || kb.matches(crate::keybindings::Action::MenuNext, key)
            || kb.matches(crate::keybindings::Action::MenuPrev, key)
    }
}

#[derive(Debug, Clone, Copy)]
struct WmMenuItem {
    label: &'static str,
    icon: Option<&'static str>,
    action: WmMenuAction,
}
fn wm_menu_items(
    mouse_capture_enabled: bool,
    clipboard_enabled: bool,
    clipboard_available: bool,
) -> [WmMenuItem; 8] {
    let mouse_label = if mouse_capture_enabled {
        "Mouse Capture: On"
    } else {
        "Mouse Capture: Off"
    };
    let clipboard_label = if clipboard_available {
        if clipboard_enabled {
            "Clipboard Mode: On"
        } else {
            "Clipboard Mode: Off"
        }
    } else {
        "Clipboard Mode: Unavailable"
    };
    [
        WmMenuItem {
            label: "Resume",
            icon: None,
            action: WmMenuAction::CloseMenu,
        },
        WmMenuItem {
            label: mouse_label,
            icon: Some("🖱"),
            action: WmMenuAction::ToggleMouseCapture,
        },
        WmMenuItem {
            label: clipboard_label,
            icon: Some("📋"),
            action: WmMenuAction::ToggleClipboardMode,
        },
        WmMenuItem {
            label: "Floating Front",
            icon: Some("↑"),
            action: WmMenuAction::BringFloatingFront,
        },
        WmMenuItem {
            label: "New Window",
            icon: Some("+"),
            action: WmMenuAction::NewWindow,
        },
        WmMenuItem {
            label: "Debug Log",
            icon: Some("≣"),
            action: WmMenuAction::ToggleDebugWindow,
        },
        WmMenuItem {
            label: "Help",
            icon: Some("?"),
            action: WmMenuAction::Help,
        },
        WmMenuItem {
            label: "Exit UI",
            icon: Some("⏻"),
            action: WmMenuAction::ExitUi,
        },
    ]
}

fn esc_passthrough_window_default() -> Duration {
    #[cfg(windows)]
    {
        Duration::from_millis(1200)
    }
    #[cfg(not(windows))]
    {
        Duration::from_millis(600)
    }
}

fn clamp_rect(area: Rect, bounds: Rect) -> Rect {
    let x0 = area.x.max(bounds.x);
    let y0 = area.y.max(bounds.y);
    let x1 = area
        .x
        .saturating_add(area.width)
        .min(bounds.x.saturating_add(bounds.width));
    let y1 = area
        .y
        .saturating_add(area.height)
        .min(bounds.y.saturating_add(bounds.height));
    if x1 <= x0 || y1 <= y0 {
        return Rect::default();
    }
    Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    }
}

fn float_rect_visible(rect: crate::window::FloatRect, bounds: Rect) -> Rect {
    let bounds_x0 = bounds.x as i32;
    let bounds_y0 = bounds.y as i32;
    let bounds_x1 = bounds_x0 + bounds.width as i32;
    let bounds_y1 = bounds_y0 + bounds.height as i32;
    let rect_x0 = rect.x;
    let rect_y0 = rect.y;
    let rect_x1 = rect.x + rect.width as i32;
    let rect_y1 = rect.y + rect.height as i32;
    let x0 = rect_x0.max(bounds_x0);
    let y0 = rect_y0.max(bounds_y0);
    let x1 = rect_x1.min(bounds_x1);
    let y1 = rect_y1.min(bounds_y1);
    if x1 <= x0 || y1 <= y0 {
        return Rect::default();
    }
    Rect {
        x: x0 as u16,
        y: y0 as u16,
        width: (x1 - x0) as u16,
        height: (y1 - y0) as u16,
    }
}

fn map_layout_node<R: Copy + Eq + Ord>(node: &LayoutNode<R>) -> LayoutNode<WindowId<R>> {
    match node {
        LayoutNode::Leaf(id) => LayoutNode::leaf(WindowId::app(*id)),
        LayoutNode::Split {
            direction,
            children,
            weights,
            constraints,
            resizable,
        } => LayoutNode::Split {
            direction: *direction,
            children: children.iter().map(map_layout_node).collect(),
            weights: weights.clone(),
            constraints: constraints.clone(),
            resizable: *resizable,
        },
    }
}

#[cfg(test)]
fn rects_intersect(a: Rect, b: Rect) -> bool {
    if a.width == 0 || a.height == 0 || b.width == 0 || b.height == 0 {
        return false;
    }
    let a_right = a.x.saturating_add(a.width);
    let a_bottom = a.y.saturating_add(a.height);
    let b_right = b.x.saturating_add(b.width);
    let b_bottom = b.y.saturating_add(b.height);
    a.x < b_right && a_right > b.x && a.y < b_bottom && a_bottom > b.y
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::{Direction, Rect};

    #[test]
    fn clamp_rect_inside_and_outside() {
        let area = Rect {
            x: 2,
            y: 2,
            width: 4,
            height: 4,
        };
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let r = clamp_rect(area, bounds);
        assert_eq!(r.x, 2);
        assert_eq!(r.y, 2);

        // non-overlapping
        let area2 = Rect {
            x: 50,
            y: 50,
            width: 1,
            height: 1,
        };
        let r2 = clamp_rect(area2, bounds);
        assert_eq!(r2, Rect::default());
    }

    #[test]
    fn float_rect_visible_clips_negative_offsets() {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let rect = crate::window::FloatRect {
            x: -5,
            y: 3,
            width: 20,
            height: 6,
        };
        let visible = float_rect_visible(rect, bounds);
        assert_eq!(visible.x, 0);
        assert_eq!(visible.y, 3);
        assert_eq!(visible.width, 15);
        assert_eq!(visible.height, 6);
    }

    #[test]
    fn rects_intersect_true_and_false() {
        let a = Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 5,
        };
        let b = Rect {
            x: 4,
            y: 4,
            width: 5,
            height: 5,
        };
        assert!(rects_intersect(a, b));
        let c = Rect {
            x: 10,
            y: 10,
            width: 1,
            height: 1,
        };
        assert!(!rects_intersect(a, c));
    }

    #[test]
    fn map_layout_node_maps_leaf_to_windowid_app() {
        let node = LayoutNode::leaf(3usize);
        let mapped = map_layout_node(&node);
        match mapped {
            LayoutNode::Leaf(id) => match id {
                WindowId::App(r) => assert_eq!(r, 3usize),
                _ => panic!("expected App window id"),
            },
            _ => panic!("expected leaf"),
        }
    }

    #[test]
    fn esc_passthrough_default_nonzero() {
        let d = esc_passthrough_window_default();
        assert!(d.as_millis() > 0);
    }

    #[test]
    fn focus_ring_wraps_and_advances() {
        let mut ring = FocusRing::new(2usize);
        ring.set_order(vec![1usize, 2usize, 3usize]);
        assert_eq!(ring.current(), 2);
        ring.advance(true);
        assert_eq!(ring.current(), 3);
        ring.advance(true);
        assert_eq!(ring.current(), 1);
        ring.advance(false);
        assert_eq!(ring.current(), 3);
    }

    #[test]
    fn scroll_state_apply_and_bump() {
        let mut s = ScrollState::default();
        s.bump(5);
        s.apply(100, 10);
        assert_eq!(s.offset, 5usize);

        s.offset = 1000;
        s.apply(20, 5);
        let max_off = 20usize.saturating_sub(5usize);
        assert_eq!(s.offset, max_off);
    }

    #[test]
    fn click_focusing_topmost_window() {
        use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut wm = WindowManager::<usize, usize>::new_managed(0);

        // Two overlapping regions: window 1 underneath, window 2 on top
        let r1 = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let r2 = Rect {
            x: 5,
            y: 5,
            width: 10,
            height: 10,
        };
        wm.regions.set(WindowId::app(1usize), r1);
        wm.regions.set(WindowId::app(2usize), r2);
        wm.z_order.push(WindowId::app(1usize));
        wm.z_order.push(WindowId::app(2usize));
        wm.managed_draw_order = wm.z_order.clone();

        // initial wm focus defaults to a system window (DebugLog)
        assert!(matches!(wm.wm_focus.current(), WindowId::System(_)));

        // Click inside the overlapping area that belongs to window 2 (topmost)
        let clicked_col = 6u16;
        let clicked_row = 6u16;
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: clicked_col,
            row: clicked_row,
            modifiers: KeyModifiers::NONE,
        };
        let evt = Event::Mouse(mouse);
        // Call the public handler path as in runtime
        let _handled = wm.handle_managed_event(&evt);
        assert_eq!(wm.wm_focus.current(), WindowId::app(2usize));
    }

    #[test]
    fn enforce_min_visible_margin_horizontal() {
        use crate::window::{FloatRect, FloatRectSpec};
        let mut wm = WindowManager::<usize, usize>::new_managed(0);
        wm.set_floating_resize_offscreen(true);
        // place a floating window such that only 2 columns are visible but margin is 4
        wm.set_floating_rect(
            WindowId::app(1usize),
            Some(FloatRectSpec::Absolute(FloatRect {
                x: -4,
                y: 0,
                width: 6,
                height: 3,
            })),
        );
        wm.register_managed_layout(ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        });
        let got = wm
            .floating_rect(WindowId::app(1))
            .expect("floating rect present");
        match got {
            FloatRectSpec::Absolute(fr) => {
                let bounds = wm.managed_area;
                let left_allowed = bounds.x as i32
                    - (6i32 - crate::constants::MIN_FLOATING_VISIBLE_MARGIN.min(6) as i32);
                assert_eq!(fr.x, left_allowed);
            }
            _ => panic!("expected absolute rect"),
        }
    }

    #[test]
    fn enforce_min_visible_margin_vertical() {
        use crate::window::{FloatRect, FloatRectSpec};
        let mut wm = WindowManager::<usize, usize>::new_managed(0);
        wm.set_floating_resize_offscreen(true);
        // place a floating window such that only 1 row is visible but margin is 4
        wm.set_floating_rect(
            WindowId::app(2usize),
            Some(FloatRectSpec::Absolute(FloatRect {
                x: 0,
                y: -3,
                width: 6,
                height: 4,
            })),
        );
        wm.register_managed_layout(ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        });
        let got = wm
            .floating_rect(WindowId::app(2))
            .expect("floating rect present");
        match got {
            FloatRectSpec::Absolute(fr) => {
                // top_allowed = 0 - (4 - MIN_MARGIN) => 0 - (4-4) = 0
                // but since original y=-3, it should clamp up to 0
                assert!(fr.y >= 0);
            }
            _ => panic!("expected absolute rect"),
        }
    }

    #[test]
    fn maximize_persists_across_resize() {
        use crate::window::FloatRectSpec;
        let mut wm = WindowManager::<usize, usize>::new_managed(0);
        // initial managed area
        wm.register_managed_layout(ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 15,
        });
        // maximize window 3
        wm.toggle_maximize(WindowId::app(3usize));
        // change managed area (simulate resize)
        wm.register_managed_layout(ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 20,
        });
        let got = wm
            .floating_rect(WindowId::app(3))
            .expect("floating rect present");
        match got {
            FloatRectSpec::Absolute(fr) => {
                // should match the current managed_area after resize
                assert_eq!(fr.width, wm.managed_area.width);
                assert_eq!(fr.height, wm.managed_area.height);
            }
            _ => panic!("expected absolute rect"),
        }
    }

    #[test]
    fn localize_event_converts_to_local_coords() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let mut wm = WindowManager::<usize, usize>::new_managed(0);
        let target_rect = ratatui::layout::Rect {
            x: 10,
            y: 5,
            width: 20,
            height: 8,
        };
        wm.set_region(1, target_rect);
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 15,
            row: 9,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        let event = Event::Mouse(mouse);
        // Window-local coordinates include chrome offsets.
        let window_local = wm
            .localize_event(WindowId::app(1), &event)
            .expect("window-local event");
        if let Event::Mouse(local) = window_local {
            assert_eq!(local.column, 5); // 15 - target_rect.x
            assert_eq!(local.row, 4); // 9 - target_rect.y
        } else {
            panic!("expected mouse event");
        }

        // Content-local coordinates subtract decorator padding.
        let content_local = wm
            .localize_event_to_app(1, &event)
            .expect("content-local event");
        if let Event::Mouse(local) = content_local {
            assert_eq!(local.column, 4);
            assert_eq!(local.row, 2);
        } else {
            panic!("expected mouse event");
        }
    }

    #[test]
    fn localize_event_handles_negative_origin() {
        use crate::window::{FloatRect, FloatRectSpec};
        use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut wm = WindowManager::<usize, usize>::new_managed(0);
        wm.set_floating_resize_offscreen(true);
        wm.set_floating_rect(
            WindowId::app(1usize),
            Some(FloatRectSpec::Absolute(FloatRect {
                x: -5,
                y: 1,
                width: 10,
                height: 5,
            })),
        );
        wm.register_managed_layout(ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 20,
        });
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 3,
            modifiers: KeyModifiers::NONE,
        };
        let event = Event::Mouse(mouse);

        let window_local = wm
            .localize_event(WindowId::app(1), &event)
            .expect("window-local event");
        if let Event::Mouse(local) = window_local {
            assert_eq!(local.column, 5);
            assert_eq!(local.row, 2);
        } else {
            panic!("expected mouse event");
        }

        let content_local = wm
            .localize_event_to_app(1, &event)
            .expect("content-local event");
        if let Event::Mouse(local) = content_local {
            assert_eq!(local.column, 4);
            assert_eq!(local.row, 0);
        } else {
            panic!("expected mouse event");
        }
    }

    #[test]
    fn hit_test_uses_visible_bounds_for_floating_windows() {
        use crate::window::{FloatRect, FloatRectSpec};
        let mut wm = WindowManager::<usize, usize>::new_managed(0);
        wm.set_floating_resize_offscreen(true);
        wm.set_floating_rect(
            WindowId::app(1usize),
            Some(FloatRectSpec::Absolute(FloatRect {
                x: -5,
                y: 0,
                width: 10,
                height: 5,
            })),
        );
        wm.register_managed_layout(ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 10,
        });
        // Background window occupies most of the visible area.
        wm.regions.set(
            WindowId::app(2usize),
            ratatui::layout::Rect {
                x: 0,
                y: 0,
                width: 30,
                height: 10,
            },
        );
        wm.managed_draw_order = vec![WindowId::app(2usize), WindowId::app(1usize)];

        // Click to the right of the clipped floating window. Without clipping, window 1 would eat
        // the event; with visible bounds it should fall through to the background window.
        let hit = wm.hit_test_region_topmost(8, 2, &wm.managed_draw_order);
        assert_eq!(hit, Some(WindowId::app(2usize)));
    }

    #[test]
    fn hover_targets_respects_occlusion() {
        use crate::layout::floating::{ResizeEdge, ResizeHandle};
        use crate::layout::tiling::SplitHandle;
        let mut wm = WindowManager::<usize, usize>::new_managed(0);
        wm.regions.set(
            WindowId::app(1usize),
            Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 5,
            },
        );
        wm.regions.set(
            WindowId::app(2usize),
            Rect {
                x: 0,
                y: 0,
                width: 5,
                height: 5,
            },
        );
        wm.managed_draw_order = vec![WindowId::app(1usize), WindowId::app(2usize)];
        // Two resize handles sharing the same coordinates but belonging to different windows
        let overlapping = Rect {
            x: 2,
            y: 1,
            width: 1,
            height: 1,
        };
        wm.resize_handles.push(ResizeHandle {
            id: WindowId::app(1usize),
            rect: overlapping,
            edge: ResizeEdge::Left,
        });
        wm.resize_handles.push(ResizeHandle {
            id: WindowId::app(2usize),
            rect: overlapping,
            edge: ResizeEdge::Left,
        });
        // Background-only handle to ensure uncovered areas still hover
        wm.resize_handles.push(ResizeHandle {
            id: WindowId::app(1usize),
            rect: Rect {
                x: 8,
                y: 1,
                width: 1,
                height: 1,
            },
            edge: ResizeEdge::Right,
        });
        // Split handle positioned outside any window to verify handle hover only triggers there
        wm.handles.push(SplitHandle {
            rect: Rect {
                x: 15,
                y: 1,
                width: 1,
                height: 1,
            },
            path: Vec::new(),
            index: 0,
            direction: Direction::Horizontal,
        });

        wm.hover = Some((2, 1));
        let (handle_hover, resize_hover) = wm.hover_targets();
        assert!(
            handle_hover.is_none(),
            "floating window should mask layout handles"
        );
        assert_eq!(
            resize_hover.map(|handle| handle.id),
            Some(WindowId::app(2usize)),
            "topmost window should own the hover"
        );

        wm.hover = Some((8, 1));
        let (_, resize_hover) = wm.hover_targets();
        assert_eq!(
            resize_hover.map(|handle| handle.id),
            Some(WindowId::app(1usize)),
            "background window should hover once it is exposed"
        );

        wm.hover = Some((15, 1));
        let (handle_hover, resize_hover) = wm.hover_targets();
        assert!(resize_hover.is_none());
        assert!(
            handle_hover.is_some(),
            "layout handles should respond off-window"
        );
    }

    #[test]
    fn system_window_header_drag_detaches_to_floating() {
        use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        let mut wm = WindowManager::<usize, usize>::new_managed(0);
        wm.set_panel_visible(false);
        wm.show_system_window(SystemWindowId::DebugLog);
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });

        let debug_id = WindowId::system(SystemWindowId::DebugLog);
        let header_rect = wm
            .floating_headers
            .iter()
            .find(|handle| handle.id == debug_id)
            .expect("debug header present")
            .rect;
        assert!(!wm.is_window_floating(debug_id));

        let down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: header_rect.x,
            row: header_rect.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(wm.handle_managed_event(&down));
        assert!(wm.is_window_floating(debug_id));
        let start_rect = match wm.floating_rect(debug_id).expect("floating rect present") {
            crate::window::FloatRectSpec::Absolute(fr) => fr,
            _ => panic!("expected absolute rect"),
        };

        let drag_col = header_rect.x.saturating_add(2);
        let drag_row = header_rect.y.saturating_add(1);
        let drag = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: drag_col,
            row: drag_row,
            modifiers: KeyModifiers::NONE,
        });
        assert!(wm.handle_managed_event(&drag));

        let moved = match wm.floating_rect(debug_id).expect("floating rect present") {
            crate::window::FloatRectSpec::Absolute(fr) => fr,
            _ => panic!("expected absolute rect"),
        };
        assert_eq!(moved.x, start_rect.x + 2);
        assert_eq!(moved.y, start_rect.y + 1);

        let up = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: drag_col,
            row: drag_row,
            modifiers: KeyModifiers::NONE,
        });
        assert!(wm.handle_managed_event(&up));
        assert!(wm.drag_header.is_none());
    }

    #[test]
    fn adjust_event_rebases_app_mouse_coordinates() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut wm = WindowManager::<usize, usize>::new_managed(0);
        let full = Rect {
            x: 10,
            y: 3,
            width: 12,
            height: 8,
        };
        wm.regions.set(WindowId::app(1usize), full);

        let global = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 16,
            row: 9,
            modifiers: KeyModifiers::NONE,
        };
        let content = wm.region_for_id(WindowId::app(1));
        let localized = Event::Mouse(MouseEvent {
            column: global.column.saturating_sub(content.x),
            row: global.row.saturating_sub(content.y),
            kind: global.kind,
            modifiers: global.modifiers,
        });

        let rebased = wm.adjust_event_for_window(WindowId::app(1), &localized);
        let Event::Mouse(result) = rebased else {
            panic!("expected mouse event");
        };
        assert_eq!(result.column, global.column - full.x);
        assert_eq!(result.row, global.row - full.y);
    }

    #[test]
    fn adjust_event_rebases_system_mouse_coordinates() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut wm = WindowManager::<usize, usize>::new_managed(0);
        let full = Rect {
            x: 2,
            y: 4,
            width: 15,
            height: 6,
        };
        wm.regions
            .set(WindowId::system(SystemWindowId::DebugLog), full);

        let global = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 7,
            row: 8,
            modifiers: KeyModifiers::NONE,
        };
        let content = wm.region_for_id(WindowId::system(SystemWindowId::DebugLog));
        let localized = Event::Mouse(MouseEvent {
            column: global.column.saturating_sub(content.x),
            row: global.row.saturating_sub(content.y),
            kind: global.kind,
            modifiers: global.modifiers,
        });

        let rebased =
            wm.adjust_event_for_window(WindowId::system(SystemWindowId::DebugLog), &localized);
        let Event::Mouse(result) = rebased else {
            panic!("expected mouse event");
        };
        assert_eq!(result.column, global.column - full.x);
        assert_eq!(result.row, global.row - full.y);
    }
}
