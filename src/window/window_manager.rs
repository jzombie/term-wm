use super::Window;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, MouseEventKind};
use ratatui::prelude::Rect;
use ratatui::widgets::Clear;

use super::decorator::{DefaultDecorator, HeaderAction, WindowDecorator};
use crate::components::{
    Component, ConfirmAction, ConfirmOverlay, DebugLogComponent, DialogOverlay, install_panic_hook,
    set_global_debug_log,
};
use crate::layout::floating::*;
use crate::layout::{
    FloatingPane, InsertPosition, LayoutNode, LayoutPlan, RectSpec, RegionMap, SplitHandle,
    TilingLayout, rect_contains, render_handles_masked,
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
}

#[derive(Debug, Clone, Copy)]
pub struct AppWindowDraw<R: Copy + Eq + Ord> {
    pub id: R,
    pub surface: WindowSurface,
    pub focused: bool,
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
    drag_resize: Option<ResizeDrag<WindowId<R>>>,
    hover: Option<(u16, u16)>,
    capture_deadline: Option<Instant>,
    pending_deadline: Option<Instant>,
    state: AppState,
    layout_contract: LayoutContract,
    wm_overlay_opened_at: Option<Instant>,
    esc_passthrough_window: Duration,
    wm_overlay: DialogOverlay,
    exit_confirm: ConfirmOverlay,
    decorator: Box<dyn WindowDecorator>,
    floating_resize_offscreen: bool,
    z_order: Vec<WindowId<R>>,
    drag_snap: Option<(Option<WindowId<R>>, InsertPosition, Rect)>,
    debug_log: DebugLogComponent,
    debug_log_id: WindowId<R>,
    next_window_seq: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WmMenuAction {
    CloseMenu,
    NewWindow,
    ToggleDebugWindow,
    ExitUi,
    BringFloatingFront,
    MinimizeWindow,
    MaximizeWindow,
    CloseWindow,
    ToggleMouseCapture,
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

    fn floating_rect(&self, id: WindowId<R>) -> Option<RectSpec> {
        self.window(id).and_then(|window| window.floating_rect)
    }

    fn set_floating_rect(&mut self, id: WindowId<R>, rect: Option<RectSpec>) {
        self.window_mut(id).floating_rect = rect;
    }

    fn clear_floating_rect(&mut self, id: WindowId<R>) {
        self.window_mut(id).floating_rect = None;
    }

    fn set_prev_floating_rect(&mut self, id: WindowId<R>, rect: Option<RectSpec>) {
        self.window_mut(id).prev_floating_rect = rect;
    }

    fn take_prev_floating_rect(&mut self, id: WindowId<R>) -> Option<RectSpec> {
        self.window_mut(id).prev_floating_rect.take()
    }
    fn is_window_floating(&self, id: WindowId<R>) -> bool {
        self.window(id).is_some_and(|window| window.is_floating())
    }

    fn window_title(&self, id: WindowId<R>) -> String {
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

    pub fn new(current: W) -> Self {
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
            drag_resize: None,
            hover: None,
            capture_deadline: None,
            pending_deadline: None,
            state: AppState::new(),
            layout_contract: LayoutContract::AppManaged,
            wm_overlay_opened_at: None,
            esc_passthrough_window: esc_passthrough_window_default(),
            wm_overlay: DialogOverlay::new(),
            exit_confirm: ConfirmOverlay::new(),
            decorator: Box::new(DefaultDecorator),
            floating_resize_offscreen: true,
            z_order: Vec::new(),
            drag_snap: None,
            debug_log: {
                let (component, handle) = DebugLogComponent::new_default();
                let _ = set_global_debug_log(handle);
                install_panic_hook();
                component
            },
            debug_log_id: WindowId::system(SystemWindowId::DebugLog),
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
        if crate::components::take_panic_pending() {
            self.state.set_debug_log_visible(true);
            self.ensure_debug_log_in_layout();
            self.bring_to_front_id(self.debug_log_id);
            self.set_wm_focus(self.debug_log_id);
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
        self.wm_overlay.set_visible(false);
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
        self.wm_overlay.set_visible(true);
        self.state.set_wm_menu_selected(0);
    }

    pub fn close_wm_overlay(&mut self) {
        self.state.set_overlay_visible(false);
        self.wm_overlay_opened_at = None;
        self.wm_overlay.set_visible(false);
        self.state.set_wm_menu_selected(0);
    }

    pub fn open_exit_confirm(&mut self) {
        self.exit_confirm.open(
            "Exit App",
            "Exit the application?\nUnsaved changes will be lost.",
        );
    }

    pub fn close_exit_confirm(&mut self) {
        self.exit_confirm.close();
    }

    pub fn exit_confirm_visible(&self) -> bool {
        self.exit_confirm.visible()
    }

    pub fn wm_overlay_visible(&self) -> bool {
        self.state.overlay_visible()
    }

    pub fn toggle_debug_window(&mut self) {
        self.state.toggle_debug_log_visible();
        if self.state.debug_log_visible() {
            self.ensure_debug_log_in_layout();
            self.bring_to_front_id(self.debug_log_id);
            self.set_wm_focus(self.debug_log_id);
        } else {
            self.remove_debug_log_from_layout();
            if self.wm_focus.current() == self.debug_log_id {
                self.select_fallback_focus();
            }
        }
    }

    fn ensure_debug_log_in_layout(&mut self) {
        if self.layout_contract != LayoutContract::WindowManaged {
            return;
        }
        if self.layout_contains(self.debug_log_id) {
            return;
        }
        if self.managed_layout.is_none() {
            self.managed_layout = Some(TilingLayout::new(LayoutNode::leaf(self.debug_log_id)));
            return;
        }
        let _ = self.tile_window_id(self.debug_log_id);
    }

    fn remove_debug_log_from_layout(&mut self) {
        self.clear_floating_rect(self.debug_log_id);
        if let Some(layout) = &mut self.managed_layout {
            if matches!(layout.root(), LayoutNode::Leaf(id) if *id == self.debug_log_id) {
                self.managed_layout = None;
            } else {
                layout.root_mut().remove_leaf(self.debug_log_id);
            }
        }
        self.z_order.retain(|id| *id != self.debug_log_id);
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

    pub fn set_wm_focus(&mut self, focus: WindowId<R>) {
        self.wm_focus.set_current(focus);
        if let Some(app_id) = focus.as_app()
            && let Some(app_focus) = self.focus_for_region(app_id)
        {
            self.app_focus.set_current(app_focus);
        }
    }

    pub fn set_wm_focus_order(&mut self, order: Vec<WindowId<R>>) {
        self.wm_focus.set_order(order);
        if !self.wm_focus.order.is_empty() && !self.wm_focus.order.contains(&self.wm_focus.current)
        {
            self.wm_focus.current = self.wm_focus.order[0];
        }
    }

    pub fn advance_wm_focus(&mut self, forward: bool) {
        self.wm_focus.advance(forward);
        if let Some(app_id) = self.wm_focus.current().as_app()
            && let Some(app_focus) = self.focus_for_region(app_id)
        {
            self.app_focus.set_current(app_focus);
        }
    }

    fn select_fallback_focus(&mut self) {
        if let Some(fallback) = self.wm_focus.order.first().copied() {
            self.set_wm_focus(fallback);
        }
    }

    pub fn bring_focus_to_front<F>(&mut self, map_focus: F)
    where
        F: Fn(W) -> Option<R>,
    {
        if self.layout_contract != LayoutContract::WindowManaged {
            return;
        }
        let _ = map_focus;
        let focused = self.wm_focus.current();
        self.bring_floating_to_front_id(focused);
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
        if self.state.debug_log_visible() {
            self.ensure_debug_log_in_layout();
        }
    }

    pub fn set_panel_visible(&mut self, visible: bool) {
        self.panel.set_visible(visible);
    }

    pub fn set_panel_height(&mut self, height: u16) {
        self.panel.set_height(height);
    }

    pub fn register_managed_layout(&mut self, area: Rect) {
        let (_, managed_area) = self.panel.split_area(self.panel_active(), area);
        self.managed_area = managed_area;
        self.clamp_floating_to_bounds();
        if self.state.debug_log_visible() {
            self.ensure_debug_log_in_layout();
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
            self.resize_handles.extend(resize_handles_for_region(
                floating_id,
                rect,
                self.managed_area,
            ));
            if let Some(header) = floating_header_for_region(floating_id, rect, self.managed_area) {
                self.floating_headers.push(header);
            }
            active_ids.push(floating_id);
        }

        self.z_order.retain(|id| active_ids.contains(id));
        for id in active_ids {
            if !self.z_order.contains(&id) {
                self.z_order.push(id);
            }
        }
        self.managed_draw_order = self.z_order.clone();
        self.set_wm_focus_order(self.managed_draw_order.clone());
        self.managed_draw_order_app = self
            .managed_draw_order
            .iter()
            .filter_map(|id| id.as_app())
            .collect();
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
    pub fn set_app_title(&mut self, id: R, title: impl Into<String>) {
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
            } else if let Some(id) = self.panel.hit_test_window(event) {
                // If the clicked window is minimized, restore it first so it appears
                // in the layout; otherwise just focus and bring to front.
                if self.is_minimized(id) {
                    self.restore_minimized(id);
                }
                self.set_wm_focus(id);
                self.bring_floating_to_front_id(id);
            }
            return true;
        }
        if self.state.debug_log_visible() {
            match event {
                Event::Mouse(mouse) => {
                    let rect = self.full_region_for_id(self.debug_log_id);
                    if rect_contains(rect, mouse.column, mouse.row) {
                        if matches!(mouse.kind, MouseEventKind::Down(_)) {
                            self.set_wm_focus(self.debug_log_id);
                            self.bring_floating_to_front_id(self.debug_log_id);
                        }
                        if self.debug_log.handle_event(event) {
                            return true;
                        }
                    } else if matches!(mouse.kind, MouseEventKind::Down(_))
                        && self.wm_focus.current() == self.debug_log_id
                    {
                        self.select_fallback_focus();
                    }
                }
                Event::Key(_) if self.wm_focus.current() == self.debug_log_id => {
                    if self.debug_log.handle_event(event) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        if let Event::Mouse(mouse) = event {
            self.hover = Some((mouse.column, mouse.row));
        }
        if self.handle_resize_event(event) {
            return true;
        }
        if self.handle_header_drag_event(event) {
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
        let full = RectSpec::Absolute(self.managed_area);
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
            RectSpec::Absolute(rect)
        } else {
            RectSpec::Percent {
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
                            return true;
                        }
                        HeaderAction::Maximize => {
                            self.toggle_maximize(header.id);
                            return true;
                        }
                        HeaderAction::Close => {
                            self.close_window(header.id);
                            return true;
                        }
                        HeaderAction::Drag => {
                            // Proceed to drag below
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

                    self.drag_header = Some(HeaderDrag {
                        id: header.id,
                        offset_x: mouse.column.saturating_sub(rect.x),
                        offset_y: mouse.row.saturating_sub(rect.y),
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
                            drag.offset_x,
                            drag.offset_y,
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
                    self.drag_resize = Some(ResizeDrag {
                        id: handle.id,
                        edge: handle.edge,
                        start_rect: rect,
                        start_col: mouse.column,
                        start_row: mouse.row,
                    });
                    return true;
                }
            }
            MouseEventKind::Drag(_) => {
                if let Some(drag) = self.drag_resize.as_ref()
                    && self.is_window_floating(drag.id)
                {
                    let resized = apply_resize_drag(
                        drag.start_rect,
                        drag.edge,
                        mouse.column,
                        mouse.row,
                        drag.start_col,
                        drag.start_row,
                        self.managed_area,
                        self.floating_resize_offscreen,
                    );
                    self.set_floating_rect(drag.id, Some(RectSpec::Absolute(resized)));
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
            Some(RectSpec::Absolute(Rect {
                x,
                y,
                width,
                height,
            })),
        );
        self.bring_to_front_id(id);
        true
    }

    fn layout_contains(&self, id: WindowId<R>) -> bool {
        self.managed_layout
            .as_ref()
            .is_some_and(|layout| layout.root().subtree_any(|node_id| node_id == id))
    }

    fn move_floating(
        &mut self,
        id: WindowId<R>,
        column: u16,
        row: u16,
        offset_x: u16,
        offset_y: u16,
    ) {
        let panel_active = self.panel_active();
        let bounds = self.managed_area;
        let Some(RectSpec::Absolute(rect)) = self.floating_rect(id) else {
            return;
        };
        let width = rect.width.max(1);
        let height = rect.height.max(1);
        let x = column.saturating_sub(offset_x);
        let mut y = row.saturating_sub(offset_y);
        if panel_active && y < bounds.y {
            y = bounds.y;
        }
        self.set_floating_rect(
            id,
            Some(RectSpec::Absolute(Rect {
                x,
                y,
                width,
                height,
            })),
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
                    self.set_floating_rect(id, Some(RectSpec::Absolute(preview)));
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
            self.bring_to_front_id(id);
            return true;
        }
        if self.managed_layout.is_none() {
            self.managed_layout = Some(TilingLayout::new(LayoutNode::leaf(id)));
            self.bring_to_front_id(id);
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
                self.bring_to_front_id(id);
                return true;
            }
        }

        // Fallback: split root
        layout.split_root(id, InsertPosition::Right);
        self.bring_to_front_id(id);
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
        let mut updates: Vec<(WindowId<R>, RectSpec)> = Vec::new();
        let floating_ids: Vec<WindowId<R>> = self
            .windows
            .iter()
            .filter_map(|(&id, window)| window.floating_rect.as_ref().map(|_| id))
            .collect();
        for id in floating_ids {
            let Some(RectSpec::Absolute(rect)) = self.floating_rect(id) else {
                continue;
            };
            if rects_intersect(rect, bounds) {
                continue;
            }
            // Only recover panes that are fully off-screen; keep normal dragging untouched.
            let rect_right = rect.x.saturating_add(rect.width);
            let rect_bottom = rect.y.saturating_add(rect.height);
            let bounds_right = bounds.x.saturating_add(bounds.width);
            let bounds_bottom = bounds.y.saturating_add(bounds.height);
            // Clamp only the axis that is fully outside the viewport.
            let out_x = rect_right <= bounds.x || rect.x >= bounds_right;
            let out_y = rect_bottom <= bounds.y || rect.y >= bounds_bottom;
            let min_w = FLOATING_MIN_WIDTH.min(bounds.width.max(1));
            let min_h = FLOATING_MIN_HEIGHT.min(bounds.height.max(1));

            // Ensure at least a small portion of the window (e.g. handle) is always visible
            // so the user can grab it back.
            let min_visible_margin = 4u16;

            let width = if self.floating_resize_offscreen {
                rect.width.max(min_w)
            } else {
                rect.width.max(min_w).min(bounds.width)
            };
            let height = if self.floating_resize_offscreen {
                rect.height.max(min_h)
            } else {
                rect.height.max(min_h).min(bounds.height)
            };

            let max_x = if self.floating_resize_offscreen {
                bounds
                    .x
                    .saturating_add(bounds.width)
                    .saturating_sub(min_visible_margin.min(width))
            } else {
                bounds.x.saturating_add(bounds.width.saturating_sub(width))
            };

            let max_y = if self.floating_resize_offscreen {
                bounds.y.saturating_add(bounds.height).saturating_sub(1) // Header is usually top line
            } else {
                bounds
                    .y
                    .saturating_add(bounds.height.saturating_sub(height))
            };

            let x = if out_x || !self.floating_resize_offscreen {
                rect.x.clamp(bounds.x, max_x)
            } else {
                rect.x.max(bounds.x).min(max_x)
            };

            let y = if out_y || !self.floating_resize_offscreen {
                rect.y.clamp(bounds.y, max_y)
            } else {
                rect.y.max(bounds.y).min(max_y)
            };
            updates.push((
                id,
                RectSpec::Absolute(Rect {
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

    pub fn window_draw_plan(&mut self, frame: &mut UiFrame<'_>) -> Vec<AppWindowDraw<R>> {
        let mut plan = Vec::new();
        let focused_app = self.wm_focus.current().as_app();
        for &id in &self.managed_draw_order {
            let full = self.full_region_for_id(id);
            if full.width == 0 || full.height == 0 {
                continue;
            }
            frame.render_widget(Clear, full);
            let WindowId::App(app_id) = id else {
                continue;
            };
            let inner = self.region(app_id);
            if inner.width == 0 || inner.height == 0 {
                continue;
            }
            plan.push(AppWindowDraw {
                id: app_id,
                surface: WindowSurface { full, inner },
                focused: focused_app == Some(app_id),
            });
        }
        plan
    }

    pub fn render_overlays(&mut self, frame: &mut UiFrame<'_>) {
        let hovered = self.hover.and_then(|(column, row)| {
            self.handles
                .iter()
                .find(|handle| rect_contains(handle.rect, column, row))
        });
        let hovered_resize = self.hover.and_then(|(column, row)| {
            self.resize_handles
                .iter()
                .find(|handle| rect_contains(handle.rect, column, row))
        });
        let obscuring: Vec<Rect> = self
            .managed_draw_order
            .iter()
            .filter_map(|&id| self.regions.get(id))
            .collect();
        let is_obscured =
            |x: u16, y: u16| -> bool { obscuring.iter().any(|r| rect_contains(*r, x, y)) };
        render_handles_masked(frame, &self.handles, hovered, is_obscured);
        let focused = self.wm_focus.current();

        for (i, &id) in self.managed_draw_order.iter().enumerate() {
            let Some(rect) = self.regions.get(id) else {
                continue;
            };
            if rect.width < 3 || rect.height < 3 {
                continue;
            }

            if id == self.debug_log_id && self.state.debug_log_visible() {
                let area = self.region_for_id(id);
                if area.width > 0 && area.height > 0 {
                    self.debug_log.render(frame, area, id == focused);
                }
            }

            // Collect obscuring rects (windows above this one)
            let obscuring: Vec<Rect> = self.managed_draw_order[i + 1..]
                .iter()
                .filter_map(|&above_id| self.regions.get(above_id))
                .collect();

            let is_obscured =
                |x: u16, y: u16| -> bool { obscuring.iter().any(|r| rect_contains(*r, x, y)) };

            let title = self.window_title(id);
            let focused_window = id == focused;
            self.decorator.render_window(
                frame,
                rect,
                self.managed_area,
                &title,
                focused_window,
                &is_obscured,
            );
        }

        // Build floating panes list from per-window entries for resize outline rendering
        let floating_panes: Vec<FloatingPane<WindowId<R>>> = self
            .windows
            .iter()
            .filter_map(|(&id, window)| window.floating_rect.map(|rect| FloatingPane { id, rect }))
            .collect();

        render_resize_outline(
            frame,
            hovered_resize.map(|handle| handle.id),
            self.drag_resize.as_ref().map(|drag| drag.id),
            &self.regions,
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
            self.wm_overlay_visible(),
            move |id| {
                titles_map.get(&id).cloned().unwrap_or_else(|| match id {
                    WindowId::App(app_id) => format!("{:?}", app_id),
                    WindowId::System(SystemWindowId::DebugLog) => "Debug Log".to_string(),
                })
            },
        );
        let menu_labels = wm_menu_items(self.mouse_capture_enabled())
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
        if self.exit_confirm.visible() {
            self.exit_confirm.render(frame, frame.area(), false);
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
            if let Some(rect) = self.regions.get(WindowId::app(*id))
                && rect_contains(rect, column, row)
            {
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
            if let Some(rect) = self.regions.get(*id)
                && rect_contains(rect, column, row)
            {
                return Some(*id);
            }
        }
        None
    }

    pub fn handle_focus_event<F>(&mut self, event: &Event, hit_targets: &[R], map: F) -> bool
    where
        F: Fn(R) -> W,
    {
        match event {
            Event::Key(key) => match key.code {
                KeyCode::Tab => {
                    if self.layout_contract == LayoutContract::WindowManaged {
                        self.advance_wm_focus(true);
                    } else {
                        self.app_focus.advance(true);
                    }
                    true
                }
                KeyCode::BackTab => {
                    if self.layout_contract == LayoutContract::WindowManaged {
                        self.advance_wm_focus(false);
                    } else {
                        self.app_focus.advance(false);
                    }
                    true
                }
                _ => false,
            },
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
        if let Event::Mouse(mouse) = event
            && matches!(mouse.kind, MouseEventKind::Down(_))
        {
            if let Some(index) = self.panel.hit_test_menu_item(event) {
                let items = wm_menu_items(self.mouse_capture_enabled());
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
        match key.code {
            KeyCode::Up => {
                let total = wm_menu_items(self.mouse_capture_enabled()).len();
                if total > 0 {
                    let current = self.state.wm_menu_selected();
                    if current == 0 {
                        self.state.set_wm_menu_selected(total - 1);
                    } else {
                        self.state.set_wm_menu_selected(current - 1);
                    }
                }
                None
            }
            KeyCode::Down => {
                let total = wm_menu_items(self.mouse_capture_enabled()).len();
                if total > 0 {
                    let current = self.state.wm_menu_selected();
                    self.state.set_wm_menu_selected((current + 1) % total);
                }
                None
            }
            KeyCode::Char('k') => {
                let total = wm_menu_items(self.mouse_capture_enabled()).len();
                if total > 0 {
                    let current = self.state.wm_menu_selected();
                    if current == 0 {
                        self.state.set_wm_menu_selected(total - 1);
                    } else {
                        self.state.set_wm_menu_selected(current - 1);
                    }
                }
                None
            }
            KeyCode::Char('j') => {
                let total = wm_menu_items(self.mouse_capture_enabled()).len();
                if total > 0 {
                    let current = self.state.wm_menu_selected();
                    self.state.set_wm_menu_selected((current + 1) % total);
                }
                None
            }
            KeyCode::Enter => wm_menu_items(self.mouse_capture_enabled())
                .get(self.state.wm_menu_selected())
                .map(|item| item.action),
            _ => None,
        }
    }

    pub fn handle_exit_confirm_event(&mut self, event: &Event) -> Option<ConfirmAction> {
        if !self.exit_confirm.visible() {
            return None;
        }
        self.exit_confirm.handle_confirm_event(event)
    }

    pub fn wm_menu_consumes_event(&self, event: &Event) -> bool {
        if !self.wm_overlay_visible() {
            return false;
        }
        let Event::Key(key) = event else {
            return false;
        };
        matches!(
            key.code,
            KeyCode::Up | KeyCode::Down | KeyCode::Enter | KeyCode::Char('j') | KeyCode::Char('k')
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct WmMenuItem {
    label: &'static str,
    icon: Option<&'static str>,
    action: WmMenuAction,
}
fn wm_menu_items(mouse_capture_enabled: bool) -> [WmMenuItem; 6] {
    let mouse_label = if mouse_capture_enabled {
        "Mouse Capture: On"
    } else {
        "Mouse Capture: Off"
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
mod tests {
    use super::*;
    use ratatui::layout::Rect;

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
}
