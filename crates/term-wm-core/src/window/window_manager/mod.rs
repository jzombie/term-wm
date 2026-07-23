mod chrome;
mod command_menu;
mod drag;
mod focus;
pub(crate) mod layer_manager;
mod layout;
mod overlays;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::Rect;
use crate::events::{Event, MouseEvent, MouseEventKind};
use slotmap::SlotMap;

use super::ComponentKey;
use super::OverlayKey;
use super::WindowKey;
use super::entry::{Window, WindowState};
use crate::actions::{EventResult, SystemTask, TermWmAction};
use crate::app_context::AppContext;
use crate::components::{
    Component, ComponentAction, ComponentContext, MenuItem, Overlay, WmComponent,
};
use crate::hitbox_registry::{ComponentOwner, HitboxRegistry};
use crate::keybindings::KeyBindings;
use crate::layout::floating::*;
use crate::layout::{InsertPosition, LayoutNode, RegionMap, SplitHandle, TilingLayout};
use crate::notification::NotificationQueue;
use crate::power_profile::PowerProfile;
use crate::reaper::Reaper;
use crate::task_scheduler::{TaskHandle, TaskId};
#[cfg(test)]
use crate::window::test_component::TestComponent;
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
    ComponentInteraction {
        /// The component that owns this interaction. Direct dispatch —
        /// no layer array scan or equality checks needed.
        owner: ComponentOwner,
        /// Immutable snapshot of the hitbox area at Press time.
        screen_area: LayoutRect,
        /// The exact HitboxId of the component that initiated the capture.
        hitbox_id: crate::hitbox_registry::HitboxId,
    },
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

/// Cached snap preview state to avoid recalculating the full
/// position-based layout on every drag frame.
/// Invalidates when the cursor exits the cached rect or crosses
/// a quadrant boundary.
struct SnapPreviewCache {
    positions: Vec<(WindowKey, Rect)>,
    cached_rect: Option<Rect>,
    cached_quadrant: Option<(bool, bool)>,
    dragged_key: Option<WindowKey>,
}

impl SnapPreviewCache {
    fn needs_recalc(&self, mouse_x: u16, mouse_y: u16, dragged: WindowKey) -> bool {
        let Some(rect) = self.cached_rect else {
            return true;
        };
        if self.dragged_key != Some(dragged) {
            return true;
        }
        let mx = mouse_x as i32;
        let my = mouse_y as i32;
        let right = rect.x.saturating_add(i32::from(rect.width));
        let bottom = rect.y.saturating_add(i32::from(rect.height));
        let out_of_bounds = mx < rect.x || mx >= right || my < rect.y || my >= bottom;
        let cur_q = (
            mx >= rect.x.saturating_add(i32::from(rect.width) / 2),
            my >= rect.y.saturating_add(i32::from(rect.height) / 2),
        );
        out_of_bounds || self.cached_quadrant != Some(cur_q)
    }

    fn update(
        &mut self,
        mouse_x: u16,
        mouse_y: u16,
        rect: Rect,
        dragged: WindowKey,
        pos: Vec<(WindowKey, Rect)>,
    ) {
        let mx = mouse_x as i32;
        let my = mouse_y as i32;
        self.cached_quadrant = Some((
            mx >= rect.x.saturating_add(i32::from(rect.width) / 2),
            my >= rect.y.saturating_add(i32::from(rect.height) / 2),
        ));
        self.cached_rect = Some(rect);
        self.dragged_key = Some(dragged);
        self.positions = pos;
    }

    fn clear(&mut self) {
        self.cached_rect = None;
        self.cached_quadrant = None;
        self.dragged_key = None;
        self.positions.clear();
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

/// Monocle display mode. Cycling: Auto → On → Off → Auto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonocleMode {
    Auto,
    On,
    Off,
}

impl MonocleMode {
    pub fn cycle(self) -> Self {
        match self {
            MonocleMode::Auto => MonocleMode::On,
            MonocleMode::On => MonocleMode::Off,
            MonocleMode::Off => MonocleMode::Auto,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MonocleMode::Auto => "Auto",
            MonocleMode::On => "On",
            MonocleMode::Off => "Off",
        }
    }
}

pub struct WindowManager<
    C: Component<TermWmAction>,
    L: WmComponent = crate::components::NoopWmComponent,
    O: Overlay<TermWmAction> = crate::components::NoopOverlay,
> {
    #[allow(dead_code)]
    focus: FocusRing<WindowKey>,
    #[allow(dead_code)]
    macro_focus: layer_manager::MacroFocus,
    pub(crate) layer_manager: layer_manager::LayerManager<L>,
    windows: SlotMap<WindowKey, Window>,
    /// Dense arena storing all window-root components inline.
    components: SlotMap<ComponentKey, C>,
    pub(crate) regions: RegionMap<WindowKey>,
    scroll: BTreeMap<WindowKey, ScrollState>,
    pub(crate) handles: Vec<SplitHandle>,
    pub(crate) managed_draw_order: Vec<WindowKey>,
    pub(crate) managed_layout: Option<TilingLayout<WindowKey>>,
    closed_windows: Vec<WindowKey>,
    pub(crate) managed_area: Rect,
    pub(crate) monocle_mode: MonocleMode,
    monocle_width_threshold: u16,
    last_terminal_width: u16,
    pub(crate) hitbox_registry: HitboxRegistry,
    app_ctx: Arc<AppContext>,
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
    selection_active: bool,
    selection_dragging: bool,
    selection_text: Option<String>,
    config: WmConfig,
    hint_visibility: HintVisibility,
    command_menu_opened_at: Option<Instant>,
    /// ID of the drag-snap timer in the TaskScheduler, for cancellation.
    drag_timer_id: Option<TaskId>,
    /// ID of the temporal-dwell tick timer, for cancellation and guard.
    temporal_timer_id: Option<TaskId>,
    /// Handle to the shared `TaskScheduler<SystemTask>` for registering/cancelling
    /// system-level timers (super-passthrough, drag-snap).
    system_task_handle: Option<TaskHandle<SystemTask>>,
    pub(crate) last_frame_area: LayoutRect,
    overlays: SlotMap<OverlayKey, O>,
    help_key: Option<OverlayKey>,
    exit_confirm_key: Option<OverlayKey>,
    command_palette_key: Option<OverlayKey>,
    /// When `Some(instant)`, tab outline mode is active until that instant.
    pub(crate) tab_outline_until: Option<Instant>,
    scroll_keyboard_enabled_default: bool,
    floating_resize_offscreen: bool,
    pub(crate) z_order: Vec<WindowKey>,
    pub(crate) drag_snap: Option<(Option<WindowKey>, InsertPosition, Rect)>,
    /// Active snap preview state for ghost window rendering during drag.
    pub(crate) snap_preview: Option<SnapPreviewState>,
    /// Cache for BSP dry-run projection to avoid deep-cloning the layout
    /// tree on every drag frame. Keyed by (target, position, area).
    snap_projection_cache: Option<(SnapPreviewState, Rect, Option<Rect>)>,
    snap_preview_cache: SnapPreviewCache,
    drag_last_event: Option<Instant>,
    next_window_seq: usize,
    next_title_seq: usize,
    synthetic_event: Option<Event>,
    clipboard: Option<crate::clipboard::Clipboard>,
    power_profile: PowerProfile,
    pub(crate) reaper: Reaper,
    quit_requested: bool,
    /// Flag indicating the layout has changed and needs re-projection
    layout_dirty: bool,
    /// Active toast notifications
    notification_queue: NotificationQueue,
    /// Universal input mode state machine
    pub(crate) input_mode: crate::actions::WmInputMode,
    /// Whether the FAB component is enabled
    pub(crate) fab_enabled: bool,
    /// Namespaced semantic registry for programmatic component lookup.
    /// Hotkeys and structural routing query this instead of hardcoded fields.
    pub semantic_registry: HashMap<layer_manager::ComponentTag, layer_manager::LayerId>,
    /// Tap-swap targeting state
    pub(crate) tap_swap_state: Option<TapSwapState>,
    // Chrome metrics managers (pure synchronous pipelines, zero allocation).
    // resize_map/drag_map/split_ids removed — chrome routing now uses
    // ComponentOwner::Chrome(target) directly from HitboxRegistry.
}

/// State for tap-to-swap targeting mode.
#[derive(Debug, Clone)]
pub(crate) struct TapSwapState {
    /// The source window being moved
    pub source_key: WindowKey,
    /// The target window to swap with (highlighted)
    pub target_key: Option<WindowKey>,
}

impl<C: Component<TermWmAction>, L: WmComponent, O: Overlay<TermWmAction>> WindowManager<C, L, O> {
    /// Allocate a new window entry in the SlotMap and return its key.
    /// The window starts with default state (no title, not floating, etc.).
    pub fn create_window(&mut self, component: C) -> WindowKey {
        let order = self.next_window_seq;
        self.next_window_seq = self.next_window_seq.saturating_add(1);
        let component_key = self.components.insert(component);
        tracing::debug!(seq = order, "opened window");
        self.windows.insert(Window::new(order, component_key))
    }

    /// Register a component and invoke its `on_mount` hook with the assigned key.
    pub fn spawn(&mut self, component: C) -> WindowKey {
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

    pub fn set_floating_rect(
        &mut self,
        key: WindowKey,
        rect: Option<crate::window::FloatRectSpec>,
    ) {
        if let Some(w) = self.windows.get_mut(key) {
            w.floating_rect = rect;
        }
    }

    fn clear_floating_rect(&mut self, key: WindowKey) {
        if let Some(w) = self.windows.get_mut(key) {
            w.floating_rect = None;
            w.is_maximized = false;
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

    /// Compute a centered, cascading fallback rectangle for unrendered floating
    /// windows using core workspace constants.
    pub fn default_cascading_rect(&self, index: usize) -> Rect {
        let area = self.managed_area;
        let w = crate::constants::DEFAULT_FLOAT_WIDTH
            .min(area.width)
            .max(crate::constants::MIN_FLOAT_WIDTH);
        let h = crate::constants::DEFAULT_FLOAT_HEIGHT
            .min(area.height)
            .max(crate::constants::MIN_FLOAT_HEIGHT);
        let offset = (index as i32) * crate::constants::CASCADE_OFFSET_STEP;
        Rect {
            x: area.x + (area.width.saturating_sub(w) / 2) as i32 + offset,
            y: area.y + (area.height.saturating_sub(h) / 2) as i32 + offset,
            width: w,
            height: h,
        }
    }

    /// Return the cached region for a window, or compute a safe cascading fallback.
    pub fn region_or_fallback(&self, key: WindowKey, index: usize) -> Rect {
        self.regions
            .get(key)
            .unwrap_or_else(|| self.default_cascading_rect(index))
    }

    pub fn direct_mode(&self, key: WindowKey) -> bool {
        self.window(key).is_some_and(|window| window.direct_mode)
    }

    pub fn set_direct_mode(&mut self, key: WindowKey, value: bool) {
        if let Some(w) = self.windows.get_mut(key) {
            w.direct_mode = value;
            if value && let Some(c) = self.components.get_mut(w.component_key) {
                c.clear_selection();
            }
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

    pub fn with_config(
        config: WmConfig,
        app_ctx: Arc<AppContext>,
        supported_menu_actions: Option<Vec<TermWmAction>>,
        layer_manager: layer_manager::LayerManager<L>,
        semantic_registry: HashMap<layer_manager::ComponentTag, layer_manager::LayerId>,
    ) -> Self {
        let supported_menu_actions = supported_menu_actions.unwrap_or_else(|| {
            vec![
                TermWmAction::CloseMenu,
                TermWmAction::ToggleMouseCapture,
                TermWmAction::ToggleClipboardMode,
                TermWmAction::ToggleWindowSelection,
                TermWmAction::NewWindow,
                TermWmAction::ToggleDebugWindow,
                TermWmAction::ToggleSystemPanel,
                TermWmAction::Help,
                TermWmAction::ExitUi,
                TermWmAction::MaximizeWindow,
                TermWmAction::MinimizeWindow,
                TermWmAction::CloseWindow,
                TermWmAction::ToggleDirectMode,
                TermWmAction::ToggleMonocle,
                TermWmAction::ToggleTiling,
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
            macro_focus: layer_manager::MacroFocus::FocusRing(slotmap::DefaultKey::default()),
            layer_manager,
            windows: SlotMap::with_capacity(crate::constants::INITIAL_WINDOW_CAPACITY),
            components: SlotMap::<ComponentKey, C>::with_capacity_and_key(
                crate::constants::INITIAL_COMPONENT_CAPACITY,
            ),
            regions: RegionMap::default(),
            scroll: BTreeMap::new(),
            handles: Vec::new(),
            managed_draw_order: Vec::new(),
            managed_layout: None,
            closed_windows: Vec::new(),
            managed_area: Rect::default(),
            monocle_mode: MonocleMode::Auto,
            monocle_width_threshold: crate::constants::MONOCLE_WIDTH_THRESHOLD,
            last_terminal_width: 0,
            hitbox_registry: HitboxRegistry::new(),
            app_ctx,
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
            selection_active: false,
            selection_dragging: false,
            selection_text: None,
            window_selection_enabled: config.window_selection_enabled,
            window_selection_dirty: false,
            hint_visibility: config.hint_visibility,
            config,
            command_menu_opened_at: None,
            drag_timer_id: None,
            temporal_timer_id: None,
            system_task_handle: None,
            last_frame_area: Rect::default(),
            scroll_keyboard_enabled_default: true,
            floating_resize_offscreen,
            z_order: Vec::new(),
            drag_snap: None,
            snap_preview: None,
            snap_projection_cache: None,
            snap_preview_cache: SnapPreviewCache {
                positions: Vec::new(),
                cached_rect: None,
                cached_quadrant: None,
                dragged_key: None,
            },
            drag_last_event: None,
            next_window_seq: 0,
            next_title_seq: 0,
            synthetic_event: None,
            clipboard,
            power_profile: PowerProfile::PowerSaver,
            reaper: Reaper::default(),
            quit_requested: false,
            layout_dirty: true,
            notification_queue: NotificationQueue::default(),
            semantic_registry,
            overlays: SlotMap::with_key(),
            help_key: None,
            exit_confirm_key: None,
            command_palette_key: None,
            tab_outline_until: None,
            input_mode: crate::actions::WmInputMode::Passthrough,
            fab_enabled: true,
            tap_swap_state: None,
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

    /// Get the current input mode.
    pub fn input_mode(&self) -> crate::actions::WmInputMode {
        self.input_mode
    }

    /// Set the input mode.
    pub fn set_input_mode(&mut self, mode: crate::actions::WmInputMode) {
        self.input_mode = mode;
    }

    /// Check if the FAB is enabled.
    pub fn fab_enabled(&self) -> bool {
        self.fab_enabled
    }

    /// Set the FAB enabled state.
    pub fn set_fab_enabled(&mut self, enabled: bool) {
        self.fab_enabled = enabled;
    }

    /// Get a mutable reference to the FAB component.
    /// Uniform, open-ended component locator via semantic tag.
    /// Replaces all hardcoded `*_component_mut()` methods.
    pub fn get_semantic_component_mut(
        &mut self,
        tag: layer_manager::ComponentTag,
    ) -> Option<&mut L> {
        self.semantic_registry
            .get(&tag)
            .copied()
            .and_then(|id| self.layer_manager.get_mut(id))
    }

    /// Immutable component locator via semantic tag.
    pub fn get_semantic_component(&self, tag: layer_manager::ComponentTag) -> Option<&L> {
        self.semantic_registry
            .get(&tag)
            .copied()
            .and_then(|id| self.layer_manager.get(id))
    }

    /// Begin tap-to-swap targeting for the given window.
    pub fn begin_tap_swap(&mut self, source_key: WindowKey) {
        self.tap_swap_state = Some(TapSwapState {
            source_key,
            target_key: None,
        });
        self.input_mode = crate::actions::WmInputMode::TapToSwapTargeting;
    }

    /// Select a target window for tap-to-swap.
    pub fn select_tap_swap_target(&mut self, target_key: WindowKey) {
        if let Some(ref mut state) = self.tap_swap_state {
            state.target_key = Some(target_key);
        }
    }

    /// Execute the tap-to-swap operation.
    pub fn execute_tap_swap(&mut self, target_key: WindowKey) {
        if let Some(state) = self.tap_swap_state.take() {
            // Swap the nodes in the layout tree
            if let Some(ref mut layout) = self.managed_layout {
                layout.swap_nodes(&state.source_key, &target_key);
            }
            self.input_mode = crate::actions::WmInputMode::Passthrough;
            self.mark_layout_dirty();
        }
    }

    /// Cancel the tap-to-swap operation.
    pub fn cancel_tap_swap(&mut self) {
        self.tap_swap_state = None;
        self.input_mode = crate::actions::WmInputMode::Passthrough;
    }

    /// Check if tap-swap is active.
    pub fn tap_swap_active(&self) -> bool {
        self.tap_swap_state.is_some()
    }

    /// Get the tap-swap source key.
    pub fn tap_swap_source(&self) -> Option<WindowKey> {
        self.tap_swap_state.as_ref().map(|s| s.source_key)
    }

    /// Get the tap-swap target key.
    pub fn tap_swap_target(&self) -> Option<WindowKey> {
        self.tap_swap_state.as_ref().and_then(|s| s.target_key)
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
        let mut ctx = self
            .component_context(focused)
            .with_direct_mode(self.direct_mode(key))
            .with_window_key(key)
            .with_screen_area(self.region(key));
        // Inject the window's active keyboard focus into the context.
        if let Some(window) = self.windows.get(key)
            && let Some(focus_id) = window.active_keyboard_focus
        {
            ctx = ctx.with_keyboard_focus_id(focus_id);
        }
        ctx
    }

    /// Number of overlays that will be rendered this frame.
    pub fn visible_overlay_count(&self) -> usize {
        let mut n = 0usize;
        if self.command_menu_visible() {
            n += 1;
        }
        if self.exit_confirm_visible() {
            n += 1;
        }
        if self.help_overlay_visible() {
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
        if let Some(p) = self.get_semantic_component_mut(layer_manager::ComponentTag::TopPanel) {
            p.begin_frame();
        }
        let power_profile = self.power_profile;
        if let Some(p) = self.get_semantic_component_mut(layer_manager::ComponentTag::BottomPanel) {
            p.begin_frame();
            p.process_action(&ComponentAction::SetPowerProfile(power_profile));
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
        // Check if cursor is over any tiling split handle
        self.handles.iter().find(|h| pos.is_inside(h.rect)).cloned()
    }

    /// Return the currently hovered floating resize handle, if any.
    /// Queries the hitbox registry directly (console registers ChromeTarget::Resize
    /// hitboxes during the render pass).
    pub fn hovered_resize_handle(
        &self,
    ) -> Option<crate::layout::floating::ResizeHandle<WindowKey>> {
        let (column, row) = self.hover?;
        use crate::chrome::ChromeTarget;
        use crate::hitbox_registry::ComponentOwner;
        use crate::mouse_coord::{CoordSpace, MousePosition};
        let pos = MousePosition {
            column: column as i16,
            row: row as i16,
            space: CoordSpace::Screen,
        };
        if let Some((_, ComponentOwner::Chrome(ChromeTarget::Resize(key, edge)), area)) =
            self.hitbox_registry.hit_test(pos)
        {
            Some(crate::layout::floating::ResizeHandle {
                key,
                rect: area,
                edge,
                hitbox_id: crate::hitbox_registry::HitboxId::new(),
            })
        } else {
            None
        }
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

    /// Set the hover position (used in tests or to prime state before rendering).
    pub fn set_hover_pos(&mut self, col: u16, row: u16) {
        self.hover = Some((col, row));
    }

    /// Set the mouse capture state (used in tests to simulate active interaction).
    pub fn set_mouse_captured(&mut self, captured: bool) {
        self.mouse_capture = if captured {
            Some(MouseCaptureState::ComponentInteraction {
                owner: ComponentOwner::Test,
                screen_area: LayoutRect::default(),
                hitbox_id: crate::hitbox_registry::HitboxId::default(),
            })
        } else {
            None
        };
    }

    /// Return a mutable reference to the hitbox registry (for render pipeline).
    pub fn hitbox_registry_mut(&mut self) -> &mut crate::hitbox_registry::HitboxRegistry {
        &mut self.hitbox_registry
    }

    /// Return the persistent content hitbox ID for a window.
    pub fn window_content_hitbox_id(
        &self,
        key: WindowKey,
    ) -> Option<crate::hitbox_registry::HitboxId> {
        self.windows.get(key).map(|w| w.content_hitbox_id)
    }

    /// Dispatch a mouse event through the hitbox registry.
    ///
    /// Phases:
    ///   Phase 1 — Active capture: ongoing drag/resize/component-interaction
    ///   Phase 2 — Chrome hit-test (header, close button, etc.)
    ///   Phase 3 — Moved events: update hover and forward to component.
    ///   Phase 4 — Press events hit-test the registry.
    ///             dispatch to components on content hits.
    ///   Phase 3 — Unhandled events fall through to Ignored.
    ///
    /// Returns `EventResult<(Option<WindowKey>, TermWmAction)>` — the action
    /// and the exact spatial target key from the hit test.
    #[allow(clippy::collapsible_if)]
    pub fn dispatch_mouse(
        &mut self,
        event: &crate::events::WmEvent,
    ) -> EventResult<(Option<WindowKey>, TermWmAction)> {
        use crate::events::WmEvent;
        let WmEvent::Mouse {
            kind,
            modifiers,
            position,
        } = event
        else {
            return EventResult::Ignored;
        };
        let col = position.column as u16;
        let row = position.row as u16;
        // Record hover position on every mouse event — must happen before the
        // Phase-1 active-capture branch (which returns early for Drag events
        // during window resize/move) so that the cursor overlay always draws
        // at the current pointer location.
        self.hover = Some((col, row));

        // Phase 1 — Active capture: extract-operate-restore pattern.
        if !matches!(kind, MouseEventKind::Press(_)) {
            if let Some(mut capture) = self.mouse_capture.take() {
                // Stores an action generated during active capture (e.g., button
                // release inside a captured component) that must be returned to
                // the runner rather than swallowed.
                let mut capture_action: Option<(Option<WindowKey>, TermWmAction)> = None;
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
                                        // Reposition window so the cursor lands on
                                        // the header (row 1 of the frame), not the
                                        // top border.
                                        let new_x = col as i32 - fr.width as i32 / 2;
                                        let new_y = row as i32
                                            - i32::from(crate::chrome::TOP_BORDER_HEIGHT);
                                        self.set_floating_rect(
                                            *key,
                                            Some(crate::window::FloatRectSpec::Absolute(
                                                crate::window::FloatRect {
                                                    x: new_x,
                                                    y: new_y,
                                                    width: fr.width,
                                                    height: fr.height,
                                                },
                                            )),
                                        );
                                        *initial_x = new_x;
                                        *initial_y = new_y;
                                        *start_x = col;
                                        *start_y = row;
                                        *prev_col = col;
                                        *prev_row = row;
                                    }
                                }

                                if detach_coordinate.is_none() {
                                    // Defer setting detach_coordinate until after
                                    // update_snap_preview runs, so the first Drag
                                    // event is not suppressed.
                                }

                                self.drag_last_event = Some(Instant::now());
                                self.reset_drag_snap_timer();

                                // If the window is not yet floating, this is the
                                // first Drag event that breached the kinetic
                                // deadzone — install the floating rect now.
                                // start_x/start_y/initial_x/initial_y were
                                // already set at Press time and must NOT be
                                // touched here — they anchor the cursor-to-
                                // window offset for move_floating.
                                if !self.is_window_floating(*key) {
                                    let rect = self.visible_region_for_key(*key);
                                    self.set_floating_rect(
                                        *key,
                                        Some(crate::window::FloatRectSpec::Absolute(
                                            crate::window::FloatRect {
                                                x: rect.x,
                                                y: rect.y,
                                                width: rect.width.max(1),
                                                height: rect.height.max(1),
                                            },
                                        )),
                                    );
                                    self.bring_to_front_key(*key);
                                }

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
                                    // Set detach_coordinate AFTER snap preview runs
                                    if detach_coordinate.is_none() {
                                        *detach_coordinate = Some((col, row));
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
                                self.drag_snap = None;
                                self.snap_preview = None;
                            } else if self.drag_snap.is_some() {
                                // Snap target found — apply snap (removes from
                                // tiling tree and inserts at snap position)
                                self.apply_snap(*key);
                            } else if !*snap_applied {
                                // Only detach if the user actually dragged the
                                // window.
                                let drag_dist = col.abs_diff(*anchor_x) + row.abs_diff(*anchor_y);
                                if drag_dist > 0 {
                                    self.detach_from_tiling_layout(*key);
                                    self.execute_float_all(*key);
                                }
                            }
                            // else: snap was already applied by Moved handler — the
                            // window is correctly positioned in the tiling tree; do
                            // NOT detach it again.
                            self.snap_preview = None;
                            self.snap_projection_cache = None;
                            // Only clear the double-click timer if a drag actually
                            // occurred.  A click-only release preserves the timer so
                            // a subsequent click can still be detected as a
                            // double-click (toggle maximize).
                            let drag_dist = col.abs_diff(*anchor_x) + row.abs_diff(*anchor_y);
                            if drag_dist > 0 {
                                self.last_header_click = None;
                            }
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
                    MouseCaptureState::ComponentInteraction {
                        owner,
                        screen_area,
                        hitbox_id,
                    } => {
                        let core_event = Event::Mouse(MouseEvent {
                            kind: *kind,
                            column: col,
                            row,
                            modifiers: *modifiers,
                        });

                        let consumed = match owner {
                            ComponentOwner::Window(key) => {
                                if !self.windows.contains_key(*key) {
                                    false
                                } else {
                                    let focused = *self.focus.current() == *key;
                                    let ctx = self
                                        .component_context_for(focused, *key)
                                        .with_screen_area(*screen_area)
                                        .with_active_hitbox(*hitbox_id);
                                    if let Some(comp) = self.component_for_key_mut(*key) {
                                        let res = comp.handle_events(&core_event, &ctx);
                                        if let Some(action) = res.clone().into_action() {
                                            capture_action = Some((Some(*key), action));
                                        }
                                        !res.is_ignored()
                                    } else {
                                        false
                                    }
                                }
                            }
                            ComponentOwner::Overlay(key) => {
                                let ctx = self
                                    .component_context(false)
                                    .with_screen_area(*screen_area)
                                    .with_active_hitbox(*hitbox_id);
                                if let Some(overlay) = self.overlays.get_mut(*key) {
                                    let res = overlay.handle_events(&core_event, &ctx);
                                    if let Some(action) = res.clone().into_action() {
                                        capture_action = Some((None, action));
                                    }
                                    !res.is_ignored()
                                } else {
                                    false
                                }
                            }
                            ComponentOwner::Layer(id) => {
                                let ctx = self
                                    .component_context(false)
                                    .with_screen_area(*screen_area)
                                    .with_active_hitbox(*hitbox_id);
                                if let Some(layer_comp) = self.layer_manager.get_mut(*id) {
                                    let res = layer_comp.handle_events(&core_event, &ctx);
                                    if let Some(action) = res.clone().into_action() {
                                        capture_action = Some((None, action));
                                    }
                                    !res.is_ignored()
                                } else {
                                    false
                                }
                            }
                            ComponentOwner::Chrome(_) | ComponentOwner::Test => false,
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
                // If active capture produced an action (e.g., button release),
                // return it to the runner rather than swallowing it.
                if let Some((capture_key, capture_act)) = capture_action {
                    return EventResult::Action((capture_key, capture_act));
                }
                return if result {
                    EventResult::Consumed
                } else {
                    EventResult::Ignored
                };
            }
        }

        // Phase 2 — Unified hit-test dispatch: single pass, exhaustive match.
        //
        // After this point, NO event routing is based on content_hitbox_id
        // equality checks, layer iteration, or handle list scanning.
        //
        // 1. Chrome maps (O(1) — resize, drag, split) intercept before
        //    the owner match, because these require WindowManager-level
        //    MouseCaptureState management.
        // 2. Exhaustive match on ComponentOwner routes to the exact
        //    component that registered the hitbox during rendering.

        let Some((hitbox_id, owner, hit_rect)) = self.hitbox_registry.hit_test(*position) else {
            // No hitbox — try focus-on-click for Press events, then fall through
            if matches!(kind, MouseEventKind::Press(_)) && self.config.wm_command_menu_enabled {
                self.focus_window_at(col, row);
            }
            return EventResult::Ignored;
        };

        let core_event = Event::Mouse(MouseEvent {
            kind: *kind,
            column: col,
            row,
            modifiers: *modifiers,
        });

        // --- Chrome interception ---
        // Match directly on ComponentOwner::Chrome(target) for O(1) routing.
        // Only Press events initiate capture state. Moved events forward to layout.
        if matches!(kind, MouseEventKind::Press(_)) {
            if let ComponentOwner::Chrome(target) = &owner {
                return match target {
                    crate::chrome::ChromeTarget::Resize(_, _) => {
                        let crate::chrome::ChromeTarget::Resize(h_key, h_edge) = target else {
                            unreachable!()
                        };
                        if self.is_monocle()
                            || !self.config.floating_windows_enabled
                            || !self.is_window_floating(*h_key)
                        {
                            return EventResult::Ignored;
                        }
                        self.bring_floating_to_front_key(*h_key);
                        // Unset maximized so further resize/move isn't restricted,
                        // but keep the current rect so the cursor stays on the handle.
                        if let Some(w) = self.windows.get_mut(*h_key) {
                            w.is_maximized = false;
                            w.borders_enabled = true;
                        }
                        let rect = self.full_region_for_key(*h_key);
                        let (start_x, start_y, start_width, start_height) =
                            if let Some(crate::window::FloatRectSpec::Absolute(fr)) =
                                self.floating_rect(*h_key)
                            {
                                (fr.x, fr.y, fr.width, fr.height)
                            } else {
                                (rect.x, rect.y, rect.width, rect.height)
                            };
                        self.mouse_capture = Some(MouseCaptureState::ResizingWindow {
                            key: *h_key,
                            edge: *h_edge,
                            start_rect: rect,
                            start_col: col,
                            start_row: row,
                            start_x,
                            start_y,
                            start_width,
                            start_height,
                        });
                        EventResult::Consumed
                    }
                    crate::chrome::ChromeTarget::Drag(key) => {
                        Self::init_window_drag(self, *key, col, row)
                    }
                    crate::chrome::ChromeTarget::CloseButton(key) => {
                        if matches!(kind, MouseEventKind::Press(_)) {
                            self.close_window(*key);
                            self.last_header_click = None;
                            EventResult::Consumed
                        } else {
                            EventResult::Ignored
                        }
                    }
                    crate::chrome::ChromeTarget::MaximizeButton(key) => {
                        if matches!(kind, MouseEventKind::Press(_)) {
                            self.toggle_maximize(*key);
                            self.last_header_click = None;
                            EventResult::Consumed
                        } else {
                            EventResult::Ignored
                        }
                    }
                    crate::chrome::ChromeTarget::MinimizeButton(key) => {
                        if matches!(kind, MouseEventKind::Press(_)) {
                            self.minimize_window(*key);
                            self.last_header_click = None;
                            EventResult::Consumed
                        } else {
                            EventResult::Ignored
                        }
                    }
                    crate::chrome::ChromeTarget::ToggleDirectMode(key) => {
                        if matches!(kind, MouseEventKind::Press(_)) {
                            self.toggle_direct_mode(*key);
                            self.last_header_click = None;
                            EventResult::Consumed
                        } else {
                            EventResult::Ignored
                        }
                    }
                    crate::chrome::ChromeTarget::SplitHandle(_id) => {
                        self.mouse_capture = Some(MouseCaptureState::LayoutHandle);
                        if let Some(layout) = self.managed_layout.as_mut() {
                            let _ = layout.handle_event(&core_event, self.managed_area);
                        }
                        EventResult::Consumed
                    }
                    crate::chrome::ChromeTarget::EmptyStatePlaceholder => {
                        EventResult::Action((None, TermWmAction::OpenCommandPalette))
                    }
                };
            }
        }

        // Forward Moved events to tiling layout for hover feedback on split handles.
        if matches!(kind, MouseEventKind::Moved) {
            if let Some(layout) = self.managed_layout.as_mut() {
                layout.handle_event(&core_event, self.managed_area);
            }
        }

        // --- Exhaustive owner match ---
        // Every hitbox has exactly one owner. No iteration, no fallback.
        match owner {
            ComponentOwner::Window(key) => {
                // Z-stack elevation: clicking a floating window brings it to front
                if matches!(kind, MouseEventKind::Press(_)) && self.is_window_floating(key) {
                    self.bring_floating_to_front_key(key);
                }
                let focused = *self.focus.current() == key;
                let ctx = self
                    .component_context_for(focused, key)
                    .with_screen_area(hit_rect)
                    .with_active_hitbox(hitbox_id);
                if let Some(comp) = self.component_for_key_mut(key) {
                    let result = comp.handle_events(&core_event, &ctx);
                    if !result.is_ignored() && matches!(kind, MouseEventKind::Press(_)) {
                        self.mouse_capture = Some(MouseCaptureState::ComponentInteraction {
                            owner: ComponentOwner::Window(key),
                            screen_area: hit_rect,
                            hitbox_id,
                        });
                    }
                    result.map(|action| (Some(key), action))
                } else {
                    EventResult::Ignored
                }
            }
            ComponentOwner::Overlay(key) => {
                let ctx = self
                    .component_context(false)
                    .with_screen_area(hit_rect)
                    .with_active_hitbox(hitbox_id);
                if let Some(overlay) = self.overlays.get_mut(key) {
                    let result = overlay.handle_events(&core_event, &ctx);
                    if !result.is_ignored() && matches!(kind, MouseEventKind::Press(_)) {
                        self.mouse_capture = Some(MouseCaptureState::ComponentInteraction {
                            owner: ComponentOwner::Overlay(key),
                            screen_area: hit_rect,
                            hitbox_id,
                        });
                    }
                    result.map(|action| (None, action))
                } else {
                    EventResult::Ignored
                }
            }
            ComponentOwner::Layer(layer_id) => {
                let ctx = self
                    .component_context(false)
                    .with_screen_area(hit_rect)
                    .with_active_hitbox(hitbox_id);
                if let Some(layer_comp) = self.layer_manager.get_mut(layer_id) {
                    let result = layer_comp.handle_events(&core_event, &ctx);
                    if !result.is_ignored() && matches!(kind, MouseEventKind::Press(_)) {
                        self.mouse_capture = Some(MouseCaptureState::ComponentInteraction {
                            owner: ComponentOwner::Layer(layer_id),
                            screen_area: hit_rect,
                            hitbox_id,
                        });
                    }
                    result.map(|action| (None, action))
                } else {
                    EventResult::Ignored
                }
            }
            ComponentOwner::Chrome(_) | ComponentOwner::Test => EventResult::Ignored,
        }
    }

    /// Initialize window drag capture state.
    /// Extracted as a helper so both `ChromeTarget::Drag` and `ChromeTarget::Button(Drag)`
    /// can share the same logic.
    fn init_window_drag(
        &mut self,
        key: WindowKey,
        col: u16,
        row: u16,
    ) -> EventResult<(Option<WindowKey>, TermWmAction)> {
        if self.is_monocle() {
            return EventResult::Ignored;
        }
        let now = Instant::now();
        if let Some((prev_key, prev)) = self.last_header_click
            && prev_key == key
            && now.duration_since(prev) <= Duration::from_millis(500)
        {
            self.toggle_maximize(key);
            self.last_header_click = None;
            return EventResult::Consumed;
        }
        self.last_header_click = Some((key, now));
        if self.is_window_floating(key) {
            self.bring_floating_to_front_key(key);
        }
        let rect = self.visible_region_for_key(key);
        let (initial_x, initial_y) =
            if let Some(crate::window::FloatRectSpec::Absolute(fr)) = self.floating_rect(key) {
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
        EventResult::Consumed
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
        self.close_command_menu();
        self.command_menu_opened_at = None;
        if let Some(handle) = &self.system_task_handle
            && let Some(id) = self.drag_timer_id.take()
        {
            handle.cancel(id);
        }
        if let Some(menu) =
            self.get_semantic_component_mut(layer_manager::ComponentTag::CommandPalette)
        {
            menu.process_action(&ComponentAction::Restore);
        }
    }

    pub fn capture_active(&mut self) -> bool {
        if !self.mouse_capture_enabled {
            return false;
        }
        if self.config.wm_command_menu_enabled && self.command_menu_visible() {
            return true;
        }
        self.refresh_capture();
        self.capture_deadline.is_some()
    }

    pub fn mouse_capture_enabled(&self) -> bool {
        self.mouse_capture_enabled
    }

    /// Returns `true` if any mouse capture is active (drag, resize, component
    /// interaction, or layout handle manipulation).  Read-only — no side
    /// effects, no deadline pruning.  Safe to call from the render path.
    pub fn is_mouse_captured(&self) -> bool {
        self.mouse_capture.is_some()
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
        self.selection_active = active;
        self.selection_dragging = dragging;
        self.selection_text = text;
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
            self.push_notification(
                "Selection copied to clipboard",
                std::time::Duration::from_secs(2),
            );
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
            overlay.set_selection_enabled(enabled);
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
    pub fn set_system_window(&mut self, component: C) -> WindowKey {
        let key = self.create_window(component);
        if let Some(w) = self.windows.get_mut(key) {
            w.is_system_window = true;
        }
        key
    }

    /// Render-phase access: borrow component immutably.
    /// Returns &C — no vtable cast. Callers in generic context get static dispatch.
    pub fn component_for_key(&self, key: WindowKey) -> Option<&C> {
        let w = self.windows.get(key)?;
        self.components.get(w.component_key)
    }

    /// Event/update-phase access: borrow component mutably.
    /// Returns &mut C — no vtable cast. Callers in generic context get static dispatch.
    pub fn component_for_key_mut(&mut self, key: WindowKey) -> Option<&mut C> {
        let w = self.windows.get_mut(key)?;
        self.components.get_mut(w.component_key)
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

    pub fn overlay_for_key_mut(&mut self, key: OverlayKey) -> Option<&mut O> {
        self.overlays.get_mut(key)
    }

    pub fn close_overlay(&mut self, key: OverlayKey) {
        self.overlays.remove(key);
    }

    pub fn overlay_keys(&self) -> Vec<OverlayKey> {
        self.overlays.keys().collect()
    }

    pub fn open_help_overlay(&mut self, overlay: O) {
        self.help_key = Some(self.overlays.insert(overlay));
        self.input_mode = crate::actions::WmInputMode::Help;
    }

    pub fn open_exit_confirm_overlay(&mut self, overlay: O) {
        self.exit_confirm_key = Some(self.overlays.insert(overlay));
    }

    pub fn open_command_palette_overlay(&mut self, overlay: O) {
        self.command_palette_key = Some(self.overlays.insert(overlay));
        self.input_mode = crate::actions::WmInputMode::CommandPalette;
    }

    pub fn set_scroll_keyboard_enabled(&mut self, enabled: bool) {
        self.scroll_keyboard_enabled_default = enabled;
    }

    /// Enter tab outline mode — palette becomes dim overlay, panels hide in monocle.
    pub fn set_tab_outline_mode(&mut self, duration: Duration) {
        let expires = Instant::now() + duration;
        self.tab_outline_until = Some(expires);
        if let Some(key) = self.command_palette_key
            && let Some(overlay) = self.overlays.get_mut(key)
        {
            overlay.set_tab_outline(Some(expires));
        }
        if let Some(handle) = &self.system_task_handle {
            let _ = handle.schedule_once(duration, crate::actions::SystemTask::ClearTabOutline);
        }
    }

    /// Clear tab outline mode — restore palette/panels to normal.
    pub fn clear_tab_outline(&mut self) {
        self.tab_outline_until = None;
        if let Some(key) = self.command_palette_key
            && let Some(overlay) = self.overlays.get_mut(key)
        {
            overlay.set_tab_outline(None);
        }
    }

    /// Whether tab outline mode is currently active.
    pub fn is_tab_outline_active(&self) -> bool {
        self.tab_outline_until
            .is_some_and(|until| Instant::now() < until)
    }

    pub fn panel_active(&self) -> bool {
        if self.is_monocle_cramped() {
            return false;
        }
        self.config.panels_enabled
            && self
                .get_semantic_component(layer_manager::ComponentTag::TopPanel)
                .is_some_and(|p| p.visible())
    }

    /// Register panel hitboxes (top and bottom) into the draw-time registry.
    /// Called before the window loop so panels are at the lowest Z-order.
    pub fn register_panel_hitboxes(
        &mut self,
        top_owner: ComponentOwner,
        bottom_owner: ComponentOwner,
    ) {
        if let Some(top) = self.get_semantic_component(layer_manager::ComponentTag::TopPanel)
            && !self.top_claimed.is_empty()
            && let Some(id) = top.hitbox_id()
        {
            self.hitbox_registry
                .register(id, top_owner, self.top_claimed);
        }
        if let Some(bottom) = self.get_semantic_component(layer_manager::ComponentTag::BottomPanel)
            && !self.bottom_claimed.is_empty()
            && let Some(id) = bottom.hitbox_id()
        {
            self.hitbox_registry
                .register(id, bottom_owner, self.bottom_claimed);
        }
    }

    /// No-op: chrome hitbox registration is now handled by the console
    /// during the rendering pass. See `render_window_chrome` in
    /// `term-wm-console/src/draw_plan_renderer.rs`.
    pub fn register_layout_handle_hitboxes(&mut self) {
        // Handled by console during render
    }

    /// No-op: chrome hitbox registration is now handled by the console
    /// during the rendering pass.
    pub fn register_window_chrome_hitboxes(&mut self, _key: WindowKey) {
        // Handled by console during render
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
            match &act {
                TermWmAction::RequestKeyboardFocus(id) => {
                    self.set_keyboard_focus(k, *id);
                }
                _ => {
                    let ctx = self.component_context_for(true, k);
                    if let Some(comp) = self.component_for_key_mut(k) {
                        comp.update(act, &ctx, &mut queue);
                    }
                }
            }
        }
    }

    /// Set the shared `TaskHandle<SystemTask>` for registering/cancelling system
    /// timers.  Called once by the runner during startup.
    pub fn set_system_task_handle(&mut self, handle: TaskHandle<SystemTask>) {
        self.system_task_handle = Some(handle);
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

    // ── Event Routing & Update Accessors ─────────────────────────────

    pub fn overlays_mut(&mut self) -> &mut SlotMap<OverlayKey, O> {
        &mut self.overlays
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

    pub fn overlays(&self) -> &SlotMap<OverlayKey, O> {
        &self.overlays
    }

    pub fn supported_menu_actions(&self) -> &[TermWmAction] {
        &self.supported_menu_actions
    }

    /// Push a notification and schedule its auto-dismiss via the system task scheduler.
    pub fn push_notification(&mut self, message: impl Into<String>, ttl: Duration) -> u64 {
        let id = self.notification_queue.push(message);
        tracing::info!(
            "push_notification: id={}, queue_len={}",
            id,
            self.notification_queue.len()
        );
        // Mark layout dirty so the draw plan regenerates with notification regions
        self.mark_layout_dirty();
        if let Some(handle) = &self.system_task_handle {
            handle.schedule_once(ttl, SystemTask::DismissNotification(id));
        }
        id
    }

    /// Dismiss a notification by ID.
    pub fn dismiss_notification(&mut self, id: u64) {
        tracing::info!("dismiss_notification: id={}", id);
        self.notification_queue.dismiss(id);
        self.mark_layout_dirty();
    }

    /// Read-only access to the notification queue.
    pub fn notifications(&self) -> &NotificationQueue {
        &self.notification_queue
    }

    /// Set the notification area component (called during app init).
    /// Pushes into LayerManager with ZPlane::Foreground.
    pub fn set_notification_component(&mut self, comp: L) {
        let id = self
            .layer_manager
            .insert(comp, layer_manager::ZPlane::Foreground);
        self.semantic_registry
            .insert(layer_manager::ComponentTag::NotificationArea, id);
    }

    /// Get a mutable reference to the notification component.
    pub fn notification_component_mut(&mut self) -> Option<&mut L> {
        self.semantic_registry
            .get(&layer_manager::ComponentTag::NotificationArea)
            .copied()
            .and_then(|id| self.layer_manager.get_mut(id))
    }

    /// Set the keyboard focus for a window. Called when a component returns
    /// `RequestKeyboardFocus` action.
    pub fn set_keyboard_focus(
        &mut self,
        key: WindowKey,
        hitbox_id: crate::hitbox_registry::HitboxId,
    ) {
        if let Some(window) = self.windows.get_mut(key) {
            window.active_keyboard_focus = Some(hitbox_id);
        }
    }

    /// Returns the window management buttons appropriate for the current mode.
    /// In monocle mode, minimize/maximize are excluded (meaningless when
    /// the focused window fills the screen). All window-specific buttons are
    /// excluded when there is no focused window.
    pub fn window_management_buttons(&self) -> Vec<WmButton> {
        let has_focused = self.window(self.focused_window()).is_some();
        if !has_focused {
            return Vec::new();
        }
        let mut btns = vec![WmButton {
            action: TermWmAction::CloseWindow,
            label: "Close Window",
            symbol: "X",
        }];
        if !self.is_monocle() {
            let focused = self.focused_window();
            let is_maxed = self.window(focused).is_some_and(|w| w.is_maximized);
            btns.push(WmButton {
                action: TermWmAction::MaximizeWindow,
                label: if is_maxed { "Restore Window" } else { "Maximize Window" },
                symbol: if is_maxed { "─" } else { "▢" },
            });
            btns.push(WmButton {
                action: TermWmAction::MinimizeWindow,
                label: "Minimize Window",
                symbol: "_",
            });
        }
        btns.push(WmButton {
            action: TermWmAction::ToggleDirectMode,
            label: "Toggle Direct Mode",
            symbol: "D",
        });
        btns
    }

    pub fn wm_menu_items(&self) -> Vec<MenuItem<crate::actions::TermWmAction>> {
        let mouse_label = if self.mouse_capture_enabled {
            "Mouse Capture: On"
        } else {
            "Mouse Capture: Off"
        };
        let clipboard_label = if self.clipboard_enabled {
            "Clipboard Mode: On"
        } else {
            "Clipboard Mode: Off"
        };
        let selection_label = if self.window_selection_enabled {
            "Window Selection: On"
        } else {
            "Window Selection: Off"
        };
        let mut items = vec![
            MenuItem {
                label: "Resume".into(),
                icon: Some("▶"),
                action: crate::actions::TermWmAction::CloseMenu,
                disabled: false,
            },
            MenuItem {
                label: mouse_label.into(),
                icon: Some("◆"),
                action: crate::actions::TermWmAction::ToggleMouseCapture,
                disabled: false,
            },
            MenuItem {
                label: clipboard_label.into(),
                icon: Some("■"),
                action: crate::actions::TermWmAction::ToggleClipboardMode,
                disabled: false,
            },
            MenuItem {
                label: selection_label.into(),
                icon: Some("●"),
                action: crate::actions::TermWmAction::ToggleWindowSelection,
                disabled: false,
            },
            MenuItem {
                label: "New Window".into(),
                icon: Some("+"),
                action: crate::actions::TermWmAction::NewWindow,
                disabled: false,
            },
            MenuItem {
                label: "Toggle Debug Log".into(),
                icon: Some("≣"),
                action: crate::actions::TermWmAction::ToggleDebugWindow,
                disabled: false,
            },
            MenuItem {
                label: "Toggle System Panel".into(),
                icon: Some("⚙"),
                action: crate::actions::TermWmAction::ToggleSystemPanel,
                disabled: false,
            },
            MenuItem {
                label: "Help".into(),
                icon: Some("?"),
                action: crate::actions::TermWmAction::Help,
                disabled: false,
            },
            MenuItem {
                label: "Exit UI".into(),
                icon: Some("⏻"),
                action: crate::actions::TermWmAction::ExitUi,
                disabled: false,
            },
            MenuItem {
                label: format!("Monocle Mode: {}", self.monocle_mode.label()).into(),
                icon: Some("▢"),
                action: crate::actions::TermWmAction::ToggleMonocle,
                disabled: false,
            },
            MenuItem {
                label: {
                    let mode = if self.managed_layout.is_some() {
                        "Float"
                    } else {
                        "Tile"
                    };
                    format!("Toggle {mode} Mode").into()
                },
                icon: Some("⊞"),
                action: crate::actions::TermWmAction::ToggleTiling,
                disabled: self.is_monocle(),
            },
        ];

        let has_focused_window = self.window_count() > 0;
        if has_focused_window {
            // Window management buttons from centralized list
            for btn in self.window_management_buttons() {
                items.push(MenuItem {
                    label: btn.label.into(),
                    icon: Some(btn.symbol),
                    action: btn.action,
                    disabled: false,
                });
            }
            // Switch-to navigation for all windows
            let focused = *self.focus.current();
            for (key, title) in self.window_titles() {
                items.push(MenuItem {
                    label: format!("Switch to: {}", title).into(),
                    icon: Some("→"),
                    action: crate::actions::TermWmAction::FocusWindow(key),
                    disabled: key == focused,
                });
            }
        }
        items
    }
}

#[derive(Clone)]
pub struct WmButton {
    pub action: TermWmAction,
    pub label: &'static str,
    pub symbol: &'static str,
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
            resizable,
        } => LayoutNode::Split {
            direction: *direction,
            children: children.iter().map(map_layout_node).collect(),
            weights: weights.clone(),
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
fn make_keys<L: WmComponent, O: Overlay<TermWmAction>>(
    wm: &mut WindowManager<TestComponent, L, O>,
    n: usize,
) -> Vec<WindowKey> {
    (0..n)
        .map(|_| wm.create_window(TestComponent::Noop(crate::components::NoopComponent)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::NoopOverlay;
    use crate::events::{KeyModifiers, MouseButton};
    use crate::hitbox_registry::HitboxId;
    use crate::layout::Direction;
    use crate::window::test_component::{ActionRecorder, SelComponent, TestComponent};
    use std::collections::VecDeque;
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        let key = wm.create_window(TestComponent::Noop(crate::components::NoopComponent));
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
    fn handle_mouse_focus_click_skipped_in_monocle() {
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        let keys = make_keys(&mut wm, 2);
        let key_a = keys[0];
        let key_b = keys[1];

        // Set up managed_layout so is_monocle() works
        wm.managed_layout = Some(crate::layout::TilingLayout::new(
            crate::layout::LayoutNode::leaf(key_a),
        ));
        wm.update_monocle_mode(50);
        assert!(wm.is_monocle(), "monocle must be active (width 50 < 80)");

        wm.managed_draw_order = vec![key_a, key_b];
        wm.z_order = vec![key_a, key_b];
        wm.regions.set(
            key_a,
            LayoutRect {
                x: 0,
                y: 0,
                width: 25,
                height: 24,
            },
        );
        wm.regions.set(
            key_b,
            LayoutRect {
                x: 25,
                y: 0,
                width: 25,
                height: 24,
            },
        );

        wm.focus_app_window(key_a);
        assert_eq!(*wm.focus.current(), key_a);

        // Click at coordinate that would match key_b's region
        wm.handle_mouse_focus_click(30, 12);

        assert_eq!(
            *wm.focus.current(),
            key_a,
            "monocle mode must prevent mouse focus switching"
        );
    }

    #[test]
    fn enforce_min_visible_margin_horizontal() {
        use crate::window::{FloatRect, FloatRectSpec};
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
    fn floating_window_offscreen_click_past_right_edge_hits_window_behind() {
        use crate::window::{FloatRect, FloatRectSpec};

        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        let keys = make_keys(&mut wm, 100);
        wm.set_floating_resize_offscreen(true);
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });

        // Float key[1]: 50-col window, 30 columns off-screen left.
        // Visible portion: cols 0–19 (20 columns). Right edge at x=20.
        wm.set_floating_rect(
            keys[1],
            Some(FloatRectSpec::Absolute(FloatRect {
                x: -30,
                y: 0,
                width: 50,
                height: 20,
            })),
        );
        // key[2] stays tiled behind it at the full managed area.
        wm.regions.set(
            keys[2],
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );
        wm.managed_draw_order = vec![keys[2], keys[1]];

        // Simulate render pipeline: tiled (back) first, floating (front) last.
        // Tiled window behind (registered first = lower z-order)
        wm.hitbox_registry_mut().register(
            HitboxId::new(),
            ComponentOwner::Window(keys[2]),
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );

        // Floating window on top (registered last = higher z-order),
        // hitboxes clipped to visible area by the active clip rect.
        let managed = wm.managed_area();
        wm.hitbox_registry_mut().push_clip(managed);
        wm.hitbox_registry_mut().register(
            HitboxId::new(),
            ComponentOwner::Chrome(crate::chrome::ChromeTarget::Drag(keys[1])),
            Rect {
                x: 0,
                y: 1,
                width: 19,
                height: 1,
            },
        );
        wm.hitbox_registry_mut().register(
            HitboxId::new(),
            ComponentOwner::Window(keys[1]),
            Rect {
                x: 0,
                y: 2,
                width: 19,
                height: 17,
            },
        );
        wm.hitbox_registry_mut().pop_clip();

        use crate::mouse_coord::{CoordSpace, MousePosition};
        let screen = |col, row| MousePosition {
            column: col,
            row,
            space: CoordSpace::Screen,
        };

        // Click past floating window's right edge (19) → must hit tiled window
        let hit = wm.hitbox_registry.hit_test(screen(25, 10));
        assert!(
            matches!(hit, Some((_, ComponentOwner::Window(k), _)) if k == keys[2]),
            "click at col 25 (past floating window's right edge) should hit tiled window keys[2], got {:?}",
            hit,
        );

        // Click inside floating window's area → must hit floating window
        let hit_inside = wm.hitbox_registry.hit_test(screen(10, 10));
        assert!(
            matches!(hit_inside, Some((_, ComponentOwner::Window(k), _)) if k == keys[1]),
            "click at col 10 (inside floating window) should hit floating window keys[1], got {:?}",
            hit_inside,
        );
    }

    #[test]
    fn hit_test_uses_visible_bounds_for_floating_windows() {
        use crate::window::{FloatRect, FloatRectSpec};
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        use crate::layout::tiling::SplitHandle;
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
            hitbox_id: crate::hitbox_registry::HitboxId::new(),
        });

        wm.hover = Some((2, 1));
        let handle_hover = wm.hover_targets();
        assert!(
            handle_hover.is_none(),
            "floating window should mask layout handles"
        );

        wm.hover = Some((15, 1));
        let handle_hover = wm.hover_targets();
        assert!(
            handle_hover.is_some(),
            "layout handles should respond off-window"
        );
    }

    #[test]
    fn drag_hitbox_detaches_to_floating() {
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        let debug_key = wm.set_system_window(TestComponent::Noop(crate::components::NoopComponent));
        wm.set_panel_visible(false);
        wm.set_managed_layout(TilingLayout::new(LayoutNode::leaf(debug_key)));
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        assert!(!wm.is_window_floating(debug_key));

        let start_rect = wm.full_region(debug_key);
        let header_pos = start_rect.x.saturating_add(5) as u16;

        // Simulate console registering a Drag hitbox in the header area
        let hitbox_id = crate::hitbox_registry::HitboxId::new();
        wm.hitbox_registry_mut().register(
            hitbox_id,
            ComponentOwner::Chrome(crate::chrome::ChromeTarget::Drag(debug_key)),
            Rect {
                x: i32::from(header_pos),
                y: start_rect.y,
                width: 5,
                height: 1,
            },
        );

        let down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: header_pos,
            row: start_rect.y as u16,
            modifiers: KeyModifiers::NONE,
        });
        let wm_down = crate::events::core_event_to_wm(&down).unwrap();
        assert!(wm.dispatch_mouse(&wm_down).is_consumed());
        // Floating rect is deferred — Press alone must not decouple.
        assert!(!wm.is_window_floating(debug_key));

        let drag_col = header_pos.saturating_add(5);
        let drag_row = (start_rect.y as u16).saturating_add(1);
        let drag = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: drag_col,
            row: drag_row,
            modifiers: KeyModifiers::NONE,
        });
        let wm_drag = crate::events::core_event_to_wm(&drag).unwrap();
        assert!(wm.dispatch_mouse(&wm_drag).is_consumed());
        assert!(wm.is_window_floating(debug_key));

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
        assert!(wm.dispatch_mouse(&wm_up).is_consumed());
        assert!(wm.mouse_capture.is_none());
    }

    #[test]
    fn moved_event_commits_stale_drag_snap() {
        use crate::events::{Event, KeyModifiers, MouseEvent, MouseEventKind};
        use crate::layout::InsertPosition;
        use crate::window::{FloatRect, FloatRectSpec};

        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        assert!(wm.dispatch_mouse(&wm_moved).is_consumed());
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

        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        wm.set_panel_visible(false);
        let keys = make_keys(&mut wm, 100);

        // Two-window horizontal tiling layout.
        let split = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::Leaf(keys[1]), LayoutNode::Leaf(keys[2])],
            weights: vec![1u16, 1u16],
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
        assert!(wm.dispatch_mouse(&wm_moved).is_consumed());

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
        assert!(wm.dispatch_mouse(&wm_release).is_consumed());

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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        let keys = make_keys(&mut wm, 100);
        wm.focus_app_window(keys[0]);
        let focus = wm.focused_window();
        assert!(!wm.direct_mode(focus));
    }

    #[test]
    fn direct_mode_toggle_cycles_state() {
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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

        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        assert!(!wm.direct_mode(win_key), "starts off");

        // Register a ToggleDirectMode hitbox at the test click position.
        wm.hitbox_registry_mut().register(
            crate::hitbox_registry::HitboxId::new(),
            ComponentOwner::Chrome(crate::chrome::ChromeTarget::ToggleDirectMode(win_key)),
            Rect {
                x: i32::from(kb_x),
                y: i32::from(kb_y),
                width: 1,
                height: 1,
            },
        );

        let click = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: kb_x,
            row: kb_y,
            modifiers: KeyModifiers::NONE,
        });
        let wm_click = crate::events::core_event_to_wm(&click).unwrap();
        assert!(
            wm.dispatch_mouse(&wm_click).is_consumed(),
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
        assert!(wm.dispatch_mouse(&wm_click2).is_consumed());
        assert!(
            !wm.direct_mode(win_key),
            "second click toggles back to false"
        );
    }

    #[test]
    fn direct_mode_header_click_on_non_button_area_does_not_toggle() {
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use crate::layout::{LayoutNode, TilingLayout};

        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let drag_x = 10u16;
        let drag_y = 5u16;

        let hitbox_id = crate::hitbox_registry::HitboxId::new();
        wm.hitbox_registry_mut().register(
            hitbox_id,
            ComponentOwner::Chrome(crate::chrome::ChromeTarget::Drag(win_key)),
            Rect {
                x: 10,
                y: 5,
                width: 5,
                height: 1,
            },
        );

        assert!(!wm.direct_mode(win_key));

        let click = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: drag_x,
            row: drag_y,
            modifiers: KeyModifiers::NONE,
        });
        let wm_click = crate::events::core_event_to_wm(&click).unwrap();
        assert!(wm.dispatch_mouse(&wm_click).is_consumed());
        assert!(!wm.direct_mode(win_key), "drag area click must not toggle");
    }

    #[test]
    fn drag_snap_timeout_none_disables_remaining() {
        let mut config = WmConfig::standalone();
        config.drag_snap_timeout = None;
        let mut wm = WindowManager::<TestComponent>::with_config(
            config,
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        let _keys = make_keys(&mut wm, 100);
        assert!(wm.drag_snap_remaining().is_none());
    }

    #[test]
    fn drag_snap_remaining_none_when_no_drag() {
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        let _keys = make_keys(&mut wm, 100);
        assert!(wm.drag_snap_remaining().is_none());
    }

    #[test]
    fn drag_snap_remaining_returns_some_when_dragging() {
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            config,
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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

        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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

        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        wm.hitbox_registry_mut().register(
            crate::hitbox_registry::HitboxId::new(),
            ComponentOwner::Chrome(crate::chrome::ChromeTarget::ToggleDirectMode(win_key)),
            Rect {
                x: i32::from(kb_x),
                y: i32::from(kb_y),
                width: 1,
                height: 1,
            },
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
        assert!(
            result.is_consumed(),
            "header D click must be consumed by chrome"
        );
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

        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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

        let header_rect = Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 1,
        };
        let hitbox_id = crate::hitbox_registry::HitboxId::new();
        wm.hitbox_registry_mut().register(
            hitbox_id,
            ComponentOwner::Chrome(crate::chrome::ChromeTarget::Drag(win_key)),
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
        assert!(
            result_down.is_consumed(),
            "down event must be consumed by chrome"
        );
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
            result_drag.is_consumed(),
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
        assert!(
            result_up.is_consumed(),
            "up event must be consumed by chrome"
        );
        assert!(wm.mouse_capture.is_none(), "drag must be finished after up");
    }

    #[test]
    fn dispatch_focused_event_normal_behavior_when_not_direct_mode() {
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use crate::layout::{LayoutNode, TilingLayout};

        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );

        let debug_key = wm.set_system_window(TestComponent::Noop(crate::components::NoopComponent));
        wm.set_panel_visible(false);
        wm.set_managed_layout(TilingLayout::new(LayoutNode::leaf(debug_key)));
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });

        let header_rect = Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 1,
        };
        let hitbox_id = crate::hitbox_registry::HitboxId::new();
        wm.hitbox_registry_mut().register(
            hitbox_id,
            ComponentOwner::Chrome(crate::chrome::ChromeTarget::Drag(debug_key)),
            header_rect,
        );

        let down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: header_rect.x as u16,
            row: header_rect.y as u16,
            modifiers: KeyModifiers::NONE,
        });
        let wm_down = crate::events::core_event_to_wm(&down).unwrap();
        assert!(wm.dispatch_mouse(&wm_down).is_consumed());
        assert!(wm.drag_last_event.is_some());

        wm.drag_last_event = Some(Instant::now() - STALE_EVENT_OFFSET);
        let drag = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: (header_rect.x + 5) as u16,
            row: header_rect.y as u16,
            modifiers: KeyModifiers::NONE,
        });
        let wm_drag = crate::events::core_event_to_wm(&drag).unwrap();
        assert!(wm.dispatch_mouse(&wm_drag).is_consumed());
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
                WindowManager::<TestComponent>::is_valid_transition(old, new),
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
                    WindowManager::<TestComponent>::is_valid_transition(old, new),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        let key = wm.create_window(TestComponent::Noop(crate::components::NoopComponent));
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

    fn mapped_keys(wm: &mut WindowManager<TestComponent>, n: usize) -> Vec<WindowKey> {
        let raw = make_keys(wm, n);
        for &k in &raw {
            wm.transition_window(k, WindowState::Mapped);
        }
        raw
    }

    #[test]
    fn transition_window_mapped_to_iconic_removes_from_z_order() {
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        let key = wm.set_system_window(TestComponent::Noop(crate::components::NoopComponent));
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        wm.set_panel_visible(false);

        let comp = TestComponent::SelComponent(SelComponent::default());
        let key = wm.create_window(comp);
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

        // Enable selection on the component
        let sel = wm.component_for_key_mut(key).expect("component must exist");
        if let TestComponent::SelComponent(sel) = sel {
            sel.enabled = true;
        }

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
        match wm.component_for_key_mut(key).expect("component must exist") {
            TestComponent::SelComponent(sel) => {
                assert!(sel.received_down, "component must receive mouse Down");
                assert!(sel.enabled, "selection_enabled must persist");
            }
            _ => panic!("component must be SelComponent"),
        }
    }

    /// Phase 4 (Press events) must call `process_action` so `MouseToBytes` reaches `update()`.
    #[test]
    fn phase4_down_dispatches_mouse_action_to_update() {
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        wm.set_panel_visible(false);

        let key = wm.create_window(TestComponent::ActionRecorder(ActionRecorder::default()));
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
        wm.hitbox_registry.register(
            wm.window_content_hitbox_id(key).unwrap_or_default(),
            ComponentOwner::Window(key),
            hit_rect,
        );

        // Send Down at (15, 8) — inside the hitbox
        let down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 15,
            row: 8,
            modifiers: KeyModifiers::NONE,
        });
        let wm_down = crate::events::core_event_to_wm(&down).unwrap();
        if let Some((action_key, action)) = wm.dispatch_mouse(&wm_down).into_action() {
            let k = action_key.unwrap_or(key);
            wm.process_action(k, action);
        }

        // Verify the action reached update()
        match wm.component_for_key_mut(key).expect("component must exist") {
            TestComponent::ActionRecorder(recorder) => {
                assert!(
                    recorder.received_mouse_bytes,
                    "Phase 4 Down must process MouseToBytes action via process_action"
                );
            }
            _ => panic!("component must be ActionRecorder"),
        }
    }

    /// Phase 3 (Moved events without active capture) must call `process_action`.
    #[test]
    fn phase3_moved_dispatches_mouse_action_to_update() {
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        wm.set_panel_visible(false);

        let key = wm.create_window(TestComponent::ActionRecorder(ActionRecorder::default()));
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
        wm.hitbox_registry.register(
            wm.window_content_hitbox_id(key).unwrap_or_default(),
            ComponentOwner::Window(key),
            hit_rect,
        );

        // Send Moved at (15, 8) — no active capture, so Phase 3 runs
        let moved = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 15,
            row: 8,
            modifiers: KeyModifiers::NONE,
        });
        let wm_moved = crate::events::core_event_to_wm(&moved).unwrap();
        if let Some((action_key, action)) = wm.dispatch_mouse(&wm_moved).into_action() {
            let k = action_key.unwrap_or(key);
            wm.process_action(k, action);
        }

        match wm.component_for_key_mut(key).expect("component must exist") {
            TestComponent::ActionRecorder(recorder) => {
                assert!(
                    recorder.received_mouse_bytes,
                    "Phase 3 Moved must process MouseToBytes action via process_action"
                );
            }
            _ => panic!("component must be ActionRecorder"),
        }
    }

    // ── LayoutHandle split-resize tests ────────────────────────────────

    /// Helper: create a WindowManager with a 2-window horizontal tiling layout.
    /// Returns (wm, keys, gap_col, gap_row) where gap is the center of the split handle.
    fn setup_tiling_with_gap() -> (WindowManager<TestComponent>, Vec<WindowKey>, u16, u16) {
        let mut wm = WindowManager::<TestComponent>::with_config(
            crate::wm_config::WmConfig::standalone(),
            std::sync::Arc::new(crate::app_context::AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        wm.set_panel_visible(false);
        let keys = make_keys(&mut wm, 100);
        let split = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::Leaf(keys[0]), LayoutNode::Leaf(keys[1])],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        wm.set_managed_layout(TilingLayout::new(split));
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        wm.register_layout_handle_hitboxes();
        let handles = wm.handles.clone();
        assert!(!handles.is_empty(), "tiling must produce split handles");
        use crate::chrome::ChromeTarget;
        use crate::hitbox_registry::ComponentOwner;
        for handle in &handles {
            wm.hitbox_registry.register(
                handle.hitbox_id,
                ComponentOwner::Chrome(ChromeTarget::SplitHandle(handle.hitbox_id)),
                handle.rect,
            );
        }
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
    fn moved_over_split_handle_does_not_set_capture() {
        // Regression test: chrome maps (resize/drag/split) must only fire
        // on Press events. A Moved event over a split handle must NOT
        // initiate MouseCaptureState — only the tiling layout hover
        // should be updated.
        use crate::events::{Event, KeyModifiers, MouseEvent, MouseEventKind};
        let (mut wm, _keys, gap_col, gap_row) = setup_tiling_with_gap();

        // Moved over the gap — no Down, just hover
        let moved = crate::events::core_event_to_wm(&Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: gap_col,
            row: gap_row,
            modifiers: KeyModifiers::NONE,
        }))
        .unwrap();
        wm.dispatch_mouse(&moved);

        // The event must not set capture state
        assert!(
            wm.mouse_capture.is_none(),
            "Moved over split handle must not set capture"
        );
        // Hover state should still be updated (layout handle hover)
        let layout = wm.managed_layout.as_ref().unwrap();
        assert!(
            layout.hovered_handle(wm.managed_area).is_some(),
            "Moved must update hover feedback on split handles"
        );
    }

    #[test]
    fn register_layout_handle_hitboxes_registers_entries() {
        let mut wm = WindowManager::<TestComponent>::with_config(
            crate::wm_config::WmConfig::standalone(),
            std::sync::Arc::new(crate::app_context::AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        wm.set_panel_visible(false);
        let keys = make_keys(&mut wm, 100);
        let split = LayoutNode::Split {
            direction: Direction::Horizontal,
            children: vec![LayoutNode::Leaf(keys[0]), LayoutNode::Leaf(keys[1])],
            weights: vec![1u16, 1u16],
            resizable: true,
        };
        wm.set_managed_layout(TilingLayout::new(split));
        wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });

        wm.register_layout_handle_hitboxes();
        let handles = wm.handles.clone();
        for handle in &handles {
            wm.hitbox_registry.register(
                handle.hitbox_id,
                ComponentOwner::Chrome(crate::chrome::ChromeTarget::SplitHandle(handle.hitbox_id)),
                handle.rect,
            );
        }

        // Verify at least one entry exists at the gap position.
        let gap = &handles[0].rect;
        let pos = crate::mouse_coord::MousePosition {
            column: (gap.x + i32::from(gap.width) / 2) as i16,
            row: (gap.y + i32::from(gap.height) / 2) as i16,
            space: crate::mouse_coord::CoordSpace::Screen,
        };
        let hit = wm.hitbox_registry.hit_test(pos);
        assert!(hit.is_some(), "registry must contain an entry at split gap");
    }

    #[test]
    fn close_button_hitbox_dispatches_close() {
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use crate::layout::{LayoutNode, TilingLayout};

        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        wm.transition_window(keys[1], crate::window::entry::WindowState::Mapped);

        let win_key = keys[1];
        let hitbox_id = crate::hitbox_registry::HitboxId::new();

        // Simulate console registering a close button hitbox
        wm.hitbox_registry_mut().register(
            hitbox_id,
            ComponentOwner::Chrome(crate::chrome::ChromeTarget::CloseButton(win_key)),
            Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
        );

        let click = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let wm_click = crate::events::core_event_to_wm(&click).unwrap();
        assert!(wm.dispatch_mouse(&wm_click).is_consumed());
    }

    #[test]
    fn maximize_button_hitbox_dispatches_maximize() {
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use crate::layout::{LayoutNode, TilingLayout};

        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
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
        wm.transition_window(keys[1], crate::window::entry::WindowState::Mapped);

        let win_key = keys[1];
        let hitbox_id = crate::hitbox_registry::HitboxId::new();

        // Simulate console registering a maximize button hitbox
        wm.hitbox_registry_mut().register(
            hitbox_id,
            ComponentOwner::Chrome(crate::chrome::ChromeTarget::MaximizeButton(win_key)),
            Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
        );

        let click = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let wm_click = crate::events::core_event_to_wm(&click).unwrap();
        assert!(wm.dispatch_mouse(&wm_click).is_consumed());
        assert!(wm.is_window_floating(win_key));
        assert!(wm.windows.get(win_key).unwrap().is_maximized);
    }

    #[test]
    fn overlay_dispatch_passes_screen_area_to_context() {
        use crate::components::{
            Component as Cmp, ComponentContext as Ctx, EventResult as EvtRes, WmComponent as WmCmp,
        };
        use crate::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use crate::layout::{LayoutNode, TilingLayout};

        #[derive(Debug)]
        struct ConsumeLayer;
        impl Cmp<TermWmAction> for ConsumeLayer {
            fn handle_events(&mut self, _: &Event, _: &Ctx) -> EvtRes<TermWmAction> {
                EvtRes::Consumed
            }
            fn update(
                &mut self,
                _: TermWmAction,
                _: &Ctx,
                _: &mut VecDeque<(WindowKey, TermWmAction)>,
            ) {
            }
            fn render(
                &mut self,
                _: &mut dyn term_wm_render::RenderBackend,
                _: LayoutRect,
                _: &Ctx,
                _: &mut crate::hitbox_registry::HitboxRegistry,
            ) {
            }
        }
        impl WmCmp for ConsumeLayer {}

        let mut wm = WindowManager::<TestComponent, ConsumeLayer>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            layer_manager::LayerManager::<ConsumeLayer>::new(),
            std::collections::HashMap::new(),
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

        let overlay_rect = LayoutRect {
            x: 10,
            y: 5,
            width: 30,
            height: 15,
        };

        // Register overlay's hitbox and store it
        let overlay_obj = ConsumeLayer;
        let _overlay_id = wm
            .layer_manager
            .insert(overlay_obj, layer_manager::ZPlane::Foreground);
        // The foreground dispatch calls handle_events on all layers.
        // Register the hitbox with the correct overlay area.
        wm.hitbox_registry.register(
            HitboxId::new(),
            ComponentOwner::Layer(_overlay_id),
            overlay_rect,
        );

        let click = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 15,
            row: 8,
            modifiers: KeyModifiers::NONE,
        });
        let wm_click = crate::events::core_event_to_wm(&click).unwrap();
        assert!(wm.dispatch_mouse(&wm_click).is_consumed());
    }

    #[test]
    fn overlay_close_exit_confirm_removes_overlay() {
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        assert!(!wm.exit_confirm_visible());

        wm.open_exit_confirm_overlay(NoopOverlay);
        assert!(wm.exit_confirm_visible());
        wm.close_exit_confirm();
        assert!(!wm.exit_confirm_visible());
    }

    #[test]
    fn overlay_help_visible_and_close() {
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        assert!(!wm.help_overlay_visible());

        wm.open_help_overlay(NoopOverlay);
        assert!(wm.help_overlay_visible());
        wm.close_help_overlay();
        assert!(!wm.help_overlay_visible());
    }

    #[test]
    fn handle_exit_confirm_event_returns_none_without_overlay() {
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        let event = Event::Key(crate::events::KeyEvent {
            code: crate::events::KeyCode::Esc,
            modifiers: crate::events::KeyModifiers::NONE,
            kind: crate::events::KeyKind::Press,
        });
        assert!(wm.handle_exit_confirm_event(&event).is_none());
    }

    #[test]
    fn command_palette_empty_map_returns_no_action() {
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        assert!(!wm.command_menu_visible());
        let event = crate::events::Event::Key(crate::events::KeyEvent {
            code: crate::events::KeyCode::Esc,
            modifiers: crate::events::KeyModifiers::NONE,
            kind: crate::events::KeyKind::Press,
        });
        assert!(wm.handle_command_palette_event(&event).is_none());
    }

    #[test]
    fn command_menu_visible_derived_from_overlay_map() {
        let mut wm = WindowManager::<TestComponent>::with_config(
            WmConfig::standalone(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            crate::window::LayerManager::new(),
            std::collections::HashMap::new(),
        );
        assert!(!wm.command_menu_visible());
        wm.close_command_menu();
        assert!(!wm.command_menu_visible());
    }
}
