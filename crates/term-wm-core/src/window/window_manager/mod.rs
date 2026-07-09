mod chrome;
mod command_menu;
mod drag;
mod focus;
mod layout;
mod overlays;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::Rect;
use crate::events::{
    Event, KeyEvent, KeyKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use slotmap::SlotMap;

use super::WindowKey;
use super::decorator::WindowDecorator;
use super::entry::{Window, WindowState};
use crate::actions::{SystemTask, TermWmAction};
use crate::app_context::AppContext;
use crate::components::{
    Component, ComponentAction, ComponentContext, MenuItem, Overlay, WmComponent,
};
use crate::hitbox_registry::{HitTarget, HitboxRegistry};
use crate::keybindings::KeyBindings;
use crate::layout::floating::*;
use crate::layout::{InsertPosition, LayoutNode, RegionMap, SplitHandle, TilingLayout};
use crate::power_profile::PowerProfile;
use crate::reaper::Reaper;
use crate::task_scheduler::{TaskHandle, TaskId};
use crate::wm_config::{HintVisibility, WmConfig};
use term_wm_layout_engine::FocusRing;
use term_wm_layout_engine::{LayoutRect, apply_resize_drag_signed};

/// State machine for in-progress mouse operations (drag, resize).
///
/// Locked on `Press` via registry hit-test; all subsequent `Drag`/`Release`
/// events route through this state, bypassing the registry entirely.
/// Cleared on `Release`, lost focus, or timeout.
#[derive(Debug, Clone)]
pub(crate) enum MouseCaptureState {
    DraggingWindow {
        key: WindowKey,
        /// Persistent edge-resistance state, stored here so that temporal
        /// threshold and hysteresis work across frames (not stack-allocated).
        resistance: term_wm_layout_engine::EdgeResistance,
        /// Column at press time, used for deadzone-based detach guard.
        anchor_x: u16,
        /// Row at press time, used for deadzone-based detach guard.
        anchor_y: u16,
        initial_x: i32,
        initial_y: i32,
        start_x: u16,
        start_y: u16,
        /// Previous mouse column for velocity calculation.
        prev_col: u16,
        /// Previous mouse row for velocity calculation.
        prev_row: u16,
        /// Raw nanosecond timestamp of the previous drag event.
        prev_time_ns: u64,
        /// Cursor position at the moment the drag decoupled from tiling,
        /// used to suppress snap previews for a short distance after decouple.
        detach_coordinate: Option<(u16, u16)>,
        /// Whether `apply_snap` has already committed the window to the
        /// tiling tree.  Guards against double-detach when `Release`
        /// fires after a `Moved`-triggered snap.
        snap_applied: bool,
    },
    ResizingWindow {
        key: WindowKey,
        edge: ResizeEdge,
        #[allow(dead_code)]
        start_rect: Rect,
        start_col: u16,
        start_row: u16,
        start_x: i32,
        start_y: i32,
        start_width: u16,
        start_height: u16,
    },
    /// A Press hit a Window or Component target — subsequent Drag/Release/Moved
    /// events are forwarded to that component until the Up releases.
    ComponentInteraction { key: WindowKey },
    /// A Press hit a tiling layout split handle — Drag/Release events route to
    /// `TilingLayout::handle_event()` for split-ratio adjustment.
    LayoutHandle,
}

/// Preview state for ghost window rendering during drag operations.
/// Evaluated in spatial priority order (smallest region first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnapPreviewState {
    /// Corner quarter-screen snap (TopLeft/TopRight/BottomLeft/BottomRight).
    Corner(InsertPosition),
    /// Sacred top edge — full-screen maximize on release (preview only during drag).
    Maximize,
    /// Edge snap to left/right/bottom half-screen.
    Edge(InsertPosition),
    /// Tiled insert next to an existing window (quadrant-based).
    #[allow(dead_code)]
    TiledInsert(WindowKey, InsertPosition),
    /// Drop into an empty void placeholder (stores void ID).
    #[allow(dead_code)]
    VoidInsert(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OverlayId {
    Help,
    Keybindings,
    ExitConfirm,
    SelectionPreview,
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

/// Result of a double-Esc press check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuperPressResult {
    /// Second press of WmToggleOverlay within passthrough window → open overlay.
    DoubleSuper,
    /// First press of WmToggleOverlay — deferred until timeout or second press.
    Pending,
    /// Not a WmToggleOverlay key — forward immediately.
    Forward,
}

#[derive(Debug, Clone, Copy)]
pub struct WindowSurface {
    pub full: Rect,
    pub inner: Rect,
    pub dest: crate::window::FloatRect,
    /// Whether a drop-shadow should be rendered behind this window
    /// (derived from `WmConfig.shadow_enabled` + floating status).
    pub draw_shadow: bool,
    /// Normalized z-order depth [0.0–1.0] used to interpolate the shadow
    /// background color — bottommost windows get the lighter `shadow_tint`
    /// while topmost windows get the darker `shadow_bg`.
    pub z_depth: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct WindowDrawContext {
    pub key: WindowKey,
    pub surface: WindowSurface,
    pub focused: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum DrawTask {
    App(WindowDrawContext),
}

pub struct WindowManager {
    focus: FocusRing<WindowKey>,
    windows: SlotMap<WindowKey, Window>,
    pub(crate) regions: RegionMap<WindowKey>,
    scroll: BTreeMap<WindowKey, ScrollState>,
    pub(crate) handles: Vec<SplitHandle>,
    pub(crate) resize_handles: Vec<ResizeHandle<WindowKey>>,
    pub(crate) floating_headers: Vec<DragHandle<WindowKey>>,
    pub(crate) managed_draw_order: Vec<WindowKey>,
    pub(crate) managed_layout: Option<TilingLayout<WindowKey>>,
    closed_windows: Vec<WindowKey>,
    pub(crate) managed_area: Rect,
    pub(crate) hitbox_registry: HitboxRegistry,
    app_ctx: Arc<AppContext>,
    top_component: Option<Box<dyn WmComponent>>,
    bottom_component: Option<Box<dyn WmComponent>>,
    command_menu_component: Option<Box<dyn WmComponent>>,
    #[allow(dead_code)]
    supported_menu_actions: Vec<TermWmAction>,
    top_claimed: Rect,
    bottom_claimed: Rect,
    // Replaces drag_header + drag_resize
    pub(crate) mouse_capture: Option<MouseCaptureState>,
    pub(crate) last_header_click: Option<(WindowKey, Instant)>,
    pub(crate) hover: Option<(u16, u16)>,
    capture_deadline: Option<Instant>,
    pending_deadline: Option<Instant>,
    mouse_capture_enabled: bool,
    mouse_capture_dirty: bool,
    window_selection_enabled: bool,
    window_selection_dirty: bool,
    clipboard_enabled: bool,
    clipboard_dirty: bool,
    command_menu_visible: bool,
    selection_active: bool,
    selection_dragging: bool,
    selection_text: Option<String>,
    selection_copied: bool,
    selection_copied_text: Option<String>,
    config: WmConfig,
    hint_visibility: HintVisibility,
    command_menu_opened_at: Option<Instant>,
    /// The pending super-key event (no timer — managed by the TaskScheduler).
    super_pending_event: Option<KeyEvent>,
    /// Timestamp when the super-key timer was armed, used for panel countdown
    /// display only.  The actual expiry is handled by the TaskScheduler.
    super_pending_at: Option<Instant>,
    /// ID of the super-passthrough timer in the TaskScheduler, for cancellation.
    super_timer_id: Option<TaskId>,
    /// ID of the drag-snap timer in the TaskScheduler, for cancellation.
    drag_timer_id: Option<TaskId>,
    /// ID of the temporal-dwell tick timer, for cancellation and guard.
    temporal_timer_id: Option<TaskId>,
    /// Handle to the shared `TaskScheduler<SystemTask>` for registering/cancelling
    /// system-level timers (super-passthrough, drag-snap).
    system_task_handle: Option<TaskHandle<SystemTask>>,
    pub(crate) last_frame_area: LayoutRect,
    overlays: BTreeMap<OverlayId, Box<dyn Overlay<TermWmAction>>>,
    scroll_keyboard_enabled_default: bool,
    floating_resize_offscreen: bool,
    pub(crate) z_order: Vec<WindowKey>,
    pub(crate) drag_snap: Option<(Option<WindowKey>, InsertPosition, Rect)>,
    /// Active snap preview state for ghost window rendering during drag.
    pub(crate) snap_preview: Option<SnapPreviewState>,
    /// Cache for BSP dry-run projection to avoid deep-cloning the layout
    /// tree on every drag frame. Keyed by (target, position, area).
    snap_projection_cache: Option<(SnapPreviewState, Rect, Option<Rect>)>,
    drag_last_event: Option<Instant>,
    // No separate component map — components live on the Window struct
    // in the SlotMap.  See `Window.component`.
    next_window_seq: usize,
    next_title_seq: usize,
    synthetic_event: Option<Event>,
    clipboard: Option<crate::clipboard::Clipboard>,
    power_profile: PowerProfile,
    pub(crate) reaper: Reaper,
    quit_requested: bool,
    /// Flag indicating the layout has changed and needs re-projection
    layout_dirty: bool,
}

impl WindowManager {
    /// Allocate a new window entry in the SlotMap and return its key.
    /// The window starts with default state (no title, not floating, etc.).
    pub fn create_window(
        &mut self,
        component: Box<dyn crate::components::Component<TermWmAction>>,
    ) -> WindowKey {
        let order = self.next_window_seq;
        self.next_window_seq = self.next_window_seq.saturating_add(1);
        tracing::debug!(seq = order, "opened window");
        self.windows.insert(Window::new(order, component))
    }

    /// Register a component and invoke its `on_mount` hook with the assigned key.
    /// Prefer this over `create_window` for components that need their WindowKey.
    pub fn spawn<C>(&mut self, component: C) -> WindowKey
    where
        C: Component<TermWmAction> + 'static,
    {
        let app_ctx = self.app_ctx().clone();
        let key = self.create_window(Box::new(component));
        if let Some(comp) = self.component_for_key_mut(key) {
            comp.on_mount(key, &app_ctx);
        }
        key
    }

    /// Register a pre-boxed component and invoke its `on_mount` hook.
    pub fn spawn_boxed(&mut self, component: Box<dyn Component<TermWmAction>>) -> WindowKey {
        let app_ctx = self.app_ctx().clone();
        let key = self.create_window(component);
        if let Some(comp) = self.component_for_key_mut(key) {
            comp.on_mount(key, &app_ctx);
        }
        key
    }

    /// Returns true if the key references a live window in the SlotMap.
    /// O(1) — no component extraction or vtable resolution.
    pub fn has_window(&self, key: WindowKey) -> bool {
        self.windows.contains_key(key)
    }

    /// Access the Reaper for async child-process teardown.
    pub fn reaper(&self) -> &Reaper {
        &self.reaper
    }

    fn window_mut(&mut self, key: WindowKey) -> &mut Window {
        self.windows
            .get_mut(key)
            .unwrap_or_else(|| panic!("window_mut called for unknown key {:?}", key))
    }

    fn window(&self, key: WindowKey) -> Option<&Window> {
        self.windows.get(key)
    }

    pub fn window_state(&self, key: WindowKey) -> Option<WindowState> {
        self.window(key).map(|w| w.state)
    }

    /// Validate that a state transition is legal.
    fn is_valid_transition(old: WindowState, new: WindowState) -> bool {
        matches!(
            (old, new),
            (WindowState::Realized, WindowState::Mapped)
                | (WindowState::Realized, WindowState::Unmapped)
                | (WindowState::Mapped, WindowState::Iconic)
                | (WindowState::Iconic, WindowState::Mapped)
                | (WindowState::Mapped, WindowState::Unmapped)
                | (WindowState::Unmapped, WindowState::Mapped)
                | (WindowState::Mapped, WindowState::Shaded)
                | (WindowState::Shaded, WindowState::Mapped)
        )
    }

    /// Transition a window to a new state, applying all side-effects atomically.
    /// Borrows `self.windows` briefly then drops before layout/focus mutations.
    pub fn transition_window(&mut self, key: WindowKey, new_state: WindowState) {
        // Step 1: Read old state (immutable borrow, immediately dropped)
        let old_state = match self.window(key) {
            Some(w) => w.state,
            None => {
                tracing::warn!("transition_window: unknown key {:?}", key);
                return;
            }
        };
        if old_state == new_state {
            return;
        }
        debug_assert!(
            Self::is_valid_transition(old_state, new_state),
            "Illegal window state transition: {:?} -> {:?}",
            old_state,
            new_state
        );

        // Step 2: Mutate state (brief mutable borrow, immediately dropped)
        self.window_mut(key).state = new_state;

        // Step 3: Side-effects — full &mut self available
        match (old_state, new_state) {
            (WindowState::Realized, WindowState::Mapped) => {
                self.z_order.push(key);
                self.managed_draw_order.push(key);
                self.focus_add(key);
            }
            (WindowState::Mapped, WindowState::Iconic) => {
                self.z_order.retain(|x| *x != key);
                self.managed_draw_order.retain(|x| *x != key);
                if *self.focus.current() == key {
                    self.select_fallback_focus();
                }
                self.detach_from_tiling_layout(key);
            }
            (WindowState::Iconic, WindowState::Mapped) => {
                if !self.z_order.contains(&key) {
                    self.z_order.push(key);
                }
                if !self.managed_draw_order.contains(&key) {
                    self.managed_draw_order.push(key);
                }
                self.reattach_to_tiling_layout(key);
                self.focus_add(key);
            }
            (_, WindowState::Unmapped) => {
                self.clear_floating_rect(key);
                self.z_order.retain(|x| *x != key);
                self.managed_draw_order.retain(|x| *x != key);
                self.regions.remove(key);
                self.scroll.remove(&key);
                self.remove_from_focus_ring(key);
                if *self.focus.current() == key {
                    self.select_fallback_focus();
                }
                self.detach_from_tiling_layout(key);
            }
            (WindowState::Unmapped, WindowState::Mapped) => {
                if !self.z_order.contains(&key) {
                    self.z_order.push(key);
                }
                if !self.managed_draw_order.contains(&key) {
                    self.managed_draw_order.push(key);
                }
                self.reattach_to_tiling_layout(key);
                self.focus_add(key);
            }
            (WindowState::Mapped, WindowState::Shaded) => {
                // Keep in z-order and draw order so chrome renders,
                // but remove content region so only the title bar shows.
                self.regions.remove(key);
            }
            (WindowState::Shaded, WindowState::Mapped) => {
                self.regions.remove(key);
                self.reattach_to_tiling_layout(key);
            }
            _ => {}
        }
    }

    fn floating_rect(&self, key: WindowKey) -> Option<crate::window::FloatRectSpec> {
        self.window(key).and_then(|window| window.floating_rect)
    }

    fn set_floating_rect(&mut self, key: WindowKey, rect: Option<crate::window::FloatRectSpec>) {
        if let Some(w) = self.windows.get_mut(key) {
            w.floating_rect = rect;
        }
    }

    fn clear_floating_rect(&mut self, key: WindowKey) {
        if let Some(w) = self.windows.get_mut(key) {
            w.floating_rect = None;
        }
    }

    fn set_prev_floating_rect(
        &mut self,
        key: WindowKey,
        rect: Option<crate::window::FloatRectSpec>,
    ) {
        if let Some(w) = self.windows.get_mut(key) {
            w.prev_floating_rect = rect;
        }
    }

    fn take_prev_floating_rect(&mut self, key: WindowKey) -> Option<crate::window::FloatRectSpec> {
        self.windows.get_mut(key)?.prev_floating_rect.take()
    }

    pub fn is_window_floating(&self, key: WindowKey) -> bool {
        self.window(key).is_some_and(|window| window.is_floating())
    }

    pub fn direct_mode(&self, key: WindowKey) -> bool {
        self.window(key).is_some_and(|window| window.direct_mode)
    }

    pub fn set_direct_mode(&mut self, key: WindowKey, value: bool) {
        if let Some(w) = self.windows.get_mut(key) {
            w.direct_mode = value;
        }
    }

    pub fn toggle_direct_mode(&mut self, key: WindowKey) {
        let current = self.direct_mode(key);
        self.set_direct_mode(key, !current);
    }

    pub fn window_title(&self, key: WindowKey) -> String {
        let base = self
            .window(key)
            .map(|window| window.title_or_default(key))
            .unwrap_or_else(|| format!("{:?}", key));
        let order = self.build_display_order();
        let same: Vec<&WindowKey> = order
            .iter()
            .filter(|oid| {
                self.window(**oid)
                    .map(|w| w.title_or_default(**oid))
                    .as_deref()
                    == Some(base.as_str())
            })
            .collect();
        if same.len() <= 1 {
            return base;
        }
        let nth = same.iter().position(|&&oid| oid == key).unwrap_or(0) + 1;
        format!("{} ({})", base, nth)
    }

    /// Pre-compute all display titles in one pass from a single snapshot of
    /// `build_display_order()`.  Same-title windows are numbered by the order
    /// the title was assigned (`title_set_order`), so the first window to get
    /// "htop" is "htop (keys\[1\])" regardless of creation order.
    pub fn window_titles(&self) -> Vec<(WindowKey, String)> {
        let order = self.build_display_order();
        #[allow(clippy::type_complexity)]
        let mut groups: std::collections::BTreeMap<
            String,
            Vec<(WindowKey, Option<usize>)>,
        > = std::collections::BTreeMap::new();
        for &oid in &order {
            let base = self
                .window(oid)
                .map(|w| w.title_or_default(oid))
                .unwrap_or_else(|| format!("{:?}", oid));
            let set_order = self.window(oid).and_then(|w| w.title_set_order);
            groups.entry(base).or_default().push((oid, set_order));
        }
        for group in groups.values_mut() {
            group.sort_by_key(|(_, order)| order.unwrap_or(usize::MAX));
        }
        let mut out = Vec::with_capacity(order.len());
        for &oid in &order {
            let base = self
                .window(oid)
                .map(|w| w.title_or_default(oid))
                .unwrap_or_else(|| format!("{:?}", oid));
            let group = &groups[&base];
            let title = if group.len() <= 1 {
                base
            } else {
                let nth = group.iter().position(|&(g, _)| g == oid).unwrap_or(0) + 1;
                format!("{} ({})", base, nth)
            };
            out.push((oid, title));
        }
        out
    }

    fn clear_all_floating(&mut self) {
        for (_key, window) in self.windows.iter_mut() {
            window.floating_rect = None;
            window.prev_floating_rect = None;
        }
    }

    pub(crate) fn with_config(
        config: WmConfig,
        app_ctx: Arc<AppContext>,
        top_component: Option<Box<dyn WmComponent>>,
        bottom_component: Option<Box<dyn WmComponent>>,
        command_menu_component: Option<Box<dyn WmComponent>>,
        supported_menu_actions: Option<Vec<TermWmAction>>,
    ) -> Self {
        let supported_menu_actions = supported_menu_actions.unwrap_or_else(|| {
            vec![
                TermWmAction::CloseMenu,
                TermWmAction::ToggleMouseCapture,
                TermWmAction::ToggleClipboardMode,
                TermWmAction::ToggleWindowSelection,
                TermWmAction::BringFloatingFront,
                TermWmAction::NewWindow,
                TermWmAction::ToggleDebugWindow,
                TermWmAction::Help,
                TermWmAction::ExitUi,
            ]
        });
        let mouse_capture_enabled = config.mouse_capture_enabled;
        let clipboard = Some(crate::clipboard::Clipboard::new());
        let floating_resize_offscreen = config.floating_resize_offscreen;
        Self {
            focus: FocusRing::new(
                /* placeholder, will be set on first window */
                slotmap::DefaultKey::default(),
            ),
            windows: SlotMap::with_capacity(32),
            regions: RegionMap::default(),
            scroll: BTreeMap::new(),
            handles: Vec::new(),
            resize_handles: Vec::new(),
            floating_headers: Vec::new(),
            managed_draw_order: Vec::new(),
            managed_layout: None,
            closed_windows: Vec::new(),
            managed_area: Rect::default(),
            hitbox_registry: HitboxRegistry::new(),
            app_ctx,
            top_component,
            bottom_component,
            command_menu_component,
            supported_menu_actions,
            top_claimed: Rect::default(),
            bottom_claimed: Rect::default(),
            mouse_capture: None,
            last_header_click: None,
            hover: None,
            capture_deadline: None,
            pending_deadline: None,
            mouse_capture_enabled,
            mouse_capture_dirty: false,
            clipboard_enabled: config.clipboard_enabled,
            clipboard_dirty: false,
            command_menu_visible: false,
            selection_active: false,
            selection_dragging: false,
            selection_text: None,
            selection_copied: false,
            selection_copied_text: None,
            window_selection_enabled: config.window_selection_enabled,
            window_selection_dirty: false,
            hint_visibility: config.hint_visibility,
            config,
            command_menu_opened_at: None,
            super_pending_event: None,
            super_pending_at: None,
            super_timer_id: None,
            drag_timer_id: None,
            temporal_timer_id: None,
            system_task_handle: None,
            last_frame_area: Rect::default(),
            overlays: BTreeMap::new(),
            scroll_keyboard_enabled_default: true,
            floating_resize_offscreen,
            z_order: Vec::new(),
            drag_snap: None,
            snap_preview: None,
            snap_projection_cache: None,
            drag_last_event: None,
            next_window_seq: 0,
            next_title_seq: 0,
            synthetic_event: None,
            clipboard,
            power_profile: PowerProfile::PowerSaver,
            reaper: Reaper::default(),
            quit_requested: false,
            layout_dirty: true,
        }
    }

    /// Request a clean shutdown on the next idle tick.
    pub fn request_quit(&mut self) {
        self.quit_requested = true;
    }

    /// Returns `true` if a quit has been requested.
    pub fn quit_requested(&self) -> bool {
        self.quit_requested
    }

    /// Check if the layout has changed and needs re-projection.
    pub fn layout_dirty(&self) -> bool {
        self.layout_dirty
    }

    /// Mark the layout as dirty (needs re-projection).
    pub fn mark_layout_dirty(&mut self) {
        self.layout_dirty = true;
    }

    /// Clear the layout dirty flag (after projection).
    pub fn clear_layout_dirty(&mut self) {
        self.layout_dirty = false;
    }

    /// Remove a key from the focus ring's order (called after closing a window).
    fn remove_from_focus_ring(&mut self, key: WindowKey) {
        let order: Vec<WindowKey> = self
            .focus
            .order()
            .iter()
            .copied()
            .filter(|k| *k != key)
            .collect();
        self.focus.set_order(order);
    }

    /// Add a key to the focus ring if not already present, and set it as current.
    fn focus_add(&mut self, key: WindowKey) {
        if !self.focus.order().contains(&key) {
            let mut order = self.focus.order().to_vec();
            order.push(key);
            self.focus.set_order(order);
        }
        self.focus.set_current(key);
    }

    pub fn take_closed_windows(&mut self) -> Vec<WindowKey> {
        std::mem::take(&mut self.closed_windows)
    }

    pub fn take_synthetic_event(&mut self) -> Option<Event> {
        self.synthetic_event.take()
    }

    pub fn config(&self) -> &WmConfig {
        &self.config
    }

    pub fn keybindings(&self) -> &KeyBindings {
        &self.config.keybindings
    }

    pub fn hint_visibility(&self) -> HintVisibility {
        self.hint_visibility
    }

    pub fn set_hint_visibility(&mut self, visibility: HintVisibility) {
        self.hint_visibility = visibility;
    }

    pub fn set_floating_resize_offscreen(&mut self, enabled: bool) {
        self.floating_resize_offscreen = enabled;
    }

    pub fn floating_resize_offscreen(&self) -> bool {
        self.floating_resize_offscreen
    }

    pub fn app_ctx(&self) -> &Arc<AppContext> {
        &self.app_ctx
    }

    /// Create a [`ComponentContext`] pre-populated with the application
    /// identity from this window manager's [`AppContext`].
    pub fn component_context(&self, focused: bool) -> ComponentContext {
        ComponentContext::new(focused)
            .with_app_context(Arc::clone(&self.app_ctx))
            .with_config(Arc::new(self.config.clone()))
    }

    /// Create a [`ComponentContext`] for a specific window, including the
    /// window's direct-mode state so children (scroll view, terminal) can
    /// adapt their rendering and event handling automatically.
    pub fn component_context_for(&self, focused: bool, key: WindowKey) -> ComponentContext {
        self.component_context(focused)
            .with_direct_mode(self.direct_mode(key))
            .with_window_key(key)
            .with_screen_area(self.region(key))
    }

    /// Number of overlays that will be rendered this frame.
    pub fn visible_overlay_count(&self) -> usize {
        let mut n = 0usize;
        if self.command_menu_visible() {
            n += 1;
        }
        if self.overlays.contains_key(&OverlayId::ExitConfirm) {
            n += 1;
        }
        if self.overlays.contains_key(&OverlayId::Help) {
            n += 1;
        }
        n
    }

    /// Normalised z-depth [0.0–1.0] for a drawable at `position` in a
    /// stack of `total` items (windows + overlays).  The topmost item
    /// always maps to 1.0 (darkest shadow).
    pub fn compute_z_depth(position: usize, total: usize) -> f32 {
        if total <= 1 {
            return 1.0;
        }
        position as f32 / (total - 1) as f32
    }

    pub fn begin_frame(&mut self) {
        if let Some(p) = &mut self.top_component {
            p.begin_frame();
        }
        if let Some(p) = &mut self.bottom_component {
            p.begin_frame();
            p.process_action(&ComponentAction::SetPowerProfile(self.power_profile));
        }
        if !self.config.wm_command_menu_enabled {
            self.clear_capture();
        } else {
            self.refresh_capture();
        }
    }

    /// Clear draw-time state that gets repopulated during `output.draw()`.
    /// Must be called immediately before each draw (not in `begin_frame()`)
    /// so that skipped idle renders don't destroy data needed by mouse events.
    pub fn prepare_draw(&mut self) {
        self.regions = RegionMap::default();
        self.handles.clear();
        self.resize_handles.clear();
        self.floating_headers.clear();
        self.managed_draw_order.clear();
        self.hitbox_registry.clear();
    }

    /// Dispatch a mouse event using the hitbox registry.
    ///
    /// Three-phase architecture:
    ///   Phase 1 — Active capture (ongoing drag/resize) takes priority.
    ///   Phase 2 — On Down: hit-test registry, lock capture state on chrome hits,
    /// Clear the cached hover position (e.g. on focus loss).
    pub fn clear_hover(&mut self) {
        self.hover = None;
    }

    /// Return the current tiling split handles (for rendering).
    pub fn tiling_handles(&self) -> &[crate::layout::tiling::SplitHandle] {
        &self.handles
    }

    /// Return the currently hovered tiling split handle, if any.
    pub fn hovered_tiling_handle(&self) -> Option<crate::layout::tiling::SplitHandle> {
        let (col, row) = self.hover?;
        let pos = crate::mouse_coord::MousePosition {
            column: col as i16,
            row: row as i16,
            space: crate::mouse_coord::CoordSpace::Screen,
        };
        if let Some((crate::hitbox_registry::HitTarget::LayoutHandle, _)) =
            self.hitbox_registry.hit_test(pos)
        {
            self.managed_layout
                .as_ref()?
                .hovered_handle(self.managed_area)
        } else {
            None
        }
    }

    /// Return the currently hovered floating resize handle, if any.
    pub fn hovered_resize_handle(
        &self,
    ) -> Option<&crate::layout::floating::ResizeHandle<WindowKey>> {
        let (column, row) = self.hover?;
        let topmost = self.hit_test_region_topmost(column, row, &self.managed_draw_order);
        self.resize_handles.iter().find(|handle| {
            crate::layout::rect_contains(handle.rect, column, row) && topmost == Some(handle.key)
        })
    }

    /// Return the window region map (for resize outline rendering).
    pub fn regions(&self) -> &crate::layout::RegionMap<WindowKey> {
        &self.regions
    }

    /// Return the active resize drag state (key + edge), if any.
    pub fn active_resize_drag(&self) -> Option<(WindowKey, crate::layout::floating::ResizeEdge)> {
        if let Some(crate::window::window_manager::MouseCaptureState::ResizingWindow {
            edge,
            key,
            ..
        }) = &self.mouse_capture
        {
            Some((*key, *edge))
        } else {
            None
        }
    }

    /// Return the current drag snap preview rect, if any.
    pub fn drag_snap_rect(&self) -> Option<Rect> {
        self.drag_snap.as_ref().map(|(_, _, rect)| *rect)
    }

    /// Return the full drag snap data (key, position, rect), if any.
    pub fn drag_snap_rect_data(
        &self,
    ) -> &Option<(Option<WindowKey>, crate::layout::InsertPosition, Rect)> {
        &self.drag_snap
    }

    /// Return the target window key for dimming during a tiled-insert snap
    /// preview, if the preview is currently active.
    pub fn snap_preview_target_key(&self) -> Option<WindowKey> {
        match self.snap_preview {
            Some(SnapPreviewState::TiledInsert(key, _)) => Some(key),
            _ => None,
        }
    }

    /// Return a human-readable label for the current snap preview action.
    pub fn snap_preview_action_label(&self) -> Option<&'static str> {
        self.snap_preview.as_ref().map(|s| match s {
            SnapPreviewState::Maximize => "maximize",
            SnapPreviewState::Edge(_) => "snap to edge",
            SnapPreviewState::Corner(_) => "snap to corner",
            SnapPreviewState::TiledInsert(_, _) => "tile",
            SnapPreviewState::VoidInsert(_) => "fill void",
        })
    }

    /// Return floating pane info for rendering (key + rect).
    pub fn floating_panes(&self) -> Vec<(WindowKey, crate::window::FloatRectSpec)> {
        self.windows
            .iter()
            .filter_map(|(key, window)| window.floating_rect.map(|rect| (key, rect)))
            .collect()
    }

    /// Return the current mouse hover position, if any.
    pub fn hover_pos(&self) -> Option<(u16, u16)> {
        self.hover
    }

    /// Return a mutable reference to the hitbox registry (for render pipeline).
    pub fn hitbox_registry_mut(&mut self) -> &mut crate::hitbox_registry::HitboxRegistry {
        &mut self.hitbox_registry
    }

    /// Split borrow: return a mutable ref to the hitbox registry and
    /// a mutable ref to the top component simultaneously.
    pub fn top_and_registry(&mut self) -> (&mut Option<Box<dyn WmComponent>>, &mut HitboxRegistry) {
        (&mut self.top_component, &mut self.hitbox_registry)
    }

    /// Split borrow: return a mutable ref to the hitbox registry and
    /// a mutable ref to the bottom component simultaneously.
    pub fn bottom_and_registry(
        &mut self,
    ) -> (&mut Option<Box<dyn WmComponent>>, &mut HitboxRegistry) {
        (&mut self.bottom_component, &mut self.hitbox_registry)
    }

    /// Dispatch a mouse event through the hitbox registry.
    ///
    /// Phases:
    ///   Phase 1 — Active capture: ongoing drag/resize/component-interaction
    ///   Phase 2 — Chrome hit-test (header, close button, etc.)
    ///   Phase 3 — Moved events: update hover and forward to component.
    ///   Phase 4 — Press events hit-test the registry.
    ///             dispatch to components on content hits.
    ///   Phase 3 — Unhandled events fall through to false.
    ///
    /// Returns `true` if the event was consumed.
    #[allow(clippy::collapsible_if)]
    pub fn dispatch_mouse(&mut self, event: &crate::events::WmEvent) -> bool {
        use crate::events::WmEvent;
        use crate::window::decorator::HeaderAction;
        let WmEvent::Mouse {
            kind,
            modifiers,
            position,
        } = event
        else {
            return false;
        };
        let col = position.column as u16;
        let row = position.row as u16;

        // Phase 1 — Active capture: extract-operate-restore pattern.
        if !matches!(kind, MouseEventKind::Press(_)) {
            if let Some(mut capture) = self.mouse_capture.take() {
                let (result, restore) = match &mut capture {
                    MouseCaptureState::DraggingWindow {
                        key,
                        resistance,
                        anchor_x,
                        anchor_y,
                        initial_x,
                        initial_y,
                        start_x,
                        start_y,
                        prev_col,
                        prev_row,
                        prev_time_ns,
                        detach_coordinate,
                        snap_applied,
                    } => match kind {
                        MouseEventKind::Drag(_) => {
                            let dx = col.abs_diff(*anchor_x);
                            let dy = row.abs_diff(*anchor_y);
                            let is_maximized =
                                self.windows.get(*key).is_some_and(|w| w.is_maximized);

                            if dx + dy <= 2 {
                                // Deadzone guard: ignore micro-nudges
                                (true, true)
                            } else if is_maximized && !(row > *anchor_y && row - *anchor_y > 2) {
                                // Maximized and not pulling down — consume event, keep capture
                                (true, true)
                            } else {
                                // Downward-drag restore for maximized windows
                                if is_maximized {
                                    self.toggle_maximize(*key);
                                    if let Some(crate::window::FloatRectSpec::Absolute(fr)) =
                                        self.floating_rect(*key)
                                    {
                                        *initial_x = fr.x;
                                        *initial_y = fr.y;
                                        *start_x = col;
                                        *start_y = row;
                                        *prev_col = col;
                                        *prev_row = row;
                                    }
                                }

                                if detach_coordinate.is_none() {
                                    *detach_coordinate = Some((col, row));
                                }

                                self.drag_last_event = Some(Instant::now());
                                self.reset_drag_snap_timer();
                                if self.is_window_floating(*key) {
                                    let dx = col.abs_diff(*prev_col);
                                    let dy = row.abs_diff(*prev_row);
                                    let now_ns = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_nanos() as u64)
                                        .unwrap_or(*prev_time_ns);
                                    let dt_ns = now_ns.saturating_sub(*prev_time_ns).max(1_000_000);
                                    let dy_weighted = u32::from(dy).saturating_mul(2);
                                    let dist_sq = u32::from(dx)
                                        .saturating_mul(u32::from(dx))
                                        .saturating_add(dy_weighted.saturating_mul(dy_weighted));
                                    let threshold_cells_sq = 10u64.saturating_mul(10);
                                    let threshold_time_sq =
                                        100_000_000u64.saturating_mul(100_000_000);
                                    let velocity_exceeded = u64::from(dist_sq)
                                        .saturating_mul(threshold_time_sq)
                                        > threshold_cells_sq
                                            .saturating_mul(dt_ns.saturating_mul(dt_ns));
                                    self.move_floating(
                                        *key,
                                        col,
                                        row,
                                        *start_x,
                                        *start_y,
                                        *initial_x,
                                        *initial_y,
                                        velocity_exceeded,
                                        resistance,
                                    );
                                    let dx_total = col.abs_diff(*start_x);
                                    let dy_total = row.abs_diff(*start_y);
                                    if dx_total + dy_total > 2 {
                                        self.update_snap_preview(*key, col, row, detach_coordinate);
                                    } else {
                                        self.drag_snap = None;
                                    }
                                    *prev_col = col;
                                    *prev_row = row;
                                    *prev_time_ns = now_ns;
                                }
                                (true, true)
                            }
                        }
                        MouseEventKind::Release(_) => {
                            self.cancel_drag_snap_timer();
                            self.drag_last_event = None;
                            if self.snap_preview == Some(SnapPreviewState::Maximize) {
                                self.toggle_maximize(*key);
                                self.snap_preview = None;
                            } else if self.drag_snap.is_some() {
                                // Snap target found — apply snap (removes from
                                // tiling tree and inserts at snap position)
                                self.apply_snap(*key);
                            } else if !*snap_applied {
                                // No snap target and snap was not already applied
                                // by a Moved event — finalize as floating.  Remove
                                // from tiling tree now, keep floating rect.
                                self.detach_from_tiling_layout(*key);
                            }
                            // else: snap was already applied by Moved handler — the
                            // window is correctly positioned in the tiling tree; do
                            // NOT detach it again.
                            self.snap_preview = None;
                            self.snap_projection_cache = None;
                            (true, false)
                        }
                        MouseEventKind::Moved if self.drag_snap.is_some() => {
                            self.cancel_drag_snap_timer();
                            self.drag_last_event = None;
                            self.apply_snap(*key);
                            *snap_applied = true;
                            self.snap_preview = None;
                            self.snap_projection_cache = None;
                            (true, true)
                        }
                        _ => (false, true),
                    },
                    MouseCaptureState::ResizingWindow {
                        key,
                        edge,
                        start_col,
                        start_row,
                        start_x,
                        start_y,
                        start_width,
                        start_height,
                        ..
                    } => match kind {
                        MouseEventKind::Drag(_) => {
                            if self.is_window_floating(*key) {
                                let bounds = LayoutRect {
                                    x: self.managed_area.x,
                                    y: self.managed_area.y,
                                    width: self.managed_area.width,
                                    height: self.managed_area.height,
                                };
                                let resized = apply_resize_drag_signed(
                                    *start_x,
                                    *start_y,
                                    *start_width,
                                    *start_height,
                                    *edge,
                                    col,
                                    row,
                                    *start_col,
                                    *start_row,
                                    bounds,
                                    self.floating_resize_offscreen,
                                );
                                self.set_floating_rect(
                                    *key,
                                    Some(crate::window::FloatRectSpec::Absolute(resized)),
                                );
                            }
                            (true, true)
                        }
                        MouseEventKind::Release(_) => (true, false),
                        _ => (false, true),
                    },
                    MouseCaptureState::ComponentInteraction { key } => {
                        let focused = *self.focus.current() == *key;
                        let mut ctx = self.component_context_for(focused, *key);
                        if let Some(area) = self.hitbox_registry.component_area(*key) {
                            ctx = ctx.with_screen_area(area);
                        }
                        let core_event = Event::Mouse(MouseEvent {
                            kind: *kind,
                            column: col,
                            row,
                            modifiers: *modifiers,
                        });
                        let consumed = if let Some(comp) = self.component_for_key_mut(*key) {
                            let result = comp.handle_events(&core_event, &ctx);
                            let was_consumed = !result.is_ignored();
                            if let Some(action) = result.into_action() {
                                self.process_action(*key, action);
                            }
                            was_consumed
                        } else {
                            false
                        };
                        (consumed, !matches!(kind, MouseEventKind::Release(_)))
                    }
                    MouseCaptureState::LayoutHandle => match kind {
                        MouseEventKind::Drag(_) | MouseEventKind::Release(_) => {
                            let event = Event::Mouse(MouseEvent {
                                kind: *kind,
                                column: col,
                                row,
                                modifiers: *modifiers,
                            });
                            let handled = self
                                .managed_layout
                                .as_mut()
                                .map(|l| l.handle_event(&event, self.managed_area))
                                .unwrap_or(false);
                            (handled, !matches!(kind, MouseEventKind::Release(_)))
                        }
                        _ => (false, true),
                    },
                };
                if restore {
                    self.mouse_capture = Some(capture);
                }
                return result;
            }
        }

        // Phase 2 — Scroll events: hover-to-scroll to the window under cursor.
        if matches!(kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
            // Use registry hit-test for scroll dispatch.
            if let Some((target, hit_rect)) = self.hitbox_registry.hit_test(*position)
                && let HitTarget::Window(key) | HitTarget::Component(key, ..) = target
            {
                let focused = *self.focus.current() == key;
                let ctx = self
                    .component_context_for(focused, key)
                    .with_screen_area(hit_rect);
                let core_event = Event::Mouse(MouseEvent {
                    kind: *kind,
                    column: col,
                    row,
                    modifiers: *modifiers,
                });
                if let Some(comp) = self.component_for_key_mut(key) {
                    let result = comp.handle_events(&core_event, &ctx);
                    let was_consumed = !result.is_ignored();
                    if let Some(action) = result.into_action() {
                        self.process_action(key, action);
                    }
                    return was_consumed;
                }
            }
            return false;
        }

        // Phase 3 — Moved events: update hover and forward to component.
        if matches!(kind, MouseEventKind::Moved) {
            self.hover = Some((col, row));
            if let Some((target, hit_rect)) = self.hitbox_registry.hit_test(*position)
                && let HitTarget::Window(key) | HitTarget::Component(key, ..) = target
            {
                let focused = *self.focus.current() == key;
                let ctx = self
                    .component_context_for(focused, key)
                    .with_screen_area(hit_rect);
                let core_event = Event::Mouse(MouseEvent {
                    kind: *kind,
                    column: col,
                    row,
                    modifiers: *modifiers,
                });
                if let Some(comp) = self.component_for_key_mut(key) {
                    let result = comp.handle_events(&core_event, &ctx);
                    let was_consumed = !result.is_ignored();
                    if let Some(action) = result.into_action() {
                        self.process_action(key, action);
                    }
                    return was_consumed;
                }
            }
            // Forward Moved to tiling layout for hover feedback on split handles.
            // Always forward so the hover state stays current regardless of
            // which hit target was found (window, overlay, etc.).
            {
                let event = Event::Mouse(MouseEvent {
                    kind: *kind,
                    column: col,
                    row,
                    modifiers: *modifiers,
                });
                if let Some(layout) = self.managed_layout.as_mut() {
                    layout.handle_event(&event, self.managed_area);
                }
            }
            return false;
        }

        // Phase 4 — Press events hit-test the registry.
        if !matches!(kind, MouseEventKind::Press(_)) {
            return false;
        }

        // Record hover position for decorator rendering
        self.hover = Some((col, row));

        let Some((target, hit_rect)) = self.hitbox_registry.hit_test(*position) else {
            // No hitbox — try focus-on-click, then fall through
            if self.config.wm_command_menu_enabled {
                self.focus_window_at(col, row);
            }
            return false;
        };

        // Build core Event for Component::handle_events.
        let core_event = Event::Mouse(MouseEvent {
            kind: *kind,
            column: col,
            row,
            modifiers: *modifiers,
        });

        match target {
            HitTarget::Window(key) | HitTarget::Component(key, ..) => {
                let focused = *self.focus.current() == key;
                let ctx = self
                    .component_context_for(focused, key)
                    .with_screen_area(hit_rect);
                let consumed = if let Some(comp) = self.component_for_key_mut(key) {
                    let result = comp.handle_events(&core_event, &ctx);
                    let was_consumed = !result.is_ignored();
                    if let Some(action) = result.into_action() {
                        self.process_action(key, action);
                    }
                    was_consumed
                } else {
                    false
                };
                // Lock capture so subsequent Drag/Up/Moved go to this component.
                self.mouse_capture = Some(MouseCaptureState::ComponentInteraction { key });
                consumed
            }
            HitTarget::ChromeHeader(key, _) => {
                let rect = self.visible_region_for_key(key);
                match self.decorator().hit_test(rect, col, row) {
                    HeaderAction::Close => {
                        self.close_window(key);
                        self.last_header_click = None;
                        true
                    }
                    HeaderAction::Maximize => {
                        self.toggle_maximize(key);
                        self.last_header_click = None;
                        true
                    }
                    HeaderAction::Minimize => {
                        self.minimize_window(key);
                        self.last_header_click = None;
                        true
                    }
                    HeaderAction::ToggleDirectMode => {
                        self.toggle_direct_mode(key);
                        self.last_header_click = None;
                        true
                    }
                    HeaderAction::Drag => {
                        let now = Instant::now();
                        if let Some((prev_key, prev)) = self.last_header_click
                            && prev_key == key
                            && now.duration_since(prev) <= Duration::from_millis(500)
                        {
                            self.toggle_maximize(key);
                            self.last_header_click = None;
                            return true;
                        }
                        self.last_header_click = Some((key, now));

                        if self.is_window_floating(key) {
                            self.bring_floating_to_front_key(key);
                        } else {
                            // Make window visually floating WITHOUT removing
                            // from tiling tree.  The tiling layout stays intact
                            // until release.
                            let width = rect.width.max(1);
                            let height = rect.height.max(1);
                            self.set_floating_rect(
                                key,
                                Some(crate::window::FloatRectSpec::Absolute(
                                    crate::window::FloatRect {
                                        x: rect.x,
                                        y: rect.y,
                                        width,
                                        height,
                                    },
                                )),
                            );
                            self.bring_to_front_key(key);
                        }

                        let (initial_x, initial_y) =
                            if let Some(crate::window::FloatRectSpec::Absolute(fr)) =
                                self.floating_rect(key)
                            {
                                (fr.x, fr.y)
                            } else {
                                (rect.x, rect.y)
                            };
                        self.mouse_capture = Some(MouseCaptureState::DraggingWindow {
                            key,
                            resistance: term_wm_layout_engine::EdgeResistance::default_tui(),
                            anchor_x: col,
                            anchor_y: row,
                            initial_x,
                            initial_y,
                            start_x: col,
                            start_y: row,
                            prev_col: col,
                            prev_row: row,
                            prev_time_ns: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_nanos() as u64)
                                .unwrap_or(0),
                            detach_coordinate: None,
                            snap_applied: false,
                        });
                        self.drag_last_event = Some(Instant::now());
                        self.arm_drag_snap_timer();
                        true
                    }
                    HeaderAction::None => false,
                }
            }
            HitTarget::ChromeResize(key, edge) => {
                if !self.config.floating_windows_enabled || !self.is_window_floating(key) {
                    return false;
                }
                self.bring_floating_to_front_key(key);
                let rect = self.full_region_for_key(key);
                let (start_x, start_y, start_width, start_height) =
                    if let Some(crate::window::FloatRectSpec::Absolute(fr)) =
                        self.floating_rect(key)
                    {
                        (fr.x, fr.y, fr.width, fr.height)
                    } else {
                        (rect.x, rect.y, rect.width, rect.height)
                    };
                self.mouse_capture = Some(MouseCaptureState::ResizingWindow {
                    key,
                    edge,
                    start_rect: rect,
                    start_col: col,
                    start_row: row,
                    start_x,
                    start_y,
                    start_width,
                    start_height,
                });
                true
            }
            HitTarget::TopPanel => {
                if self.config.wm_command_menu_enabled && self.panel_active() {
                    let panel_handled = self.handle_panel_click(col, row);
                    if panel_handled {
                        return true;
                    }
                }
                if self.config.wm_command_menu_enabled {
                    self.focus_window_at(col, row);
                }
                false
            }
            HitTarget::BottomPanel => {
                let ctx = self.component_context(false);
                if let Some(p) = &mut self.bottom_component
                    && let crate::actions::EventResult::Action(action) =
                        p.handle_events(&core_event, &ctx)
                    && let Some(combo) = self.keybindings().first_combo(action)
                {
                    self.synthetic_event = Some(Event::Key(KeyEvent {
                        code: combo.code,
                        modifiers: combo.mods,
                        kind: KeyKind::Press,
                    }));
                    return true;
                }
                false
            }
            HitTarget::Overlay(id) => {
                let ctx = self.component_context_for(false, slotmap::DefaultKey::default());
                if let Some(overlay) = self.overlays.get_mut(&id) {
                    let result = overlay.handle_events(&core_event, &ctx);
                    !result.is_ignored()
                } else {
                    false
                }
            }
            HitTarget::LayoutHandle => {
                self.mouse_capture = Some(MouseCaptureState::LayoutHandle);
                if let Some(layout) = self.managed_layout.as_mut() {
                    layout.handle_event(&core_event, self.managed_area)
                } else {
                    false
                }
            }
        }
    }

    fn reset_drag_snap_timer(&mut self) {
        if let Some(handle) = &self.system_task_handle {
            if let Some(old) = self.drag_timer_id.take() {
                handle.cancel(old);
            }
            if let Some(timeout) = self.config.drag_snap_timeout {
                self.drag_timer_id = Some(handle.schedule_once(timeout, SystemTask::DragSnap));
            }
        }
    }

    fn cancel_drag_snap_timer(&mut self) {
        if let Some(handle) = &self.system_task_handle
            && let Some(old) = self.drag_timer_id.take()
        {
            handle.cancel(old);
        }
    }

    fn arm_drag_snap_timer(&mut self) {
        if let Some(handle) = &self.system_task_handle {
            if let Some(old) = self.drag_timer_id.take() {
                handle.cancel(old);
            }
            if let Some(timeout) = self.config.drag_snap_timeout {
                self.drag_timer_id = Some(handle.schedule_once(timeout, SystemTask::DragSnap));
            }
        }
    }

    /// Arm a 50ms single-shot timer for temporal-dwell visual feedback.
    /// Cancels any previously-armed temporal tick before creating a new one.
    /// The tick handler (on_temporal_dwell_tick) may re-arm for continuous
    /// cycling while the cursor remains inside the magnetic zone.
    fn arm_temporal_dwell_timer(&mut self) {
        if let Some(handle) = &self.system_task_handle {
            if let Some(old) = self.temporal_timer_id.take() {
                handle.cancel(old);
            }
            self.temporal_timer_id = Some(handle.schedule_once(
                std::time::Duration::from_millis(50),
                SystemTask::TemporalDwellTick,
            ));
        }
    }

    /// Called by the runner on each `SystemTask::TemporalDwellTick`.
    /// If the cursor is still held stationary inside a magnetic zone,
    /// triggers a render frame and re-arms the 50ms tick.  Otherwise
    /// stops the cycle.
    pub fn on_temporal_dwell_tick(&mut self) {
        let in_magnetic_zone = matches!(
            &self.mouse_capture,
            Some(MouseCaptureState::DraggingWindow { resistance, .. })
                if resistance.entered_magnetic_x_at.is_some()
                || resistance.entered_magnetic_y_at.is_some()
        );
        if in_magnetic_zone {
            self.mark_layout_dirty();
            self.arm_temporal_dwell_timer();
        } else {
            self.temporal_timer_id = None;
        }
    }

    /// Handle clicks on top-panel icons (menu, mouse capture, selection, etc.).
    /// Returns true if the click was consumed.
    fn handle_panel_click(&mut self, col: u16, row: u16) -> bool {
        if self.top_component.is_none() {
            return false;
        }
        if !crate::layout::rect_contains(self.top_claimed, col, row) {
            return false;
        }
        // Use handle_event which routes through all hit-test methods
        let down_event = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        });
        let ctx = self.component_context(false);
        let Some(p) = self.top_component.as_mut() else {
            return false;
        };
        match p.handle_events(&down_event, &ctx) {
            crate::actions::EventResult::Action(action) => match action {
                TermWmAction::WmToggleOverlay => {
                    if self.command_menu_visible() {
                        self.close_command_menu();
                    } else {
                        self.open_command_menu();
                    }
                    true
                }
                TermWmAction::ToggleMouseCapture => {
                    self.toggle_mouse_capture();
                    true
                }
                TermWmAction::ToggleWindowSelection => {
                    self.toggle_window_selection();
                    true
                }
                TermWmAction::ToggleClipboardMode => {
                    self.toggle_clipboard_enabled();
                    true
                }
                TermWmAction::CopySelection => {
                    self.copy_selection_to_clipboard();
                    true
                }
                TermWmAction::FocusWindow(key) => {
                    if self.window_state(key) == Some(WindowState::Iconic) {
                        self.transition_window(key, WindowState::Mapped);
                    }
                    self.focus_window_key(key);
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    pub fn arm_capture(&mut self, timeout: Duration) {
        self.capture_deadline = Some(Instant::now() + timeout);
        self.pending_deadline = None;
    }

    pub fn arm_pending(&mut self, timeout: Duration) {
        self.pending_deadline = Some(Instant::now() + timeout);
    }

    pub fn clear_capture(&mut self) {
        self.capture_deadline = None;
        self.pending_deadline = None;
        self.command_menu_visible = false;
        self.command_menu_opened_at = None;
        self.super_pending_event = None;
        self.super_pending_at = None;
        if let Some(handle) = &self.system_task_handle {
            if let Some(id) = self.super_timer_id.take() {
                handle.cancel(id);
            }
            if let Some(id) = self.drag_timer_id.take() {
                handle.cancel(id);
            }
        }
        if let Some(menu) = &mut self.command_menu_component {
            menu.process_action(&ComponentAction::Restore);
        }
    }

    pub fn capture_active(&mut self) -> bool {
        if !self.mouse_capture_enabled {
            return false;
        }
        if self.config.wm_command_menu_enabled && self.command_menu_visible {
            return true;
        }
        self.refresh_capture();
        self.capture_deadline.is_some()
    }

    pub fn mouse_capture_enabled(&self) -> bool {
        self.mouse_capture_enabled
    }

    pub fn keyboard_focus_enabled(&self) -> bool {
        self.config.keyboard_focus_enabled
    }

    pub fn set_keyboard_focus_enabled(&mut self, enabled: bool) {
        self.config.keyboard_focus_enabled = enabled;
    }

    pub fn mouse_focus_click_enabled(&self) -> bool {
        self.config.mouse_focus_click_enabled
    }

    pub fn set_mouse_focus_click_enabled(&mut self, enabled: bool) {
        self.config.mouse_focus_click_enabled = enabled;
    }

    pub fn set_mouse_capture_enabled(&mut self, enabled: bool) {
        if self.mouse_capture_enabled == enabled {
            return;
        }
        self.mouse_capture_enabled = enabled;
        self.mouse_capture_dirty = true;
        if !enabled {
            self.clear_capture();
        }
    }

    pub fn toggle_mouse_capture(&mut self) {
        self.mouse_capture_enabled = !self.mouse_capture_enabled;
        self.mouse_capture_dirty = true;
        if !self.mouse_capture_enabled {
            self.clear_capture();
        }
    }

    pub fn take_mouse_capture_change(&mut self) -> Option<bool> {
        if self.mouse_capture_dirty {
            self.mouse_capture_dirty = false;
            Some(self.mouse_capture_enabled)
        } else {
            None
        }
    }

    pub fn take_clipboard_change(&mut self) -> Option<bool> {
        if self.clipboard_dirty {
            self.clipboard_dirty = false;
            Some(self.clipboard_enabled)
        } else {
            None
        }
    }

    pub fn clipboard_enabled(&self) -> bool {
        self.clipboard_enabled
    }

    pub fn clipboard_mut(&mut self) -> Option<&mut crate::clipboard::Clipboard> {
        self.clipboard.as_mut()
    }

    pub fn power_profile(&self) -> PowerProfile {
        self.power_profile
    }

    pub fn set_power_profile(&mut self, profile: PowerProfile) {
        if self.power_profile == profile {
            return;
        }
        self.power_profile = profile;
        profile.report_change();
    }

    pub fn set_selection_snapshot(&mut self, active: bool, dragging: bool, text: Option<String>) {
        let changed = text.as_ref() != self.selection_text.as_ref();
        self.selection_active = active;
        self.selection_dragging = dragging;
        self.selection_text = text;
        if !self.selection_active || self.selection_text.is_none() || changed {
            self.selection_copied = false;
            self.selection_copied_text = None;
        }
    }

    pub fn selection_active(&self) -> bool {
        self.selection_active
    }

    pub fn selection_dragging(&self) -> bool {
        self.selection_dragging
    }

    pub fn selection_text(&self) -> Option<&str> {
        self.selection_text.as_deref()
    }

    pub fn selection_copied(&self) -> bool {
        self.selection_copied
    }

    pub fn copy_selection_to_clipboard(&mut self) {
        if !self.clipboard_enabled() {
            return;
        }
        let Some(text) = self.selection_text.clone() else {
            return;
        };
        if let Some(cb) = &mut self.clipboard
            && cb.set(&text).is_ok()
        {
            self.selection_copied = true;
            self.selection_copied_text = Some(text);
        }
    }

    pub fn window_selection_enabled(&self) -> bool {
        self.window_selection_enabled
    }

    pub fn toggle_window_selection(&mut self) {
        let next = !self.window_selection_enabled;
        self.set_window_selection_enabled(next);
    }

    pub fn set_window_selection_enabled(&mut self, enabled: bool) {
        if self.window_selection_enabled == enabled {
            return;
        }
        self.window_selection_enabled = enabled;
        self.window_selection_dirty = true;
    }

    pub fn take_window_selection_change(&mut self) -> Option<bool> {
        if self.window_selection_dirty {
            self.window_selection_dirty = false;
            Some(self.window_selection_enabled)
        } else {
            None
        }
    }

    pub fn set_clipboard_enabled(&mut self, enabled: bool) {
        if self.clipboard_enabled == enabled {
            return;
        }
        self.clipboard_enabled = enabled;
        self.clipboard_dirty = true;
        for overlay in self.overlays.values_mut() {
            let o: &mut dyn crate::components::Overlay<TermWmAction> = &mut **overlay;
            o.set_selection_enabled(enabled);
        }
    }

    pub fn toggle_clipboard_enabled(&mut self) {
        let next = !self.clipboard_enabled;
        self.set_clipboard_enabled(next);
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

    /// Register a component directly on a window in the SlotMap.
    /// Creates the SlotMap entry, stores the component, and returns the
    /// WindowKey.  The App or runner retrieves the component via
    /// Helper to create a window with a WM-owned component (debug log, etc.).
    /// The component is stored directly in the Window struct, NOT in
    /// an app-sidecar HashMap. Callers that need post-creation access
    /// should configure the component before boxing it.
    pub fn set_system_window(
        &mut self,
        component: Box<dyn crate::components::Component<TermWmAction>>,
    ) -> WindowKey {
        let key = self.create_window(component);
        if let Some(w) = self.windows.get_mut(key) {
            w.is_system_window = true;
        }
        key
    }

    /// Render-phase access: borrow component immutably.
    pub fn component_for_key(
        &self,
        key: WindowKey,
    ) -> Option<&dyn crate::components::Component<TermWmAction>> {
        let w = self.windows.get(key)?;
        Some(w.component.as_ref())
    }

    /// Event/update-phase access: borrow component mutably.
    pub fn component_for_key_mut(
        &mut self,
        key: WindowKey,
    ) -> Option<&mut dyn crate::components::Component<TermWmAction>> {
        let w = self.windows.get_mut(key)?;
        Some(w.component.as_mut())
    }

    /// Return all window keys currently in the SlotMap.
    pub fn all_window_keys(&self) -> Vec<WindowKey> {
        self.windows.keys().collect()
    }

    /// Return the number of windows currently in the SlotMap.
    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    /// Return keys of all windows in `WindowState::Mapped`.
    pub fn mapped_windows(&self) -> Vec<WindowKey> {
        self.windows
            .iter()
            .filter(|(_, w)| w.state == WindowState::Mapped)
            .map(|(key, _)| key)
            .collect()
    }

    /// Pull the pending pane title from a window's component, if any.
    pub fn window_pane_title(&mut self, key: WindowKey) -> Option<String> {
        self.component_for_key_mut(key)
            .and_then(|c| c.take_pending_title())
    }

    pub fn open_overlay(&mut self, id: OverlayId, overlay: Option<Box<dyn Overlay<TermWmAction>>>) {
        if let Some(o) = overlay {
            self.overlays.insert(id, o);
        }
    }

    pub fn set_scroll_keyboard_enabled(&mut self, enabled: bool) {
        self.scroll_keyboard_enabled_default = enabled;
    }

    pub fn panel_active(&self) -> bool {
        self.config.panel_enabled && self.top_component.as_ref().is_some_and(|p| p.visible())
    }

    /// Register panel hitboxes (top and bottom) into the draw-time registry.
    /// Called before the window loop so panels are at the lowest Z-order.
    pub fn register_panel_hitboxes(&self, registry: &mut HitboxRegistry) {
        if self.top_component.is_some() && !self.top_claimed.is_empty() {
            registry.register(HitTarget::TopPanel, self.top_claimed);
        }
        if self.bottom_component.is_some() && !self.bottom_claimed.is_empty() {
            registry.register(HitTarget::BottomPanel, self.bottom_claimed);
        }
    }

    /// Register tiling layout split handle hitboxes.
    /// Called after panel registration, before the window loop, so handles sit
    /// below windows in Z-order (floating windows correctly occlude them).
    pub fn register_layout_handle_hitboxes(&self, registry: &mut HitboxRegistry) {
        for handle in &self.handles {
            registry.register(HitTarget::LayoutHandle, handle.rect);
        }
    }

    /// Register chrome hitboxes (resize handles + header) for a specific window.
    /// Called after `composite_window` so chrome is on top of content.
    pub fn register_window_chrome_hitboxes(&self, key: WindowKey, registry: &mut HitboxRegistry) {
        for handle in &self.resize_handles {
            if handle.key == key {
                registry.register(
                    HitTarget::ChromeResize(handle.key, handle.edge),
                    handle.rect,
                );
            }
        }
        for header in &self.floating_headers {
            if header.key == key {
                registry.register(
                    HitTarget::ChromeHeader(
                        header.key,
                        crate::window::decorator::HeaderAction::Drag,
                    ),
                    header.rect,
                );
            }
        }
    }

    /// Process a `TermWmAction` produced by a component's `handle_events`.
    ///
    /// Mirrors the runner's `drain_action_queue` but operates on the
    /// WindowManager's own component storage (no external `app` borrow needed).
    pub fn process_action(&mut self, key: WindowKey, action: TermWmAction) {
        use std::collections::VecDeque;
        let mut queue = VecDeque::new();
        queue.push_back((key, action));
        while let Some((k, act)) = queue.pop_front() {
            let ctx = self.component_context_for(true, k);
            if let Some(comp) = self.component_for_key_mut(k) {
                comp.update(act, &ctx, &mut queue);
            }
        }
    }

    /// Set the shared `TaskHandle<SystemTask>` for registering/cancelling system
    /// timers.  Called once by the runner during startup.
    pub fn set_system_task_handle(&mut self, handle: TaskHandle<SystemTask>) {
        self.system_task_handle = Some(handle);
    }

    /// Unified double-Esc press handler.
    /// - `Pending`: first press of WmToggleOverlay — deferred, timeout will forward.
    /// - `DoubleSuper`: second press within window — caller should open overlay.
    /// - `Forward`: not a WmToggleOverlay key — forward immediately.
    ///
    /// Timer registration and cancellation are handled via the
    /// `system_task_handle`.
    pub fn handle_super_press(
        &mut self,
        key: &KeyEvent,
        is_wm_toggle_key: bool,
    ) -> SuperPressResult {
        if is_wm_toggle_key {
            if self.super_pending_at.is_some()
                && self
                    .super_pending_at
                    .is_some_and(|at| at.elapsed() < self.config.super_passthrough_window)
            {
                // Second press within window — cancel timer, clear state
                if let Some(handle) = &self.system_task_handle
                    && let Some(id) = self.super_timer_id.take()
                {
                    handle.cancel(id);
                }
                self.super_pending_event = None;
                self.super_pending_at = None;
                return SuperPressResult::DoubleSuper;
            }
            // First press — register timer via scheduler
            self.super_pending_event = Some(*key);
            self.super_pending_at = Some(Instant::now());
            if let Some(handle) = &self.system_task_handle {
                if let Some(old) = self.super_timer_id.take() {
                    handle.cancel(old);
                }
                self.super_timer_id = Some(handle.schedule_once(
                    self.config.super_passthrough_window,
                    SystemTask::SuperPassthrough {
                        event: Event::Key(*key),
                    },
                ));
            }
            SuperPressResult::Pending
        } else {
            // Non-toggle key — clear pending state
            if let Some(handle) = &self.system_task_handle
                && let Some(id) = self.super_timer_id.take()
            {
                handle.cancel(id);
            }
            self.super_pending_event = None;
            self.super_pending_at = None;
            SuperPressResult::Forward
        }
    }

    /// Clear the pending super-key state (called by the runner when the
    /// scheduler fires a `SuperPassthrough` task).
    pub fn clear_super_pending(&mut self) {
        self.super_pending_event = None;
        self.super_pending_at = None;
        self.super_timer_id = None;
    }

    /// Time remaining for the panel countdown display.
    /// Returns `None` when no super-key is pending or the timer has expired.
    pub fn super_pending_remaining(&self) -> Option<Duration> {
        let at = self.super_pending_at?;
        let elapsed = at.elapsed();
        if elapsed >= self.config.super_passthrough_window {
            return None;
        }
        Some(self.config.super_passthrough_window.saturating_sub(elapsed))
    }

    /// Time remaining before the drag snap preview is auto-applied.
    /// Returns `None` when the feature is disabled or no drag is active.
    pub fn drag_snap_remaining(&self) -> Option<Duration> {
        let timeout = self.config.drag_snap_timeout?;
        if !self
            .mouse_capture
            .as_ref()
            .is_some_and(|c| matches!(c, MouseCaptureState::DraggingWindow { .. }))
        {
            return None;
        }
        let last = self.drag_last_event?;
        let elapsed = last.elapsed();
        if elapsed >= timeout {
            return Some(Duration::ZERO);
        }
        Some(timeout.saturating_sub(elapsed))
    }

    /// Called by the runner when the scheduler fires a `DragSnap` task.
    /// If a drag was in progress, applies the pending snap and cleans up.
    pub fn apply_drag_snap_if_pending(&mut self) {
        if let Some(capture) = self.mouse_capture.take()
            && let MouseCaptureState::DraggingWindow { key, .. } = capture
        {
            self.drag_last_event = None;
            self.drag_timer_id = None;
            if self.snap_preview == Some(SnapPreviewState::Maximize) {
                self.toggle_maximize(key);
            } else if self.drag_snap.is_some() {
                self.apply_snap(key);
            }
        }
        // Unconditional flush — prevents stale ghost previews
        self.drag_snap = None;
        self.snap_preview = None;
        self.snap_projection_cache = None;
    }

    pub fn super_passthrough_active(&self) -> bool {
        self.super_passthrough_remaining().is_some()
    }

    pub fn super_passthrough_remaining(&self) -> Option<Duration> {
        if !self.command_menu_visible() {
            return None;
        }
        let opened_at = self.command_menu_opened_at?;
        let elapsed = opened_at.elapsed();
        if elapsed >= self.config.super_passthrough_window {
            return None;
        }
        Some(self.config.super_passthrough_window.saturating_sub(elapsed))
    }

    // ── Event Routing & Update Accessors ─────────────────────────────

    pub fn top_component_mut(&mut self) -> &mut Option<Box<dyn WmComponent>> {
        &mut self.top_component
    }

    pub fn bottom_component_mut(&mut self) -> &mut Option<Box<dyn WmComponent>> {
        &mut self.bottom_component
    }

    pub fn command_menu_component_mut(&mut self) -> &mut Option<Box<dyn WmComponent>> {
        &mut self.command_menu_component
    }

    pub fn overlays_mut(&mut self) -> &mut BTreeMap<OverlayId, Box<dyn Overlay<TermWmAction>>> {
        &mut self.overlays
    }

    // ── Immutable state queries (used by both rendering and event dispatch) ─

    pub fn top_component(&self) -> Option<&dyn WmComponent> {
        self.top_component.as_deref()
    }

    pub fn bottom_component(&self) -> Option<&dyn WmComponent> {
        self.bottom_component.as_deref()
    }

    pub fn command_menu_component(&self) -> Option<&dyn WmComponent> {
        self.command_menu_component.as_deref()
    }

    pub fn top_claimed_area(&self) -> LayoutRect {
        self.top_claimed
    }

    pub fn bottom_claimed_area(&self) -> LayoutRect {
        self.bottom_claimed
    }

    pub fn managed_area(&self) -> LayoutRect {
        self.managed_area
    }

    pub fn overlays(&self) -> &BTreeMap<OverlayId, Box<dyn Overlay<TermWmAction>>> {
        &self.overlays
    }

    pub fn supported_menu_actions(&self) -> &[TermWmAction] {
        &self.supported_menu_actions
    }

    pub fn resize_handles(&self) -> &[ResizeHandle<WindowKey>] {
        &self.resize_handles
    }

    pub fn floating_headers(&self) -> &[DragHandle<WindowKey>] {
        &self.floating_headers
    }
}

pub fn wm_menu_items(
    mouse_capture_enabled: bool,
    clipboard_enabled: bool,
    window_selection_enabled: bool,
) -> Vec<MenuItem<crate::actions::TermWmAction>> {
    let mouse_label = if mouse_capture_enabled {
        "Mouse Capture: On"
    } else {
        "Mouse Capture: Off"
    };
    let clipboard_label = if clipboard_enabled {
        "Clipboard Mode: On"
    } else {
        "Clipboard Mode: Off"
    };
    let selection_label = if window_selection_enabled {
        "Window Selection: On"
    } else {
        "Window Selection: Off"
    };
    vec![
        MenuItem {
            label: "Resume",
            icon: Some("▶"),
            action: crate::actions::TermWmAction::CloseMenu,
        },
        MenuItem {
            label: mouse_label,
            icon: Some("◆"),
            action: crate::actions::TermWmAction::ToggleMouseCapture,
        },
        MenuItem {
            label: clipboard_label,
            icon: Some("■"),
            action: crate::actions::TermWmAction::ToggleClipboardMode,
        },
        MenuItem {
            label: selection_label,
            icon: Some("●"),
            action: crate::actions::TermWmAction::ToggleWindowSelection,
        },
        MenuItem {
            label: "Floating Front",
            icon: Some("↑"),
            action: crate::actions::TermWmAction::BringFloatingFront,
        },
        MenuItem {
            label: "New Window",
            icon: Some("+"),
            action: crate::actions::TermWmAction::NewWindow,
        },
        MenuItem {
            label: "Debug Log",
            icon: Some("≣"),
            action: crate::actions::TermWmAction::ToggleDebugWindow,
        },
        MenuItem {
            label: "Help",
            icon: Some("?"),
            action: crate::actions::TermWmAction::Help,
        },
        MenuItem {
            label: "Exit UI",
            icon: Some("⏻"),
            action: crate::actions::TermWmAction::ExitUi,
        },
    ]
}

fn clamp_rect(area: Rect, bounds: Rect) -> Rect {
    let x0 = area.x.max(bounds.x);
    let y0 = area.y.max(bounds.y);
    let x1 = area
        .x
        .saturating_add(i32::from(area.width))
        .min(bounds.x.saturating_add(i32::from(bounds.width)));
    let y1 = area
        .y
        .saturating_add(i32::from(area.height))
        .min(bounds.y.saturating_add(i32::from(bounds.height)));
    if x1 <= x0 || y1 <= y0 {
        return Rect::default();
    }
    Rect {
        x: x0,
        y: y0,
        width: x1.saturating_sub(x0) as u16,
        height: y1.saturating_sub(y0) as u16,
    }
}

fn float_rect_visible(rect: crate::window::FloatRect, bounds: Rect) -> Rect {
    let bounds_x0 = bounds.x;
    let bounds_y0 = bounds.y;
    let bounds_x1 = bounds_x0.saturating_add(i32::from(bounds.width));
    let bounds_y1 = bounds_y0.saturating_add(i32::from(bounds.height));
    let rect_x0 = rect.x;
    let rect_y0 = rect.y;
    let rect_x1 = rect.x.saturating_add(i32::from(rect.width));
    let rect_y1 = rect.y.saturating_add(i32::from(rect.height));
    let x0 = rect_x0.max(bounds_x0);
    let y0 = rect_y0.max(bounds_y0);
    let x1 = rect_x1.min(bounds_x1);
    let y1 = rect_y1.min(bounds_y1);
    if x1 <= x0 || y1 <= y0 {
        return Rect::default();
    }
    Rect {
        x: x0,
        y: y0,
        width: x1.saturating_sub(x0) as u16,
        height: y1.saturating_sub(y0) as u16,
    }
}

fn map_layout_node(node: &LayoutNode<WindowKey>) -> LayoutNode<WindowKey> {
    match node {
        LayoutNode::Leaf(key) => LayoutNode::leaf(*key),
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
        LayoutNode::Void(id) => LayoutNode::Void(*id),
    }
}

#[cfg(test)]
fn rects_intersect(a: Rect, b: Rect) -> bool {
    if a.width == 0 || a.height == 0 || b.width == 0 || b.height == 0 {
        return false;
    }
    let a_right = a.x.saturating_add(i32::from(a.width));
    let a_bottom = a.y.saturating_add(i32::from(a.height));
    let b_right = b.x.saturating_add(i32::from(b.width));
    let b_bottom = b.y.saturating_add(i32::from(b.height));
    a.x < b_right && a_right > b.x && a.y < b_bottom && a_bottom > b.y
}

#[cfg(test)]
fn make_keys(wm: &mut WindowManager, n: usize) -> Vec<WindowKey> {
    (0..n)
        .map(|_| wm.create_window(Box::new(crate::components::NoopComponent)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Constraint, Direction};
    use term_wm_layout_engine::LayoutRect;

    /// Test fixture: how far back to set `drag_last_event` to simulate
    /// a stale/expired drag (10 seconds).
    const STALE_EVENT_OFFSET: Duration = Duration::from_secs(10);

    /// Test fixture: short drag-snap timeout (1 second).
    const SHORT_SNAP_TIMEOUT: Duration = Duration::from_secs(1);

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
    fn map_layout_node_maps_leaf_to_windowkey() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let key = wm.create_window(Box::new(crate::components::NoopComponent));
        let node = LayoutNode::leaf(key);
        let mapped = map_layout_node(&node);
        match mapped {
            LayoutNode::Leaf(key) => assert_eq!(key, key),
            _ => panic!("expected leaf"),
        }
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
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);

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
        wm.regions.set(keys[1], r1);
        wm.regions.set(keys[2], r2);
        wm.z_order.push(keys[1]);
        wm.z_order.push(keys[2]);
        wm.managed_draw_order = wm.z_order.clone();

        assert!(
            !wm.windows.contains_key(*wm.focus.current()),
            "initial focus should be a placeholder, not a real window"
        );

        let clicked_col = 6u16;
        let clicked_row = 6u16;
        let mouse = MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: clicked_col,
            row: clicked_row,
            modifiers: KeyModifiers::NONE,
        };
        let evt = Event::Mouse(mouse);
        let wm_event = crate::events::core_event_to_wm(&evt).unwrap();
        let _handled = wm.dispatch_mouse(&wm_event);
        assert_eq!(*wm.focus.current(), keys[2]);
    }

    #[test]
    fn enforce_min_visible_margin_horizontal() {
        use crate::window::{FloatRect, FloatRectSpec};
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.set_floating_resize_offscreen(true);
        wm.set_floating_rect(
            keys[1],
            Some(FloatRectSpec::Absolute(FloatRect {
                x: -4,
                y: 0,
                width: 6,
                height: 3,
            })),
        );
        wm.register_managed_layout(LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        });
        let got = wm.floating_rect(keys[1]).expect("floating rect present");
        match got {
            FloatRectSpec::Absolute(fr) => {
                let bounds = wm.managed_area;
                let left_allowed =
                    bounds.x - (6i32 - crate::constants::MIN_FLOATING_VISIBLE_MARGIN.min(6) as i32);
                assert_eq!(fr.x, left_allowed);
            }
            _ => panic!("expected absolute rect"),
        }
    }

    #[test]
    fn enforce_min_visible_margin_vertical() {
        use crate::window::{FloatRect, FloatRectSpec};
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.set_floating_resize_offscreen(true);
        wm.set_floating_rect(
            keys[2],
            Some(FloatRectSpec::Absolute(FloatRect {
                x: 0,
                y: -3,
                width: 6,
                height: 4,
            })),
        );
        wm.register_managed_layout(LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        });
        let got = wm.floating_rect(keys[2]).expect("floating rect present");
        match got {
            FloatRectSpec::Absolute(fr) => {
                assert!(fr.y >= 0);
            }
            _ => panic!("expected absolute rect"),
        }
    }

    #[test]
    fn maximize_persists_across_resize() {
        use crate::window::FloatRectSpec;
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.register_managed_layout(LayoutRect {
            x: 0,
            y: 0,
            width: 20,
            height: 15,
        });
        wm.toggle_maximize(keys[3]);
        wm.register_managed_layout(LayoutRect {
            x: 0,
            y: 0,
            width: 30,
            height: 20,
        });
        let got = wm.floating_rect(keys[3]).expect("floating rect present");
        match got {
            FloatRectSpec::Absolute(fr) => {
                assert_eq!(fr.width, wm.managed_area.width);
                assert_eq!(fr.height, wm.managed_area.height);
            }
            _ => panic!("expected absolute rect"),
        }
    }

    #[test]
    fn minimize_and_restore_preserves_floating_rect() {
        use crate::window::{FloatRect, FloatRectSpec};
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        // Map all windows first — minimize requires Mapped state
        for &k in &keys {
            wm.transition_window(k, crate::window::entry::WindowState::Mapped);
        }
        let original = FloatRect {
            x: 5,
            y: 3,
            width: 10,
            height: 8,
        };
        wm.set_floating_rect(keys[1], Some(FloatRectSpec::Absolute(original)));
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 15,
        });
        wm.minimize_window(keys[1]);
        assert_eq!(
            wm.window_state(keys[1]),
            Some(crate::window::entry::WindowState::Iconic),
            "window should be minimized"
        );
        let after_minimize = wm.floating_rect(keys[1]);
        assert!(
            after_minimize.is_some(),
            "floating rect should survive minimize"
        );
        assert_eq!(
            after_minimize,
            Some(FloatRectSpec::Absolute(original)),
            "floating rect should be unchanged after minimize"
        );
        wm.restore_minimized(keys[1]);
        assert_eq!(
            wm.window_state(keys[1]),
            Some(crate::window::entry::WindowState::Mapped),
            "window should be restored"
        );
        let after_restore = wm.floating_rect(keys[1]);
        assert_eq!(
            after_restore,
            Some(FloatRectSpec::Absolute(original)),
            "floating rect should be preserved across restore"
        );
    }

    #[test]
    fn localize_event_converts_to_local_coords() {
        use crate::events::{MouseButton, MouseEvent, MouseEventKind};
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        let target_rect = LayoutRect {
            x: 10,
            y: 5,
            width: 20,
            height: 8,
        };
        wm.set_region(keys[1], target_rect);
        let mouse = MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 15,
            row: 9,
            modifiers: KeyModifiers::NONE,
        };
        let event = Event::Mouse(mouse);
        let window_local = wm
            .localize_event(keys[1], &event)
            .expect("window-local event");
        if let Event::Mouse(local) = window_local {
            assert_eq!(local.column, 5);
            assert_eq!(local.row, 4);
        } else {
            panic!("expected mouse event");
        }

        let content_local = wm
            .localize_event_to_app(keys[1], &event)
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
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use crate::window::{FloatRect, FloatRectSpec};
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.set_floating_resize_offscreen(true);
        wm.set_floating_rect(
            keys[1],
            Some(FloatRectSpec::Absolute(FloatRect {
                x: -5,
                y: 1,
                width: 10,
                height: 5,
            })),
        );
        wm.register_managed_layout(LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 20,
        });
        let mouse = MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 0,
            row: 3,
            modifiers: KeyModifiers::NONE,
        };
        let event = Event::Mouse(mouse);

        let window_local = wm
            .localize_event(keys[1], &event)
            .expect("window-local event");
        if let Event::Mouse(local) = window_local {
            assert_eq!(local.column, 5);
            assert_eq!(local.row, 2);
        } else {
            panic!("expected mouse event");
        }

        let content_local = wm
            .localize_event_to_app(keys[1], &event)
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
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.set_floating_resize_offscreen(true);
        wm.set_floating_rect(
            keys[1],
            Some(FloatRectSpec::Absolute(FloatRect {
                x: -5,
                y: 0,
                width: 10,
                height: 5,
            })),
        );
        wm.register_managed_layout(LayoutRect {
            x: 0,
            y: 0,
            width: 30,
            height: 10,
        });
        wm.regions.set(
            keys[2],
            LayoutRect {
                x: 0,
                y: 0,
                width: 30,
                height: 10,
            },
        );
        wm.managed_draw_order = vec![keys[2], keys[1]];

        let hit = wm.hit_test_region_topmost(8, 2, &wm.managed_draw_order);
        assert_eq!(hit, Some(keys[2]));
    }

    #[test]
    fn hover_targets_respects_occlusion() {
        use crate::layout::floating::{ResizeEdge, ResizeHandle};
        use crate::layout::tiling::SplitHandle;
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.regions.set(
            keys[1],
            Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 5,
            },
        );
        wm.regions.set(
            keys[2],
            Rect {
                x: 0,
                y: 0,
                width: 5,
                height: 5,
            },
        );
        wm.managed_draw_order = vec![keys[1], keys[2]];
        let overlapping = Rect {
            x: 2,
            y: 1,
            width: 1,
            height: 1,
        };
        wm.resize_handles.push(ResizeHandle {
            key: keys[1],
            rect: overlapping,
            edge: ResizeEdge::Left,
        });
        wm.resize_handles.push(ResizeHandle {
            key: keys[2],
            rect: overlapping,
            edge: ResizeEdge::Left,
        });
        wm.resize_handles.push(ResizeHandle {
            key: keys[1],
            rect: Rect {
                x: 8,
                y: 1,
                width: 1,
                height: 1,
            },
            edge: ResizeEdge::Right,
        });
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
            resize_hover.map(|handle| handle.key),
            Some(keys[2]),
            "topmost window should own the hover"
        );

        wm.hover = Some((8, 1));
        let (_, resize_hover) = wm.hover_targets();
        assert_eq!(
            resize_hover.map(|handle| handle.key),
            Some(keys[1]),
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
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use crate::layout::LayoutNode;

        struct DummyComponent;
        impl crate::components::Component<TermWmAction> for DummyComponent {
            fn render(
                &mut self,
                _backend: &mut dyn term_wm_render::RenderBackend,
                _area: LayoutRect,
                _ctx: &crate::components::ComponentContext,
                _registry: &mut crate::hitbox_registry::HitboxRegistry,
            ) {
            }
            fn handle_events(
                &mut self,
                _event: &Event,
                _ctx: &crate::components::ComponentContext,
            ) -> crate::actions::EventResult<TermWmAction> {
                crate::actions::EventResult::Consumed
            }
            fn update(
                &mut self,
                _action: TermWmAction,
                _ctx: &crate::components::ComponentContext,
                _queue: &mut std::collections::VecDeque<(super::WindowKey, TermWmAction)>,
            ) {
            }
            fn destroy(&mut self) {}
        }

        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let debug_key = wm.set_system_window(Box::new(DummyComponent));
        wm.set_panel_visible(false);
        wm.set_managed_layout(TilingLayout::new(LayoutNode::leaf(debug_key)));
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });

        let header_rect = wm
            .floating_headers
            .iter()
            .find(|handle| handle.key == debug_key)
            .expect("debug header present")
            .rect;
        assert!(!wm.is_window_floating(debug_key));

        wm.hitbox_registry.register(
            HitTarget::ChromeHeader(debug_key, crate::window::decorator::HeaderAction::Drag),
            header_rect,
        );

        let down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: header_rect.x as u16,
            row: header_rect.y as u16,
            modifiers: KeyModifiers::NONE,
        });
        let wm_down = crate::events::core_event_to_wm(&down).unwrap();
        assert!(wm.dispatch_mouse(&wm_down));
        assert!(wm.is_window_floating(debug_key));
        let start_rect = match wm.floating_rect(debug_key).expect("floating rect present") {
            crate::window::FloatRectSpec::Absolute(fr) => fr,
            _ => panic!("expected absolute rect"),
        };

        let drag_col = header_rect.x.saturating_add(5) as u16;
        let drag_row = header_rect.y.saturating_add(1) as u16;
        let drag = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: drag_col,
            row: drag_row,
            modifiers: KeyModifiers::NONE,
        });
        let wm_drag = crate::events::core_event_to_wm(&drag).unwrap();
        assert!(wm.dispatch_mouse(&wm_drag));

        let moved = match wm.floating_rect(debug_key).expect("floating rect present") {
            crate::window::FloatRectSpec::Absolute(fr) => fr,
            _ => panic!("expected absolute rect"),
        };
        assert_eq!(moved.x, start_rect.x + 5);
        assert_eq!(moved.y, start_rect.y + 1);

        let up = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Release(MouseButton::Left),
            column: drag_col,
            row: drag_row,
            modifiers: KeyModifiers::NONE,
        });
        let wm_up = crate::events::core_event_to_wm(&up).unwrap();
        assert!(wm.dispatch_mouse(&wm_up));
        assert!(wm.mouse_capture.is_none());
    }

    #[test]
    fn moved_event_commits_stale_drag_snap() {
        use crate::events::{Event, KeyModifiers, MouseEvent, MouseEventKind};
        use crate::layout::InsertPosition;
        use crate::window::{FloatRect, FloatRectSpec};

        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.set_panel_visible(false);
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });

        let key = keys[1];
        // Set up a floating window so apply_snap has a valid target.
        wm.set_floating_rect(
            key,
            Some(FloatRectSpec::Absolute(FloatRect {
                x: 10,
                y: 5,
                width: 20,
                height: 10,
            })),
        );

        // Simulate abandoned drag (mouse released outside terminal).
        wm.mouse_capture = Some(MouseCaptureState::DraggingWindow {
            key,
            resistance: term_wm_layout_engine::EdgeResistance::default_tui(),
            anchor_x: 15,
            anchor_y: 10,
            initial_x: 10,
            initial_y: 5,
            start_x: 15,
            start_y: 10,
            prev_col: 15,
            prev_row: 10,
            prev_time_ns: 0,
            detach_coordinate: None,
            snap_applied: false,
        });
        wm.drag_snap = Some((
            None,
            InsertPosition::Left,
            Rect {
                x: 0,
                y: 0,
                width: 40,
                height: 24,
            },
        ));

        let moved = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        let wm_moved = crate::events::core_event_to_wm(&moved).unwrap();
        assert!(wm.dispatch_mouse(&wm_moved));
        // Capture is kept alive so Release can clean up properly
        assert!(
            wm.mouse_capture.is_some(),
            "mouse_capture must survive Moved snap commit"
        );
        assert!(
            wm.drag_snap.is_none(),
            "drag_snap should be consumed by apply_snap"
        );
    }

    #[test]
    fn moved_snap_then_release_does_not_corrupt_layout() {
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use crate::layout::{Direction, LayoutNode, TilingLayout};
        use crate::window::{FloatRect, FloatRectSpec};

        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        wm.set_panel_visible(false);
        let keys = make_keys(&mut wm, 100);

        // Two-window horizontal tiling layout.
        let split = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::Leaf(keys[1]), LayoutNode::Leaf(keys[2])],
            weights: vec![1.0, 1.0],
            constraints: vec![],
            resizable: true,
        };
        wm.managed_layout = Some(TilingLayout::new(split));
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });

        let leaves_before: Vec<_> = wm.managed_layout.as_ref().unwrap().root().collect_leaves();
        assert_eq!(leaves_before.len(), 2, "must start with 2 tiled windows");

        let dragged_key = keys[1];
        wm.set_floating_rect(
            dragged_key,
            Some(FloatRectSpec::Absolute(FloatRect {
                x: 5,
                y: 5,
                width: 30,
                height: 12,
            })),
        );

        wm.mouse_capture = Some(MouseCaptureState::DraggingWindow {
            key: dragged_key,
            resistance: term_wm_layout_engine::EdgeResistance::default_tui(),
            anchor_x: 10,
            anchor_y: 10,
            initial_x: 5,
            initial_y: 5,
            start_x: 10,
            start_y: 10,
            prev_col: 10,
            prev_row: 10,
            prev_time_ns: 0,
            detach_coordinate: None,
            snap_applied: false,
        });

        // Snap target: insert keys[1] to the RIGHT of keys[2].
        wm.drag_snap = Some((
            Some(keys[2]),
            InsertPosition::Right,
            Rect {
                x: 40,
                y: 0,
                width: 40,
                height: 24,
            },
        ));

        // Phase 1: Moved event fires — applies snap.
        let moved = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        let wm_moved = crate::events::core_event_to_wm(&moved).unwrap();
        assert!(wm.dispatch_mouse(&wm_moved));

        // Capture must still be alive.
        assert!(
            wm.mouse_capture.is_some(),
            "capture must survive Moved snap commit"
        );

        // Verify snap was applied.
        assert!(
            wm.layout_contains(dragged_key),
            "dragged window must be in tiling tree after Moved snap"
        );
        assert!(
            wm.drag_snap.is_none(),
            "drag_snap must be consumed by apply_snap"
        );
        assert!(
            wm.snap_preview.is_none(),
            "snap_preview must be cleared after Moved snap commit"
        );

        // Verify snap_applied flag was set.
        match wm.mouse_capture.as_ref() {
            Some(MouseCaptureState::DraggingWindow { snap_applied, .. }) => {
                assert!(
                    *snap_applied,
                    "snap_applied must be true after Moved commits snap"
                );
            }
            _ => panic!("expected DraggingWindow capture"),
        }

        // Phase 2: Release event fires — should NOT double-detach.
        let release = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Release(MouseButton::Left),
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        let wm_release = crate::events::core_event_to_wm(&release).unwrap();
        assert!(wm.dispatch_mouse(&wm_release));

        assert!(
            wm.mouse_capture.is_none(),
            "capture must be cleared after Release"
        );

        // Verify layout is not corrupted: both windows present.
        let leaves_after: Vec<_> = wm.managed_layout.as_ref().unwrap().root().collect_leaves();
        assert_eq!(
            leaves_after.len(),
            2,
            "layout must still have exactly 2 windows after Moved+Release"
        );
        assert!(
            leaves_after.contains(&keys[1]),
            "keys[1] must remain in layout"
        );
        assert!(
            leaves_after.contains(&keys[2]),
            "keys[2] must remain in layout"
        );

        // Verify regions can be computed without panic.
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let regions = wm.managed_layout.as_ref().unwrap().root().layout(area);
        assert_eq!(regions.len(), 2, "layout must produce 2 regions");

        // Verify no overlapping regions.
        for (i, (_, r1)) in regions.iter().enumerate() {
            for (j, (_, r2)) in regions.iter().enumerate() {
                if i != j {
                    assert!(
                        !rects_intersect(*r1, *r2),
                        "regions must not overlap: region {} = {:?}, region {} = {:?}",
                        i,
                        r1,
                        j,
                        r2,
                    );
                }
            }
        }
    }

    #[test]
    fn adjust_event_rebases_app_mouse_coordinates() {
        use crate::events::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        let full = Rect {
            x: 10,
            y: 3,
            width: 12,
            height: 8,
        };
        wm.regions.set(keys[1], full);

        let global = MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 16,
            row: 9,
            modifiers: KeyModifiers::NONE,
        };
        let content = wm.region_for_key(keys[1]);
        let localized = Event::Mouse(MouseEvent {
            column: global.column.saturating_sub(content.x as u16),
            row: global.row.saturating_sub(content.y as u16),
            kind: global.kind,
            modifiers: global.modifiers,
        });

        let rebased = wm.adjust_event_for_window(keys[1], &localized);
        let Event::Mouse(result) = rebased else {
            panic!("expected mouse event");
        };
        assert_eq!(result.column, global.column - full.x as u16);
        assert_eq!(result.row, global.row - full.y as u16);
    }

    #[test]
    fn hover_scroll_routes_to_non_focused_window() {
        use crate::events::{Event, KeyModifiers, MouseEvent, MouseEventKind};
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);

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
        wm.regions.set(keys[1], r1);
        wm.regions.set(keys[2], r2);
        // Window 2 is topmost (last in draw order)
        wm.z_order = vec![keys[1], keys[2]];
        wm.managed_draw_order = wm.z_order.clone();
        // Focus on window 1 without altering z_order (unlike focus_app_window which brings to front)
        wm.focus.set_current(keys[1]);
        wm.focus.set_current(keys[1]);
        assert_eq!(wm.focused_window(), keys[1]);
        assert_eq!(wm.focused_window(), keys[1]);

        let scroll = Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 6,
            row: 6,
            modifiers: KeyModifiers::NONE,
        });

        let _result = wm.dispatch_focused_event(&scroll);
        // hover-to-scroll without components returns None
        assert!(wm.focused_window() == keys[1], "focus must not change");
    }

    #[test]
    fn hover_scroll_over_focused_window_routes_normally() {
        use crate::events::{Event, KeyModifiers, MouseEvent, MouseEventKind};
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);

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
        wm.regions.set(keys[1], r1);
        wm.regions.set(keys[2], r2);
        wm.z_order.push(keys[1]);
        wm.z_order.push(keys[2]);
        wm.managed_draw_order = wm.z_order.clone();

        wm.focus_app_window(keys[2]);

        let scroll = Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 6,
            row: 6,
            modifiers: KeyModifiers::NONE,
        });

        let _result = wm.dispatch_focused_event(&scroll);
        // hover-scroll without components returns None
    }

    #[test]
    fn hover_scroll_outside_all_windows_routes_to_focused() {
        use crate::events::{Event, KeyModifiers, MouseEvent, MouseEventKind};
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);

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
        wm.regions.set(keys[1], r1);
        wm.regions.set(keys[2], r2);
        wm.z_order.push(keys[1]);
        wm.z_order.push(keys[2]);
        wm.managed_draw_order = wm.z_order.clone();

        wm.focus_app_window(keys[1]);

        let scroll = Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 20,
            row: 20,
            modifiers: KeyModifiers::NONE,
        });

        let _result = wm.dispatch_focused_event(&scroll);
        // no component, result is None
    }

    #[test]
    fn direct_mode_defaults_to_false() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.focus_app_window(keys[0]);
        let focus = wm.focused_window();
        assert!(!wm.direct_mode(focus));
    }

    #[test]
    fn direct_mode_toggle_cycles_state() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.focus_app_window(keys[0]);
        let focus = wm.focused_window();

        assert!(!wm.direct_mode(focus));
        wm.toggle_direct_mode(focus);
        assert!(wm.direct_mode(focus));
        wm.toggle_direct_mode(focus);
        assert!(!wm.direct_mode(focus));
    }

    #[test]
    fn direct_mode_set_get_roundtrip() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        let key = keys[42];
        assert!(!wm.direct_mode(key), "default is false");

        wm.set_direct_mode(key, true);
        assert!(wm.direct_mode(key));

        wm.set_direct_mode(key, false);
        assert!(!wm.direct_mode(key));
    }

    #[test]
    fn direct_mode_is_per_window() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        let id_a = keys[1];
        let id_b = keys[2];

        wm.set_direct_mode(id_a, true);
        assert!(wm.direct_mode(id_a));
        assert!(!wm.direct_mode(id_b));
    }

    #[test]
    fn direct_mode_header_click_toggles_flag() {
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use crate::layout::{LayoutNode, TilingLayout};

        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.set_panel_visible(false);

        // Create a proper managed layout with window 1
        wm.set_managed_layout(TilingLayout::new(LayoutNode::leaf(keys[1])));
        wm.managed_draw_order = vec![keys[1]];
        wm.z_order = vec![keys[1]];

        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        wm.focus_app_window(keys[1]);

        let win_key = keys[1];

        // The K button position must match what hit_test computes
        // using the full window rect (not the inset header rect).
        let full_rect = wm.full_region_for_key(win_key);
        let outer_right = full_rect
            .x
            .saturating_add(i32::from(full_rect.width))
            .saturating_sub(1);
        let close_x = outer_right.saturating_sub(2);
        let max_x = close_x.saturating_sub(2);
        let min_x = max_x.saturating_sub(2);
        let kb_x = min_x.saturating_sub(2) as u16;
        let kb_y = full_rect.y.saturating_add(1) as u16; // header row
        assert_eq!(
            wm.decorator().hit_test(full_rect, kb_x, kb_y),
            crate::window::decorator::HeaderAction::ToggleDirectMode,
            "hit_test should detect D button at ({},{}) on {:?}",
            kb_x,
            kb_y,
            full_rect
        );

        assert!(!wm.direct_mode(win_key), "starts off");

        wm.hitbox_registry.register(
            HitTarget::ChromeHeader(win_key, crate::window::decorator::HeaderAction::Drag),
            full_rect,
        );

        let click = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: kb_x,
            row: kb_y,
            modifiers: KeyModifiers::NONE,
        });
        let wm_click = crate::events::core_event_to_wm(&click).unwrap();
        assert!(
            wm.dispatch_mouse(&wm_click),
            "header D button click should be handled"
        );
        assert!(
            wm.direct_mode(win_key),
            "clicking D toggles direct_mode to true"
        );

        let click2 = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: kb_x,
            row: kb_y,
            modifiers: KeyModifiers::NONE,
        });
        let wm_click2 = crate::events::core_event_to_wm(&click2).unwrap();
        assert!(wm.dispatch_mouse(&wm_click2));
        assert!(
            !wm.direct_mode(win_key),
            "second click toggles back to false"
        );
    }

    #[test]
    fn direct_mode_header_click_on_non_button_area_does_not_toggle() {
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use crate::layout::{LayoutNode, TilingLayout};

        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.set_panel_visible(false);

        // Create a proper managed layout with window 1
        wm.set_managed_layout(TilingLayout::new(LayoutNode::leaf(keys[1])));
        wm.managed_draw_order = vec![keys[1]];
        wm.z_order = vec![keys[1]];

        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        wm.focus_app_window(keys[1]);

        let win_key = keys[1];
        let header = wm
            .floating_headers
            .iter()
            .find(|h| h.key == win_key)
            .expect("floating header for window 1");

        let drag_x = (header.rect.x.saturating_add(i32::from(header.rect.width)) / 2) as u16;
        let drag_y = header.rect.y as u16;

        wm.hitbox_registry.register(
            HitTarget::ChromeHeader(win_key, crate::window::decorator::HeaderAction::Drag),
            header.rect,
        );

        assert!(!wm.direct_mode(win_key));

        let click = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: drag_x,
            row: drag_y,
            modifiers: KeyModifiers::NONE,
        });
        let wm_click = crate::events::core_event_to_wm(&click).unwrap();
        assert!(wm.dispatch_mouse(&wm_click));
        assert!(!wm.direct_mode(win_key), "drag area click must not toggle");
    }

    #[test]
    fn drag_snap_timeout_none_disables_remaining() {
        let mut config = WmConfig::standalone();
        config.drag_snap_timeout = None;
        let mut wm = WindowManager::with_config(
            config,
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let _keys = make_keys(&mut wm, 100);
        assert!(wm.drag_snap_remaining().is_none());
    }

    #[test]
    fn drag_snap_remaining_none_when_no_drag() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let _keys = make_keys(&mut wm, 100);
        assert!(wm.drag_snap_remaining().is_none());
    }

    #[test]
    fn drag_snap_remaining_returns_some_when_dragging() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.mouse_capture = Some(MouseCaptureState::DraggingWindow {
            key: keys[1],
            resistance: term_wm_layout_engine::EdgeResistance::default_tui(),
            anchor_x: 0,
            anchor_y: 0,
            initial_x: 0,
            initial_y: 0,
            start_x: 0,
            start_y: 0,
            prev_col: 0,
            prev_row: 0,
            prev_time_ns: 0,
            detach_coordinate: None,
            snap_applied: false,
        });
        wm.drag_last_event = Some(Instant::now());
        let remaining = wm.drag_snap_remaining();
        assert!(remaining.is_some());
        assert!(remaining.unwrap() > Duration::from_millis(1000));
    }

    #[test]
    fn drag_snap_remaining_zero_when_expired() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.mouse_capture = Some(MouseCaptureState::DraggingWindow {
            key: keys[1],
            resistance: term_wm_layout_engine::EdgeResistance::default_tui(),
            anchor_x: 0,
            anchor_y: 0,
            initial_x: 0,
            initial_y: 0,
            start_x: 0,
            start_y: 0,
            prev_col: 0,
            prev_row: 0,
            prev_time_ns: 0,
            detach_coordinate: None,
            snap_applied: false,
        });
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.mouse_capture = Some(MouseCaptureState::DraggingWindow {
            key: keys[1],
            resistance: term_wm_layout_engine::EdgeResistance::default_tui(),
            anchor_x: 0,
            anchor_y: 0,
            initial_x: 0,
            initial_y: 0,
            start_x: 0,
            start_y: 0,
            prev_col: 0,
            prev_row: 0,
            prev_time_ns: 0,
            detach_coordinate: None,
            snap_applied: false,
        });
        wm.drag_last_event = Some(Instant::now() - STALE_EVENT_OFFSET);
        assert_eq!(wm.drag_snap_remaining(), Some(Duration::ZERO));
    }

    #[test]
    fn apply_drag_snap_if_pending_no_drag_is_noop() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        wm.apply_drag_snap_if_pending();
        // The method should not panic when there is no drag in progress.
    }

    #[test]
    fn apply_drag_snap_if_pending_applies_when_drag_active() {
        use crate::layout::InsertPosition;
        use crate::window::{FloatRect, FloatRectSpec};

        let mut config = WmConfig::standalone();
        config.drag_snap_timeout = Some(SHORT_SNAP_TIMEOUT);
        let mut wm = WindowManager::with_config(
            config,
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.set_panel_visible(false);
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });

        let key = keys[1];
        wm.set_floating_rect(
            key,
            Some(FloatRectSpec::Absolute(FloatRect {
                x: 10,
                y: 5,
                width: 20,
                height: 10,
            })),
        );

        let window_key = keys[2];
        wm.regions.set(
            window_key,
            Rect {
                x: 0,
                y: 0,
                width: 40,
                height: 24,
            },
        );
        wm.managed_layout = Some(crate::layout::TilingLayout::new(
            crate::layout::LayoutNode::leaf(window_key),
        ));

        wm.mouse_capture = Some(MouseCaptureState::DraggingWindow {
            key,
            resistance: term_wm_layout_engine::EdgeResistance::default_tui(),
            anchor_x: 15,
            anchor_y: 10,
            initial_x: 10,
            initial_y: 5,
            start_x: 15,
            start_y: 10,
            prev_col: 15,
            prev_row: 10,
            prev_time_ns: 0,
            detach_coordinate: None,
            snap_applied: false,
        });
        wm.drag_snap = Some((
            Some(window_key),
            InsertPosition::Right,
            Rect {
                x: 40,
                y: 0,
                width: 40,
                height: 24,
            },
        ));
        wm.drag_last_event = Some(Instant::now() - STALE_EVENT_OFFSET);

        wm.apply_drag_snap_if_pending();
        assert!(wm.mouse_capture.is_none());
        assert!(wm.drag_snap.is_none());
        assert!(
            wm.layout_contains(key),
            "snapped window should be in layout"
        );
    }

    #[test]
    fn power_profile_change_updates_value() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let _keys = make_keys(&mut wm, 100);
        assert_eq!(wm.power_profile, PowerProfile::PowerSaver);
        wm.set_power_profile(PowerProfile::Interactive);
        assert_eq!(wm.power_profile, PowerProfile::Interactive);
        wm.set_power_profile(PowerProfile::PowerSaver);
        assert_eq!(wm.power_profile, PowerProfile::PowerSaver);
    }

    #[test]
    fn dispatch_focused_event_skips_chrome_in_direct_mode_content_area() {
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use crate::layout::{LayoutNode, TilingLayout};

        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.set_panel_visible(false);
        wm.set_managed_layout(TilingLayout::new(LayoutNode::leaf(keys[1])));
        wm.managed_draw_order = vec![keys[1]];
        wm.z_order = vec![keys[1]];
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        wm.focus_app_window(keys[1]);

        let win_key = keys[1];
        wm.set_direct_mode(win_key, true);
        assert!(wm.direct_mode(win_key));

        // Click within the content area — should go to focused window's
        // callback, NOT be consumed by chrome (handle_managed_event skipped).
        let content = wm.region_for_key(win_key);
        let click = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: (content.x + 1) as u16,
            row: (content.y + 1) as u16,
            modifiers: KeyModifiers::NONE,
        });

        let result = wm.dispatch_focused_event(&click);
        // In direct mode, content clicks bypass chrome and reach the component.
        assert!(result.is_some(), "event must route to window component");
    }

    #[test]
    fn dispatch_focused_event_still_routes_header_d_click_in_direct_mode() {
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use crate::layout::{LayoutNode, TilingLayout};

        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.set_panel_visible(false);
        wm.set_managed_layout(TilingLayout::new(LayoutNode::leaf(keys[1])));
        wm.managed_draw_order = vec![keys[1]];
        wm.z_order = vec![keys[1]];
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        wm.focus_app_window(keys[1]);

        let win_key = keys[1];
        wm.set_direct_mode(win_key, true);

        // Header D button click — coordinates on the header, NOT in content area.
        let full_rect = wm.full_region_for_key(win_key);
        let outer_right = full_rect
            .x
            .saturating_add(i32::from(full_rect.width))
            .saturating_sub(1);
        let close_x = outer_right.saturating_sub(2);
        let max_x = close_x.saturating_sub(2);
        let min_x = max_x.saturating_sub(2);
        let kb_x = min_x.saturating_sub(2) as u16;
        let kb_y = full_rect.y.saturating_add(1) as u16; // header row
        assert_eq!(
            wm.decorator().hit_test(full_rect, kb_x, kb_y),
            crate::window::decorator::HeaderAction::ToggleDirectMode,
        );

        wm.hitbox_registry.register(
            HitTarget::ChromeHeader(win_key, crate::window::decorator::HeaderAction::Drag),
            full_rect,
        );

        assert!(wm.direct_mode(win_key), "direct mode enabled before click");

        // This click is on the header (not content area) — chrome should still
        // handle it despite direct mode being on.
        let click = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: kb_x,
            row: kb_y,
            modifiers: KeyModifiers::NONE,
        });

        let wm_click = crate::events::core_event_to_wm(&click).unwrap();
        let result = wm.dispatch_mouse(&wm_click);

        // The header D button click should be consumed by chrome, toggling direct_mode off.
        assert!(result, "header D click must be consumed by chrome");
        assert!(
            !wm.direct_mode(win_key),
            "header D click must toggle direct_mode off"
        );
    }

    #[test]
    fn dispatch_focused_event_still_drags_header_in_direct_mode() {
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use crate::layout::{LayoutNode, TilingLayout};
        use crate::window::FloatRectSpec;

        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.set_panel_visible(false);
        wm.set_managed_layout(TilingLayout::new(LayoutNode::leaf(keys[1])));
        wm.managed_draw_order = vec![keys[1]];
        wm.z_order = vec![keys[1]];
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        wm.focus_app_window(keys[1]);

        let win_key = keys[1];
        wm.set_direct_mode(win_key, true);
        assert!(wm.direct_mode(win_key));

        // Set a known floating rect so we can verify movement.
        wm.set_floating_rect(
            win_key,
            Some(FloatRectSpec::Absolute(crate::window::FloatRect {
                x: 0,
                y: 0,
                width: 40,
                height: 20,
            })),
        );

        // The window is now floating; floating_headers should contain its header.
        let header_rect = wm
            .floating_headers
            .iter()
            .find(|h| h.key == win_key)
            .expect("floating header should exist")
            .rect;

        wm.hitbox_registry.register(
            HitTarget::ChromeHeader(win_key, crate::window::decorator::HeaderAction::Drag),
            header_rect,
        );

        // Press on the header — should start a drag via chrome.
        let click_col = header_rect.x;
        let click_row = header_rect.y;
        let down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: click_col as u16,
            row: click_row as u16,
            modifiers: KeyModifiers::NONE,
        });

        let wm_down = crate::events::core_event_to_wm(&down).unwrap();
        let result_down = wm.dispatch_mouse(&wm_down);
        assert!(result_down, "down event must be consumed by chrome");
        assert!(wm.mouse_capture.is_some(), "drag must be in progress");

        // Now send a Drag event deep into the content area.
        let content = wm.region_for_key(win_key);
        let drag_col = (content.x + i32::from(content.width) / 2) as u16;
        let drag_row = (content.y + i32::from(content.height) / 2) as u16;
        let drag = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: drag_col,
            row: drag_row,
            modifiers: KeyModifiers::NONE,
        });

        let wm_drag = crate::events::core_event_to_wm(&drag).unwrap();
        let result_drag = wm.dispatch_mouse(&wm_drag);
        assert!(
            result_drag,
            "drag event in content area must be consumed by chrome"
        );

        // The window should have moved.
        let moved = match wm.floating_rect(win_key) {
            Some(FloatRectSpec::Absolute(fr)) => fr,
            _ => panic!("expected absolute floating rect"),
        };
        assert!(
            moved.y > 0 || moved.x > 0,
            "window must have moved during drag (moved: {:?}, start: (0,0))",
            (moved.x, moved.y)
        );

        // Release — should end the drag.
        let up = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Release(MouseButton::Left),
            column: drag_col,
            row: drag_row,
            modifiers: KeyModifiers::NONE,
        });
        let wm_up = crate::events::core_event_to_wm(&up).unwrap();
        let result_up = wm.dispatch_mouse(&wm_up);
        assert!(result_up, "up event must be consumed by chrome");
        assert!(wm.mouse_capture.is_none(), "drag must be finished after up");
    }

    #[test]
    fn dispatch_focused_event_normal_behavior_when_not_direct_mode() {
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use crate::layout::{LayoutNode, TilingLayout};

        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = make_keys(&mut wm, 100);
        wm.set_panel_visible(false);
        wm.set_managed_layout(TilingLayout::new(LayoutNode::leaf(keys[1])));
        wm.managed_draw_order = vec![keys[1]];
        wm.z_order = vec![keys[1]];
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        wm.focus_app_window(keys[1]);

        let win_key = keys[1];
        assert!(!wm.direct_mode(win_key), "direct mode is off");

        let content = wm.region_for_key(win_key);
        let click = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: (content.x + 1) as u16,
            row: (content.y + 1) as u16,
            modifiers: KeyModifiers::NONE,
        });

        // In normal mode, a click in the content area bypasses chrome.
        // Result is the NoopComponent's EventResult::Consumed.
        let result = wm.dispatch_focused_event(&click);
        assert!(result.is_some(), "event must route to window component");
    }

    #[test]
    fn drag_last_event_updated_on_drag_events() {
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        struct DummyComponent;
        impl crate::components::Component<TermWmAction> for DummyComponent {
            fn render(
                &mut self,
                _backend: &mut dyn term_wm_render::RenderBackend,
                _area: LayoutRect,
                _ctx: &crate::components::ComponentContext,
                _registry: &mut crate::hitbox_registry::HitboxRegistry,
            ) {
            }
            fn handle_events(
                &mut self,
                _event: &Event,
                _ctx: &crate::components::ComponentContext,
            ) -> crate::actions::EventResult<TermWmAction> {
                crate::actions::EventResult::Consumed
            }
            fn update(
                &mut self,
                _action: TermWmAction,
                _ctx: &crate::components::ComponentContext,
                _queue: &mut std::collections::VecDeque<(super::WindowKey, TermWmAction)>,
            ) {
            }
            fn destroy(&mut self) {}
        }

        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );

        let debug_key = wm.set_system_window(Box::new(DummyComponent));
        wm.set_panel_visible(false);
        wm.set_managed_layout(TilingLayout::new(LayoutNode::leaf(debug_key)));
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });

        let header_rect = wm
            .floating_headers
            .iter()
            .find(|h| h.key == debug_key)
            .expect("header should exist")
            .rect;

        wm.hitbox_registry.register(
            HitTarget::ChromeHeader(debug_key, crate::window::decorator::HeaderAction::Drag),
            header_rect,
        );

        let down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: header_rect.x as u16,
            row: header_rect.y as u16,
            modifiers: KeyModifiers::NONE,
        });
        let wm_down = crate::events::core_event_to_wm(&down).unwrap();
        assert!(wm.dispatch_mouse(&wm_down));
        assert!(wm.drag_last_event.is_some());

        wm.drag_last_event = Some(Instant::now() - STALE_EVENT_OFFSET);
        let drag = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: (header_rect.x + 5) as u16,
            row: header_rect.y as u16,
            modifiers: KeyModifiers::NONE,
        });
        let wm_drag = crate::events::core_event_to_wm(&drag).unwrap();
        assert!(wm.dispatch_mouse(&wm_drag));
        if let Some(last) = wm.drag_last_event {
            assert!(
                last.elapsed() < SHORT_SNAP_TIMEOUT,
                "drag should refresh drag_last_event"
            );
        } else {
            panic!("drag_last_event should be set after drag");
        }
    }

    // ── is_valid_transition (pure model) ──────────────────────────────

    #[test]
    fn is_valid_transition_accepts_all_legal_pairs() {
        use crate::window::entry::WindowState::*;
        let legal = &[
            (Realized, Mapped),
            (Realized, Unmapped),
            (Mapped, Iconic),
            (Iconic, Mapped),
            (Mapped, Unmapped),
            (Unmapped, Mapped),
            (Mapped, Shaded),
            (Shaded, Mapped),
        ];
        for &(old, new) in legal {
            assert!(
                WindowManager::is_valid_transition(old, new),
                "legal transition {:?} -> {:?} should be valid",
                old,
                new,
            );
        }
    }

    #[test]
    fn is_valid_transition_rejects_all_illegal_pairs() {
        use crate::window::entry::WindowState::*;
        let states = [Realized, Mapped, Unmapped, Iconic, Shaded];
        let legal = &[
            (Realized, Mapped),
            (Realized, Unmapped),
            (Mapped, Iconic),
            (Iconic, Mapped),
            (Mapped, Unmapped),
            (Unmapped, Mapped),
            (Mapped, Shaded),
            (Shaded, Mapped),
        ];
        for &old in &states {
            for &new in &states {
                let is_legal = legal.contains(&(old, new));
                assert_eq!(
                    WindowManager::is_valid_transition(old, new),
                    is_legal,
                    "transition {:?} -> {:?} validity mismatch",
                    old,
                    new,
                );
            }
        }
    }

    // ── transition_window (side-effect assertions) ────────────────────

    #[test]
    fn transition_window_realized_to_mapped() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let key = wm.create_window(Box::new(crate::components::NoopComponent));
        assert_eq!(wm.window_state(key), Some(WindowState::Realized));

        wm.transition_window(key, WindowState::Mapped);
        assert_eq!(wm.window_state(key), Some(WindowState::Mapped));
        assert!(wm.z_order.contains(&key), "must be in z_order");
        assert!(
            wm.managed_draw_order.contains(&key),
            "must be in managed_draw_order"
        );
        assert_eq!(*wm.focus.current(), key, "must become focused");
    }

    fn mapped_keys(wm: &mut WindowManager, n: usize) -> Vec<WindowKey> {
        let raw = make_keys(wm, n);
        for &k in &raw {
            wm.transition_window(k, WindowState::Mapped);
        }
        raw
    }

    #[test]
    fn transition_window_mapped_to_iconic_removes_from_z_order() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = mapped_keys(&mut wm, 100);
        let target = keys[1];
        let focus = keys[0];
        wm.focus_window_key(focus);

        wm.transition_window(target, WindowState::Iconic);
        assert_eq!(wm.window_state(target), Some(WindowState::Iconic));
        assert!(!wm.z_order.contains(&target), "removed from z_order");
        assert!(
            !wm.managed_draw_order.contains(&target),
            "removed from draw order"
        );
        assert_eq!(*wm.focus.current(), focus, "focus unchanged");
    }

    #[test]
    fn transition_window_iconic_to_mapped_restores_z_order() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = mapped_keys(&mut wm, 100);
        let target = keys[1];
        wm.transition_window(target, WindowState::Iconic);
        assert!(!wm.z_order.contains(&target), "was removed");

        wm.transition_window(target, WindowState::Mapped);
        assert_eq!(wm.window_state(target), Some(WindowState::Mapped));
        assert!(wm.z_order.contains(&target), "restored to z_order");
        assert!(
            wm.managed_draw_order.contains(&target),
            "restored to draw order"
        );
    }

    #[test]
    fn transition_window_mapped_to_unmapped_cleans_up() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = mapped_keys(&mut wm, 100);
        let target = keys[1];
        wm.focus_window_key(target);
        wm.set_floating_rect(
            target,
            Some(crate::window::FloatRectSpec::Absolute(
                crate::window::FloatRect {
                    x: 5,
                    y: 5,
                    width: 10,
                    height: 10,
                },
            )),
        );
        assert!(wm.floating_rect(target).is_some(), "was floating");

        wm.transition_window(target, WindowState::Unmapped);
        assert_eq!(wm.window_state(target), Some(WindowState::Unmapped));
        assert!(!wm.z_order.contains(&target), "removed from z_order");
        assert!(
            wm.window(target).is_some_and(|w| w.floating_rect.is_none()),
            "floating rect cleared"
        );
        // Focus ring auto-fallbacks via set_order removing current → first()
        assert!(
            *wm.focus.current() != target,
            "focus must move away from unmapped window"
        );
    }

    // ── close_window ──────────────────────────────────────────────────

    #[test]
    fn close_window_cleans_up_layout_and_state() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = mapped_keys(&mut wm, 100);
        let target = keys[1];
        wm.focus_window_key(target);
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });

        wm.close_window(target);
        // For a non-system window, close_window destroys the component
        // and removes it from the SlotMap immediately.
        assert_eq!(
            wm.window_state(target),
            None,
            "window removed from SlotMap by close_window"
        );
        assert!(
            wm.closed_windows.is_empty(),
            "non-system windows are not queued"
        );
        assert!(!wm.z_order.contains(&target), "not in z_order");
        assert!(
            !wm.managed_draw_order.contains(&target),
            "not in draw order"
        );
    }

    #[test]
    fn close_window_system_window_keeps_slotmap_entry() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        struct Dummy;
        impl crate::components::Component<TermWmAction> for Dummy {
            fn render(
                &mut self,
                _backend: &mut dyn term_wm_render::RenderBackend,
                _area: Rect,
                _ctx: &crate::components::ComponentContext,
                _registry: &mut crate::hitbox_registry::HitboxRegistry,
            ) {
            }
            fn handle_events(
                &mut self,
                _event: &Event,
                _ctx: &crate::components::ComponentContext,
            ) -> crate::actions::EventResult<TermWmAction> {
                crate::actions::EventResult::Consumed
            }
            fn update(
                &mut self,
                _action: TermWmAction,
                _ctx: &crate::components::ComponentContext,
                _queue: &mut std::collections::VecDeque<(super::WindowKey, TermWmAction)>,
            ) {
            }
            fn destroy(&mut self) {}
        }
        let key = wm.set_system_window(Box::new(Dummy));
        wm.transition_window(key, WindowState::Mapped);
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });

        wm.close_window(key);
        assert!(
            wm.window(key).is_some(),
            "SlotMap entry preserved for system window"
        );
        assert_eq!(
            wm.window_state(key),
            Some(WindowState::Unmapped),
            "state is Unmapped"
        );
    }

    // ── shade_window / unshade_window ─────────────────────────────────

    #[test]
    fn shade_and_unshade_window() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = mapped_keys(&mut wm, 100);
        let target = keys[1];

        // Before shade: state is Mapped, in z_order
        assert_eq!(wm.window_state(target), Some(WindowState::Mapped));
        assert!(wm.z_order.contains(&target), "in z_order before shade");

        wm.shade_window(target);
        assert_eq!(
            wm.window_state(target),
            Some(WindowState::Shaded),
            "state is Shaded after shade_window"
        );
        assert!(
            wm.z_order.contains(&target),
            "still in z_order (chrome visible)"
        );

        wm.unshade_window(target);
        assert_eq!(
            wm.window_state(target),
            Some(WindowState::Mapped),
            "state is Mapped after unshade_window"
        );
        assert!(
            wm.z_order.contains(&target),
            "still in z_order after unshade"
        );
    }

    #[test]
    fn shade_is_idempotent() {
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        let keys = mapped_keys(&mut wm, 100);
        let target = keys[1];
        wm.shade_window(target);
        wm.shade_window(target); // second shade is a no-op
        assert_eq!(
            wm.window_state(target),
            Some(WindowState::Shaded),
            "still Shaded after double shade"
        );
    }

    // ── dispatch_focused_event mouse routing ──────────────────────────

    #[test]
    fn dispatch_focused_event_routes_mouse_to_selection_component() {
        use crate::actions::EventResult;
        use crate::components::{Component, ComponentContext, SelectionStatus};
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        struct SelComponent {
            enabled: bool,
            received_down: bool,
        }
        impl Component<TermWmAction> for SelComponent {
            fn render(
                &mut self,
                _f: &mut dyn term_wm_render::RenderBackend,
                _a: Rect,
                _c: &ComponentContext,
                _registry: &mut crate::hitbox_registry::HitboxRegistry,
            ) {
            }
            fn on_mouse(
                &mut self,
                mouse: &crate::events::LocalMouseEvent,
                ctx: &ComponentContext,
            ) -> EventResult<TermWmAction> {
                if !ctx.direct_mode()
                    && self.enabled
                    && matches!(mouse.kind, MouseEventKind::Press(_))
                {
                    self.received_down = true;
                    return EventResult::Consumed;
                }
                EventResult::Ignored
            }
            fn selection_status(&self) -> SelectionStatus {
                SelectionStatus {
                    active: self.received_down,
                    dragging: false,
                }
            }
            fn set_selection_enabled(&mut self, enabled: bool) {
                self.enabled = enabled;
            }
        }

        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        wm.set_panel_visible(false);

        let comp = SelComponent {
            enabled: true,
            received_down: false,
        };
        let key = wm.create_window(Box::new(comp));
        wm.set_managed_layout(TilingLayout::new(LayoutNode::leaf(key)));
        wm.managed_draw_order = vec![key];
        wm.z_order = vec![key];
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        wm.focus_app_window(key);
        assert!(!wm.direct_mode(key));

        // Click inside the content area
        let content = wm.region_for_key(key);
        let click = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: (content.x + 1) as u16,
            row: (content.y + 1) as u16,
            modifiers: KeyModifiers::NONE,
        });
        let result = wm.dispatch_focused_event(&click);
        assert!(result.is_some(), "event must route to window component");

        // Verify the component received the event
        let comp = wm
            .component_for_key_mut(key)
            .and_then(|c| crate::components::component_downcast_mut::<SelComponent>(c))
            .expect("component must be SelComponent");
        assert!(comp.received_down, "component must receive mouse Down");
        assert!(comp.enabled, "selection_enabled must persist");
    }

    /// Phase 4 (Press events) must call `process_action` so `MouseToBytes` reaches `update()`.
    #[test]
    fn phase4_down_dispatches_mouse_action_to_update() {
        use crate::actions::EventResult;
        use crate::components::{Component, ComponentContext};
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use std::collections::VecDeque;

        struct ActionRecorder {
            received_mouse_bytes: bool,
        }
        impl Component<TermWmAction> for ActionRecorder {
            fn render(
                &mut self,
                _f: &mut dyn term_wm_render::RenderBackend,
                _a: Rect,
                _c: &ComponentContext,
                _registry: &mut crate::hitbox_registry::HitboxRegistry,
            ) {
            }
            fn on_mouse(
                &mut self,
                mouse: &crate::events::LocalMouseEvent,
                _ctx: &ComponentContext,
            ) -> EventResult<TermWmAction> {
                EventResult::Action(TermWmAction::MouseToBytes(vec![
                    mouse.col as u8,
                    mouse.row as u8,
                ]))
            }
            fn on_key(
                &mut self,
                _event: &Event,
                _ctx: &ComponentContext,
            ) -> EventResult<TermWmAction> {
                EventResult::Ignored
            }
            fn update(
                &mut self,
                action: TermWmAction,
                _ctx: &ComponentContext,
                _queue: &mut VecDeque<(WindowKey, TermWmAction)>,
            ) {
                if matches!(action, TermWmAction::MouseToBytes(_)) {
                    self.received_mouse_bytes = true;
                }
            }
        }

        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        wm.set_panel_visible(false);

        let key = wm.create_window(Box::new(ActionRecorder {
            received_mouse_bytes: false,
        }));
        wm.transition_window(key, WindowState::Mapped);
        wm.set_managed_layout(TilingLayout::new(LayoutNode::leaf(key)));
        wm.managed_draw_order = vec![key];
        wm.z_order = vec![key];
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        wm.focus_app_window(key);

        // Register a Window hitbox at (10,5)-(30,15)
        let hit_rect = Rect {
            x: 10,
            y: 5,
            width: 20,
            height: 10,
        };
        wm.hitbox_registry
            .register(HitTarget::Window(key), hit_rect);

        // Send Down at (15, 8) — inside the hitbox
        let down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 15,
            row: 8,
            modifiers: KeyModifiers::NONE,
        });
        let wm_down = crate::events::core_event_to_wm(&down).unwrap();
        wm.dispatch_mouse(&wm_down);

        // Verify the action reached update()
        let comp = wm
            .component_for_key_mut(key)
            .and_then(|c| crate::components::component_downcast_mut::<ActionRecorder>(c))
            .expect("component must be ActionRecorder");
        assert!(
            comp.received_mouse_bytes,
            "Phase 4 Down must process MouseToBytes action via process_action"
        );
    }

    /// Phase 3 (Moved events without active capture) must call `process_action`.
    #[test]
    fn phase3_moved_dispatches_mouse_action_to_update() {
        use crate::actions::EventResult;
        use crate::components::{Component, ComponentContext};
        use crate::events::{Event, KeyModifiers, MouseEvent, MouseEventKind};
        use std::collections::VecDeque;

        struct ActionRecorder {
            received_mouse_bytes: bool,
        }
        impl Component<TermWmAction> for ActionRecorder {
            fn render(
                &mut self,
                _f: &mut dyn term_wm_render::RenderBackend,
                _a: Rect,
                _c: &ComponentContext,
                _registry: &mut crate::hitbox_registry::HitboxRegistry,
            ) {
            }
            fn on_mouse(
                &mut self,
                mouse: &crate::events::LocalMouseEvent,
                _ctx: &ComponentContext,
            ) -> EventResult<TermWmAction> {
                EventResult::Action(TermWmAction::MouseToBytes(vec![
                    mouse.col as u8,
                    mouse.row as u8,
                ]))
            }
            fn on_key(
                &mut self,
                _event: &Event,
                _ctx: &ComponentContext,
            ) -> EventResult<TermWmAction> {
                EventResult::Ignored
            }
            fn update(
                &mut self,
                action: TermWmAction,
                _ctx: &ComponentContext,
                _queue: &mut VecDeque<(WindowKey, TermWmAction)>,
            ) {
                if matches!(action, TermWmAction::MouseToBytes(_)) {
                    self.received_mouse_bytes = true;
                }
            }
        }

        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        wm.set_panel_visible(false);

        let key = wm.create_window(Box::new(ActionRecorder {
            received_mouse_bytes: false,
        }));
        wm.transition_window(key, WindowState::Mapped);
        wm.set_managed_layout(TilingLayout::new(LayoutNode::leaf(key)));
        wm.managed_draw_order = vec![key];
        wm.z_order = vec![key];
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        wm.focus_app_window(key);

        let hit_rect = Rect {
            x: 10,
            y: 5,
            width: 20,
            height: 10,
        };
        wm.hitbox_registry
            .register(HitTarget::Window(key), hit_rect);

        // Send Moved at (15, 8) — no active capture, so Phase 3 runs
        let moved = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 15,
            row: 8,
            modifiers: KeyModifiers::NONE,
        });
        let wm_moved = crate::events::core_event_to_wm(&moved).unwrap();
        wm.dispatch_mouse(&wm_moved);

        let comp = wm
            .component_for_key_mut(key)
            .and_then(|c| crate::components::component_downcast_mut::<ActionRecorder>(c))
            .expect("component must be ActionRecorder");
        assert!(
            comp.received_mouse_bytes,
            "Phase 3 Moved must process MouseToBytes action via process_action"
        );
    }

    // ── LayoutHandle split-resize tests ────────────────────────────────

    /// Helper: create a WindowManager with a 2-window horizontal tiling layout.
    /// Returns (wm, keys, gap_col, gap_row) where gap is the center of the split handle.
    fn setup_tiling_with_gap() -> (WindowManager, Vec<WindowKey>, u16, u16) {
        let mut wm = WindowManager::with_config(
            crate::wm_config::WmConfig::standalone(),
            std::sync::Arc::new(crate::app_context::AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        wm.set_panel_visible(false);
        let keys = make_keys(&mut wm, 100);
        let split = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::Leaf(keys[0]), LayoutNode::Leaf(keys[1])],
            weights: vec![1.0, 1.0],
            constraints: vec![Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)],
            resizable: true,
        };
        wm.set_managed_layout(TilingLayout::new(split));
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        // Manually register hitboxes (tests bypass the render pipeline).
        let handle_rects: Vec<_> = wm.handles.iter().map(|h| h.rect).collect();
        for rect in handle_rects {
            wm.hitbox_registry
                .register(crate::hitbox_registry::HitTarget::LayoutHandle, rect);
        }
        let handles = wm.handles.clone();
        assert!(!handles.is_empty(), "tiling must produce split handles");
        let gap = handles[0].rect;
        let gap_col = (gap.x + i32::from(gap.width) / 2) as u16;
        let gap_row = (gap.y + i32::from(gap.height) / 2) as u16;
        (wm, keys, gap_col, gap_row)
    }

    #[test]
    fn layout_handle_down_sets_capture_state() {
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let (mut wm, _keys, gap_col, gap_row) = setup_tiling_with_gap();
        let down = crate::events::core_event_to_wm(&Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: gap_col,
            row: gap_row,
            modifiers: KeyModifiers::NONE,
        }))
        .unwrap();
        wm.dispatch_mouse(&down);
        assert!(
            matches!(wm.mouse_capture, Some(MouseCaptureState::LayoutHandle)),
            "Down on split handle must set LayoutHandle capture"
        );
    }

    #[test]
    fn layout_handle_drag_adjusts_split() {
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let (mut wm, keys, gap_col, gap_row) = setup_tiling_with_gap();
        let region_before_left = wm.region(keys[0]);
        let region_before_right = wm.region(keys[1]);

        // Down at gap
        let down = crate::events::core_event_to_wm(&Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: gap_col,
            row: gap_row,
            modifiers: KeyModifiers::NONE,
        }))
        .unwrap();
        wm.dispatch_mouse(&down);

        // Drag right by 5 columns
        let drag = crate::events::core_event_to_wm(&Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: gap_col + 5,
            row: gap_row,
            modifiers: KeyModifiers::NONE,
        }))
        .unwrap();
        wm.dispatch_mouse(&drag);

        // Up to release
        let up = crate::events::core_event_to_wm(&Event::Mouse(MouseEvent {
            kind: MouseEventKind::Release(MouseButton::Left),
            column: gap_col + 5,
            row: gap_row,
            modifiers: KeyModifiers::NONE,
        }))
        .unwrap();
        wm.dispatch_mouse(&up);

        // Re-register layout to get updated regions
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        let region_after_left = wm.region(keys[0]);
        let region_after_right = wm.region(keys[1]);

        assert!(
            region_after_left.width > region_before_left.width,
            "left window must grow after dragging split right: {} -> {}",
            region_before_left.width,
            region_after_left.width
        );
        assert!(
            region_after_right.width < region_before_right.width,
            "right window must shrink after dragging split right: {} -> {}",
            region_before_right.width,
            region_after_right.width
        );
    }

    #[test]
    fn layout_handle_up_clears_capture() {
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let (mut wm, _keys, gap_col, gap_row) = setup_tiling_with_gap();

        // Down
        let down = crate::events::core_event_to_wm(&Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: gap_col,
            row: gap_row,
            modifiers: KeyModifiers::NONE,
        }))
        .unwrap();
        wm.dispatch_mouse(&down);
        assert!(matches!(
            wm.mouse_capture,
            Some(MouseCaptureState::LayoutHandle)
        ));

        // Up
        let up = crate::events::core_event_to_wm(&Event::Mouse(MouseEvent {
            kind: MouseEventKind::Release(MouseButton::Left),
            column: gap_col,
            row: gap_row,
            modifiers: KeyModifiers::NONE,
        }))
        .unwrap();
        wm.dispatch_mouse(&up);
        assert!(wm.mouse_capture.is_none(), "Up must clear capture state");
    }

    #[test]
    fn layout_handle_moved_updates_hover() {
        use crate::events::{Event, KeyModifiers, MouseEvent, MouseEventKind};
        let (mut wm, _keys, gap_col, gap_row) = setup_tiling_with_gap();

        // Moved over the gap (no Down — Moved is only emitted without buttons pressed).
        let moved = crate::events::core_event_to_wm(&Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: gap_col,
            row: gap_row,
            modifiers: KeyModifiers::NONE,
        }))
        .unwrap();
        wm.dispatch_mouse(&moved);

        // Verify hover was updated by checking that hovered_handle finds the gap.
        let layout = wm.managed_layout.as_ref().unwrap();
        let hovered = layout.hovered_handle(wm.managed_area);
        assert!(
            hovered.is_some(),
            "Moved over split handle must update hover so hovered_handle returns a handle"
        );
    }

    #[test]
    fn register_layout_handle_hitboxes_registers_entries() {
        use crate::layout::Constraint;
        let mut wm = WindowManager::with_config(
            crate::wm_config::WmConfig::standalone(),
            std::sync::Arc::new(crate::app_context::AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
        );
        wm.set_panel_visible(false);
        let keys = make_keys(&mut wm, 100);
        let split = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::Leaf(keys[0]), LayoutNode::Leaf(keys[1])],
            weights: vec![1.0, 1.0],
            constraints: vec![Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)],
            resizable: true,
        };
        wm.set_managed_layout(TilingLayout::new(split));
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });

        let mut reg = crate::hitbox_registry::HitboxRegistry::new();
        wm.register_layout_handle_hitboxes(&mut reg);

        // Verify at least one LayoutHandle entry exists at the gap position.
        let gap = &wm.handles[0].rect;
        let pos = crate::mouse_coord::MousePosition {
            column: (gap.x + i32::from(gap.width) / 2) as i16,
            row: (gap.y + i32::from(gap.height) / 2) as i16,
            space: crate::mouse_coord::CoordSpace::Screen,
        };
        let hit = reg.hit_test(pos);
        assert!(
            hit.is_some_and(|(target, _)| matches!(
                target,
                crate::hitbox_registry::HitTarget::LayoutHandle
            )),
            "registry must contain LayoutHandle at split gap"
        );
    }
}
