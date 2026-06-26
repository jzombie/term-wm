use crossterm::event::{Event, MouseEventKind};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
};

use term_wm_core::{
    components::{Component, ComponentContext},
    keybindings::Action,
    layout::rect_contains,
    panel_trait::Panel as PanelTrait,
    theme,
    ui::{safe_set_string, truncate_to_width, UiFrame},
};

#[derive(Debug, Clone, Copy)]
struct PanelWindowHit<R: Copy + Eq + Ord> {
    id: R,
    rect: Rect,
}

#[derive(Debug)]
struct WindowList<R: Copy + Eq + Ord> {
    window_hits: Vec<PanelWindowHit<R>>,
}

impl<R: Copy + Eq + Ord> WindowList<R> {
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
    mouse_capture_rect: Option<Rect>,
    clipboard_rect: Option<Rect>,
    selection_rect: Option<Rect>,
    copy_rect: Option<Rect>,
}

impl NotificationArea {
    fn new() -> Self {
        Self {
            mouse_capture_rect: None,
            clipboard_rect: None,
            selection_rect: None,
            copy_rect: None,
        }
    }

    fn begin_frame(&mut self) {
        self.mouse_capture_rect = None;
        self.clipboard_rect = None;
        self.selection_rect = None;
        self.copy_rect = None;
    }
}

#[derive(Debug)]
pub struct PanelComponent<R: Copy + Eq + Ord> {
    visible: bool,
    height: u16,
    area: Rect,
    bottom_area: Rect,
    menu_rect: Option<Rect>,
    list: WindowList<R>,
    notifications: NotificationArea,
    app_name: String,
    app_version: String,
    hostname: Option<String>,
    keybinding_hints: Vec<(Action, Vec<String>)>,
    hint_rects: Vec<(Rect, Action)>,
}

impl<R: Copy + Eq + Ord + std::fmt::Debug> PanelComponent<R> {
    pub fn new(app_name: &str, app_version: &str, hostname: Option<&str>) -> Self {
        Self {
            visible: true,
            height: 1,
            area: Rect::default(),
            bottom_area: Rect::default(),
            menu_rect: None,
            list: WindowList::new(),
            notifications: NotificationArea::new(),
            app_name: app_name.to_string(),
            app_version: app_version.to_string(),
            hostname: hostname.map(|h| h.to_string()),
            keybinding_hints: Vec::new(),
            hint_rects: Vec::new(),
        }
    }

    pub fn set_hostname(&mut self, hostname: &str) {
        self.hostname = Some(hostname.to_string());
    }

    pub fn set_keybinding_hints(&mut self, hints: Vec<(Action, Vec<String>)>) {
        self.keybinding_hints = hints;
    }

    pub fn keybinding_hints(&self) -> &[(Action, Vec<String>)] {
        &self.keybinding_hints
    }

    pub fn begin_frame(&mut self) {
        self.list.begin_frame();
        self.menu_rect = None;
        self.notifications.begin_frame();
        self.hint_rects.clear();
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn area(&self) -> Rect {
        self.area
    }

    pub fn bottom_area(&self) -> Rect {
        self.bottom_area
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub fn set_height(&mut self, height: u16) {
        self.height = height.max(1);
    }

    pub fn menu_icon_rect(&self) -> Option<Rect> {
        self.menu_rect
    }

    pub fn menu_icon_contains_point(&self, column: u16, row: u16) -> bool {
        if let Some(rect) = self.menu_rect {
            return rect_contains(rect, column, row);
        }
        false
    }

    pub fn split_area(&mut self, active: bool, area: Rect) -> (Rect, Rect, Rect) {
        let top_h = if active {
            self.height.min(area.height)
        } else {
            0
        };
        let has_hints = !self.keybinding_hints.is_empty();
        let bottom_h = if has_hints || active {
            1u16.min(area.height.saturating_sub(top_h))
        } else {
            0
        };
        let panel = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: top_h,
        };
        let bottom = Rect {
            x: area.x,
            y: area.y.saturating_add(area.height).saturating_sub(bottom_h),
            width: area.width,
            height: bottom_h,
        };
        let managed_height = area.height.saturating_sub(top_h).saturating_sub(bottom_h);
        let managed = Rect {
            x: area.x,
            y: area.y.saturating_add(top_h),
            width: area.width,
            height: managed_height,
        };
        self.area = panel;
        self.bottom_area = bottom;
        (panel, bottom, managed)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render<F>(
        &mut self,
        frame: &mut UiFrame<'_>,
        active: bool,
        focus_current: R,
        display_order: &[R],
        status_line: Option<&str>,
        mouse_capture_enabled: bool,
        clipboard_enabled: bool,
        window_selection_enabled: bool,
        _selection_active: bool,
        _selection_dragging: bool,
        selection_copy_available: bool,
        selection_copied: bool,
        menu_open: bool,
        label_for: F,
    ) where
        F: Fn(R) -> String,
    {
        if !active {
            if !self.keybinding_hints.is_empty() {
                self.render_hints(frame);
            }
            return;
        }
        let area = self.area;
        if area.width == 0 || area.height == 0 {
            return;
        }
        let buffer = frame.buffer_mut();
        let bounds = area.intersection(buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        for yy in bounds.y..bounds.y.saturating_add(bounds.height) {
            for xx in bounds.x..bounds.x.saturating_add(bounds.width) {
                if let Some(cell) = buffer.cell_mut((xx, yy)) {
                    let mut st = cell.style();
                    st.bg = Some(theme::bottom_panel_bg());
                    st.fg = Some(theme::bottom_panel_fg());
                    cell.set_style(st);
                }
            }
        }
        let mut x = area.x;
        let y = area.y;
        let max_x = area.x.saturating_add(area.width);
        let menu_icon = format!("≡ {}", self.app_name);
        let menu_width = menu_icon.chars().count() as u16;
        if x.saturating_add(menu_width) <= max_x {
            let menu_style = if menu_open {
                Style::default()
                    .bg(theme::menu_bg())
                    .fg(theme::menu_fg())
            } else {
                Style::default()
            };
            safe_set_string(buffer, bounds, x, y, menu_icon.as_str(), menu_style);
            self.menu_rect = Some(Rect {
                x,
                y,
                width: menu_width,
                height: 1,
            });
            x = x.saturating_add(menu_width);
        }
        if x < max_x {
            safe_set_string(buffer, bounds, x, y, " ", Style::default());
            x = x.saturating_add(1);
        }
        if let Some(status) = status_line {
            let available = max_x.saturating_sub(x).max(1);
            let text = truncate_to_width(status, available as usize);
            safe_set_string(buffer, bounds, x, y, &text, Style::default());
        } else {
            for id in display_order.iter().copied() {
                let focused = id == focus_current;
                let mut label = label_for(id);
                let max_label = max_x.saturating_sub(x).saturating_sub(2) as usize;
                if label.chars().count() > max_label {
                    label = truncate_to_width(&label, max_label);
                }
                let chunk = format!(" {label} ");
                let chunk_width = chunk.chars().count() as u16;
                if x.saturating_add(chunk_width) > max_x {
                    break;
                }
                let item_style = if focused {
                    Style::default()
                        .bg(theme::menu_selected_bg())
                        .fg(theme::menu_selected_fg())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::panel_inactive_fg())
                };
                safe_set_string(buffer, bounds, x, y, &chunk, item_style);
                self.list.window_hits.push(PanelWindowHit {
                    id,
                    rect: Rect {
                        x,
                        y,
                        width: chunk_width,
                        height: 1,
                    },
                });
                x = x.saturating_add(chunk_width);
            }
        }

        let selection_chunk = "[ selection ]";
        let copy_chunk = "[ copy ]";
        let mouse_chunk = "[ mouse ]";
        let clip_chunk = "[ clipboard ]";
        let selection_width = selection_chunk.chars().count() as u16;
        let copy_width = copy_chunk.chars().count() as u16;
        let mouse_width = mouse_chunk.chars().count() as u16;
        let clip_width = clip_chunk.chars().count() as u16;
        let total_width = selection_width
            .saturating_add(copy_width)
            .saturating_add(mouse_width)
            .saturating_add(clip_width);
        let indicator_x = if total_width >= bounds.width {
            bounds.x
        } else {
            max_x.saturating_sub(total_width)
        };
        if total_width > 0 && indicator_x < max_x {
            let selection_style = if window_selection_enabled {
                Style::default()
                    .fg(theme::success_bg())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::panel_inactive_fg())
            };
            let copy_style = if selection_copy_available && clipboard_enabled {
                let mut style = Style::default()
                    .fg(theme::success_bg())
                    .add_modifier(Modifier::BOLD);
                if selection_copied {
                    style = style.fg(theme::accent());
                }
                style
            } else {
                Style::default().fg(theme::panel_inactive_fg())
            };
            let mouse_style = if mouse_capture_enabled {
                Style::default()
                    .fg(theme::success_bg())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::panel_inactive_fg())
            };
            let clip_style = if clipboard_enabled {
                Style::default()
                    .fg(theme::success_bg())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::panel_inactive_fg())
            };
            let mut cursor = indicator_x;
            if selection_width > 0 && cursor < max_x {
                safe_set_string(buffer, bounds, cursor, y, selection_chunk, selection_style);
                let width = selection_width.min(max_x.saturating_sub(cursor));
                if width > 0 {
                    self.notifications.selection_rect = Some(Rect {
                        x: cursor,
                        y,
                        width,
                        height: 1,
                    });
                }
            }
            cursor = cursor.saturating_add(selection_width);
            if copy_width > 0 && cursor < max_x {
                safe_set_string(buffer, bounds, cursor, y, copy_chunk, copy_style);
                let width = copy_width.min(max_x.saturating_sub(cursor));
                if width > 0 && selection_copy_available && clipboard_enabled {
                    self.notifications.copy_rect = Some(Rect {
                        x: cursor,
                        y,
                        width,
                        height: 1,
                    });
                }
            }
            cursor = cursor.saturating_add(copy_width);
            if mouse_width > 0 && cursor < max_x {
                safe_set_string(buffer, bounds, cursor, y, mouse_chunk, mouse_style);
                let width = mouse_width.min(max_x.saturating_sub(cursor));
                if width > 0 {
                    self.notifications.mouse_capture_rect = Some(Rect {
                        x: cursor,
                        y,
                        width,
                        height: 1,
                    });
                }
            }
            cursor = cursor.saturating_add(mouse_width);
            if clip_width > 0 && cursor < max_x {
                safe_set_string(buffer, bounds, cursor, y, clip_chunk, clip_style);
                let width = clip_width.min(max_x.saturating_sub(cursor));
                if width > 0 {
                    self.notifications.clipboard_rect = Some(Rect {
                        x: cursor,
                        y,
                        width,
                        height: 1,
                    });
                }
            }
        }
        if self.bottom_area.width > 0 && self.bottom_area.height > 0 {
            self.render_bottom(frame);
        }
    }

    pub fn render_bottom(&mut self, frame: &mut UiFrame<'_>) {
        self.render_bottom_impl(frame, true);
    }

    pub fn render_hints(&mut self, frame: &mut UiFrame<'_>) {
        self.render_bottom_impl(frame, false);
    }

    fn render_bottom_impl(&mut self, frame: &mut UiFrame<'_>, show_info: bool) {
        let area = self.bottom_area;
        if area.width == 0 || area.height == 0 {
            return;
        }
        let buffer = frame.buffer_mut();
        let bounds = area.intersection(buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        for yy in bounds.y..bounds.y.saturating_add(bounds.height) {
            for xx in bounds.x..bounds.x.saturating_add(bounds.width) {
                if let Some(cell) = buffer.cell_mut((xx, yy)) {
                    let mut st = cell.style();
                    st.bg = Some(theme::bottom_panel_bg());
                    st.fg = Some(theme::bottom_panel_fg());
                    cell.set_style(st);
                }
            }
        }
        let style = Style::default()
            .fg(theme::bottom_panel_fg())
            .bg(theme::bottom_panel_bg());

        let info_opt = if show_info {
            let platform = std::env::consts::OS;
            let pkg_label = format!("{} {}", self.app_name, self.app_version);
            let hostname = self.hostname.clone().unwrap_or_else(|| "unknown-host".to_string());
            Some(format!("{pkg_label} · {platform} · {hostname}"))
        } else {
            None
        };

        let info_width = info_opt
            .as_ref()
            .map(|s| s.chars().count() as u16)
            .unwrap_or(0);
        let max_hint_x = if info_width > 0 {
            bounds
                .x
                .saturating_add(bounds.width)
                .saturating_sub(info_width + 2)
        } else {
            bounds.x.saturating_add(bounds.width)
        };

        if !self.keybinding_hints.is_empty() {
            let combo_style = Style::default()
                .fg(theme::menu_selected_fg())
                .bg(theme::menu_selected_bg())
                .add_modifier(Modifier::BOLD);
            let mut cursor_x = bounds.x;
            self.hint_rects.clear();
            for (action, combos) in &self.keybinding_hints {
                if cursor_x >= max_hint_x {
                    break;
                }
                let combo_str = combos.join("/");
                let entry = format!("{combo_str} {action}");
                let entry_width = entry.chars().count() as u16;

                if cursor_x.saturating_add(entry_width) > max_hint_x && cursor_x > bounds.x {
                    break;
                }

                let available_w = max_hint_x.saturating_sub(cursor_x);
                if cursor_x == bounds.x && entry_width > available_w {
                    let text = truncate_to_width(&entry, available_w as usize);
                    safe_set_string(buffer, bounds, cursor_x, area.y, &text, style);
                    self.hint_rects.push((
                        Rect {
                            x: cursor_x,
                            y: area.y,
                            width: available_w,
                            height: 1,
                        },
                        *action,
                    ));
                    break;
                }

                self.hint_rects.push((
                    Rect {
                        x: cursor_x,
                        y: area.y,
                        width: entry_width,
                        height: 1,
                    },
                    *action,
                ));

                safe_set_string(buffer, bounds, cursor_x, area.y, &combo_str, combo_style);
                cursor_x = cursor_x.saturating_add(combo_str.chars().count() as u16);
                let desc = format!(" {}", action);
                safe_set_string(buffer, bounds, cursor_x, area.y, &desc, style);
                cursor_x = cursor_x.saturating_add(desc.chars().count() as u16);

                if cursor_x < max_hint_x {
                    safe_set_string(buffer, bounds, cursor_x, area.y, "|", Style::default());
                    cursor_x = cursor_x.saturating_add(1);
                }
            }
        }

        if let Some(ref info) = info_opt {
            let text = truncate_to_width(info, bounds.width as usize);
            let text_width = text.chars().count() as u16;
            let start_x = if text_width >= bounds.width {
                bounds.x
            } else {
                bounds
                    .x
                    .saturating_add(bounds.width)
                    .saturating_sub(text_width)
            };
            safe_set_string(buffer, bounds, start_x.max(bounds.x), area.y, &text, style);
        }
    }

    pub fn hit_test_hint(&self, event: &Event) -> Option<Action> {
        let Event::Mouse(mouse) = event else {
            return None;
        };
        if !matches!(mouse.kind, MouseEventKind::Down(_)) {
            return None;
        }
        for (rect, action) in &self.hint_rects {
            if rect_contains(*rect, mouse.column, mouse.row) {
                return Some(*action);
            }
        }
        None
    }

    pub fn hit_test_menu(&self, event: &Event) -> bool {
        let Event::Mouse(mouse) = event else {
            return false;
        };
        if !matches!(mouse.kind, MouseEventKind::Down(_)) {
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
        if !matches!(mouse.kind, MouseEventKind::Down(_)) {
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
        if !matches!(mouse.kind, MouseEventKind::Down(_)) {
            return false;
        }
        if let Some(rect) = self.notifications.clipboard_rect {
            return rect_contains(rect, mouse.column, mouse.row);
        }
        false
    }

    pub fn hit_test_copy(&self, event: &Event) -> bool {
        let Event::Mouse(mouse) = event else {
            return false;
        };
        if !matches!(mouse.kind, MouseEventKind::Down(_)) {
            return false;
        }
        if let Some(rect) = self.notifications.copy_rect {
            return rect_contains(rect, mouse.column, mouse.row);
        }
        false
    }

    pub fn hit_test_selection(&self, event: &Event) -> bool {
        let Event::Mouse(mouse) = event else {
            return false;
        };
        if !matches!(mouse.kind, MouseEventKind::Down(_)) {
            return false;
        }
        if let Some(rect) = self.notifications.selection_rect {
            return rect_contains(rect, mouse.column, mouse.row);
        }
        false
    }

    pub fn hit_test_window(&self, event: &Event) -> Option<R> {
        let Event::Mouse(mouse) = event else {
            return None;
        };
        if !matches!(mouse.kind, MouseEventKind::Down(_)) {
            return None;
        }
        self.list
            .window_hits
            .iter()
            .find(|hit| rect_contains(hit.rect, mouse.column, mouse.row))
            .map(|hit| hit.id)
    }
}

impl<R: Copy + Eq + Ord + std::fmt::Debug> PanelTrait<R> for PanelComponent<R> {
    fn begin_frame(&mut self) {
        self.begin_frame()
    }

    fn visible(&self) -> bool {
        self.visible()
    }

    fn height(&self) -> u16 {
        self.height()
    }

    fn area(&self) -> Rect {
        self.area()
    }

    fn bottom_area(&self) -> Rect {
        self.bottom_area()
    }

    fn set_visible(&mut self, visible: bool) {
        self.set_visible(visible);
    }

    fn set_height(&mut self, height: u16) {
        self.set_height(height);
    }

    fn set_keybinding_hints(&mut self, hints: Vec<(Action, Vec<String>)>) {
        self.set_keybinding_hints(hints);
    }

    fn keybinding_hints(&self) -> &[(Action, Vec<String>)] {
        self.keybinding_hints()
    }

    fn set_hostname(&mut self, hostname: &str) {
        self.set_hostname(hostname);
    }

    fn split_area(&mut self, active: bool, area: Rect) -> (Rect, Rect, Rect) {
        self.split_area(active, area)
    }

    #[allow(clippy::too_many_arguments)]
    fn render(
        &mut self,
        frame: &mut UiFrame<'_>,
        active: bool,
        focus_current: R,
        display_order: &[R],
        status_line: Option<&str>,
        mouse_capture_enabled: bool,
        clipboard_enabled: bool,
        window_selection_enabled: bool,
        _selection_active: bool,
        _selection_dragging: bool,
        selection_copy_available: bool,
        selection_copied: bool,
        menu_open: bool,
        label_for: &dyn Fn(R) -> String,
    ) {
        self.render(
            frame,
            active,
            focus_current,
            display_order,
            status_line,
            mouse_capture_enabled,
            clipboard_enabled,
            window_selection_enabled,
            _selection_active,
            _selection_dragging,
            selection_copy_available,
            selection_copied,
            menu_open,
            label_for,
        );
    }

    fn menu_icon_rect(&self) -> Option<Rect> {
        self.menu_icon_rect()
    }

    fn menu_icon_contains_point(&self, column: u16, row: u16) -> bool {
        self.menu_icon_contains_point(column, row)
    }

    fn hit_test_mouse_capture(&self, event: &Event) -> bool {
        self.hit_test_mouse_capture(event)
    }

    fn hit_test_selection(&self, event: &Event) -> bool {
        self.hit_test_selection(event)
    }

    fn hit_test_clipboard(&self, event: &Event) -> bool {
        self.hit_test_clipboard(event)
    }

    fn hit_test_copy(&self, event: &Event) -> bool {
        self.hit_test_copy(event)
    }

    fn hit_test_window(&self, event: &Event) -> Option<R> {
        self.hit_test_window(event)
    }

    fn hit_test_hint(&self, event: &Event) -> Option<Action> {
        self.hit_test_hint(event)
    }
}

impl<R: Copy + Eq + Ord + std::fmt::Debug + 'static> Component for PanelComponent<R> {
    fn render(&mut self, _frame: &mut UiFrame<'_>, _area: Rect, _ctx: &ComponentContext) {
    }

    fn handle_event(&mut self, _event: &Event, _ctx: &ComponentContext) -> bool {
        false
    }
}

impl<R: Copy + Eq + Ord + std::fmt::Debug> Default for PanelComponent<R> {
    fn default() -> Self {
        Self::new("unknown", "0.0.0", None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{Event, MouseEvent, MouseEventKind};
    use ratatui::buffer::Buffer;

    #[test]
    fn panel_basic_methods_and_split_area() {
        let mut p: PanelComponent<usize> = PanelComponent::new("test-app", "1.0.0", Some("test-host"));
        assert!(p.visible());
        p.set_visible(false);
        assert!(!p.visible());
        p.set_height(0);
        assert!(p.height() >= 1);
        let area = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 5,
        };
        let (panel_rect, bottom_rect, managed) = p.split_area(true, area);
        assert_eq!(panel_rect.width, 10);
        assert_eq!(bottom_rect.width, 10);
        assert_eq!(managed.width, 10);

        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        assert!(!p.hit_test_mouse_capture(&ev));
        assert!(p.hit_test_window(&ev).is_none());
    }

    #[test]
    fn render_bottom_renders_provided_hostname() {
        let mut p: PanelComponent<usize> = PanelComponent::new("app", "1.0", Some("my-machine"));
        assert_eq!(p.hostname, Some("my-machine".to_string()));

        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        p.bottom_area = area;
        let mut buf = Buffer::empty(area);
        let mut ui = UiFrame::from_parts(area, &mut buf);

        p.render_bottom(&mut ui);

        let mut rendered = String::new();
        for xx in area.x..area.x.saturating_add(area.width) {
            let cell = buf.cell((xx, area.y)).expect("cell present");
            rendered.push_str(cell.symbol());
        }
        assert!(
            rendered.contains("my-machine"),
            "bottom bar should include hostname"
        );
    }

    #[test]
    fn render_bottom_fills_background_and_right_aligns_text() {
        let mut p: PanelComponent<usize> = PanelComponent::new("test", "0.0.1", Some("h"));
        let area = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 1,
        };
        p.bottom_area = area;
        let mut buf = Buffer::empty(area);
        let mut ui = UiFrame::from_parts(area, &mut buf);

        p.render_bottom(&mut ui);

        for xx in area.x..area.x.saturating_add(area.width) {
            let cell = buf.cell_mut((xx, area.y)).expect("cell present");
            assert_eq!(cell.style().bg, Some(theme::bottom_panel_bg()));
            assert_eq!(cell.style().fg, Some(theme::bottom_panel_fg()));
        }

        let mut found = false;
        for dx in (0..area.width).rev() {
            let cell = buf.cell((area.x + dx, area.y)).expect("cell present");
            if !cell.symbol().trim().is_empty() {
                found = true;
                break;
            }
        }
        assert!(found, "expected non-space text in bottom row");
    }

    #[test]
    fn render_bottom_includes_app_name_and_version() {
        let mut p: PanelComponent<usize> = PanelComponent::new("my-app", "2.0.0", Some("my-host"));
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        p.bottom_area = area;
        let mut buf = Buffer::empty(area);
        let mut ui = UiFrame::from_parts(area, &mut buf);

        p.render_bottom(&mut ui);

        let mut rendered = String::new();
        for xx in area.x..area.x.saturating_add(area.width) {
            let cell = buf.cell((xx, area.y)).expect("cell present");
            rendered.push_str(cell.symbol());
        }
        assert!(rendered.contains("my-app"));
        assert!(rendered.contains("2.0.0"));
        assert!(rendered.contains("my-host"));
    }
}
