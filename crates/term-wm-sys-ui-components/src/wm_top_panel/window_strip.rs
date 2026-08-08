//! Window strip applet — the scrollable, drag-reorderable window entry list.
//!
//! Owns its horizontal scroll offset, overflow chevrons, and drag-to-reorder
//! state. It renders and interacts strictly within the `rect` the parent
//! allocates it (after reserving the menu on the left and the tiling indicator
//! on the right), so its `◀`/`▶` indicators are always contained.

use std::collections::BTreeMap;

use ratatui::style::{Modifier, Style};

use term_wm_core::{
    actions::{EventResult, TermWmAction},
    events::MouseEventKind,
    layout::rect_contains,
    theme::Theme,
    utils::truncate_with_ellipsis,
    window::WindowKey,
};
use term_wm_layout_engine::LayoutRect;
use term_wm_ui_components::helpers::{color_to_ratatui, layout_rect_to_clipped_rect, safe_set_string, slice_by_columns};

/// One space on each side of a window entry label (`" {label} "`).
const ENTRY_PADDING: usize = 2;
/// Horizontal scroll step (columns) for wheel / indicator / edge-pan scrolling.
pub(crate) const SCROLL_STEP: u16 = 8;
/// Per-entry label cap (columns) before ellipsis truncation, so extremely long
/// titles don't blow up the measured content width.
const MAX_ENTRY_LABEL: usize = 40;
/// Left overflow indicator glyph.
const LEFT_INDICATOR: &str = "◀";
/// Right overflow indicator glyph.
const RIGHT_INDICATOR: &str = "▶";

#[derive(Debug, Clone, Copy)]
pub(crate) struct PanelWindowHit {
    pub(crate) id: WindowKey,
    pub(crate) rect: LayoutRect,
}

/// Scrollable, draggable window entry strip.
#[derive(Debug)]
pub(crate) struct WindowStrip {
    /// Horizontal scroll offset into the entry strip.
    pub(crate) h_scroll: u16,
    /// Entry being dragged, if a live drag is in progress.
    pub(crate) drag_source: Option<WindowKey>,
    /// Whether the cursor moved (any `Drag` event) since the press. Gates the
    /// "dragging" visual style so a plain click-to-focus doesn't flicker.
    pub(crate) drag_moved: bool,
    // Geometry from the most recent render, reused by the drag handler.
    pub(crate) entries_start_x: i32,
    pub(crate) scroll_viewport_width: i32,
    pub(crate) max_scroll: u16,
    pub(crate) entry_geometry: Vec<(WindowKey, i32, u16)>,
    pub(crate) left_indicator_rect: Option<LayoutRect>,
    pub(crate) right_indicator_rect: Option<LayoutRect>,
    pub(crate) window_hits: Vec<PanelWindowHit>,
    // Frame snapshot of the order this strip rendered (drag index math).
    pub(crate) display_order: Vec<WindowKey>,
    // The rect the strip last rendered into (event routing + containment).
    pub(crate) rect: LayoutRect,
}

impl WindowStrip {
    pub(crate) fn new() -> Self {
        Self {
            h_scroll: 0,
            drag_source: None,
            drag_moved: false,
            entries_start_x: 0,
            scroll_viewport_width: 0,
            max_scroll: 0,
            entry_geometry: Vec::new(),
            left_indicator_rect: None,
            right_indicator_rect: None,
            window_hits: Vec::new(),
            display_order: Vec::new(),
            rect: LayoutRect::default(),
        }
    }

    pub(crate) fn begin_frame(&mut self) {
        self.window_hits.clear();
        self.left_indicator_rect = None;
        self.right_indicator_rect = None;
    }

    pub(crate) fn clear_drag_state(&mut self) {
        self.drag_source = None;
        self.drag_moved = false;
    }

    pub(crate) fn hit_test_window(&self, column: u16, row: u16) -> Option<WindowKey> {
        self.window_hits
            .iter()
            .find(|hit| rect_contains(hit.rect, column, row))
            .map(|hit| hit.id)
    }

    /// Two-phase render: measure + auto-scroll first, then paint with an
    /// immutable `h_scroll`. `rect` is the strip's allocated slot (absolute).
    pub(crate) fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        rect: LayoutRect,
        display_order: &[WindowKey],
        labels: &BTreeMap<WindowKey, String>,
        focus_current: WindowKey,
        theme: &Theme,
    ) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        self.rect = rect;
        let ratatui_backend = term_wm_ui_components::helpers::downcast_ratatui(backend);
        let ratatui_area = layout_rect_to_clipped_rect(rect);
        let bounds = ratatui_area.intersection(ratatui_backend.buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        let y = rect.y;
        // Per-frame geometry — never carry stale indicator rects into a frame
        // where a given chevron is no longer present.
        self.left_indicator_rect = None;
        self.right_indicator_rect = None;

        // ── Phase 1: measure content + decide scroll geometry (NO drawing) ──
        let mut entry_geometry: Vec<(WindowKey, i32, u16)> = Vec::new();
        let mut focused_range: Option<(i32, u16)> = None;
        let mut logical_x: i32 = 0;
        for key in display_order.iter().copied() {
            let label = labels
                .get(&key)
                .cloned()
                .unwrap_or_else(|| format!("{key:?}"));
            let label = truncate_with_ellipsis(&label, MAX_ENTRY_LABEL);
            let width = (label.chars().count() + ENTRY_PADDING) as u16;
            entry_geometry.push((key, logical_x, width));
            if key == focus_current {
                focused_range = Some((logical_x, width));
            }
            logical_x = logical_x.saturating_add(i32::from(width));
        }
        let content_width = logical_x;
        let max_entries_width = i32::from(rect.width);
        // Reserve indicator columns STATICALLY for the whole frame when any
        // overflow exists, so the viewport size doesn't jitter with h_scroll.
        let overflow = content_width > max_entries_width;
        let (left_indicator_width, right_indicator_width) =
            if overflow { (1u16, 1u16) } else { (0u16, 0u16) };
        let scroll_viewport_width = if overflow {
            max_entries_width
                .saturating_sub(i32::from(left_indicator_width + right_indicator_width))
        } else {
            max_entries_width
        };
        let entries_start_x = rect.x + i32::from(left_indicator_width);
        let max_scroll = content_width.saturating_sub(scroll_viewport_width).max(0) as u16;

        self.h_scroll = self.h_scroll.min(max_scroll);
        // Auto-scroll the focused entry into view, UNLESS a drag is active.
        if self.drag_source.is_none()
            && let Some((flx, fw)) = focused_range
        {
            let h = i32::from(self.h_scroll);
            let vp_end = h.saturating_add(scroll_viewport_width);
            if flx < h {
                self.h_scroll = flx.max(0) as u16;
            } else if flx.saturating_add(i32::from(fw)) > vp_end {
                self.h_scroll =
                    (flx.saturating_add(i32::from(fw)) - scroll_viewport_width).max(0) as u16;
            }
            self.h_scroll = self.h_scroll.min(max_scroll);
        }

        // Publish geometry for the drag handler (runs between frames).
        self.entries_start_x = entries_start_x;
        self.scroll_viewport_width = scroll_viewport_width;
        self.max_scroll = max_scroll;
        self.entry_geometry = entry_geometry.clone();
        self.display_order = display_order.to_vec();

        // ── Phase 2: paint entries with immutable h_scroll (absolute coords) ──
        let h_scroll = i32::from(self.h_scroll);
        let indicator_style = Style::default()
            .fg(color_to_ratatui(theme.panel_inactive_fg))
            .add_modifier(Modifier::BOLD);

        for (key, lx, width) in entry_geometry.iter().copied() {
            let width = i32::from(width);
            let vp_x = lx.saturating_sub(h_scroll);
            let vp_end = vp_x.saturating_add(width);
            // Skip entries fully outside the visible viewport.
            if vp_end <= 0 || vp_x >= scroll_viewport_width {
                continue;
            }
            // Uniform left/right clipping: slice the padded label to the
            // visible columns so text never bleeds over the indicators.
            let slice_start = if vp_x < 0 { (-vp_x) as usize } else { 0 };
            let visible_left = vp_x.max(0);
            let visible_right = scroll_viewport_width.min(vp_end);
            let visible_width = (visible_right.saturating_sub(visible_left)) as usize;
            let label = labels
                .get(&key)
                .cloned()
                .unwrap_or_else(|| format!("{key:?}"));
            let label = truncate_with_ellipsis(&label, MAX_ENTRY_LABEL);
            let chunk = format!(" {label} ");
            let label_slice = slice_by_columns(&chunk, slice_start, visible_width);
            let draw_x = entries_start_x.saturating_add(visible_left);

            let focused = key == focus_current;
            let dragging = self.drag_source == Some(key) && self.drag_moved;
            let item_style = if dragging {
                Style::default()
                    .bg(color_to_ratatui(theme.menu_selected_bg))
                    .fg(color_to_ratatui(theme.menu_selected_fg))
                    .add_modifier(Modifier::REVERSED)
            } else if focused {
                Style::default()
                    .bg(color_to_ratatui(theme.menu_selected_bg))
                    .fg(color_to_ratatui(theme.menu_selected_fg))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(color_to_ratatui(theme.panel_inactive_fg))
            };
            safe_set_string(
                &mut ratatui_backend.buffer,
                bounds,
                draw_x as u16,
                y as u16,
                &label_slice,
                item_style,
            );
            self.window_hits.push(PanelWindowHit {
                id: key,
                rect: LayoutRect {
                    x: draw_x,
                    y,
                    width: visible_width as u16,
                    height: 1,
                },
            });
        }

        // Overflow indicators — strictly inside the strip rect (left/right edges).
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
            let ix = rect.x.saturating_add(i32::from(rect.width.saturating_sub(1)));
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

    /// Press within the strip: scroll an indicator, focus an entry, or ignore.
    pub(crate) fn handle_press(&mut self, column: u16, row: u16) -> EventResult<TermWmAction> {
        if let Some(rect) = self.left_indicator_rect
            && rect_contains(rect, column, row)
        {
            self.h_scroll = self.h_scroll.saturating_sub(SCROLL_STEP);
            return EventResult::Consumed;
        }
        if let Some(rect) = self.right_indicator_rect
            && rect_contains(rect, column, row)
        {
            self.h_scroll = self.h_scroll.saturating_add(SCROLL_STEP).min(self.max_scroll);
            return EventResult::Consumed;
        }
        if let Some(key) = self.hit_test_window(column, row) {
            // Clicking an entry focuses it and arms a potential drag; the drag
            // style and live reorder only kick in once the cursor actually
            // moves (first Drag event).
            self.drag_source = Some(key);
            self.drag_moved = false;
            return EventResult::Action(TermWmAction::FocusWindow(key));
        }
        EventResult::Ignored
    }

    /// Drag (mouse captured): live-reorder the dragged entry and edge-pan at
    /// the strip's edges. Returns the reorder action when the target index
    /// changes, so the entry visibly moves with the cursor.
    pub(crate) fn handle_drag(&mut self, column: u16) -> EventResult<TermWmAction> {
        self.drag_moved = true;
        if let Some(source) = self.drag_source {
            // Edge-panning: indicators can't be clicked while the button is
            // held, so nudge the scroll when the cursor reaches a viewport edge.
            if i32::from(column) <= self.entries_start_x {
                self.h_scroll = self.h_scroll.saturating_sub(SCROLL_STEP);
            } else if i32::from(column) >= self.entries_start_x + self.scroll_viewport_width {
                self.h_scroll = self.h_scroll.saturating_add(SCROLL_STEP).min(self.max_scroll);
            }
            let virtual_col =
                i32::from(column) - self.entries_start_x + i32::from(self.h_scroll);
            let index = self.target_index(virtual_col);
            let current = self.display_order.iter().position(|k| *k == source);
            if current != Some(index) {
                return EventResult::Action(TermWmAction::ReorderWindow {
                    key: source,
                    index,
                });
            }
        }
        // Never Ignored while captured: a fall-through would leak the drag
        // coordinates into the terminal/PTY below the panel.
        EventResult::Consumed
    }

    pub(crate) fn handle_release(&mut self) -> EventResult<TermWmAction> {
        self.clear_drag_state();
        EventResult::Consumed
    }

    pub(crate) fn handle_scroll(&mut self, kind: MouseEventKind) -> EventResult<TermWmAction> {
        match kind {
            MouseEventKind::ScrollLeft => {
                self.h_scroll = self.h_scroll.saturating_sub(SCROLL_STEP);
            }
            MouseEventKind::ScrollRight => {
                self.h_scroll = self.h_scroll.saturating_add(SCROLL_STEP).min(self.max_scroll);
            }
            _ => {}
        }
        EventResult::Consumed
    }

    /// Insertion index for `virtual_col` in the list EXCLUDING the dragged
    /// entry (reduced space), defaulting to the end.
    fn target_index(&self, virtual_col: i32) -> usize {
        let mut reduced = 0usize;
        let mut idx = self.display_order.len().saturating_sub(1);
        for key in self.display_order.iter().copied() {
            if Some(key) == self.drag_source {
                continue;
            }
            if let Some((_, lx, width)) = self.entry_geometry.iter().find(|(k, ..)| *k == key)
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
