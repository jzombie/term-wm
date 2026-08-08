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
use term_wm_ui_components::helpers::{
    color_to_ratatui, layout_rect_to_clipped_rect, safe_set_string, slice_by_columns,
};

/// One space on each side of a window entry label (`" {label} "`).
const ENTRY_PADDING: usize = 2;
/// Horizontal scroll step (columns) for wheel / indicator / edge-pan scrolling.
pub(crate) const SCROLL_STEP: u16 = 8;
/// Horizontal gap (columns) between the entry viewport and each `◀`/`▶`
/// chevron, so the indicators don't get buried by the window titles.
pub(crate) const CHEVRON_GAP: u16 = 1;
/// Per-entry label cap (columns) before ellipsis truncation, so extremely long
/// titles don't blow up the measured content width.
const MAX_ENTRY_LABEL: usize = 40;
/// Left overflow indicator glyph.
const LEFT_INDICATOR: &str = "◀";
/// Right overflow indicator glyph.
const RIGHT_INDICATOR: &str = "▶";
/// Drop-indicator glyph drawn at the insertion boundary during drag.
const DROP_INDICATOR: &str = "▌";

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
    /// Last drag cursor column (physical/global); drives the gliding ghost.
    pub(crate) drag_cursor_col: Option<i32>,
    /// Offset from the pressed entry's left edge to the press column, so the
    /// ghost tracks the pointer 1:1 (like grabbing a scroll thumb).
    pub(crate) drag_grab_col: i32,
    /// Insertion index for the drop bar / reorder target.
    pub(crate) drop_index: Option<usize>,
    /// Whether the cursor moved (any `Drag` event) since the press. Gates the
    /// ghost so a plain click-to-focus doesn't show a drag.
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
    // Auto-scroll guards: only re-follow the focused entry when the focused
    // window, its logical bounds, or the viewport size changed, so manual
    // scrolls (chevrons / wheel / edge-pan) persist.
    pub(crate) last_auto_focus: Option<WindowKey>,
    pub(crate) last_auto_viewport: i32,
    pub(crate) last_focused_logical_bounds: Option<(i32, i32)>,
}

impl WindowStrip {
    pub(crate) fn new() -> Self {
        Self {
            h_scroll: 0,
            drag_source: None,
            drag_cursor_col: None,
            drag_grab_col: 0,
            drop_index: None,
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
            last_auto_focus: None,
            last_auto_viewport: 0,
            last_focused_logical_bounds: None,
        }
    }

    pub(crate) fn begin_frame(&mut self) {
        self.window_hits.clear();
        self.left_indicator_rect = None;
        self.right_indicator_rect = None;
    }

    pub(crate) fn clear_drag_state(&mut self) {
        self.drag_source = None;
        self.drag_cursor_col = None;
        self.drag_grab_col = 0;
        self.drop_index = None;
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
        // Reserve indicator + gap columns STATICALLY for the whole frame when
        // any overflow exists, so the viewport size doesn't jitter with
        // h_scroll and the chevrons have breathing room.
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
        // Auto-scroll the focused entry into view ONLY when the focused window,
        // its logical bounds, or the viewport size changed — so manual scrolls
        // (chevron clicks, wheel, edge-pan) persist instead of being snapped
        // back every frame. The logical bounds detect structural mutations
        // (spawn/close window, completed reorder) independently of h_scroll.
        let focused_bounds = focused_range.map(|(x, w)| (x, i32::from(w)));
        let auto_scroll = self.last_auto_focus != Some(focus_current)
            || self.last_auto_viewport != scroll_viewport_width
            || self.last_focused_logical_bounds != focused_bounds;
        if self.drag_source.is_none()
            && auto_scroll
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
        self.last_auto_focus = Some(focus_current);
        self.last_auto_viewport = scroll_viewport_width;
        self.last_focused_logical_bounds = focused_bounds;

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
            // The dragged entry is drawn as a gliding ghost AFTER the loop so it
            // floats above the static entries (highest Z-order).
            if Some(key) == self.drag_source {
                continue;
            }
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
            let item_style = if focused {
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

        // ── Drag ghost: the grabbed title glides with the cursor (scroll-thumb
        // behavior), keeping its normal style, clipped to the viewport and
        // drawn last so it floats above the static entries. ──
        if self.drag_moved
            && let Some(drag_key) = self.drag_source
            && let Some(cursor_col) = self.drag_cursor_col
        {
            let drag_width = self
                .entry_geometry
                .iter()
                .find(|(k, ..)| *k == drag_key)
                .map(|(_, _, w)| i32::from(*w))
                .unwrap_or(0);
            let vp_start = entries_start_x;
            // Clamp never receives max < min (underflow-safe on narrow strips).
            let max_thumb_x = (vp_start + scroll_viewport_width - drag_width).max(vp_start);
            let thumb_x = (cursor_col - self.drag_grab_col).clamp(vp_start, max_thumb_x);

            let gv_x = thumb_x.saturating_sub(vp_start);
            let gv_end = gv_x.saturating_add(drag_width);
            if gv_end > 0 && gv_x < scroll_viewport_width {
                let slice_start = if gv_x < 0 { (-gv_x) as usize } else { 0 };
                let gl = gv_x.max(0);
                let gr = scroll_viewport_width.min(gv_end);
                let visible_width = (gr.saturating_sub(gl)) as usize;
                let label = labels
                    .get(&drag_key)
                    .cloned()
                    .unwrap_or_else(|| format!("{drag_key:?}"));
                let label = truncate_with_ellipsis(&label, MAX_ENTRY_LABEL);
                let chunk = format!(" {label} ");
                let label_slice = slice_by_columns(&chunk, slice_start, visible_width);
                let draw_x = vp_start.saturating_add(gl);
                let focused = drag_key == focus_current;
                let ghost_style = if focused {
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
                    ghost_style,
                );
            }

            // Drop indicator at the insertion boundary among the static others.
            if let Some(drop_index) = self.drop_index {
                let mut gap_logical = content_width.saturating_sub(drag_width);
                let mut acc = 0i32;
                let mut reduced = 0usize;
                for (key, _, width) in entry_geometry.iter().copied() {
                    if Some(key) == self.drag_source {
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
            self.h_scroll = self
                .h_scroll
                .saturating_add(SCROLL_STEP)
                .min(self.max_scroll);
            return EventResult::Consumed;
        }
        if let Some(hit) = self
            .window_hits
            .iter()
            .find(|h| rect_contains(h.rect, column, row))
        {
            // Clicking an entry focuses it and arms a potential drag. The grab
            // offset (press column - entry left edge) makes the ghost track the
            // pointer 1:1, like grabbing a scroll thumb.
            self.drag_source = Some(hit.id);
            self.drag_grab_col = i32::from(column) - hit.rect.x;
            self.drag_cursor_col = Some(i32::from(column));
            self.drag_moved = false;
            self.drop_index = None;
            return EventResult::Action(TermWmAction::FocusWindow(hit.id));
        }
        EventResult::Ignored
    }

    /// Drag (mouse captured): edge-pan at the strip's edges and track the
    /// cursor + drop target. The reorder itself is committed on `Release`; the
    /// gliding ghost (rendered each frame) is the visual feedback.
    pub(crate) fn handle_drag(&mut self, column: u16) -> EventResult<TermWmAction> {
        self.drag_moved = true;
        self.drag_cursor_col = Some(i32::from(column));
        if self.drag_source.is_some() {
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
            self.drop_index = Some(self.target_index(virtual_col));
        }
        // Never Ignored while captured: a fall-through would leak the drag
        // coordinates into the terminal/PTY below the panel.
        EventResult::Consumed
    }

    /// Release: commit the reorder once if the entry actually moved.
    pub(crate) fn handle_release(&mut self) -> EventResult<TermWmAction> {
        let source = self.drag_source.take();
        let drop_index = self.drop_index.take();
        let moved = std::mem::take(&mut self.drag_moved);
        self.drag_cursor_col = None;
        self.drag_grab_col = 0;
        if moved && let Some(key) = source {
            let source_index = self.display_order.iter().position(|k| *k == key);
            let target_index = drop_index.unwrap_or(source_index.unwrap_or(0));
            if source_index != Some(target_index) {
                return EventResult::Action(TermWmAction::ReorderWindow {
                    key,
                    index: target_index,
                });
            }
        }
        EventResult::Consumed
    }

    pub(crate) fn handle_scroll(&mut self, kind: MouseEventKind) -> EventResult<TermWmAction> {
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
