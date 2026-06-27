use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyEventKind, MouseEventKind};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Block, Borders},
};

use term_wm_core::{
    components::{Component, ComponentContext, MenuItem, MenuOverlay, Overlay},
    keybindings::Action,
    layout::rect_contains,
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
    anchor: Option<(u16, u16)>,
    managed_area: Rect,
    last_action: Option<R>,
}

impl<R: Clone + std::fmt::Debug + 'static> WmMenuOverlay<R> {
    pub fn new() -> Self {
        Self {
            menu: MenuComponent::new(),
            outlined: false,
            outlined_at: None,
            outline_timeout: Duration::ZERO,
            menu_bounds_cache: None,
            item_hits: Vec::new(),
            anchor: None,
            managed_area: Rect::default(),
            last_action: None,
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

    fn render_dropdown(&mut self, frame: &mut UiFrame<'_>, ctx: &ComponentContext) {
        let item_count = self.menu.items().len();
        if item_count == 0 {
            return;
        }
        let Some(anchor) = self.anchor else {
            return;
        };
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

        self.render_backdrop(frame, self.managed_area, drop_rect);

        let buffer = frame.buffer_mut();
        let clip = drop_rect.intersection(buffer.area);
        if clip.width == 0 || clip.height == 0 {
            return;
        }

        let hovered_idx = ctx.hover_pos().and_then(|(_mx, my)| {
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

    fn render_outline(&self, frame: &mut UiFrame<'_>) {
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

impl<R: Clone + std::fmt::Debug + 'static> Component for WmMenuOverlay<R> {
    fn render(&mut self, frame: &mut UiFrame<'_>, _area: Rect, ctx: &ComponentContext) {
        self.auto_restore();
        if self.outlined {
            self.render_outline(frame);
        } else {
            self.render_dropdown(frame, ctx);
        }
    }

    fn handle_event(&mut self, event: &Event, ctx: &ComponentContext) -> bool {
        self.auto_restore();
        self.last_action = None;

        if let Event::Mouse(mouse) = event
            && matches!(mouse.kind, MouseEventKind::Down(_))
        {
            if let Some(idx) = self.hit_test_item(mouse.column, mouse.row) {
                self.menu.set_selected(idx);
                self.restore();
                self.last_action = self.menu.selected_action().cloned();
                return true;
            }
            return false;
        }

        let Event::Key(key) = event else {
            return false;
        };
        if key.kind != KeyEventKind::Press {
            return false;
        }

        let total = self.menu.items().len();
        if total == 0 {
            return false;
        }

        let kb = ctx.keybindings().unwrap_or_default();
        if kb.matches(Action::MenuUp, key)
            || kb.matches(Action::MenuPrev, key)
        {
            let current = self.menu.selected();
            self.menu.set_selected(if current == 0 { total - 1 } else { current - 1 });
            self.restore();
            true
        } else if kb.matches(Action::MenuDown, key)
            || kb.matches(Action::MenuNext, key)
        {
            let current = self.menu.selected();
            self.menu.set_selected((current + 1) % total);
            self.restore();
            true
        } else if kb.matches(Action::MenuSelect, key) {
            self.last_action = self.menu.selected_action().cloned();
            true
        } else {
            false
        }
    }
}

impl<R: Clone + std::fmt::Debug + 'static> Overlay for WmMenuOverlay<R> {
    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

impl<R: Clone + std::fmt::Debug + 'static> MenuOverlay<R> for WmMenuOverlay<R> {
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

    fn set_timeout(&mut self, timeout: Duration) {
        self.outline_timeout = timeout;
    }

    fn selected_action(&self) -> Option<&R> {
        self.last_action.as_ref()
    }

    fn set_anchor(&mut self, pos: Option<(u16, u16)>) {
        self.anchor = pos;
    }

    fn set_managed_area(&mut self, area: Rect) {
        self.managed_area = area;
    }

}

impl<R: Clone + std::fmt::Debug + 'static> Default for WmMenuOverlay<R> {
    fn default() -> Self {
        Self::new()
    }
}
