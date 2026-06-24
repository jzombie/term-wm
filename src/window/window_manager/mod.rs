mod chrome;
mod drag;
mod focus;
mod layout;
mod overlays;
mod sys;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::Event;
use ratatui::prelude::Rect;

use super::FocusRing;
use super::decorator::WindowDecorator;
use super::entry::Window;
use crate::clipboard;
use crate::components::{
    Component, ComponentContext, Overlay,
    sys::debug_log::{DebugLogComponent, install_panic_hook, set_global_debug_log},
};
use crate::layout::floating::*;
use crate::layout::{InsertPosition, LayoutNode, RegionMap, SplitHandle, TilingLayout};
use crate::panel::Panel;
use crate::ui::UiFrame;
use crate::wm_config::WmConfig;
use crate::wm_state::WmState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SystemWindowId {
    DebugLog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OverlayId {
    Help,
    ExitConfirm,
    SelectionPreview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WindowId<Id: Copy + Eq + Ord> {
    App(Id),
    System(SystemWindowId),
}

impl<Id: Copy + Eq + Ord> WindowId<Id> {
    fn app(id: Id) -> Self {
        Self::App(id)
    }

    fn system(id: SystemWindowId) -> Self {
        Self::System(id)
    }

    fn as_app(self) -> Option<Id> {
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
pub struct WindowDrawContext<Id: Copy + Eq + Ord> {
    pub id: Id,
    pub surface: WindowSurface,
    pub focused: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum DrawTask<Id: Copy + Eq + Ord> {
    App(WindowDrawContext<Id>),
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

pub struct WindowManager<Id: Copy + Eq + Ord + std::fmt::Debug> {
    app_focus: FocusRing<Id>,
    wm_focus: FocusRing<WindowId<Id>>,
    windows: BTreeMap<WindowId<Id>, Window>,
    pub(crate) regions: RegionMap<WindowId<Id>>,
    scroll: BTreeMap<Id, ScrollState>,
    pub(crate) handles: Vec<SplitHandle>,
    pub(crate) resize_handles: Vec<ResizeHandle<WindowId<Id>>>,
    pub(crate) floating_headers: Vec<DragHandle<WindowId<Id>>>,
    pub(crate) managed_draw_order: Vec<WindowId<Id>>,
    managed_draw_order_app: Vec<Id>,
    pub(crate) managed_layout: Option<TilingLayout<WindowId<Id>>>,
    closed_app_windows: Vec<Id>,
    pub(crate) managed_area: Rect,
    panel: Panel<WindowId<Id>>,
    pub(crate) drag_header: Option<HeaderDrag<WindowId<Id>>>,
    pub(crate) last_header_click: Option<(WindowId<Id>, Instant)>,
    pub(crate) drag_resize: Option<ResizeDrag<WindowId<Id>>>,
    pub(crate) hover: Option<(u16, u16)>,
    capture_deadline: Option<Instant>,
    pending_deadline: Option<Instant>,
    state: WmState,
    clipboard_available: bool,
    selection_active: bool,
    selection_dragging: bool,
    selection_text: Option<String>,
    selection_copied: bool,
    selection_copied_text: Option<String>,
    selection_preview_restore_mouse: Option<bool>,
    config: WmConfig<Id>,
    wm_overlay_opened_at: Option<Instant>,
    pub(crate) last_frame_area: ratatui::prelude::Rect,
    overlays: BTreeMap<OverlayId, Box<dyn Overlay>>,
    scroll_keyboard_enabled_default: bool,
    pub(crate) decorator: Arc<dyn WindowDecorator>,
    floating_resize_offscreen: bool,
    pub(crate) z_order: Vec<WindowId<Id>>,
    pub(crate) drag_snap: Option<(Option<WindowId<Id>>, InsertPosition, Rect)>,
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

impl<Id: Copy + Eq + Ord + std::fmt::Debug + 'static> WindowManager<Id> {
    fn window_mut(&mut self, id: WindowId<Id>) -> &mut Window {
        let seq = &mut self.next_window_seq;
        self.windows.entry(id).or_insert_with(|| {
            let order = *seq;
            *seq = order.saturating_add(1);
            tracing::debug!(window_id = ?id, seq = order, "opened window");
            Window::new(order)
        })
    }

    fn window(&self, id: WindowId<Id>) -> Option<&Window> {
        self.windows.get(&id)
    }

    fn is_minimized(&self, id: WindowId<Id>) -> bool {
        self.window(id).is_some_and(|window| window.minimized)
    }

    fn set_minimized(&mut self, id: WindowId<Id>, value: bool) {
        self.window_mut(id).minimized = value;
    }

    fn floating_rect(&self, id: WindowId<Id>) -> Option<crate::window::FloatRectSpec> {
        self.window(id).and_then(|window| window.floating_rect)
    }

    fn set_floating_rect(&mut self, id: WindowId<Id>, rect: Option<crate::window::FloatRectSpec>) {
        self.window_mut(id).floating_rect = rect;
    }

    fn clear_floating_rect(&mut self, id: WindowId<Id>) {
        self.window_mut(id).floating_rect = None;
    }

    fn set_prev_floating_rect(
        &mut self,
        id: WindowId<Id>,
        rect: Option<crate::window::FloatRectSpec>,
    ) {
        self.window_mut(id).prev_floating_rect = rect;
    }

    fn take_prev_floating_rect(
        &mut self,
        id: WindowId<Id>,
    ) -> Option<crate::window::FloatRectSpec> {
        self.window_mut(id).prev_floating_rect.take()
    }

    fn is_window_floating(&self, id: WindowId<Id>) -> bool {
        self.window(id).is_some_and(|window| window.is_floating())
    }

    pub fn window_title(&self, id: WindowId<Id>) -> String {
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

    pub fn new(current: Id) -> Self {
        Self::with_config(current, WmConfig::embedded())
    }

    pub fn new_managed(current: Id) -> Self {
        Self::with_config(current, WmConfig::standalone())
    }

    pub fn with_config(current: Id, config: WmConfig<Id>) -> Self {
        let clipboard_available = clipboard::available();
        let mut state = WmState::new();
        if !clipboard_available {
            state.set_clipboard_enabled(false);
        }
        let selection_enabled = state.clipboard_enabled();
        let decorator = config.decorator();
        let floating_resize_offscreen = config.floating_resize_offscreen;
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
            selection_active: false,
            selection_dragging: false,
            selection_text: None,
            selection_copied: false,
            selection_copied_text: None,
            selection_preview_restore_mouse: None,
            config,
            wm_overlay_opened_at: None,
            last_frame_area: Rect::default(),
            overlays: BTreeMap::new(),
            scroll_keyboard_enabled_default: true,
            decorator,
            floating_resize_offscreen,
            z_order: Vec::new(),
            drag_snap: None,
            system_windows: {
                let (mut component, handle) = DebugLogComponent::new_default();
                component.set_selection_enabled(selection_enabled);
                set_global_debug_log(handle);
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

    pub fn take_closed_app_windows(&mut self) -> Vec<Id> {
        std::mem::take(&mut self.closed_app_windows)
    }

    pub fn config(&self) -> &WmConfig<Id> {
        &self.config
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
        if crate::components::sys::debug_log::take_panic_pending() {
            self.show_system_window(SystemWindowId::DebugLog);
        }
        if !self.config.wm_overlay_enabled {
            self.clear_capture();
        } else {
            self.refresh_capture();
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
        self.state.set_overlay_visible(false);
        self.wm_overlay_opened_at = None;
        self.state.set_wm_menu_selected(0);
    }

    pub fn capture_active(&mut self) -> bool {
        if !self.state.mouse_capture_enabled() {
            return false;
        }
        if self.config.wm_overlay_enabled && self.state.overlay_visible() {
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

    fn copy_selection_to_clipboard(&mut self) {
        if !self.clipboard_available || !self.clipboard_enabled() {
            return;
        }
        let Some(text) = self.selection_text.clone() else {
            return;
        };
        if clipboard::set(&text).is_ok() {
            self.selection_copied = true;
            self.selection_copied_text = Some(text);
        }
    }

    fn open_selection_preview_from_selection(&mut self) {
        let Some(text) = self.selection_text.clone() else {
            return;
        };
        self.open_selection_preview(text);
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

    pub fn set_scroll_keyboard_enabled(&mut self, enabled: bool) {
        self.scroll_keyboard_enabled_default = enabled;
    }

    fn panel_active(&self) -> bool {
        self.config.panel_enabled && self.panel.visible() && self.panel.height() > 0
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
        if elapsed >= self.config.esc_passthrough_window {
            return None;
        }
        Some(self.config.esc_passthrough_window.saturating_sub(elapsed))
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

fn map_layout_node<Id: Copy + Eq + Ord>(node: &LayoutNode<Id>) -> LayoutNode<WindowId<Id>> {
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
        let mut wm = WindowManager::<usize>::new_managed(0);

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

        assert!(matches!(wm.wm_focus.current(), WindowId::System(_)));

        let clicked_col = 6u16;
        let clicked_row = 6u16;
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: clicked_col,
            row: clicked_row,
            modifiers: KeyModifiers::NONE,
        };
        let evt = Event::Mouse(mouse);
        let _handled = wm.handle_managed_event(&evt);
        assert_eq!(wm.wm_focus.current(), WindowId::app(2usize));
    }

    #[test]
    fn enforce_min_visible_margin_horizontal() {
        use crate::window::{FloatRect, FloatRectSpec};
        let mut wm = WindowManager::<usize>::new_managed(0);
        wm.set_floating_resize_offscreen(true);
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
        let mut wm = WindowManager::<usize>::new_managed(0);
        wm.set_floating_resize_offscreen(true);
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
                assert!(fr.y >= 0);
            }
            _ => panic!("expected absolute rect"),
        }
    }

    #[test]
    fn maximize_persists_across_resize() {
        use crate::window::FloatRectSpec;
        let mut wm = WindowManager::<usize>::new_managed(0);
        wm.register_managed_layout(ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 15,
        });
        wm.toggle_maximize(WindowId::app(3usize));
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
                assert_eq!(fr.width, wm.managed_area.width);
                assert_eq!(fr.height, wm.managed_area.height);
            }
            _ => panic!("expected absolute rect"),
        }
    }

    #[test]
    fn localize_event_converts_to_local_coords() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let mut wm = WindowManager::<usize>::new_managed(0);
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
        let window_local = wm
            .localize_event(WindowId::app(1), &event)
            .expect("window-local event");
        if let Event::Mouse(local) = window_local {
            assert_eq!(local.column, 5);
            assert_eq!(local.row, 4);
        } else {
            panic!("expected mouse event");
        }

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
        let mut wm = WindowManager::<usize>::new_managed(0);
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
        let mut wm = WindowManager::<usize>::new_managed(0);
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

        let hit = wm.hit_test_region_topmost(8, 2, &wm.managed_draw_order);
        assert_eq!(hit, Some(WindowId::app(2usize)));
    }

    #[test]
    fn hover_targets_respects_occlusion() {
        use crate::layout::floating::{ResizeEdge, ResizeHandle};
        use crate::layout::tiling::SplitHandle;
        let mut wm = WindowManager::<usize>::new_managed(0);
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

        let mut wm = WindowManager::<usize>::new_managed(0);
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
        let mut wm = WindowManager::<usize>::new_managed(0);
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
        let mut wm = WindowManager::<usize>::new_managed(0);
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
