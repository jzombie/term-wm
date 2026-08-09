//! Reusable horizontal tab bar: scrollable, drag-to-reorder, chevron overflow,
//! and per-tab close buttons.
//!
//! A generic, window-manager-agnostic widget. The host supplies an ordered list of
//! `TabItem<K>` (opaque key + label), the active key, and renders it into a rect.
//! The bar handles its own mouse input (press/select, close-glyph, drag-to-reorder
//! with a gliding ghost, wheel/edge scrolling) and emits `TabBarEvent<K>`.

use ratatui::style::{Modifier, Style};

use term_wm_core::{
    actions::EventResult,
    components::{Component, ComponentContext},
    events::{Event, MouseEventKind},
    layout::rect_contains,
    utils::truncate_with_ellipsis,
};
use term_wm_layout_engine::LayoutRect;
use unicode_width::UnicodeWidthStr;

use crate::helpers::{
    color_to_ratatui, layout_rect_to_clipped_rect, safe_set_string, slice_by_columns,
};

/// One space on each side of a tab label (`" {label} "`).
const ENTRY_PADDING: usize = 2;
/// Horizontal scroll step (columns) for wheel / indicator / edge-pan scrolling.
const SCROLL_STEP: u16 = 8;
/// Horizontal gap (columns) between the tab viewport and each `◀`/`▶` chevron.
const CHEVRON_GAP: u16 = 1;
/// Per-tab label cap (display columns) before ellipsis truncation.
const MAX_ENTRY_LABEL: usize = 40;
/// Left overflow indicator glyph.
const LEFT_INDICATOR: &str = "◀";
/// Right overflow indicator glyph.
const RIGHT_INDICATOR: &str = "▶";
/// Drop-indicator glyph drawn at the insertion boundary during drag.
const DROP_INDICATOR: &str = "▌";
/// Width (display columns) of the close affordance, `" ✕"`.
const CLOSE_BUTTON_WIDTH: u16 = 2;
/// Close glyph rendered on closable tabs.
const CLOSE_GLYPH: &str = "✕";

/// Horizontal scroll that makes a tab at logical `lx` of `width` columns fully visible.
/// Tabs wider than the viewport are left-aligned (show the leftmost columns).
fn scroll_into_view_target(h: i32, lx: i32, width: i32, viewport_width: i32, max_scroll: i32) -> i32 {
    let target = if width > viewport_width || lx < h {
        // Tabs wider than the viewport, or clipped on the left, align to their
        // left edge (show the leftmost columns of the title).
        lx.max(0)
    } else if lx.saturating_add(width) > h.saturating_add(viewport_width) {
        // Clipped on the right → reveal the right edge.
        (lx.saturating_add(width) - viewport_width).max(0)
    } else {
        h // already fully visible
    };
    target.min(max_scroll).max(0)
}

/// A single tab: an opaque key plus its display label.
#[derive(Debug, Clone)]
pub struct TabItem<K> {
    pub key: K,
    pub label: String,
    /// Render a `✕` close affordance (press → `TabBarEvent::Close`).
    pub closable: bool,
    /// Per-tab style override; `None` uses the theme defaults.
    pub style_override: Option<Style>,
}

/// Events emitted by [`TabBarComponent`]'s `handle_events`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabBarEvent<K> {
    Select(K),
    Close(K),
    Reorder { key: K, target_index: usize },
}

/// Ongoing drag state (scroll-thumb style).
#[derive(Debug)]
struct DragState<K> {
    source: K,
    cursor_col: i32,
    grab_col: i32,
    drop_index: usize,
    moved: bool,
}

/// Hit data for a rendered tab (built each frame).
#[derive(Debug, Clone, Copy)]
struct TabHit<K> {
    key: K,
    rect: LayoutRect,
    closable: bool,
}

/// Scrollable, drag-reorderable horizontal tab bar.
#[derive(Debug)]
pub struct TabBarComponent<K> {
    items: Vec<TabItem<K>>,
    active_key: Option<K>,
    h_scroll: u16,
    drag_state: Option<DragState<K>>,
    // Render geometry, reused by the event handlers between frames.
    entries_start_x: i32,
    scroll_viewport_width: i32,
    max_scroll: u16,
    entry_geometry: Vec<(K, i32, u16)>,
    left_indicator_rect: Option<LayoutRect>,
    right_indicator_rect: Option<LayoutRect>,
    item_hits: Vec<TabHit<K>>,
    rect: LayoutRect,
    // Auto-scroll guards: only re-follow the active tab when it, its logical
    // bounds, or the viewport size changed, so manual scrolls persist.
    last_auto_focus: Option<K>,
    last_auto_viewport: i32,
    last_focused_logical_bounds: Option<(i32, i32)>,
    // Tab clicked by the user; scrolled into view on the next render pass.
    // Never consumed while a drag is in progress (see render()).
    pending_scroll_to: Option<K>,
}

impl<K: Copy + PartialEq + Eq + std::hash::Hash + std::fmt::Debug + 'static> TabBarComponent<K> {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            active_key: None,
            h_scroll: 0,
            drag_state: None,
            entries_start_x: 0,
            scroll_viewport_width: 0,
            max_scroll: 0,
            entry_geometry: Vec::new(),
            left_indicator_rect: None,
            right_indicator_rect: None,
            item_hits: Vec::new(),
            rect: LayoutRect::default(),
            last_auto_focus: None,
            last_auto_viewport: 0,
            last_focused_logical_bounds: None,
            pending_scroll_to: None,
        }
    }

    pub fn begin_frame(&mut self) {
        self.item_hits.clear();
        self.left_indicator_rect = None;
        self.right_indicator_rect = None;
    }

    pub fn clear_drag_state(&mut self) {
        self.drag_state = None;
    }

    /// Replace the ordered tab list. The active key is preserved if still present.
    pub fn set_items(&mut self, items: Vec<TabItem<K>>) {
        self.active_key = self
            .active_key
            .filter(|active| items.iter().any(|t| t.key == *active));
        self.items = items;
    }

    /// Set (or clear) the active tab.
    pub fn set_active(&mut self, key: Option<K>) {
        self.active_key = key;
    }

    pub fn active(&self) -> Option<K> {
        self.active_key
    }

    pub fn items(&self) -> &[TabItem<K>] {
        &self.items
    }

    /// Hit-test a tab by screen coordinates.
    pub fn hit_test(&self, column: u16, row: u16) -> Option<K> {
        self.item_hits
            .iter()
            .find(|hit| rect_contains(hit.rect, column, row))
            .map(|hit| hit.key)
    }

    fn close_rect(hit: &TabHit<K>) -> LayoutRect {
        LayoutRect {
            x: hit.rect.x + i32::from(hit.rect.width.saturating_sub(CLOSE_BUTTON_WIDTH)),
            y: hit.rect.y,
            width: CLOSE_BUTTON_WIDTH,
            height: hit.rect.height,
        }
    }

    fn hit_test_close(&self, column: u16, row: u16) -> Option<K> {
        self.item_hits
            .iter()
            .find(|hit| hit.closable && rect_contains(Self::close_rect(hit), column, row))
            .map(|hit| hit.key)
    }

    fn hit_test_tab(&self, column: u16, row: u16) -> Option<(K, LayoutRect)> {
        self.item_hits
            .iter()
            .find(|hit| rect_contains(hit.rect, column, row))
            .map(|hit| (hit.key, hit.rect))
    }

    /// Insertion index for `virtual_col` in the list EXCLUDING the dragged tab,
    /// defaulting to the end.
    fn target_index(&self, virtual_col: i32) -> usize {
        let source = self.drag_state.as_ref().map(|s| s.source);
        let mut reduced = 0usize;
        let mut idx = self.items.len().saturating_sub(1);
        for item in self.items.iter() {
            if Some(item.key) == source {
                continue;
            }
            if let Some((_, lx, width)) = self.entry_geometry.iter().find(|(k, ..)| *k == item.key)
                && virtual_col < lx + i32::from(*width)
            {
                idx = reduced;
                break;
            }
            reduced += 1;
        }
        idx
    }
}

impl<K: Copy + PartialEq + Eq + std::hash::Hash + std::fmt::Debug + 'static> Default
    for TabBarComponent<K>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Copy + PartialEq + Eq + std::hash::Hash + std::fmt::Debug + 'static>
    Component<TabBarEvent<K>> for TabBarComponent<K>
{
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        rect: LayoutRect,
        ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        let theme = ctx.config().theme;
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        self.rect = rect;
        let ratatui_backend = crate::helpers::downcast_ratatui(backend);
        let ratatui_area = layout_rect_to_clipped_rect(rect);
        let bounds = ratatui_area.intersection(ratatui_backend.buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        let y = rect.y;
        self.left_indicator_rect = None;
        self.right_indicator_rect = None;

        // ── Phase 1: measure content + decide scroll geometry (NO drawing) ──
        let mut entry_geometry: Vec<(K, i32, u16)> = Vec::new();
        let mut focused_range: Option<(i32, u16)> = None;
        let mut logical_x: i32 = 0;
        for item in self.items.iter() {
            let label = truncate_with_ellipsis(&item.label, MAX_ENTRY_LABEL);
            let close_cols = if item.closable { CLOSE_BUTTON_WIDTH } else { 0 };
            let width = (label.width() as u16)
                .saturating_add(ENTRY_PADDING as u16)
                .saturating_add(close_cols);
            entry_geometry.push((item.key, logical_x, width));
            if Some(item.key) == self.active_key {
                focused_range = Some((logical_x, width));
            }
            logical_x = logical_x.saturating_add(i32::from(width));
        }
        let content_width = logical_x;
        let max_entries_width = i32::from(rect.width);
        let overflow = content_width > max_entries_width;
        let (left_indicator_width, right_indicator_width) =
            if overflow { (1u16, 1u16) } else { (0u16, 0u16) };
        let chevron_gap = if overflow {
            i32::from(CHEVRON_GAP) * 2
        } else {
            0
        };
        let scroll_viewport_width = if overflow {
            max_entries_width.saturating_sub(
                i32::from(left_indicator_width + right_indicator_width) + chevron_gap,
            )
        } else {
            max_entries_width
        };
        let entries_start_x = rect.x
            + i32::from(left_indicator_width)
            + if overflow { i32::from(CHEVRON_GAP) } else { 0 };
        let max_scroll = content_width.saturating_sub(scroll_viewport_width).max(0) as u16;

        self.h_scroll = self.h_scroll.min(max_scroll);
        let focused_bounds = focused_range.map(|(x, w)| (x, i32::from(w)));
        let auto_scroll = self.last_auto_focus != self.active_key
            || self.last_auto_viewport != scroll_viewport_width
            || self.last_focused_logical_bounds != focused_bounds;
        if self.drag_state.is_none()
            && auto_scroll
            && let Some((flx, fw)) = focused_range
        {
            self.h_scroll = scroll_into_view_target(
                i32::from(self.h_scroll),
                flx,
                i32::from(fw),
                scroll_viewport_width,
                i32::from(max_scroll),
            ) as u16;
        }
        self.last_auto_focus = self.active_key;
        self.last_auto_viewport = scroll_viewport_width;
        self.last_focused_logical_bounds = focused_bounds;

        // Consume a click-requested scroll. drag_state must be checked BEFORE
        // `.take()`: the `if let` scrutinee runs first, so taking would evict the
        // pending target on the frame after a click while the button is still held
        // (drag_state Some → guard fails) — silently breaking click-to-scroll.
        if self.drag_state.is_none()
            && let Some(key) = self.pending_scroll_to.take()
            && let Some((_, lx, width)) = entry_geometry.iter().find(|(k, ..)| *k == key)
        {
            self.h_scroll = scroll_into_view_target(
                i32::from(self.h_scroll),
                *lx,
                i32::from(*width),
                scroll_viewport_width,
                i32::from(max_scroll),
            ) as u16;
        }

        self.entries_start_x = entries_start_x;
        self.scroll_viewport_width = scroll_viewport_width;
        self.max_scroll = max_scroll;
        self.entry_geometry = entry_geometry.clone();

        // ── Phase 2: paint tabs with immutable h_scroll (absolute coords) ──
        let h_scroll = i32::from(self.h_scroll);
        let indicator_style = Style::default()
            .fg(color_to_ratatui(theme.panel_inactive_fg))
            .add_modifier(Modifier::BOLD);

        for (key, lx, width) in entry_geometry.iter().copied() {
            // The dragged tab is drawn as a gliding ghost AFTER the loop. Only
            // treat it as "being dragged" once the cursor has actually moved.
            let dragging = self
                .drag_state
                .as_ref()
                .is_some_and(|s| s.source == key && s.moved);
            if dragging {
                continue;
            }
            let width_i = i32::from(width);
            let vp_x = lx.saturating_sub(h_scroll);
            let vp_end = vp_x.saturating_add(width_i);
            if vp_end <= 0 || vp_x >= scroll_viewport_width {
                continue;
            }
            let slice_start = if vp_x < 0 { (-vp_x) as usize } else { 0 };
            let visible_left = vp_x.max(0);
            let visible_right = scroll_viewport_width.min(vp_end);
            let visible_width = (visible_right.saturating_sub(visible_left)) as usize;
            let item = self
                .items
                .iter()
                .find(|t| t.key == key)
                .expect("geometry from items");
            let label = truncate_with_ellipsis(&item.label, MAX_ENTRY_LABEL);
            let mut chunk = format!(" {label} ");
            if item.closable {
                chunk.push(' ');
                chunk.push_str(CLOSE_GLYPH);
            }
            let label_slice = slice_by_columns(&chunk, slice_start, visible_width);
            let draw_x = entries_start_x.saturating_add(visible_left);

            let focused = Some(key) == self.active_key;
            let item_style = item.style_override.unwrap_or_else(|| {
                if focused {
                    Style::default()
                        .bg(color_to_ratatui(theme.menu_selected_bg))
                        .fg(color_to_ratatui(theme.menu_selected_fg))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(color_to_ratatui(theme.panel_inactive_fg))
                }
            });
            safe_set_string(
                &mut ratatui_backend.buffer,
                bounds,
                draw_x as u16,
                y as u16,
                &label_slice,
                item_style,
            );
            self.item_hits.push(TabHit {
                key,
                rect: LayoutRect {
                    x: draw_x,
                    y,
                    width: visible_width as u16,
                    height: 1,
                },
                closable: item.closable,
            });
        }

        // ── Drag ghost: the grabbed tab glides with the cursor (scroll-thumb
        // behavior), keeping its normal style, clipped to the viewport, drawn
        // last so it floats above the static tabs. ──
        if let Some(state) = &self.drag_state
            && state.moved
        {
            let drag_width = self
                .entry_geometry
                .iter()
                .find(|(k, ..)| *k == state.source)
                .map(|(_, _, w)| i32::from(*w))
                .unwrap_or(0);
            let vp_start = entries_start_x;
            let max_thumb_x = (vp_start + scroll_viewport_width - drag_width).max(vp_start);
            let thumb_x = (state.cursor_col - state.grab_col).clamp(vp_start, max_thumb_x);

            let gv_x = thumb_x.saturating_sub(vp_start);
            let gv_end = gv_x.saturating_add(drag_width);
            if gv_end > 0 && gv_x < scroll_viewport_width {
                let slice_start = if gv_x < 0 { (-gv_x) as usize } else { 0 };
                let gl = gv_x.max(0);
                let gr = scroll_viewport_width.min(gv_end);
                let visible_width = (gr.saturating_sub(gl)) as usize;
                let item = self
                    .items
                    .iter()
                    .find(|t| t.key == state.source)
                    .expect("drag source from items");
                let label = truncate_with_ellipsis(&item.label, MAX_ENTRY_LABEL);
                let mut chunk = format!(" {label} ");
                if item.closable {
                    chunk.push(' ');
                    chunk.push_str(CLOSE_GLYPH);
                }
                let label_slice = slice_by_columns(&chunk, slice_start, visible_width);
                let draw_x = vp_start.saturating_add(gl);
                let focused = Some(state.source) == self.active_key;
                let ghost_style = item.style_override.unwrap_or_else(|| {
                    if focused {
                        Style::default()
                            .bg(color_to_ratatui(theme.menu_selected_bg))
                            .fg(color_to_ratatui(theme.menu_selected_fg))
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(color_to_ratatui(theme.panel_inactive_fg))
                    }
                });
                safe_set_string(
                    &mut ratatui_backend.buffer,
                    bounds,
                    draw_x as u16,
                    y as u16,
                    &label_slice,
                    ghost_style,
                );
            }

            // Drop indicator at the insertion boundary among the static tabs.
            let drop_index = state.drop_index;
            let mut gap_logical = content_width.saturating_sub(drag_width);
            let mut acc = 0i32;
            let mut reduced = 0usize;
            for (key, _, width) in entry_geometry.iter().copied() {
                if Some(key) == Some(state.source) {
                    continue;
                }
                if reduced == drop_index {
                    gap_logical = acc;
                    break;
                }
                acc = acc.saturating_add(i32::from(width));
                reduced += 1;
            }
            let vp_gap = gap_logical.saturating_sub(h_scroll);
            if vp_gap >= 0 && vp_gap < scroll_viewport_width {
                let drop_style = Style::default()
                    .fg(color_to_ratatui(theme.accent))
                    .add_modifier(Modifier::BOLD);
                safe_set_string(
                    &mut ratatui_backend.buffer,
                    bounds,
                    (entries_start_x.saturating_add(vp_gap)) as u16,
                    y as u16,
                    DROP_INDICATOR,
                    drop_style,
                );
            }
        }

        // ── Overflow indicators (inside the bar rect, left/right edges) ──
        if left_indicator_width == 1 && self.h_scroll > 0 {
            safe_set_string(
                &mut ratatui_backend.buffer,
                bounds,
                rect.x as u16,
                y as u16,
                LEFT_INDICATOR,
                indicator_style,
            );
            self.left_indicator_rect = Some(LayoutRect {
                x: rect.x,
                y,
                width: 1,
                height: 1,
            });
        }
        if right_indicator_width == 1 && self.h_scroll < max_scroll {
            let ix = rect
                .x
                .saturating_add(i32::from(rect.width.saturating_sub(1)));
            safe_set_string(
                &mut ratatui_backend.buffer,
                bounds,
                ix as u16,
                y as u16,
                RIGHT_INDICATOR,
                indicator_style,
            );
            self.right_indicator_rect = Some(LayoutRect {
                x: ix,
                y,
                width: 1,
                height: 1,
            });
        }
    }

    fn handle_events(
        &mut self,
        event: &Event,
        _ctx: &ComponentContext,
    ) -> EventResult<TabBarEvent<K>> {
        let Event::Mouse(mouse) = event else {
            return EventResult::Ignored;
        };
        match mouse.kind {
            MouseEventKind::Press(_) => self.handle_press(mouse.column, mouse.row),
            // Drag/Release/Scroll are delivered under mouse capture and must
            // never fall through to the host below.
            MouseEventKind::Drag(_) => self.handle_drag(mouse.column),
            MouseEventKind::Release(_) => self.handle_release(),
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => {
                self.handle_scroll(mouse.kind)
            }
            _ => EventResult::Consumed,
        }
    }
}

impl<K: Copy + PartialEq + Eq + std::hash::Hash + std::fmt::Debug + 'static> TabBarComponent<K> {
    /// Press within the bar: scroll a chevron, close a tab, select a tab, or ignore.
    fn handle_press(&mut self, column: u16, row: u16) -> EventResult<TabBarEvent<K>> {
        if let Some(rect) = self.left_indicator_rect
            && rect_contains(rect, column, row)
        {
            self.h_scroll = self.h_scroll.saturating_sub(SCROLL_STEP);
            return EventResult::Consumed;
        }
        if let Some(rect) = self.right_indicator_rect
            && rect_contains(rect, column, row)
        {
            self.h_scroll = self
                .h_scroll
                .saturating_add(SCROLL_STEP)
                .min(self.max_scroll);
            return EventResult::Consumed;
        }
        // Close glyph first — never arm a drag from the close button.
        if let Some(key) = self.hit_test_close(column, row) {
            self.drag_state = None;
            return EventResult::Action(TabBarEvent::Close(key));
        }
        if let Some((key, rect)) = self.hit_test_tab(column, row) {
            // Queue a scroll-into-view for the next render pass. We deliberately
            // do NOT touch h_scroll here: entry_geometry/item_hits are only
            // refreshed during render(), and mutating h_scroll mid-event would
            // desync DragState::grab_col (from the prior frame's hit rects) if a
            // Drag arrives before the next render.
            self.pending_scroll_to = Some(key);
            let grab_col = i32::from(column) - rect.x;
            let drop_index = self.items.iter().position(|t| t.key == key).unwrap_or(0);
            self.drag_state = Some(DragState {
                source: key,
                cursor_col: i32::from(column),
                grab_col,
                drop_index,
                moved: false,
            });
            return EventResult::Action(TabBarEvent::Select(key));
        }
        EventResult::Ignored
    }

    /// Drag (mouse captured): edge-pan at the bar's edges and track the cursor +
    /// drop target. The reorder is committed on Release; the gliding ghost is the
    /// visual feedback.
    fn handle_drag(&mut self, column: u16) -> EventResult<TabBarEvent<K>> {
        if self.drag_state.is_some() {
            // Edge-panning: indicators can't be clicked while the button is
            // held, so nudge the scroll when the cursor reaches a viewport edge.
            if i32::from(column) <= self.entries_start_x {
                self.h_scroll = self.h_scroll.saturating_sub(SCROLL_STEP);
            } else if i32::from(column) >= self.entries_start_x + self.scroll_viewport_width {
                self.h_scroll = self
                    .h_scroll
                    .saturating_add(SCROLL_STEP)
                    .min(self.max_scroll);
            }
            let virtual_col = i32::from(column) - self.entries_start_x + i32::from(self.h_scroll);
            let drop_index = self.target_index(virtual_col);
            if let Some(state) = &mut self.drag_state {
                state.moved = true;
                state.cursor_col = i32::from(column);
                state.drop_index = drop_index;
            }
        }
        EventResult::Consumed
    }

    /// Release: commit the reorder once if the tab actually moved.
    fn handle_release(&mut self) -> EventResult<TabBarEvent<K>> {
        let Some(state) = self.drag_state.take() else {
            return EventResult::Consumed;
        };
        if state.moved {
            let source_index = self.items.iter().position(|t| t.key == state.source);
            if source_index != Some(state.drop_index) {
                return EventResult::Action(TabBarEvent::Reorder {
                    key: state.source,
                    target_index: state.drop_index,
                });
            }
        }
        EventResult::Consumed
    }

    fn handle_scroll(&mut self, kind: MouseEventKind) -> EventResult<TabBarEvent<K>> {
        match kind {
            MouseEventKind::ScrollLeft => {
                self.h_scroll = self.h_scroll.saturating_sub(SCROLL_STEP);
            }
            MouseEventKind::ScrollRight => {
                self.h_scroll = self
                    .h_scroll
                    .saturating_add(SCROLL_STEP)
                    .min(self.max_scroll);
            }
            _ => {}
        }
        EventResult::Consumed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use term_wm_core::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    fn make_items(keys: &[usize], closable: bool) -> Vec<TabItem<usize>> {
        keys.iter()
            .enumerate()
            .map(|(i, k)| TabItem {
                key: *k,
                label: format!("Window {i}"),
                closable,
                style_override: None,
            })
            .collect()
    }

    fn make_backend(w: u16, h: u16) -> term_wm_console::RatatuiBackend {
        let buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, w, h));
        term_wm_console::RatatuiBackend::new_simple(buf, ratatui::layout::Rect::new(0, 0, w, h))
    }

    fn ctx() -> ComponentContext {
        ComponentContext::new(false)
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    /// Render the bar at a fixed rect with the given items.
    fn render_bar(bar: &mut TabBarComponent<usize>, width: u16, height: u16, rect: LayoutRect) {
        // Mirrors the production lifecycle (WmTopPanelComponent calls
        // bar.begin_frame() before render): clears per-frame hit state so
        // item_hits don't accumulate across renders.
        bar.begin_frame();
        let mut backend = make_backend(width, height);
        let c = ctx();
        let mut reg = term_wm_core::hitbox_registry::HitboxRegistry::new();
        bar.render(&mut backend, rect, &c, &mut reg);
    }

    fn rect(x: i32, y: i32, width: u16, height: u16) -> LayoutRect {
        LayoutRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn select_presses_and_close_glyph_closes() {
        let mut bar = TabBarComponent::<usize>::new();
        bar.set_items(make_items(&[0, 1, 2], true));
        bar.set_active(Some(0));
        let r = rect(0, 0, 80, 1);
        render_bar(&mut bar, 80, 1, r);
        let c = ctx();

        // Press on the body of tab 1 → Select(1).
        let (_, lx1, _) = bar.entry_geometry[1];
        let body_x = (bar.entries_start_x + lx1 + 1) as u16;
        let res = bar.handle_events(
            &mouse(MouseEventKind::Press(MouseButton::Left), body_x, 0),
            &c,
        );
        assert!(matches!(res, EventResult::Action(TabBarEvent::Select(1))));
        assert!(bar.drag_state.is_some(), "pressing a tab arms a drag");

        // Press on the close glyph (rightmost CLOSE_BUTTON_WIDTH cols) of tab 2.
        let (_, lx2, w2) = bar.entry_geometry[2];
        let close_x = (bar.entries_start_x + lx2 + i32::from(w2 - CLOSE_BUTTON_WIDTH)) as u16;
        let res = bar.handle_events(
            &mouse(MouseEventKind::Press(MouseButton::Left), close_x, 0),
            &c,
        );
        assert!(
            matches!(res, EventResult::Action(TabBarEvent::Close(2))),
            "close glyph press must emit Close, not arm a drag"
        );
        assert!(
            bar.drag_state.is_none(),
            "close-glyph press must NOT arm a drag"
        );
    }

    #[test]
    fn pressed_tab_stays_visible_before_drag() {
        let mut bar = TabBarComponent::<usize>::new();
        bar.set_items(make_items(&[0, 1, 2], false));
        bar.set_active(Some(0));
        let r = rect(0, 0, 80, 1);
        render_bar(&mut bar, 80, 1, r);
        let c = ctx();

        // Press tab 1 (body) — it must STAY rendered; no ghost yet.
        let (_, lx1, _) = bar.entry_geometry[1];
        let body_x = (bar.entries_start_x + lx1 + 1) as u16;
        let res = bar.handle_events(
            &mouse(MouseEventKind::Press(MouseButton::Left), body_x, 0),
            &c,
        );
        assert!(matches!(res, EventResult::Action(TabBarEvent::Select(1))));
        assert!(!bar.drag_state.as_ref().unwrap().moved);

        let mut backend = make_backend(80, 1);
        let mut reg = term_wm_core::hitbox_registry::HitboxRegistry::new();
        bar.render(&mut backend, r, &ctx(), &mut reg);
        let cells: Vec<char> = (0..80u16)
            .map(|xx| {
                backend
                    .buffer
                    .cell((xx, 0))
                    .map(|c| c.symbol().chars().next().unwrap_or(' '))
                    .unwrap_or(' ')
            })
            .collect();
        let needle: Vec<char> = "Window 1".chars().collect();
        let starts: Vec<usize> = (0..=cells.len().saturating_sub(needle.len()))
            .filter(|&i| cells[i..i + needle.len()] == needle)
            .collect();
        assert!(
            !starts.is_empty(),
            "the pressed tab must remain visible before any drag, starts={starts:?}"
        );
    }

    #[test]
    fn drag_release_reorders() {
        let mut bar = TabBarComponent::<usize>::new();
        bar.set_items(make_items(&[0, 1, 2], false));
        bar.set_active(Some(0));
        let r = rect(0, 0, 80, 1);
        render_bar(&mut bar, 80, 1, r);
        let c = ctx();

        // Press tab 2, drag to the left edge → drop at the front.
        let (_, lx2, _) = bar.entry_geometry[2];
        let body_x = (bar.entries_start_x + lx2 + 1) as u16;
        bar.handle_events(
            &mouse(MouseEventKind::Press(MouseButton::Left), body_x, 0),
            &c,
        );
        let res = bar.handle_events(
            &mouse(
                MouseEventKind::Drag(MouseButton::Left),
                bar.entries_start_x as u16,
                0,
            ),
            &c,
        );
        assert!(matches!(res, EventResult::Consumed));
        assert_eq!(bar.drag_state.as_ref().unwrap().drop_index, 0);
        // Release commits the reorder.
        let res = bar.handle_events(
            &mouse(
                MouseEventKind::Release(MouseButton::Left),
                bar.entries_start_x as u16,
                0,
            ),
            &c,
        );
        assert!(matches!(
            res,
            EventResult::Action(TabBarEvent::Reorder {
                key: 2,
                target_index: 0
            })
        ));
        assert!(bar.drag_state.is_none());
    }

    #[test]
    fn plain_click_does_not_reorder() {
        let mut bar = TabBarComponent::<usize>::new();
        bar.set_items(make_items(&[0, 1, 2], false));
        bar.set_active(Some(0));
        let r = rect(0, 0, 80, 1);
        render_bar(&mut bar, 80, 1, r);
        let c = ctx();

        bar.handle_events(&mouse(MouseEventKind::Press(MouseButton::Left), 18, 0), &c);
        let res = bar.handle_events(
            &mouse(MouseEventKind::Release(MouseButton::Left), 18, 0),
            &c,
        );
        assert!(
            matches!(res, EventResult::Consumed),
            "a plain click must not emit a Reorder"
        );
    }

    #[test]
    fn drag_off_right_edge_clamps_to_end() {
        let mut bar = TabBarComponent::<usize>::new();
        bar.set_items(make_items(&[0, 1, 2], false));
        bar.set_active(Some(0));
        let r = rect(0, 0, 30, 1);
        render_bar(&mut bar, 30, 1, r); // narrow → overflow
        let c = ctx();

        bar.handle_events(&mouse(MouseEventKind::Press(MouseButton::Left), 1, 0), &c);
        // Drag far right → clamps to the end (target_index = len-1 reduced = 2).
        bar.handle_events(&mouse(MouseEventKind::Drag(MouseButton::Left), 300, 0), &c);
        let res = bar.handle_events(
            &mouse(MouseEventKind::Release(MouseButton::Left), 300, 0),
            &c,
        );
        assert!(matches!(
            res,
            EventResult::Action(TabBarEvent::Reorder {
                key: 0,
                target_index: 2
            })
        ));
    }

    #[test]
    fn scroll_events_adjust_h_scroll() {
        let mut bar = TabBarComponent::<usize>::new();
        bar.set_items(make_items(&[0, 1, 2, 3, 4, 5, 6, 7], false));
        bar.set_active(Some(3));
        let r = rect(0, 0, 30, 1);
        render_bar(&mut bar, 30, 1, r);
        let c = ctx();
        let before = bar.h_scroll;

        bar.handle_events(&mouse(MouseEventKind::ScrollRight, 0, 0), &c);
        assert!(bar.h_scroll > before, "ScrollRight must increase h_scroll");
        bar.handle_events(&mouse(MouseEventKind::ScrollLeft, 0, 0), &c);
        assert!(bar.h_scroll <= before + SCROLL_STEP, "ScrollLeft decreases");
    }

    #[test]
    fn overflow_shows_chevrons_with_gap() {
        let mut bar = TabBarComponent::<usize>::new();
        bar.set_items(make_items(&[0, 1, 2, 3, 4, 5, 6, 7], false));
        bar.set_active(Some(3));
        let r = rect(0, 0, 30, 1);
        render_bar(&mut bar, 30, 1, r);
        // Overflow → both chevrons present; first tab starts 1 col (chevron) +
        // CHEVRON_GAP after the left edge.
        assert!(bar.left_indicator_rect.is_some());
        assert!(bar.right_indicator_rect.is_some());
        assert_eq!(bar.entries_start_x, 1 + i32::from(CHEVRON_GAP));
        // No tab's visible right edge reaches the right chevron.
        let right_x = bar.right_indicator_rect.unwrap().x;
        for hit in &bar.item_hits {
            assert!(hit.rect.x + i32::from(hit.rect.width) <= right_x);
        }
    }

    #[test]
    fn closable_tabs_are_wider_and_close_glyph_hits() {
        let mut bar = TabBarComponent::<usize>::new();
        // closable: each tab is ` {label} ✕` = 12 cols (8 label + 2 pad + 2 close).
        bar.set_items(make_items(&[0, 1], true));
        bar.set_active(Some(0));
        let r = rect(0, 0, 80, 1);
        render_bar(&mut bar, 80, 1, r);
        assert_eq!(
            bar.entry_geometry[1].2, 12,
            "closable tab width must include CLOSE_BUTTON_WIDTH"
        );
        // Close glyph of tab 0 sits at its rightmost 2 columns.
        let (_, lx0, w0) = bar.entry_geometry[0];
        let close_x = (bar.entries_start_x + lx0 + i32::from(w0 - CLOSE_BUTTON_WIDTH)) as u16;
        let c = ctx();
        let res = bar.handle_events(
            &mouse(MouseEventKind::Press(MouseButton::Left), close_x, 0),
            &c,
        );
        assert!(matches!(res, EventResult::Action(TabBarEvent::Close(0))));
    }

    #[test]
    fn manual_scroll_persists_until_active_changes() {
        let mut bar = TabBarComponent::<usize>::new();
        bar.set_items(make_items(&[0, 1, 2, 3, 4, 5, 6, 7], false));
        bar.set_active(Some(3));
        let r = rect(0, 0, 30, 1);
        render_bar(&mut bar, 30, 1, r);
        let after_auto = bar.h_scroll;
        assert!(after_auto > SCROLL_STEP, "auto-scroll should land mid-bar");

        // Manual scroll persists with unchanged active.
        let manual = after_auto.saturating_sub(SCROLL_STEP);
        bar.h_scroll = manual;
        render_bar(&mut bar, 30, 1, r);
        assert_eq!(bar.h_scroll, manual, "manual scroll must persist");

        // Changing active re-engages auto-scroll.
        bar.set_active(Some(6));
        bar.h_scroll = 0;
        render_bar(&mut bar, 30, 1, r);
        assert!(bar.h_scroll > 0, "auto-scroll re-engages on active change");
    }

    #[test]
    fn reorder_moves_active_off_screen_then_auto_scrolls_back() {
        let mut bar = TabBarComponent::<usize>::new();
        bar.set_items(make_items(&[0, 1, 2, 3, 4, 5, 6, 7], false));
        bar.set_active(Some(0));
        let r = rect(0, 0, 30, 1);
        render_bar(&mut bar, 30, 1, r);
        assert_eq!(bar.h_scroll, 0);

        // Manual scroll far right persists.
        bar.h_scroll = bar.max_scroll;
        render_bar(&mut bar, 30, 1, r);
        assert_eq!(bar.h_scroll, bar.max_scroll);

        // Structural change: move the ACTIVE tab (0) to the end → its logical
        // bounds change → auto-scroll re-follows it into view.
        let mut items = make_items(&[1, 2, 3, 4, 5, 6, 7, 0], false);
        for item in items.iter_mut() {
            item.label = format!("Window {}", item.key);
        }
        bar.set_items(items);
        bar.h_scroll = 0;
        render_bar(&mut bar, 30, 1, r);
        assert_eq!(
            bar.h_scroll, bar.max_scroll,
            "auto-scroll re-follows the active tab after a structural reorder"
        );
    }

    #[test]
    fn clicking_partially_visible_tab_scrolls_it_into_view() {
        let mut bar = TabBarComponent::<usize>::new();
        bar.set_items(make_items(&[0, 1, 2, 3, 4, 5, 6, 7], false));
        bar.set_active(Some(0));
        let r = rect(0, 0, 30, 1);
        render_bar(&mut bar, 30, 1, r);
        // Manual scroll so tab 6 (lx=60, width 10) is half-hidden at the right
        // edge: with h_scroll=40, viewport 26 wide, tab 6 spans vp cols 20..26.
        bar.h_scroll = 40;
        render_bar(&mut bar, 30, 1, r);
        let c = ctx();

        let res = bar.handle_events(&mouse(MouseEventKind::Press(MouseButton::Left), 23, 0), &c);
        assert!(matches!(res, EventResult::Action(TabBarEvent::Select(6))));
        assert_eq!(bar.h_scroll, 40, "press must not mutate h_scroll");
        assert_eq!(bar.pending_scroll_to, Some(6));

        // Release completes the click (a real click = press + release), clearing
        // the drag state so the next render consumes the pending scroll.
        let _ = bar.handle_events(&mouse(MouseEventKind::Release(MouseButton::Left), 23, 0), &c);
        assert_eq!(bar.h_scroll, 40, "release must not mutate h_scroll");

        // Next render consumes the pending scroll and brings tab 6 fully in view.
        render_bar(&mut bar, 30, 1, r);
        assert_eq!(bar.h_scroll, 44, "tab 6 right edge lands at viewport end");
        let vp_x = 60_i32.saturating_sub(i32::from(bar.h_scroll));
        let vp_end = vp_x.saturating_add(10);
        assert!(
            vp_x >= 0 && vp_end <= bar.scroll_viewport_width,
            "tab 6 fully within viewport"
        );
        assert_eq!(bar.pending_scroll_to, None, "pending target consumed");
    }

    #[test]
    fn clicking_oversized_tab_left_aligns() {
        let mut bar = TabBarComponent::<usize>::new();
        let mut items = make_items(&[0], false);
        items[0].label = "a".repeat(50); // truncated to MAX_ENTRY_LABEL (40 cols)
        bar.set_items(items);
        bar.set_active(Some(0));
        let r = rect(0, 0, 20, 1);
        render_bar(&mut bar, 20, 1, r);
        let (_, lx0, _) = bar.entry_geometry[0];
        assert!(lx0 >= 0, "single tab starts at logical x 0");

        // Scroll to the end so only the tab's tail is visible.
        bar.h_scroll = bar.max_scroll;
        render_bar(&mut bar, 20, 1, r);
        let c = ctx();

        // Click the visible portion → next render left-aligns the oversized tab.
        let res = bar.handle_events(&mouse(MouseEventKind::Press(MouseButton::Left), 5, 0), &c);
        assert!(matches!(res, EventResult::Action(TabBarEvent::Select(0))));
        assert_eq!(bar.pending_scroll_to, Some(0));

        // Release completes the click so the pending scroll is consumed.
        let _ = bar.handle_events(&mouse(MouseEventKind::Release(MouseButton::Left), 5, 0), &c);

        render_bar(&mut bar, 20, 1, r);
        assert_eq!(bar.h_scroll, 0, "oversized tab left-aligns to its logical origin");
    }

    #[test]
    fn clicking_active_tab_reeveals_it() {
        let mut bar = TabBarComponent::<usize>::new();
        bar.set_items(make_items(&[0, 1, 2, 3, 4, 5, 6, 7], false));
        bar.set_active(Some(6));
        let r = rect(0, 0, 30, 1);
        render_bar(&mut bar, 30, 1, r);
        // Active tab 6 is fully in view after the first render; now scroll it
        // partially out again without changing focus.
        bar.h_scroll = 40;
        render_bar(&mut bar, 30, 1, r);
        let c = ctx();

        // Clicking the already-active tab doesn't trip the auto-scroll guard,
        // but the pending path still reveals it.
        let res = bar.handle_events(&mouse(MouseEventKind::Press(MouseButton::Left), 23, 0), &c);
        assert!(matches!(res, EventResult::Action(TabBarEvent::Select(6))));

        // Release completes the click so the pending scroll is consumed.
        let _ = bar.handle_events(&mouse(MouseEventKind::Release(MouseButton::Left), 23, 0), &c);

        render_bar(&mut bar, 30, 1, r);
        assert_eq!(bar.h_scroll, 44, "clicked active tab scrolled back into view");
    }

    #[test]
    fn drag_reorder_still_works_after_pending_scroll() {
        let mut bar = TabBarComponent::<usize>::new();
        bar.set_items(make_items(&[0, 1, 2, 3, 4, 5, 6, 7], false));
        bar.set_active(Some(0));
        let r = rect(0, 0, 30, 1);
        render_bar(&mut bar, 30, 1, r);
        bar.h_scroll = 40;
        render_bar(&mut bar, 30, 1, r);
        let c = ctx();

        // Press a partially-visible tab: queues a scroll, arms a drag.
        let res = bar.handle_events(&mouse(MouseEventKind::Press(MouseButton::Left), 23, 0), &c);
        assert!(matches!(res, EventResult::Action(TabBarEvent::Select(6))));
        assert_eq!(bar.h_scroll, 40, "press does not mutate h_scroll");
        assert!(bar.drag_state.is_some());

        // Drag to column 10 WITHOUT a render: drop target uses the current
        // h_scroll (40) + logical geometry → virtual_col = 10 - 2 + 40 = 48,
        // which lands before tab 4 (lx 40..50).
        let res = bar.handle_events(&mouse(MouseEventKind::Drag(MouseButton::Left), 10, 0), &c);
        assert!(matches!(res, EventResult::Consumed));
        assert_eq!(bar.h_scroll, 40, "drag must not alter h_scroll");
        assert_eq!(
            bar.drag_state.as_ref().unwrap().drop_index, 4,
            "reorder target computed from current scroll, no drift"
        );

        // A render while the button is held must NOT consume the pending scroll.
        render_bar(&mut bar, 30, 1, r);
        assert_eq!(bar.pending_scroll_to, Some(6), "pending survives active drag");

        // Release commits the reorder.
        let res = bar.handle_events(&mouse(MouseEventKind::Release(MouseButton::Left), 10, 0), &c);
        assert!(matches!(
            res,
            EventResult::Action(TabBarEvent::Reorder {
                key: 6,
                target_index: 4
            })
        ));

        // Next render applies the pending scroll.
        render_bar(&mut bar, 30, 1, r);
        assert_eq!(bar.h_scroll, 44, "pending scroll applies after release");
    }
}
