use crossterm::event::{Event, MouseEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
};

use crate::layout::rect_contains;
use crate::ui::{safe_set_string, truncate_to_width};

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
}

impl NotificationArea {
    fn new() -> Self {
        Self {
            mouse_capture_rect: None,
        }
    }

    fn begin_frame(&mut self) {
        self.mouse_capture_rect = None;
    }
}

#[derive(Debug)]
pub struct Panel<R: Copy + Eq + Ord> {
    visible: bool,
    height: u16,
    area: Rect,
    activation: ActivationMenu,
    list: WindowList<R>,
    notifications: NotificationArea,
}

impl<R: Copy + Eq + Ord + std::fmt::Debug> Panel<R> {
    pub fn new() -> Self {
        Self {
            visible: true,
            height: 1,
            area: Rect::default(),
            activation: ActivationMenu::new(),
            list: WindowList::new(),
            notifications: NotificationArea::new(),
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

    pub fn split_area(&mut self, active: bool, area: Rect) -> (Rect, Rect) {
        if !active {
            self.area = Rect::default();
            return (Rect::default(), area);
        }
        let height = self.height.min(area.height);
        let panel = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height,
        };
        let managed = Rect {
            x: area.x,
            y: area.y.saturating_add(height),
            width: area.width,
            height: area.height.saturating_sub(height),
        };
        self.area = panel;
        (panel, managed)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        frame: &mut Frame,
        active: bool,
        focus_current: R,
        display_order: &[R],
        status_line: Option<&str>,
        mouse_capture_enabled: bool,
        menu_open: bool,
    ) {
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
                // Pretty label (uses Debug for now). Truncate to remaining space.
                let mut label = format!("{:?}", id);
                // leave room for padding
                let max_label = max_x.saturating_sub(x).saturating_sub(2).max(0) as usize;
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

        let indicator = "🖱";
        let label = if mouse_capture_enabled {
            "mouse capture: on"
        } else {
            "mouse capture: off"
        };
        let total_label = format!("{indicator} {label}");
        let total_width = total_label.chars().count() as u16;
        let indicator_x = if total_width >= bounds.width {
            bounds.x
        } else {
            max_x.saturating_sub(total_width)
        };
        if total_width > 0 && indicator_x < max_x {
            let indicator_style = if mouse_capture_enabled {
                Style::default()
                    .fg(crate::theme::success_fg())
                    .bg(crate::theme::success_bg())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(crate::theme::panel_inactive_fg())
            };
            safe_set_string(
                buffer,
                bounds,
                indicator_x,
                y,
                &total_label,
                indicator_style,
            );
            let available = max_x.saturating_sub(indicator_x);
            let rect_width = total_width.min(available);
            if rect_width > 0 {
                self.notifications.mouse_capture_rect = Some(Rect {
                    x: indicator_x,
                    y,
                    width: rect_width,
                    height: 1,
                });
            }
        }
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
        frame: &mut Frame,
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

    pub fn render_menu_backdrop(&self, frame: &mut Frame, open: bool, bounds: Rect, exclude: Rect) {
        if !open {
            return;
        }
        let Some(menu_bounds) = self.activation.menu_bounds else {
            return;
        };
        let buffer = frame.buffer_mut();
        let style = Style::default().add_modifier(Modifier::DIM);
        for y in bounds.y..bounds.y.saturating_add(bounds.height) {
            for x in bounds.x..bounds.x.saturating_add(bounds.width) {
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
        let (panel_rect, managed) = p.split_area(true, area);
        assert_eq!(panel_rect.width, 10);
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
}
