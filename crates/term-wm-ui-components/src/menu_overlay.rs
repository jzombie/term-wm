use std::time::{Duration, Instant};

use crossterm::event::{Event, MouseEventKind};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Block, Borders},
};

use term_wm_core::{
    layout::rect_contains,
    menu_trait::{MenuItem, MenuOverlay},
    theme,
    ui::UiFrame,
};

use crate::menu::MenuComponent;

#[derive(Debug)]
pub struct WmMenuOverlay<R> {
    menu: MenuComponent<R>,
    outlined: bool,
    outlined_at: Option<Instant>,
    outline_timeout: Duration,
    menu_bounds_cache: Option<Rect>,
    item_hits: Vec<(usize, Rect)>,
    hover_pos: Option<(u16, u16)>,
}

impl<R: std::fmt::Debug + Clone> WmMenuOverlay<R> {
    pub fn new() -> Self {
        Self {
            menu: MenuComponent::new(),
            outlined: false,
            outlined_at: None,
            outline_timeout: Duration::ZERO,
            menu_bounds_cache: None,
            item_hits: Vec::new(),
            hover_pos: None,
        }
    }

    fn auto_restore(&mut self) {
        if self.outlined
            && let Some(t) = self.outlined_at
            && t.elapsed() > self.outline_timeout
        {
            self.restore();
        }
    }

    fn render_menu(&mut self, frame: &mut UiFrame<'_>, anchor: (u16, u16), managed_area: Rect, hover_pos: Option<(u16, u16)>) {
        let item_count = self.menu.items().len();
        if item_count == 0 {
            return;
        }
        let bounds = frame.area();
        let start_x = anchor.0;
        let start_y = anchor.1;
        if start_x < bounds.x || start_x >= bounds.x.saturating_add(bounds.width) {
            return;
        }
        let max_width = bounds
            .width
            .saturating_sub(start_x.saturating_sub(bounds.x))
            .max(1);
        let label_width = self
            .menu
            .items()
            .iter()
            .map(|item| item.label.chars().count() as u16)
            .max()
            .unwrap_or(1);
        let icon_width = self
            .menu
            .items()
            .iter()
            .map(|item| item.icon.map(|v| v.chars().count() as u16).unwrap_or(0))
            .max()
            .unwrap_or(0);
        let width = (label_width + icon_width + 6).min(max_width);
        let max_height = bounds
            .height
            .saturating_sub(start_y.saturating_sub(bounds.y))
            .max(1);
        let height = (item_count as u16).saturating_add(2).min(max_height);

        let drop_rect = Rect {
            x: start_x,
            y: start_y,
            width,
            height,
        };

        self.menu_bounds_cache = Some(drop_rect);

        self.render_backdrop(frame, managed_area, drop_rect);

        let buffer = frame.buffer_mut();
        let clip = drop_rect.intersection(buffer.area);
        if clip.width == 0 || clip.height == 0 {
            return;
        }

        let hovered_idx = hover_pos.and_then(|(_mx, my)| {
            (my >= drop_rect.y.saturating_add(1) && my < drop_rect.y.saturating_add(item_count as u16 + 1))
                .then(|| (my - drop_rect.y - 1) as usize)
                .filter(|&idx| idx < item_count)
        });
        self.menu.render_items(frame, drop_rect, hovered_idx);

        self.item_hits.clear();
        for idx in 0..item_count.min((drop_rect.height.saturating_sub(1)) as usize) {
            let y = drop_rect.y.saturating_add(idx as u16 + 1);
            if y < clip.y || y >= clip.y.saturating_add(clip.height) {
                break;
            }
            self.item_hits.push((idx, Rect {
                x: drop_rect.x,
                y,
                width: drop_rect.width,
                height: 1,
            }));
        }
    }

    fn render_outline(&mut self, frame: &mut UiFrame<'_>) {
        let Some(menu_bounds) = self.menu_bounds_cache else {
            return;
        };
        let buffer = frame.buffer_mut();
        let clip = menu_bounds.intersection(buffer.area);
        if clip.width > 0 && clip.height > 0 {
            let dim_style = Style::default().add_modifier(Modifier::DIM);
            for y in clip.y..clip.y.saturating_add(clip.height) {
                for x in clip.x..clip.x.saturating_add(clip.width) {
                    if let Some(cell) = buffer.cell_mut((x, y)) {
                        cell.set_style(dim_style);
                    }
                }
            }
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::menu_fg()).add_modifier(Modifier::DIM));
        frame.render_widget(block, menu_bounds);
    }

    fn render_backdrop(&self, frame: &mut UiFrame<'_>, bounds: Rect, exclude: Rect) {
        let buffer = frame.buffer_mut();
        let style = Style::default().add_modifier(Modifier::DIM);
        let clip = bounds.intersection(buffer.area);
        if clip.width == 0 || clip.height == 0 {
            return;
        }
        for y in clip.y..clip.y.saturating_add(clip.height) {
            for x in clip.x..clip.x.saturating_add(clip.width) {
                if rect_contains(exclude, x, y) {
                    continue;
                }
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.set_style(style);
                }
            }
        }
    }

    fn hit_test_item(&self, column: u16, row: u16) -> Option<usize> {
        self.item_hits
            .iter()
            .find(|(_, rect)| rect_contains(*rect, column, row))
            .map(|(idx, _)| *idx)
    }
}

impl<R: std::fmt::Debug + Clone> MenuOverlay<R> for WmMenuOverlay<R> {
    fn handle_event(&mut self, event: &Event) -> Option<R> {
        self.auto_restore();

        if let Event::Mouse(mouse) = event
            && matches!(mouse.kind, MouseEventKind::Down(_))
        {
            if let Some(idx) = self.hit_test_item(mouse.column, mouse.row) {
                self.menu.set_selected(idx);
                self.restore();
                return self.menu.selected_action().cloned();
            }
            return None;
        }

        if self.menu.handle_key_event(event) {
            self.restore();
            return None;
        }

        if self.menu.handles_key_event(event) {
            return self.menu.selected_action().cloned();
        }

        None
    }

    fn consumes_event(&self, event: &Event) -> bool {
        self.menu.handles_key_event(event)
    }

    fn outline(&mut self) {
        self.outlined = true;
        self.outlined_at = Some(Instant::now());
    }

    fn restore(&mut self) {
        self.outlined = false;
        self.outlined_at = None;
    }

    fn set_items(&mut self, items: Vec<MenuItem<R>>) {
        self.menu.set_items(items);
    }

    fn set_outline_timeout(&mut self, timeout: Duration) {
        self.outline_timeout = timeout;
    }

    fn set_hover_pos(&mut self, pos: Option<(u16, u16)>) {
        self.hover_pos = pos;
    }

    fn render(
        &mut self,
        frame: &mut UiFrame<'_>,
        anchor: Option<(u16, u16)>,
        managed_area: Rect,
    ) {
        self.auto_restore();
        if self.outlined {
            self.render_outline(frame);
        } else if let Some(anchor) = anchor {
            self.render_menu(frame, anchor, managed_area, self.hover_pos);
        }
    }
}

impl<R: std::fmt::Debug + Clone> Default for WmMenuOverlay<R> {
    fn default() -> Self {
        Self::new()
    }
}
