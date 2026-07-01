mod chrome;
mod drag;
mod focus;
mod layout;
mod overlays;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyEvent};
use ratatui::prelude::Rect;
use slotmap::SlotMap;

use super::WindowKey;
use super::decorator::WindowDecorator;
use super::entry::Window;
use crate::app_context::AppContext;
use crate::bottom_panel_trait::BottomPanel;
use crate::components::{ComponentContext, MenuItem, MenuOverlay, Overlay, SelectionStatus};
use crate::keybindings::KeyBindings;
use crate::layout::floating::*;
use crate::layout::{InsertPosition, LayoutNode, RegionMap, SplitHandle, TilingLayout};
use crate::power_profile::PowerProfile;
use crate::reaper::Reaper;
use crate::top_panel_trait::TopPanel;
use crate::ui::UiFrame;
use crate::wm_config::{HintVisibility, WmConfig};
use term_wm_layout_engine::FocusRing;

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
    pub id: WindowKey,
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
    app_ctx: Arc<AppContext>,
    top_panel: Option<Box<dyn TopPanel<WindowKey>>>,
    bottom_panel: Option<Box<dyn BottomPanel>>,
    menu_overlay: Option<Box<dyn MenuOverlay<WmMenuAction>>>,
    pub(crate) drag_header: Option<HeaderDrag<WindowKey>>,
    pub(crate) last_header_click: Option<(WindowKey, Instant)>,
    pub(crate) drag_resize: Option<ResizeDrag<WindowKey>>,
    pub(crate) hover: Option<(u16, u16)>,
    capture_deadline: Option<Instant>,
    pending_deadline: Option<Instant>,
    mouse_capture_enabled: bool,
    mouse_capture_dirty: bool,
    window_selection_enabled: bool,
    window_selection_dirty: bool,
    clipboard_enabled: bool,
    clipboard_dirty: bool,
    overlay_visible: bool,
    selection_active: bool,
    selection_dragging: bool,
    selection_text: Option<String>,
    selection_copied: bool,
    selection_copied_text: Option<String>,
    config: WmConfig,
    hint_visibility: HintVisibility,
    overlay_opened_at: Option<Instant>,
    super_pending: Option<(Instant, KeyEvent)>,
    pub(crate) last_frame_area: ratatui::prelude::Rect,
    overlays: BTreeMap<OverlayId, Box<dyn Overlay>>,
    scroll_keyboard_enabled_default: bool,
    floating_resize_offscreen: bool,
    pub(crate) z_order: Vec<WindowKey>,
    pub(crate) drag_snap: Option<(Option<WindowKey>, InsertPosition, Rect)>,
    drag_last_event: Option<Instant>,
    // No separate component map — components live on the Window struct
    // in the SlotMap.  See `Window.component`.
    next_window_seq: usize,
    next_title_seq: usize,
    synthetic_event: Option<Event>,
    clipboard: Option<crate::clipboard::Clipboard>,
    power_profile: PowerProfile,
    pub(crate) reaper: Reaper,
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
    ToggleWindowSelection,
}

impl WindowManager {
    /// Allocate a new window entry in the SlotMap and return its key.
    /// The window starts with default state (no title, not floating, etc.).
    pub fn create_window(&mut self) -> WindowKey {
        let order = self.next_window_seq;
        self.next_window_seq = self.next_window_seq.saturating_add(1);
        tracing::debug!(seq = order, "opened window");
        self.windows.insert(Window::new(order))
    }

    /// Access the Reaper for async child-process teardown.
    pub fn reaper(&mut self) -> &mut Reaper {
        &mut self.reaper
    }

    fn window_mut(&mut self, key: WindowKey) -> &mut Window {
        self.windows
            .get_mut(key)
            .unwrap_or_else(|| panic!("window_mut called for unknown key {:?}", key))
    }

    fn window(&self, key: WindowKey) -> Option<&Window> {
        self.windows.get(key)
    }

    fn is_minimized(&self, key: WindowKey) -> bool {
        self.window(key).is_some_and(|window| window.minimized)
    }

    fn set_minimized(&mut self, key: WindowKey, value: bool) {
        self.window_mut(key).minimized = value;
    }

    fn floating_rect(&self, key: WindowKey) -> Option<crate::window::FloatRectSpec> {
        self.window(key).and_then(|window| window.floating_rect)
    }

    fn set_floating_rect(&mut self, key: WindowKey, rect: Option<crate::window::FloatRectSpec>) {
        self.window_mut(key).floating_rect = rect;
    }

    fn clear_floating_rect(&mut self, key: WindowKey) {
        self.window_mut(key).floating_rect = None;
    }

    fn set_prev_floating_rect(
        &mut self,
        key: WindowKey,
        rect: Option<crate::window::FloatRectSpec>,
    ) {
        self.window_mut(key).prev_floating_rect = rect;
    }

    fn take_prev_floating_rect(&mut self, key: WindowKey) -> Option<crate::window::FloatRectSpec> {
        self.window_mut(key).prev_floating_rect.take()
    }

    fn is_window_floating(&self, key: WindowKey) -> bool {
        self.window(key).is_some_and(|window| window.is_floating())
    }

    pub fn direct_mode(&self, key: WindowKey) -> bool {
        self.window(key).is_some_and(|window| window.direct_mode)
    }

    pub fn set_direct_mode(&mut self, key: WindowKey, value: bool) {
        self.window_mut(key).direct_mode = value;
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
    /// "htop" is "htop (1)" regardless of creation order.
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
        top_panel: Option<Box<dyn TopPanel<WindowKey>>>,
        bottom_panel: Option<Box<dyn BottomPanel>>,
        menu_overlay: Option<Box<dyn MenuOverlay<WmMenuAction>>>,
    ) -> Self {
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
            app_ctx,
            top_panel,
            bottom_panel,
            menu_overlay,
            drag_header: None,
            last_header_click: None,
            drag_resize: None,
            hover: None,
            capture_deadline: None,
            pending_deadline: None,
            mouse_capture_enabled,
            mouse_capture_dirty: false,
            clipboard_enabled: config.clipboard_enabled,
            clipboard_dirty: false,
            overlay_visible: false,
            selection_active: false,
            selection_dragging: false,
            selection_text: None,
            selection_copied: false,
            selection_copied_text: None,
            window_selection_enabled: config.window_selection_enabled,
            window_selection_dirty: false,
            hint_visibility: config.hint_visibility,
            config,
            overlay_opened_at: None,
            super_pending: None,
            last_frame_area: Rect::default(),
            overlays: BTreeMap::new(),
            scroll_keyboard_enabled_default: true,
            floating_resize_offscreen,
            z_order: Vec::new(),
            drag_snap: None,
            drag_last_event: None,
            next_window_seq: 0,
            next_title_seq: 0,
            synthetic_event: None,
            clipboard,
            power_profile: PowerProfile::PowerSaver,
            reaper: Reaper::default(),
        }
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
    }

    /// Number of overlays that will be rendered this frame.
    pub fn visible_overlay_count(&self) -> usize {
        let mut n = 0usize;
        if self.wm_overlay_visible() {
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
        if let Some(p) = &mut self.top_panel {
            p.begin_frame();
        }
        if let Some(p) = &mut self.bottom_panel {
            p.begin_frame();
            p.set_power_profile(self.power_profile);
        }
        if !self.config.wm_overlay_enabled {
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
        self.overlay_visible = false;
        self.overlay_opened_at = None;
        self.super_pending = None;
        if let Some(menu) = &mut self.menu_overlay {
            menu.restore();
        }
    }

    pub fn capture_active(&mut self) -> bool {
        if !self.mouse_capture_enabled {
            return false;
        }
        if self.config.wm_overlay_enabled && self.overlay_visible {
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
            crate::components::Overlay::set_selection_enabled(&mut **overlay, enabled);
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
    /// `WindowProvider::window_component` → `component_for_key`.
    pub fn set_system_window(
        &mut self,
        component: Box<dyn crate::components::Component>,
    ) -> WindowKey {
        let key = self.create_window();
        if let Some(w) = self.windows.get_mut(key) {
            w.component = Some(component);
        }
        key
    }

    /// Retrieve the component stored on a window in the SlotMap, if any.
    /// Used by `WindowProvider::window_component` implementations.
    pub fn component_for_key(
        &mut self,
        key: WindowKey,
    ) -> Option<&mut dyn crate::components::Component> {
        let w = self.windows.get_mut(key)?;
        let c = w.component.as_mut()?;
        Some(c.as_mut())
    }

    pub fn open_overlay(&mut self, id: OverlayId, overlay: Option<Box<dyn Overlay>>) {
        if let Some(o) = overlay {
            self.overlays.insert(id, o);
        }
    }

    pub fn set_scroll_keyboard_enabled(&mut self, enabled: bool) {
        self.scroll_keyboard_enabled_default = enabled;
    }

    fn panel_active(&self) -> bool {
        self.config.panel_enabled
            && self.top_panel.as_ref().is_some_and(|p| p.visible())
            && self.top_panel.as_ref().map_or(0, |p| p.height()) > 0
    }

    /// Unified double-Esc press handler.
    /// - `Pending`: first press of WmToggleOverlay — deferred, timeout will forward.
    /// - `DoubleSuper`: second press within window — caller should open overlay.
    /// - `Forward`: not a WmToggleOverlay key — forward immediately.
    pub fn handle_super_press(
        &mut self,
        key: &KeyEvent,
        is_wm_toggle_key: bool,
    ) -> SuperPressResult {
        if is_wm_toggle_key {
            if let Some((pressed_at, _)) = &self.super_pending
                && pressed_at.elapsed() < self.config.super_passthrough_window
            {
                self.super_pending = None;
                return SuperPressResult::DoubleSuper;
            }
            self.super_pending = Some((Instant::now(), *key));
            SuperPressResult::Pending
        } else {
            self.super_pending = None;
            SuperPressResult::Forward
        }
    }

    /// If a pending first-Esc has timed out, return it for forwarding to the
    /// focused window. Called once per frame in the idle event path.
    pub fn take_expired_super_event(&mut self) -> Option<Event> {
        let expired = self.super_pending.is_some_and(|(pressed_at, _)| {
            pressed_at.elapsed() >= self.config.super_passthrough_window
        });
        if expired {
            let (_, key) = self
                .super_pending
                .take()
                .expect("super_pending was just checked");
            Some(Event::Key(key))
        } else {
            None
        }
    }

    /// Time remaining before the drag snap preview is auto-applied.
    /// Returns `None` when the feature is disabled or no drag is active.
    pub fn drag_snap_remaining(&self) -> Option<Duration> {
        let timeout = self.config.drag_snap_timeout?;
        self.drag_header.as_ref()?;
        let last = self.drag_last_event?;
        let elapsed = last.elapsed();
        if elapsed >= timeout {
            return Some(Duration::ZERO);
        }
        Some(timeout.saturating_sub(elapsed))
    }

    /// If the mouse has left the terminal during a header drag (no events received
    /// within `drag_snap_timeout`), auto-apply the pending snap.
    /// Returns `true` when the snap was applied.
    pub fn take_expired_drag_snap(&mut self) -> bool {
        let timeout = match self.config.drag_snap_timeout {
            Some(t) => t,
            None => return false,
        };
        let Some(drag) = self.drag_header else {
            return false;
        };
        let Some(last) = self.drag_last_event else {
            return false;
        };
        if last.elapsed() < timeout {
            return false;
        }
        self.drag_header = None;
        self.drag_last_event = None;
        self.apply_snap(drag.id);
        true
    }

    /// Time remaining before a deferred first-Esc is forwarded to the terminal.
    /// Returns `None` when no Esc is pending.
    pub fn super_pending_remaining(&self) -> Option<Duration> {
        let (pressed_at, _) = self.super_pending.as_ref()?;
        let elapsed = pressed_at.elapsed();
        if elapsed >= self.config.super_passthrough_window {
            return None;
        }
        Some(self.config.super_passthrough_window.saturating_sub(elapsed))
    }

    pub fn super_passthrough_active(&self) -> bool {
        self.super_passthrough_remaining().is_some()
    }

    pub fn super_passthrough_remaining(&self) -> Option<Duration> {
        if !self.wm_overlay_visible() {
            return None;
        }
        let opened_at = self.overlay_opened_at?;
        let elapsed = opened_at.elapsed();
        if elapsed >= self.config.super_passthrough_window {
            return None;
        }
        Some(self.config.super_passthrough_window.saturating_sub(elapsed))
    }

    pub fn render_panel(&mut self, frame: &mut UiFrame<'_>) {
        let status_line = if self.wm_overlay_visible() {
            let esc_state = if let Some(remaining) = self.super_passthrough_remaining() {
                format!("Super passthrough: active ({}ms)", remaining.as_millis())
            } else {
                "Super passthrough: inactive".to_string()
            };
            Some(format!("{esc_state} · Tab/Shift-Tab: cycle windows"))
        } else {
            self.super_pending_remaining().map(|remaining| {
                format!(
                    "Super pending: {}ms · press Super again within window to open menu",
                    remaining.as_millis()
                )
            })
        };
        let display = self.build_display_order();
        let titles_map: std::collections::BTreeMap<WindowKey, String> =
            self.window_titles().into_iter().collect();
        let selection_copy_available = self.selection_text.is_some();
        let panel_active = self.panel_active();
        let focus_current = *self.focus.current();
        let mouse_capture_enabled = self.mouse_capture_enabled();
        let clipboard_enabled = self.clipboard_enabled();
        let window_selection_enabled = self.window_selection_enabled();
        let selection_active = self.selection_active();
        let selection_dragging = self.selection_dragging();
        let selection_copied = self.selection_copied();
        let wm_overlay_visible = self.wm_overlay_visible();
        let label_for = &move |id| {
            titles_map
                .get(&id)
                .cloned()
                .unwrap_or_else(|| format!("{:?}", id))
        };
        if let Some(p) = &mut self.top_panel {
            p.render(
                frame,
                panel_active,
                focus_current,
                &display,
                status_line.as_deref(),
                mouse_capture_enabled,
                clipboard_enabled,
                window_selection_enabled,
                selection_active,
                selection_dragging,
                selection_copy_available,
                selection_copied,
                wm_overlay_visible,
                label_for,
                &self.config.theme,
            );
        }
        if let Some(p) = &mut self.bottom_panel {
            p.render(frame, panel_active, &self.config.theme);
        }
    }
}

pub fn wm_menu_items(
    mouse_capture_enabled: bool,
    clipboard_enabled: bool,
    window_selection_enabled: bool,
) -> Vec<MenuItem<WmMenuAction>> {
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
            action: WmMenuAction::CloseMenu,
        },
        MenuItem {
            label: mouse_label,
            icon: Some("◆"),
            action: WmMenuAction::ToggleMouseCapture,
        },
        MenuItem {
            label: clipboard_label,
            icon: Some("■"),
            action: WmMenuAction::ToggleClipboardMode,
        },
        MenuItem {
            label: selection_label,
            icon: Some("●"),
            action: WmMenuAction::ToggleWindowSelection,
        },
        MenuItem {
            label: "Floating Front",
            icon: Some("↑"),
            action: WmMenuAction::BringFloatingFront,
        },
        MenuItem {
            label: "New Window",
            icon: Some("+"),
            action: WmMenuAction::NewWindow,
        },
        MenuItem {
            label: "Debug Log",
            icon: Some("≣"),
            action: WmMenuAction::ToggleDebugWindow,
        },
        MenuItem {
            label: "Help",
            icon: Some("?"),
            action: WmMenuAction::Help,
        },
        MenuItem {
            label: "Exit UI",
            icon: Some("⏻"),
            action: WmMenuAction::ExitUi,
        },
    ]
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
        width: x1.saturating_sub(x0),
        height: y1.saturating_sub(y0),
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

fn map_layout_node(node: &LayoutNode<WindowKey>) -> LayoutNode<WindowKey> {
    match node {
        LayoutNode::Leaf(id) => LayoutNode::leaf(*id),
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
mod tests {}
