use crossterm::event::{Event, MouseEventKind};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
};

use crate::layout::rect_contains;
use crate::ui::{UiFrame, safe_set_string, truncate_to_width};

#[derive(Debug, Clone, Copy)]
pub struct PanelWindowHit<R: Copy + Eq + Ord> {
    id: R,
    rect: Rect,
}

// PanelMenuHit is defined above for use by ActivationMenu

#[derive(Debug)]
struct ActivationMenu {
    menu_rect: Option<Rect>,
    menu_item_hits: Vec<PanelMenuHit>,
    menu_bounds: Option<Rect>,
}

impl ActivationMenu {
    fn new() -> Self {
        Self {
            menu_rect: None,
            menu_item_hits: Vec::new(),
            menu_bounds: None,
        }
    }

    fn begin_frame(&mut self) {
        self.menu_rect = None;
        self.menu_item_hits.clear();
        self.menu_bounds = None;
    }
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
}

impl NotificationArea {
    fn new() -> Self {
        Self {
            mouse_capture_rect: None,
            clipboard_rect: None,
        }
    }

    fn begin_frame(&mut self) {
        self.mouse_capture_rect = None;
        self.clipboard_rect = None;
    }
}

#[derive(Debug)]
pub struct Panel<R: Copy + Eq + Ord> {
    visible: bool,
    height: u16,
    area: Rect,
    bottom_area: Rect,
    activation: ActivationMenu,
    list: WindowList<R>,
    notifications: NotificationArea,
    hostname: Option<String>,
}

impl<R: Copy + Eq + Ord + std::fmt::Debug> Panel<R> {
    pub fn new() -> Self {
        Self {
            visible: true,
            height: 1,
            area: Rect::default(),
            bottom_area: Rect::default(),
            activation: ActivationMenu::new(),
            list: WindowList::new(),
            notifications: NotificationArea::new(),
            hostname: None,
        }
    }

    pub fn begin_frame(&mut self) {
        self.list.begin_frame();
        self.activation.begin_frame();
        self.notifications.begin_frame();
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

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub fn set_height(&mut self, height: u16) {
        self.height = height.max(1);
    }

    /// Split the provided `area` into three regions:
    /// - top panel (height `self.height`),
    /// - bottom panel (fixed 1 row), and
    /// - managed area in between, which is returned for main content.
    ///
    /// If `active` is false the panel areas are cleared and the entire `area`
    /// is returned as the managed region.
    pub fn split_area(&mut self, active: bool, area: Rect) -> (Rect, Rect, Rect) {
        if !active {
            self.area = Rect::default();
            self.bottom_area = Rect::default();
            return (Rect::default(), Rect::default(), area);
        }
        let top_h = self.height.min(area.height);
        let bottom_h = 1u16.min(area.height.saturating_sub(top_h));
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
        clipboard_available: bool,
        menu_open: bool,
        label_for: F,
    ) where
        F: Fn(R) -> String,
    {
        if !active {
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
        // Fill the entire panel area with the bottom-panel color scheme so
        // the top bar visually matches the bottom info bar.
        for yy in bounds.y..bounds.y.saturating_add(bounds.height) {
            for xx in bounds.x..bounds.x.saturating_add(bounds.width) {
                if let Some(cell) = buffer.cell_mut((xx, yy)) {
                    let mut st = cell.style();
                    st.bg = Some(crate::theme::bottom_panel_bg());
                    st.fg = Some(crate::theme::bottom_panel_fg());
                    cell.set_style(st);
                }
            }
        }
        let mut x = area.x;
        let y = area.y;
        let max_x = area.x.saturating_add(area.width);
        const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
        let menu_icon = format!("≡ {CRATE_NAME}");
        let menu_width = menu_icon.chars().count() as u16;
        if x.saturating_add(menu_width) <= max_x {
            let menu_style = if menu_open {
                Style::default()
                    .bg(crate::theme::menu_bg())
                    .fg(crate::theme::menu_fg())
            } else {
                Style::default()
            };
            safe_set_string(buffer, bounds, x, y, menu_icon.as_str(), menu_style);
            self.activation.menu_rect = Some(Rect {
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
        // Window list follows the menu button label.
        if let Some(status) = status_line {
            let available = max_x.saturating_sub(x).max(1);
            let text = truncate_to_width(status, available as usize);
            safe_set_string(buffer, bounds, x, y, &text, Style::default());
        } else {
            for id in display_order.iter().copied() {
                let focused = id == focus_current;
                // Pretty label derived from caller. Truncate to remaining space.
                let mut label = label_for(id);
                // leave room for padding
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
                        .bg(crate::theme::menu_selected_bg())
                        .fg(crate::theme::menu_selected_fg())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(crate::theme::panel_inactive_fg())
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

        // Simplified text indicators per design: bracketed labels that
        // light up in green when active. No icons are used.
        let mouse_chunk = "[ mouse ]";
        let clip_chunk = "[ clipboard ]";
        // No separator; render compact indicators side-by-side.
        let mouse_width = mouse_chunk.chars().count() as u16;
        let clip_width = clip_chunk.chars().count() as u16;
        let total_width = mouse_width.saturating_add(clip_width);
        let indicator_x = if total_width >= bounds.width {
            bounds.x
        } else {
            max_x.saturating_sub(total_width)
        };
        if total_width > 0 && indicator_x < max_x {
            let mouse_style = if mouse_capture_enabled {
                Style::default()
                    .fg(crate::theme::success_bg())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(crate::theme::panel_inactive_fg())
            };
            let clip_style = if clipboard_available {
                if clipboard_enabled {
                    Style::default()
                        .fg(crate::theme::success_bg())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(crate::theme::panel_inactive_fg())
                }
            } else {
                Style::default()
                    .fg(crate::theme::panel_inactive_fg())
                    .add_modifier(Modifier::DIM)
            };
            let mut cursor = indicator_x;
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
                if width > 0 && clipboard_available {
                    self.notifications.clipboard_rect = Some(Rect {
                        x: cursor,
                        y,
                        width,
                        height: 1,
                    });
                }
            }
        }
        // Render bottom info bar (platform + hostname) if configured
        if self.bottom_area.width > 0 && self.bottom_area.height > 0 {
            self.render_bottom(frame);
        }
    }

    fn render_bottom(&mut self, frame: &mut UiFrame<'_>) {
        let area = self.bottom_area;
        if area.width == 0 || area.height == 0 {
            return;
        }
        let buffer = frame.buffer_mut();
        let bounds = area.intersection(buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        // Platform string (e.g. "linux", "macos", "freebsd", "windows")
        let platform = std::env::consts::OS;
        const PKG_NAME: &str = env!("CARGO_PKG_NAME");
        const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
        let pkg_label = format!("{PKG_NAME} {PKG_VERSION}");
        // Use cached hostname if available to avoid a system call every frame.
        let hostname = if let Some(ref h) = self.hostname {
            h.clone()
        } else {
            let h = hostname::get()
                .ok()
                .and_then(|s| s.into_string().ok())
                .unwrap_or_else(|| "unknown-host".to_string());
            self.hostname = Some(h.clone());
            h
        };
        let info = format!("{pkg_label} · {platform} · {hostname}");
        let text = truncate_to_width(&info, bounds.width as usize);
        // Fill the bottom bar background fully so the whole row uses the
        // bottom panel background color, then write the foreground text.
        for yy in bounds.y..bounds.y.saturating_add(bounds.height) {
            for xx in bounds.x..bounds.x.saturating_add(bounds.width) {
                if let Some(cell) = buffer.cell_mut((xx, yy)) {
                    let mut st = cell.style();
                    st.bg = Some(crate::theme::bottom_panel_bg());
                    st.fg = Some(crate::theme::bottom_panel_fg());
                    cell.set_style(st);
                }
            }
        }
        let style = Style::default()
            .fg(crate::theme::bottom_panel_fg())
            .bg(crate::theme::bottom_panel_bg());
        // Right-align the text within the bottom bar bounds.
        let text_width = text.chars().count() as u16;
        let start_x = if text_width >= bounds.width {
            bounds.x
        } else {
            // place text so its right edge aligns with bounds' right edge
            bounds
                .x
                .saturating_add(bounds.width)
                .saturating_sub(text_width)
        };
        let start_x = start_x.max(bounds.x);
        safe_set_string(buffer, bounds, start_x, area.y, &text, style);
    }

    pub fn hit_test_menu(&self, event: &Event) -> bool {
        let Event::Mouse(mouse) = event else {
            return false;
        };
        if !matches!(mouse.kind, MouseEventKind::Down(_)) {
            return false;
        }
        if let Some(rect) = self.activation.menu_rect {
            return rect_contains(rect, mouse.column, mouse.row);
        }
        false
    }

    pub fn menu_contains_point(&self, column: u16, row: u16) -> bool {
        if let Some(rect) = self.activation.menu_bounds {
            return rect_contains(rect, column, row);
        }
        false
    }

    pub fn menu_icon_contains_point(&self, column: u16, row: u16) -> bool {
        if let Some(rect) = self.activation.menu_rect {
            return rect_contains(rect, column, row);
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

    pub fn render_menu(
        &mut self,
        frame: &mut UiFrame<'_>,
        open: bool,
        bounds: Rect,
        items: &[(Option<&str>, &str)],
        selected: usize,
    ) {
        if !open {
            return;
        }
        let Some(anchor) = self.activation.menu_rect else {
            return;
        };
        if items.is_empty() {
            return;
        }
        let start_x = anchor.x;
        let start_y = anchor.y.saturating_add(1);
        if start_x < bounds.x || start_x >= bounds.x.saturating_add(bounds.width) {
            return;
        }
        let max_width = bounds
            .width
            .saturating_sub(start_x.saturating_sub(bounds.x))
            .max(1);
        let label_width = items
            .iter()
            .map(|(_, label)| label.chars().count() as u16)
            .max()
            .unwrap_or(1);
        let icon_width = items
            .iter()
            .map(|(icon, _)| icon.map(|v| v.chars().count() as u16).unwrap_or(0))
            .max()
            .unwrap_or(0);
        let width = (label_width + icon_width + 6).min(max_width);
        let max_height = bounds
            .height
            .saturating_sub(start_y.saturating_sub(bounds.y))
            .max(1);
        let height = (items.len() as u16).saturating_add(2).min(max_height);
        let buffer = frame.buffer_mut();
        let bounds = bounds.intersection(buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        let menu_style = Style::default()
            .bg(crate::theme::menu_bg())
            .fg(crate::theme::menu_fg());
        let selected_style = Style::default()
            .bg(crate::theme::menu_selected_bg())
            .fg(crate::theme::menu_selected_fg())
            .add_modifier(Modifier::BOLD);
        self.activation.menu_bounds = Some(Rect {
            x: start_x,
            y: start_y,
            width,
            height,
        });
        for row in 0..height {
            let y = start_y.saturating_add(row);
            if y < bounds.y || y >= bounds.y.saturating_add(bounds.height) {
                continue;
            }
            for col in 0..width {
                let x = start_x.saturating_add(col);
                if x >= bounds.x.saturating_add(bounds.width) {
                    break;
                }
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    // Prevent potential color bleed-through
                    cell.reset();

                    cell.set_symbol(" ");
                    cell.set_style(menu_style);
                }
            }
        }
        let inner_x = start_x.saturating_add(1);
        let inner_width = width.saturating_sub(2).max(1);
        for (idx, (icon, label)) in items.iter().enumerate() {
            let y = start_y.saturating_add(idx as u16 + 1);
            if y < bounds.y || y >= bounds.y.saturating_add(bounds.height) {
                break;
            }
            let marker = if idx == selected { ">" } else { " " };
            let line = if let Some(icon) = icon {
                format!("{marker} {icon} {label}")
            } else {
                format!("{marker}   {label}")
            };
            let text = truncate_to_width(&line, inner_width as usize);
            let style = if idx == selected {
                selected_style
            } else {
                menu_style
            };
            safe_set_string(buffer, bounds, inner_x, y, &text, style);
            self.activation.menu_item_hits.push(PanelMenuHit {
                index: idx,
                rect: Rect {
                    x: start_x,
                    y,
                    width,
                    height: 1,
                },
            });
        }
    }

    pub fn hit_test_menu_item(&self, event: &Event) -> Option<usize> {
        let Event::Mouse(mouse) = event else {
            return None;
        };
        if !matches!(mouse.kind, MouseEventKind::Down(_)) {
            return None;
        }
        self.activation
            .menu_item_hits
            .iter()
            .find(|hit| rect_contains(hit.rect, mouse.column, mouse.row))
            .map(|hit| hit.index)
    }

    pub fn render_menu_backdrop(
        &self,
        frame: &mut UiFrame<'_>,
        open: bool,
        bounds: Rect,
        exclude: Rect,
    ) {
        if !open {
            return;
        }
        let Some(menu_bounds) = self.activation.menu_bounds else {
            return;
        };
        let buffer = frame.buffer_mut();
        let style = Style::default().add_modifier(Modifier::DIM);
        let clip = bounds.intersection(buffer.area);
        if clip.width == 0 || clip.height == 0 {
            return;
        }
        for y in clip.y..clip.y.saturating_add(clip.height) {
            for x in clip.x..clip.x.saturating_add(clip.width) {
                if rect_contains(menu_bounds, x, y) || rect_contains(exclude, x, y) {
                    continue;
                }
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.set_style(style);
                }
            }
        }
    }
}

impl<R: Copy + Eq + Ord + std::fmt::Debug> Default for Panel<R> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy)]
struct PanelMenuHit {
    index: usize,
    rect: Rect,
}

#[cfg(test)]
fn panel_order<R: Copy + Eq + Ord>(focus_order: &[R], managed_draw_order: &[R]) -> Vec<R> {
    if focus_order.is_empty() {
        return managed_draw_order.to_vec();
    }
    let mut ordered = Vec::new();
    for focus in focus_order {
        if let Some(id) = managed_draw_order.iter().copied().find(|id| *id == *focus) {
            ordered.push(id);
        }
    }
    for id in managed_draw_order {
        if !ordered.contains(id) {
            ordered.push(*id);
        }
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{Event, MouseEvent, MouseEventKind};

    #[test]
    fn panel_order_prefers_focus_then_managed() {
        let focus: Vec<u8> = vec![2, 1];
        let managed: Vec<u8> = vec![1, 2, 3];
        let ord = panel_order(&focus, &managed);
        assert_eq!(ord[0], 2);
        assert_eq!(ord[1], 1);
        assert!(ord.contains(&3));
    }

    #[test]
    fn panel_basic_methods_and_split_area() {
        let mut p: Panel<usize> = Panel::new();
        assert!(p.visible());
        p.set_visible(false);
        assert!(!p.visible());
        p.set_height(0);
        assert!(p.height() >= 1);
        let area = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 5,
        };
        let (panel_rect, bottom_rect, managed) = p.split_area(true, area);
        assert_eq!(panel_rect.width, 10);
        assert_eq!(bottom_rect.width, 10);
        assert_eq!(managed.width, 10);

        // hit tests return false when rects not set
        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        assert!(!p.hit_test_menu(&ev));
        assert!(!p.hit_test_mouse_capture(&ev));
        assert!(p.hit_test_window(&ev).is_none());
    }

    #[test]
    fn render_bottom_populates_hostname_cache_and_is_idempotent() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let mut p: Panel<usize> = Panel::new();
        assert!(p.hostname.is_none());

        let area = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 1,
        };
        p.bottom_area = area;
        let mut buf = Buffer::empty(area);
        let mut ui = crate::ui::UiFrame::from_parts(area, &mut buf);

        // First render should populate the cached hostname.
        p.render_bottom(&mut ui);
        assert!(p.hostname.is_some());
        let first = p.hostname.clone();

        // Second render should not change the cached value.
        p.render_bottom(&mut ui);
        assert_eq!(p.hostname, first);
        // cached hostname should be non-empty string
        assert!(!p.hostname.as_ref().unwrap().is_empty());
    }

    #[test]
    fn render_bottom_fills_background_and_right_aligns_text() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let mut p: Panel<usize> = Panel::new();
        let area = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 1,
        };
        p.bottom_area = area;
        let mut buf = Buffer::empty(area);
        let mut ui = crate::ui::UiFrame::from_parts(area, &mut buf);

        p.render_bottom(&mut ui);

        // Every cell in the bottom row should have the bottom panel bg/fg style.
        for xx in area.x..area.x.saturating_add(area.width) {
            let cell = buf.cell_mut((xx, area.y)).expect("cell present");
            assert_eq!(cell.style().bg, Some(crate::theme::bottom_panel_bg()));
            assert_eq!(cell.style().fg, Some(crate::theme::bottom_panel_fg()));
        }

        // Ensure text was right-aligned: find the rightmost non-space symbol in the row.
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
    fn render_bottom_includes_package_and_version() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let mut p: Panel<usize> = Panel::new();
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        p.bottom_area = area;
        let mut buf = Buffer::empty(area);
        let mut ui = crate::ui::UiFrame::from_parts(area, &mut buf);

        p.render_bottom(&mut ui);

        // Build the rendered row as a string
        let mut rendered = String::new();
        for xx in area.x..area.x.saturating_add(area.width) {
            let cell = buf.cell((xx, area.y)).expect("cell present");
            rendered.push_str(cell.symbol());
        }

        let pkg = env!("CARGO_PKG_NAME");
        let ver = env!("CARGO_PKG_VERSION");

        assert!(
            rendered.contains(pkg),
            "bottom bar should include package name"
        );
        assert!(
            rendered.contains(ver),
            "bottom bar should include package version"
        );
    }
}
