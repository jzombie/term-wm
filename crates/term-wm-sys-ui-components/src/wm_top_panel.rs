use std::collections::BTreeMap;

use ratatui::style::{Modifier, Style};
use term_wm_core::events::{Event, MouseEventKind};
use term_wm_layout_engine::LayoutRect;

use term_wm_core::{
    actions::{EventResult, TermWmAction},
    components::{
        Component, ComponentAction, ComponentContext, ComponentQuery, ComponentResponse,
        WmComponent,
    },
    layout::rect_contains,
    utils::truncate_to_width,
    window::WindowKey,
};
use term_wm_ui_components::helpers::{color_to_ratatui, layout_rect_to_rect, safe_set_string};

#[derive(Debug, Clone, Copy)]
struct PanelWindowHit {
    id: WindowKey,
    rect: LayoutRect,
}

#[derive(Debug)]
struct WindowList {
    window_hits: Vec<PanelWindowHit>,
}

impl WindowList {
    fn new() -> Self {
        Self {
            window_hits: Vec::new(),
        }
    }

    fn begin_frame(&mut self) {
        self.window_hits.clear();
    }
}

#[derive(Debug)]
struct NotificationArea {
    mouse_capture_rect: Option<LayoutRect>,
    clipboard_rect: Option<LayoutRect>,
    selection_rect: Option<LayoutRect>,
}

impl NotificationArea {
    fn new() -> Self {
        Self {
            mouse_capture_rect: None,
            clipboard_rect: None,
            selection_rect: None,
        }
    }

    fn begin_frame(&mut self) {
        self.mouse_capture_rect = None;
        self.clipboard_rect = None;
        self.selection_rect = None;
    }
}

#[derive(Debug)]
pub struct WmTopPanelComponent {
    visible: bool,
    height: u16,
    area: LayoutRect,
    menu_rect: Option<LayoutRect>,
    list: WindowList,
    notifications: NotificationArea,
    app_name: String,
    // WmComponent render state (pushed via process_action before render)
    active: bool,
    focus_current: Option<WindowKey>,
    display_order: Vec<WindowKey>,
    status_line: Option<String>,
    mouse_capture_enabled: bool,
    clipboard_enabled: bool,
    window_selection_enabled: bool,
    selection_active: bool,
    selection_dragging: bool,
    menu_open: bool,
    window_labels: BTreeMap<WindowKey, String>,
}

impl WmTopPanelComponent {
    pub fn new(app_name: &str) -> Self {
        Self {
            visible: true,
            height: 1,
            area: LayoutRect::default(),
            menu_rect: None,
            list: WindowList::new(),
            notifications: NotificationArea::new(),
            app_name: app_name.to_string(),
            active: false,
            focus_current: None,
            display_order: Vec::new(),
            status_line: None,
            mouse_capture_enabled: false,
            clipboard_enabled: false,
            window_selection_enabled: false,
            selection_active: false,
            selection_dragging: false,
            menu_open: false,
            window_labels: BTreeMap::new(),
        }
    }

    pub fn begin_frame(&mut self) {
        self.list.begin_frame();
        self.menu_rect = None;
        self.notifications.begin_frame();
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn area(&self) -> LayoutRect {
        self.area
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub fn set_height(&mut self, height: u16) {
        self.height = height.max(1);
    }

    pub fn menu_icon_rect(&self) -> Option<LayoutRect> {
        self.menu_rect
    }

    pub fn menu_icon_contains_point(&self, column: u16, row: u16) -> bool {
        if let Some(rect) = self.menu_rect {
            return rect_contains(rect, column, row);
        }
        false
    }

    pub fn split_area(&mut self, active: bool, area: LayoutRect) -> (LayoutRect, LayoutRect) {
        let top_h = if active {
            self.height.min(area.height)
        } else {
            0
        };
        let panel = LayoutRect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: top_h,
        };
        let managed_height = area.height.saturating_sub(top_h);
        let managed = LayoutRect {
            x: area.x,
            y: area.y.saturating_add(i32::from(top_h)),
            width: area.width,
            height: managed_height,
        };
        self.area = panel;
        (panel, managed)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_inner(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        active: bool,
        focus_current: WindowKey,
        display_order: &[WindowKey],
        status_line: Option<&str>,
        mouse_capture_enabled: bool,
        clipboard_enabled: bool,
        window_selection_enabled: bool,
        _selection_active: bool,
        _selection_dragging: bool,
        menu_open: bool,
        theme: &term_wm_core::theme::Theme,
    ) {
        if !active {
            return;
        }
        let area = self.area;
        if area.width == 0 || area.height == 0 {
            return;
        }
        let ratatui_backend = term_wm_ui_components::helpers::downcast_ratatui(backend);
        let buffer = &mut ratatui_backend.buffer;
        let ratatui_area = layout_rect_to_rect(area);
        let bounds = ratatui_area.intersection(buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        for yy in bounds.y..bounds.y.saturating_add(bounds.height) {
            for xx in bounds.x..bounds.x.saturating_add(bounds.width) {
                if let Some(cell) = buffer.cell_mut((xx, yy)) {
                    let mut st = cell.style();
                    st.bg = Some(color_to_ratatui(theme.bottom_panel_bg));
                    st.fg = Some(color_to_ratatui(theme.bottom_panel_fg));
                    cell.set_style(st);
                }
            }
        }
        let mut x = area.x;
        let y = area.y;
        let max_x = area.x.saturating_add(i32::from(area.width));
        let menu_icon = format!("\u{2261} {}", self.app_name);
        let menu_width = menu_icon.chars().count() as u16;
        if x.saturating_add(i32::from(menu_width)) <= max_x {
            let menu_style = if menu_open {
                Style::default()
                    .bg(color_to_ratatui(theme.menu_bg))
                    .fg(color_to_ratatui(theme.menu_fg))
            } else {
                Style::default()
            };
            safe_set_string(
                buffer,
                bounds,
                x as u16,
                y as u16,
                menu_icon.as_str(),
                menu_style,
            );
            self.menu_rect = Some(LayoutRect {
                x,
                y,
                width: menu_width,
                height: 1,
            });
            x = x.saturating_add(i32::from(menu_width));
        }
        if x < max_x {
            safe_set_string(buffer, bounds, x as u16, y as u16, " ", Style::default());
            x = x.saturating_add(1);
        }
        if let Some(status) = status_line {
            let available = (max_x.saturating_sub(x)).max(1);
            let text = truncate_to_width(status, available as usize);
            safe_set_string(buffer, bounds, x as u16, y as u16, &text, Style::default());
        } else {
            for id in display_order.iter().copied() {
                let focused = id == focus_current;
                let mut label = self
                    .window_labels
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| format!("{id:?}"));
                let max_label = (max_x.saturating_sub(x).saturating_sub(2)) as usize;
                if label.chars().count() > max_label {
                    label = truncate_to_width(&label, max_label);
                }
                let chunk = format!(" {label} ");
                let chunk_width = chunk.chars().count() as u16;
                if x.saturating_add(i32::from(chunk_width)) > max_x {
                    break;
                }
                let item_style = if focused {
                    Style::default()
                        .bg(color_to_ratatui(theme.menu_selected_bg))
                        .fg(color_to_ratatui(theme.menu_selected_fg))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(color_to_ratatui(theme.panel_inactive_fg))
                };
                safe_set_string(buffer, bounds, x as u16, y as u16, &chunk, item_style);
                self.list.window_hits.push(PanelWindowHit {
                    id,
                    rect: LayoutRect {
                        x,
                        y,
                        width: chunk_width,
                        height: 1,
                    },
                });
                x = x.saturating_add(i32::from(chunk_width));
            }
        }

        let selection_chunk = "[ selection ]";
        let mouse_chunk = "[ mouse ]";
        let clip_chunk = "[ clipboard ]";
        let selection_width = selection_chunk.chars().count() as u16;
        let mouse_width = mouse_chunk.chars().count() as u16;
        let clip_width = clip_chunk.chars().count() as u16;
        let total_width = selection_width
            .saturating_add(mouse_width)
            .saturating_add(clip_width);
        let indicator_x = if i32::from(total_width) >= i32::from(bounds.width) {
            i32::from(bounds.x)
        } else {
            max_x.saturating_sub(i32::from(total_width))
        };
        if total_width > 0 && indicator_x < max_x {
            let selection_style = if window_selection_enabled {
                Style::default()
                    .fg(color_to_ratatui(theme.success))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(color_to_ratatui(theme.panel_inactive_fg))
            };
            let mouse_style = if mouse_capture_enabled {
                Style::default()
                    .fg(color_to_ratatui(theme.success))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(color_to_ratatui(theme.panel_inactive_fg))
            };
            let clip_style = if clipboard_enabled {
                Style::default()
                    .fg(color_to_ratatui(theme.success))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(color_to_ratatui(theme.panel_inactive_fg))
            };
            let mut cursor = indicator_x;
            if selection_width > 0 && cursor < max_x {
                safe_set_string(
                    buffer,
                    bounds,
                    cursor as u16,
                    y as u16,
                    selection_chunk,
                    selection_style,
                );
                let width = selection_width.min((max_x.saturating_sub(cursor)) as u16);
                if width > 0 {
                    self.notifications.selection_rect = Some(LayoutRect {
                        x: cursor,
                        y,
                        width,
                        height: 1,
                    });
                }
            }
            cursor = cursor.saturating_add(i32::from(selection_width));
            if mouse_width > 0 && cursor < max_x {
                safe_set_string(
                    buffer,
                    bounds,
                    cursor as u16,
                    y as u16,
                    mouse_chunk,
                    mouse_style,
                );
                let width = mouse_width.min((max_x.saturating_sub(cursor)) as u16);
                if width > 0 {
                    self.notifications.mouse_capture_rect = Some(LayoutRect {
                        x: cursor,
                        y,
                        width,
                        height: 1,
                    });
                }
            }
            cursor = cursor.saturating_add(i32::from(mouse_width));
            if clip_width > 0 && cursor < max_x {
                safe_set_string(
                    buffer,
                    bounds,
                    cursor as u16,
                    y as u16,
                    clip_chunk,
                    clip_style,
                );
                let width = clip_width.min((max_x.saturating_sub(cursor)) as u16);
                if width > 0 {
                    self.notifications.clipboard_rect = Some(LayoutRect {
                        x: cursor,
                        y,
                        width,
                        height: 1,
                    });
                }
            }
        }
    }

    pub fn hit_test_menu(&self, event: &Event) -> bool {
        let Event::Mouse(mouse) = event else {
            return false;
        };
        if !matches!(mouse.kind, MouseEventKind::Press(_)) {
            return false;
        }
        if let Some(rect) = self.menu_rect {
            return rect_contains(rect, mouse.column, mouse.row);
        }
        false
    }

    pub fn hit_test_mouse_capture(&self, event: &Event) -> bool {
        let Event::Mouse(mouse) = event else {
            return false;
        };
        if !matches!(mouse.kind, MouseEventKind::Press(_)) {
            return false;
        }
        if let Some(rect) = self.notifications.mouse_capture_rect {
            return rect_contains(rect, mouse.column, mouse.row);
        }
        false
    }

    pub fn hit_test_clipboard(&self, event: &Event) -> bool {
        let Event::Mouse(mouse) = event else {
            return false;
        };
        if !matches!(mouse.kind, MouseEventKind::Press(_)) {
            return false;
        }
        if let Some(rect) = self.notifications.clipboard_rect {
            return rect_contains(rect, mouse.column, mouse.row);
        }
        false
    }

    pub fn hit_test_selection(&self, event: &Event) -> bool {
        let Event::Mouse(mouse) = event else {
            return false;
        };
        if !matches!(mouse.kind, MouseEventKind::Press(_)) {
            return false;
        }
        if let Some(rect) = self.notifications.selection_rect {
            return rect_contains(rect, mouse.column, mouse.row);
        }
        false
    }

    pub fn hit_test_window(&self, event: &Event) -> Option<WindowKey> {
        let Event::Mouse(mouse) = event else {
            return None;
        };
        if !matches!(mouse.kind, MouseEventKind::Press(_)) {
            return None;
        }
        self.list
            .window_hits
            .iter()
            .find(|hit| rect_contains(hit.rect, mouse.column, mouse.row))
            .map(|hit| hit.id)
    }
}

impl Component<TermWmAction> for WmTopPanelComponent {
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        if !self.active {
            return;
        }
        let theme = ctx.config().theme;
        let app_name = ctx.app_name().to_string();
        if app_name != self.app_name {
            self.app_name = app_name;
        }
        self.area = area;
        if let Some(focus) = self.focus_current {
            let display_order = self.display_order.clone();
            let status_line = self.status_line.clone();
            let mc = self.mouse_capture_enabled;
            let cb = self.clipboard_enabled;
            let ws = self.window_selection_enabled;
            let mo = self.menu_open;
            self.render_inner(
                backend,
                self.active,
                focus,
                &display_order,
                status_line.as_deref(),
                mc,
                cb,
                ws,
                self.selection_active,
                self.selection_dragging,
                mo,
                &theme,
            );
        }
    }

    fn handle_events(
        &mut self,
        event: &Event,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        let Event::Mouse(mouse) = event else {
            return EventResult::Ignored;
        };
        if !matches!(mouse.kind, MouseEventKind::Press(_)) {
            return EventResult::Ignored;
        }
        if self.menu_icon_contains_point(mouse.column, mouse.row) {
            return EventResult::Action(TermWmAction::WmToggleOverlay);
        }
        if self.hit_test_mouse_capture(event) {
            return EventResult::Action(TermWmAction::ToggleMouseCapture);
        }
        if self.hit_test_selection(event) {
            return EventResult::Action(TermWmAction::ToggleWindowSelection);
        }
        if self.hit_test_clipboard(event) {
            return EventResult::Action(TermWmAction::ToggleClipboardMode);
        }
        if let Some(key) = self.hit_test_window(event) {
            return EventResult::Action(TermWmAction::FocusWindow(key));
        }
        EventResult::Ignored
    }
}

impl WmComponent for WmTopPanelComponent {
    fn consume_area(&mut self, available: LayoutRect) -> (LayoutRect, LayoutRect) {
        self.split_area(self.active, available)
    }

    fn process_action(&mut self, action: &ComponentAction) {
        match action {
            ComponentAction::ToggleVisibility => {
                self.set_visible(!self.visible);
            }
            ComponentAction::SetHintVisibility(hv) => {
                use term_wm_core::wm_config::HintVisibility;
                match hv {
                    HintVisibility::Always => self.set_visible(true),
                    HintVisibility::Never => self.set_visible(false),
                    HintVisibility::OnDemand => {}
                }
            }
            ComponentAction::SetPanelActive(active) => {
                self.active = *active;
            }
            ComponentAction::SetTopPanelState(state) => {
                self.focus_current = state.focus_current;
                self.display_order = state.display_order.clone();
                self.status_line = state.status_line.clone();
                self.mouse_capture_enabled = state.mouse_capture_enabled;
                self.clipboard_enabled = state.clipboard_enabled;
                self.window_selection_enabled = state.window_selection_enabled;
                self.selection_active = state.selection_active;
                self.selection_dragging = state.selection_dragging;
                self.menu_open = state.menu_open;
            }
            ComponentAction::SetWindowLabels(labels) => {
                self.window_labels = labels.clone();
            }
            _ => {}
        }
    }

    fn query(&self, query: &ComponentQuery) -> ComponentResponse {
        match query {
            ComponentQuery::MenuIconRect => ComponentResponse::Rect(self.menu_rect),
            _ => ComponentResponse::None,
        }
    }

    fn hit_test(&self, x: u16, y: u16) -> bool {
        if !self.area.is_empty() && rect_contains(self.area, x, y) {
            return true;
        }
        false
    }

    fn begin_frame(&mut self) {
        self.begin_frame();
    }

    fn visible(&self) -> bool {
        self.visible
    }

    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }
}

impl Default for WmTopPanelComponent {
    fn default() -> Self {
        Self::new("unknown")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_wm_core::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    #[test]
    fn top_panel_basic_methods_and_split_area() {
        let mut p = WmTopPanelComponent::new("test-app");
        assert!(p.visible());
        p.set_visible(false);
        assert!(!p.visible());
        p.set_height(0);
        assert!(p.height() >= 1);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 5,
        };
        let (panel_rect, managed) = p.split_area(true, area);
        assert_eq!(panel_rect.width, 10);
        assert_eq!(managed.width, 10);

        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert!(!p.hit_test_mouse_capture(&ev));
        assert!(p.hit_test_window(&ev).is_none());
    }

    #[test]
    fn top_panel_split_area_inactive() {
        let mut p = WmTopPanelComponent::new("test");
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let (panel, managed) = p.split_area(false, area);
        assert_eq!(panel.height, 0);
        assert_eq!(managed, area);
    }
}
