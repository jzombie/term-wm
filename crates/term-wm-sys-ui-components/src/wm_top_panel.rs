use std::collections::BTreeMap;

use ratatui::style::{Modifier, Style};
use term_wm_core::events::{Event, KeyModifiers, MouseButton, MouseEventKind};
use term_wm_layout_engine::LayoutRect;

use term_wm_core::{
    actions::{EventResult, TermWmAction},
    components::{
        Component, ComponentAction, ComponentContext, ComponentQuery, ComponentResponse,
        WmComponent,
    },
    hitbox_registry::HitboxId,
    layout::rect_contains,
    utils::truncate_with_ellipsis,
    window::WindowKey,
};
use term_wm_ui_components::helpers::{
    color_to_ratatui, layout_rect_to_clipped_rect, menu_icon, safe_set_string,
    slice_by_columns,
};

/// One space on each side of a window entry label (`" {label} "`).
const ENTRY_PADDING: usize = 2;
/// Single-column gap between the menu button and the window entry strip.
const MENU_GAP: u16 = 1;
/// Horizontal scroll step (columns) for wheel / indicator / edge-pan scrolling.
const SCROLL_STEP: u16 = 8;
/// Per-entry label cap (columns) before ellipsis truncation, so extremely long
/// titles don't blow up the measured content width.
const MAX_ENTRY_LABEL: usize = 40;
/// Left overflow indicator glyph.
const LEFT_INDICATOR: &str = "◀";
/// Right overflow indicator glyph.
const RIGHT_INDICATOR: &str = "▶";
/// Drop-indicator glyph drawn at the insertion gap during drag-to-reorder.
const DROP_INDICATOR: &str = "▌";

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
pub struct WmTopPanelComponent {
    visible: bool,
    height: u16,
    area: LayoutRect,
    menu_rect: Option<LayoutRect>,
    list: WindowList,
    app_name: String,
    // WmComponent render state (pushed via process_action before render)
    active: bool,
    focus_current: Option<WindowKey>,
    display_order: Vec<WindowKey>,
    status_line: Option<String>,
    tiling_indicator: Option<(&'static str, term_wm_core::actions::TermWmAction)>,
    tiling_rect: Option<LayoutRect>,
    menu_open: bool,
    window_labels: BTreeMap<WindowKey, String>,
    hitbox_id: HitboxId,

    // ── Horizontal scroll + drag-to-reorder state ─────────────────────────
    /// Horizontal scroll offset into the window entry strip.
    h_scroll: u16,
    /// Entry being dragged, if an active drag is in progress.
    drag_source: Option<WindowKey>,
    /// Last known drag cursor column (physical/global).
    drag_cursor_col: Option<i32>,
    /// Insertion index for the drop indicator / reorder target.
    drop_index: Option<usize>,
    /// Whether the cursor moved (any `Drag` event) since the press.
    drag_moved: bool,
    // Geometry from the most recent render_inner, reused by the drag handler.
    entries_start_x: i32,
    scroll_viewport_width: i32,
    max_scroll: u16,
    entry_geometry: Vec<(WindowKey, i32, u16)>,
    left_indicator_rect: Option<LayoutRect>,
    right_indicator_rect: Option<LayoutRect>,
}

impl WmTopPanelComponent {
    pub fn new(app_name: &str) -> Self {
        Self {
            visible: true,
            height: 1,
            area: LayoutRect::default(),
            menu_rect: None,
            list: WindowList::new(),
            app_name: app_name.to_string(),
            active: false,
            focus_current: None,
            display_order: Vec::new(),
            status_line: None,
            tiling_indicator: None,
            tiling_rect: None,
            menu_open: false,
            window_labels: BTreeMap::new(),
            hitbox_id: HitboxId::new(),
            h_scroll: 0,
            drag_source: None,
            drag_cursor_col: None,
            drop_index: None,
            drag_moved: false,
            entries_start_x: 0,
            scroll_viewport_width: 0,
            max_scroll: 0,
            entry_geometry: Vec::new(),
            left_indicator_rect: None,
            right_indicator_rect: None,
        }
    }

    pub fn begin_frame(&mut self) {
        self.list.begin_frame();
        self.menu_rect = None;
        self.tiling_rect = None;
        self.left_indicator_rect = None;
        self.right_indicator_rect = None;
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
        let ratatui_area = layout_rect_to_clipped_rect(area);
        let bounds = ratatui_area.intersection(buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        // Per-frame geometry — recomputed below; never carry stale values into
        // a frame where a given indicator is no longer present.
        self.left_indicator_rect = None;
        self.right_indicator_rect = None;
        for yy in bounds.y..bounds.y.saturating_add(bounds.height) {
            for xx in bounds.x..bounds.x.saturating_add(bounds.width) {
                if let Some(cell) = buffer.cell_mut((xx, yy)) {
                    let mut st = cell.style();
                    st.bg = Some(color_to_ratatui(theme.bottom_panel_bg));
                    st.fg = Some(color_to_ratatui(theme.bottom_panel_fg));
                    cell.set_style(st);
                    cell.set_symbol(" ");
                }
            }
        }
        let y = area.y;
        let max_x = area.x.saturating_add(i32::from(area.width));
        let menu_label = menu_icon(&self.app_name);
        let menu_width = menu_label.chars().count() as u16;

        // ── Phase 1: measure content + decide scroll geometry (NO drawing) ──
        let mut entry_geometry: Vec<(WindowKey, i32, u16)> = Vec::new();
        let mut focused_range: Option<(i32, u16)> = None;
        let mut logical_x: i32 = 0;
        for key in display_order.iter().copied() {
            let label = self
                .window_labels
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
        let max_entries_width =
            i32::from(area.width.saturating_sub(menu_width.saturating_add(MENU_GAP)));
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
        let entries_start_x = area.x
            + i32::from(menu_width)
            + i32::from(MENU_GAP)
            + i32::from(left_indicator_width);
        let max_scroll = content_width.saturating_sub(scroll_viewport_width).max(0) as u16;

        self.h_scroll = self.h_scroll.min(max_scroll);
        // Auto-scroll the focused entry into view, UNLESS a drag is active
        // (otherwise the viewport would snap back and block manual scrolling
        // during a drag).
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

        // ── Paint: menu button + gap ──
        let mut x = area.x;
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
                menu_label.as_str(),
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
            x = x.saturating_add(i32::from(MENU_GAP));
        }

        if let Some(status) = status_line {
            let available = (max_x.saturating_sub(x)).max(1);
            let text = truncate_with_ellipsis(status, available as usize);
            safe_set_string(buffer, bounds, x as u16, y as u16, &text, Style::default());
        } else {
            // ── Phase 2: paint entries with immutable h_scroll (absolute coords) ──
            let h_scroll = i32::from(self.h_scroll);
            let drop_bar_style = Style::default()
                .fg(color_to_ratatui(theme.accent))
                .add_modifier(Modifier::BOLD);
            let indicator_style = Style::default()
                .fg(color_to_ratatui(theme.panel_inactive_fg))
                .add_modifier(Modifier::BOLD);

            for (i, (key, lx, width)) in entry_geometry.iter().enumerate() {
                let (key, lx, width) = (*key, *lx, i32::from(*width));
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
                let label = self
                    .window_labels
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
                safe_set_string(buffer, bounds, draw_x as u16, y as u16, &label_slice, item_style);
                self.list.window_hits.push(PanelWindowHit {
                    id: key,
                    rect: LayoutRect {
                        x: draw_x,
                        y,
                        width: visible_width as u16,
                        height: 1,
                    },
                });

                // Drop indicator at the gap just before this entry.
                if self.drop_index == Some(i) {
                    let gap_vp = lx.saturating_sub(h_scroll);
                    if gap_vp >= 0 && gap_vp < scroll_viewport_width {
                        safe_set_string(
                            buffer,
                            bounds,
                            (entries_start_x.saturating_add(gap_vp)) as u16,
                            y as u16,
                            DROP_INDICATOR,
                            drop_bar_style,
                        );
                    }
                }
            }
            // Drop indicator at the end of the list.
            if self.drop_index == Some(entry_geometry.len()) {
                let gap_vp = content_width.saturating_sub(h_scroll);
                if gap_vp >= 0 && gap_vp < scroll_viewport_width {
                    safe_set_string(
                        buffer,
                        bounds,
                        (entries_start_x.saturating_add(gap_vp)) as u16,
                        y as u16,
                        DROP_INDICATOR,
                        drop_bar_style,
                    );
                }
            }

            // Overflow indicators in their statically reserved columns.
            if left_indicator_width == 1 && self.h_scroll > 0 {
                let ix = entries_start_x.saturating_sub(1);
                safe_set_string(buffer, bounds, ix as u16, y as u16, LEFT_INDICATOR, indicator_style);
                self.left_indicator_rect = Some(LayoutRect {
                    x: ix,
                    y,
                    width: 1,
                    height: 1,
                });
            }
            if right_indicator_width == 1 && self.h_scroll < max_scroll {
                let ix = entries_start_x.saturating_add(scroll_viewport_width);
                safe_set_string(buffer, bounds, ix as u16, y as u16, RIGHT_INDICATOR, indicator_style);
                self.right_indicator_rect = Some(LayoutRect {
                    x: ix,
                    y,
                    width: 1,
                    height: 1,
                });
            }
        }
    }

    /// Render the tiling indicator label (right-aligned) and store its rect.
    fn render_tiling_indicator(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        theme: &term_wm_core::theme::Theme,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let Some((label, _)) = &self.tiling_indicator else {
            return;
        };
        let ratatui_backend = term_wm_ui_components::helpers::downcast_ratatui(backend);
        let buffer = &mut ratatui_backend.buffer;
        let ratatui_area = term_wm_ui_components::helpers::layout_rect_to_clipped_rect(area);
        let bounds = ratatui_area.intersection(buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        let y = area.y;
        let max_x = area.x.saturating_add(i32::from(area.width));
        let tw = label.chars().count() as u16;
        let ix = max_x.saturating_sub(i32::from(tw));
        if ix < area.x {
            return;
        }
        let style = ratatui::style::Style::default()
            .fg(color_to_ratatui(theme.success))
            .add_modifier(ratatui::style::Modifier::BOLD);
        term_wm_ui_components::helpers::safe_set_string(
            buffer, bounds, ix as u16, y as u16, label, style,
        );
        self.tiling_rect = Some(term_wm_layout_engine::LayoutRect {
            x: ix,
            y,
            width: tw,
            height: 1,
        });
    }

    pub fn hit_test_window(&self, column: u16, row: u16) -> Option<WindowKey> {
        self.list
            .window_hits
            .iter()
            .find(|hit| rect_contains(hit.rect, column, row))
            .map(|hit| hit.id)
    }

    fn clear_drag_state(&mut self) {
        self.drag_source = None;
        self.drag_cursor_col = None;
        self.drop_index = None;
        self.drag_moved = false;
    }

    /// Map a physical cursor column into the entries' virtual (logical)
    /// coordinate space and return the insertion index for a drag drop.
    ///
    /// The virtual column is projected from the entries' actual start column
    /// (menu + gap + left indicator) plus the current scroll offset, so the
    /// result is independent of visual clipping / indicator columns.
    fn compute_drop_index(&self, virtual_col: i32) -> usize {
        let mut idx = self.entry_geometry.len();
        for (i, (_, lx, width)) in self.entry_geometry.iter().enumerate() {
            if virtual_col < lx + i32::from(*width) {
                idx = i;
                break;
            }
        }
        idx
    }

    fn on_mouse_drag(&mut self, column: u16) -> EventResult<TermWmAction> {
        self.drag_moved = true;
        self.drag_cursor_col = Some(column as i32);
        if self.drag_source.is_some() {
            // Edge-panning: indicators can't be clicked while the button is
            // held, so nudge the scroll when the cursor reaches a viewport edge.
            if i32::from(column) <= self.entries_start_x {
                self.h_scroll = self.h_scroll.saturating_sub(SCROLL_STEP);
            } else if i32::from(column) >= self.entries_start_x + self.scroll_viewport_width {
                self.h_scroll = self.h_scroll.saturating_add(SCROLL_STEP).min(self.max_scroll);
            }
            let virtual_col =
                i32::from(column) - self.entries_start_x + i32::from(self.h_scroll);
            self.drop_index = Some(self.compute_drop_index(virtual_col));
        }
        // Never Ignored while captured: a fall-through would leak the drag
        // coordinates into the terminal/PTY below the panel.
        EventResult::Consumed
    }

    fn on_mouse_release(&mut self) -> EventResult<TermWmAction> {
        let source = self.drag_source.take();
        let drop_index = self.drop_index.take();
        let moved = std::mem::take(&mut self.drag_moved);
        self.drag_cursor_col = None;
        if moved
            && let Some(key) = source
        {
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
}

impl Component<TermWmAction> for WmTopPanelComponent {
    fn hitbox_id(&self) -> Option<HitboxId> {
        Some(self.hitbox_id)
    }

    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        let theme = ctx.config().theme;
        if !self.active {
            // Still render the tiling indicator even when inactive so the
            // label is visible and tiling_rect is populated for clicks.
            self.clear_drag_state();
            self.render_tiling_indicator(backend, area, &theme);
            return;
        }
        let app_name = ctx.app_name().to_string();
        if app_name != self.app_name {
            self.app_name = app_name;
        }
        self.area = area;
        if let Some(focus) = self.focus_current {
            let display_order = self.display_order.clone();
            let status_line = self.status_line.clone();

            self.render_inner(
                backend,
                self.active,
                focus,
                &display_order,
                status_line.as_deref(),
                self.menu_open,
                &theme,
            );
        }
        self.render_tiling_indicator(backend, area, &theme);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        let Event::Mouse(mouse) = event else {
            return EventResult::Ignored;
        };
        match mouse.kind {
            MouseEventKind::Press(_) => self.on_mouse_press(
                mouse.column,
                mouse.row,
                MouseButton::Left,
                mouse.modifiers,
                ctx,
            ),
            // Drag/Release/Scroll are delivered under mouse capture (or to the
            // panel's layer) and must never fall through to the terminal below.
            MouseEventKind::Drag(_) => self.on_mouse_drag(mouse.column),
            MouseEventKind::Release(_) => self.on_mouse_release(),
            MouseEventKind::ScrollLeft => {
                self.h_scroll = self.h_scroll.saturating_sub(SCROLL_STEP);
                EventResult::Consumed
            }
            MouseEventKind::ScrollRight => {
                self.h_scroll = self.h_scroll.saturating_add(SCROLL_STEP).min(self.max_scroll);
                EventResult::Consumed
            }
            _ => EventResult::Consumed,
        }
    }

    fn on_mouse_press(
        &mut self,
        column: u16,
        row: u16,
        _button: MouseButton,
        _modifiers: KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        if self.menu_icon_contains_point(column, row) {
            return EventResult::Action(TermWmAction::OpenCommandPalette);
        }
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
            // Clicking an entry focuses it and arms a potential drag; state is
            // only committed on a real drag (see on_mouse_release).
            self.drag_source = Some(key);
            self.drag_moved = false;
            self.drag_cursor_col = Some(column as i32);
            self.drop_index = None;
            return EventResult::Action(TermWmAction::FocusWindow(key));
        }
        if let Some((_, action)) = &self.tiling_indicator
            && let Some(rect) = self.tiling_rect
            && rect_contains(rect, column, row)
        {
            return EventResult::Action(action.clone());
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
                self.menu_open = state.menu_open;
                self.tiling_indicator = state.tiling_indicator.clone();
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
    use ratatui::buffer::Buffer;
    use term_wm_core::components::{
        ComponentAction, ComponentQuery, ComponentResponse, WmComponent,
    };
    use term_wm_core::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use term_wm_core::theme::NOIR;
    use term_wm_core::wm_config::HintVisibility;

    fn make_backend(w: u16, h: u16) -> term_wm_console::RatatuiBackend {
        let buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, w, h));
        term_wm_console::RatatuiBackend::new_simple(buf, ratatui::layout::Rect::new(0, 0, w, h))
    }

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

        assert!(p.hit_test_window(0, 0).is_none());
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

    #[test]
    fn default_is_same_as_new() {
        let p = WmTopPanelComponent::default();
        assert!(p.visible());
        assert_eq!(p.height(), 1);
    }

    #[test]
    fn hitbox_id_returns_some() {
        let p = WmTopPanelComponent::new("test");
        assert!(p.hitbox_id().is_some());
    }

    #[test]
    fn set_height_enforces_minimum() {
        let mut p = WmTopPanelComponent::new("test");
        p.set_height(0);
        assert!(p.height() >= 1);
        p.set_height(5);
        assert_eq!(p.height(), 5);
    }

    #[test]
    fn area_returns_stored_area() {
        let mut p = WmTopPanelComponent::new("test");
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let _ = p.split_area(true, area);
        assert_eq!(p.area(), area);
    }

    #[test]
    fn menu_icon_contains_point_returns_false_when_no_rect() {
        let p = WmTopPanelComponent::new("test");
        assert!(!p.menu_icon_contains_point(0, 0));
    }

    #[test]
    fn menu_icon_rect_none_initially() {
        let p = WmTopPanelComponent::new("test");
        assert!(p.menu_icon_rect().is_none());
    }

    #[test]
    fn hit_test_window_after_render_with_display_order() {
        let mut p = WmTopPanelComponent::new("test");
        p.active = true;
        let key = WindowKey::default();
        p.focus_current = Some(key);
        p.display_order = vec![key];
        p.window_labels.insert(key, "W".to_string());

        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let _ = p.split_area(true, area);
        let mut backend = make_backend(80, 24);
        p.render_inner(&mut backend, true, key, &[key], None, false, &NOIR);
        assert!(!p.list.window_hits.is_empty());
        let hit_rect = p.list.window_hits[0].rect;
        let hit_key = p.hit_test_window(hit_rect.x as u16 + 1, hit_rect.y as u16);
        assert!(hit_key.is_some());
    }

    #[test]
    fn hit_test_area_returns_true_inside() {
        let mut p = WmTopPanelComponent::new("test");
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let _ = p.split_area(true, area);
        assert!(p.hit_test(5, 0));
    }

    #[test]
    fn hit_test_area_returns_false_outside() {
        let mut p = WmTopPanelComponent::new("test");
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let _ = p.split_area(true, area);
        assert!(!p.hit_test(5, 5));
    }

    #[test]
    fn render_when_not_active_does_nothing() {
        let mut p = WmTopPanelComponent::new("test");
        p.active = false;
        let mut backend = make_backend(80, 24);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let ctx = ComponentContext::new(true);
        let mut reg = term_wm_core::hitbox_registry::HitboxRegistry::new();
        p.render(&mut backend, area, &ctx, &mut reg);
        // No panic, no-op
    }

    #[test]
    fn render_with_status_line() {
        let mut p = WmTopPanelComponent::new("test");
        p.active = true;
        let key = WindowKey::default();
        p.focus_current = Some(key);
        p.display_order = vec![key];
        p.status_line = Some("Status: OK".to_string());

        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let _ = p.split_area(true, area);
        let theme = NOIR;
        let mut backend = make_backend(80, 24);
        p.render_inner(
            &mut backend,
            true,
            key,
            &[],
            Some("Status: OK"),
            false,
            &theme,
        );
        // Should render without panic
    }

    #[test]
    fn render_menu_open_style() {
        let mut p = WmTopPanelComponent::new("test");
        p.active = true;
        let key = WindowKey::default();
        p.focus_current = Some(key);
        p.display_order = vec![key];
        p.menu_open = true;

        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let _ = p.split_area(true, area);
        let theme = NOIR;
        let mut backend = make_backend(80, 24);
        p.render_inner(&mut backend, true, key, &[key], None, true, &theme);
        // Menu rect should be set after render
        assert!(p.menu_icon_rect().is_some());
    }

    #[test]
    fn render_narrow_buffer_truncates_labels() {
        let mut p = WmTopPanelComponent::new("test");
        p.active = true;
        let key = WindowKey::default();
        p.focus_current = Some(key);
        p.display_order = vec![key];
        p.window_labels.insert(
            key,
            "A very long window label that exceeds buffer width".to_string(),
        );

        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 20,
            height: 1,
        };
        let _ = p.split_area(true, area);
        let theme = NOIR;
        let mut backend = make_backend(20, 1);
        p.render_inner(&mut backend, true, key, &[key], None, false, &theme);
    }

    #[test]
    fn process_action_toggle_visibility() {
        let mut p = WmTopPanelComponent::new("test");
        assert!(p.visible());
        p.process_action(&ComponentAction::ToggleVisibility);
        assert!(!p.visible());
        p.process_action(&ComponentAction::ToggleVisibility);
        assert!(p.visible());
    }

    #[test]
    fn process_action_set_hint_visibility_always() {
        let mut p = WmTopPanelComponent::new("test");
        p.set_visible(false);
        p.process_action(&ComponentAction::SetHintVisibility(HintVisibility::Always));
        assert!(p.visible());
    }

    #[test]
    fn process_action_set_hint_visibility_never() {
        let mut p = WmTopPanelComponent::new("test");
        p.process_action(&ComponentAction::SetHintVisibility(HintVisibility::Never));
        assert!(!p.visible());
    }

    #[test]
    fn process_action_set_hint_visibility_on_demand() {
        let mut p = WmTopPanelComponent::new("test");
        p.set_visible(false);
        p.process_action(&ComponentAction::SetHintVisibility(
            HintVisibility::OnDemand,
        ));
        assert!(!p.visible());
    }

    #[test]
    fn process_action_set_panel_active() {
        let mut p = WmTopPanelComponent::new("test");
        p.process_action(&ComponentAction::SetPanelActive(true));
        assert!(p.active);
        p.process_action(&ComponentAction::SetPanelActive(false));
        assert!(!p.active);
    }

    #[test]
    fn process_action_set_window_labels() {
        use std::collections::BTreeMap;
        let mut p = WmTopPanelComponent::new("test");
        let mut labels = BTreeMap::new();
        let key = WindowKey::default();
        labels.insert(key, "My Window".to_string());
        p.process_action(&ComponentAction::SetWindowLabels(labels));
        assert_eq!(
            p.window_labels.get(&key).map(|s| s.as_str()),
            Some("My Window")
        );
    }

    #[test]
    fn process_action_set_top_panel_state() {
        use term_wm_core::components::TopPanelState;
        let mut p = WmTopPanelComponent::new("test");
        let key = WindowKey::default();
        let state = TopPanelState {
            focus_current: Some(key),
            display_order: vec![key],
            status_line: Some("ready".to_string()),
            menu_open: true,
            tiling_indicator: None,
        };
        p.process_action(&ComponentAction::SetTopPanelState(Box::new(state)));
        assert_eq!(p.focus_current, Some(key));
        assert_eq!(p.display_order, vec![key]);
        assert_eq!(p.status_line.as_deref(), Some("ready"));
        assert!(p.menu_open);
    }

    #[test]
    fn query_non_menu_returns_none() {
        let p = WmTopPanelComponent::new("test");
        assert!(matches!(
            p.query(&ComponentQuery::SelectedAction),
            ComponentResponse::None
        ));
        assert!(matches!(
            p.query(&ComponentQuery::KeybindingHints),
            ComponentResponse::None
        ));
    }

    #[test]
    fn query_menu_icon_rect_returns_none_initially() {
        let p = WmTopPanelComponent::new("test");
        let resp = p.query(&ComponentQuery::MenuIconRect);
        assert!(matches!(resp, ComponentResponse::Rect(None)));
    }

    #[test]
    fn begin_frame_clears_state() {
        let mut p = WmTopPanelComponent::new("test");
        p.menu_rect = Some(LayoutRect {
            x: 0,
            y: 0,
            width: 5,
            height: 1,
        });
        p.list.window_hits.push(PanelWindowHit {
            id: WindowKey::default(),
            rect: LayoutRect {
                x: 0,
                y: 0,
                width: 5,
                height: 1,
            },
        });
        p.begin_frame();
        assert!(p.menu_rect.is_none());
        assert!(p.list.window_hits.is_empty());
    }

    #[test]
    fn wmbegin_frame_trait_delegates() {
        let mut p = WmTopPanelComponent::new("test");
        p.menu_rect = Some(LayoutRect {
            x: 0,
            y: 0,
            width: 5,
            height: 1,
        });
        WmComponent::begin_frame(&mut p);
        assert!(p.menu_rect.is_none());
    }

    #[test]
    fn wmvisible_trait_delegates() {
        let p = WmTopPanelComponent::new("test");
        assert!(WmComponent::visible(&p));
    }

    #[test]
    fn wmset_visible_trait_delegates() {
        let mut p = WmTopPanelComponent::new("test");
        WmComponent::set_visible(&mut p, false);
        assert!(!WmComponent::visible(&p));
    }

    #[test]
    fn handle_events_non_mouse_returns_ignored() {
        let mut p = WmTopPanelComponent::new("test");
        let ctx = ComponentContext::new(true);
        let event = term_wm_core::events::Event::Key(term_wm_core::events::KeyEvent {
            code: term_wm_core::events::KeyCode::Char('a'),
            modifiers: term_wm_core::events::KeyModifiers::NONE,
            kind: term_wm_core::events::KeyKind::Press,
        });
        let result = p.handle_events(&event, &ctx);
        assert!(result.is_ignored());
    }

    #[test]
    fn handle_events_mouse_events_never_ignored_after_capture() {
        let mut p = WmTopPanelComponent::new("test");
        let ctx = ComponentContext::new(true);
        // Captured mouse events must be Consumed, never Ignored, so drag/scroll
        // coordinates never leak into the terminal/PTY below the panel.
        for kind in [
            MouseEventKind::Moved,
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Release(MouseButton::Left),
            MouseEventKind::ScrollLeft,
            MouseEventKind::ScrollRight,
        ] {
            let event = Event::Mouse(MouseEvent {
                kind,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            });
            let result = p.handle_events(&event, &ctx);
            assert!(
                matches!(result, EventResult::Consumed),
                "mouse kind {kind:?} must be Consumed, not Ignored"
            );
        }
    }

    #[test]
    fn on_mouse_press_no_hit_returns_ignored() {
        let mut p = WmTopPanelComponent::new("test");
        let ctx = ComponentContext::new(true);
        let result = p.on_mouse_press(0, 0, MouseButton::Left, KeyModifiers::NONE, &ctx);
        assert!(result.is_ignored());
    }

    #[test]
    fn render_with_zero_area_does_nothing() {
        let mut p = WmTopPanelComponent::new("test");
        p.active = true;
        p.area = LayoutRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        let theme = NOIR;
        let mut backend = make_backend(80, 24);
        let key = WindowKey::default();
        p.render_inner(&mut backend, true, key, &[], None, false, &theme);
    }

    #[test]
    fn render_with_empty_display_order_and_no_status() {
        let mut p = WmTopPanelComponent::new("test");
        p.active = true;
        let key = WindowKey::default();
        p.focus_current = Some(key);
        p.display_order = vec![];

        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let _ = p.split_area(true, area);
        let theme = NOIR;
        let mut backend = make_backend(80, 24);
        p.render_inner(&mut backend, true, key, &[], None, false, &theme);
    }

    #[test]
    fn consume_area_delegates_to_split_area() {
        let mut p = WmTopPanelComponent::new("test");
        p.active = true;
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let (panel, managed) = p.consume_area(area);
        assert_eq!(panel.height, 1);
        assert_eq!(managed.height, 23);
    }

    fn push_windows(
        p: &mut WmTopPanelComponent,
        keys: &[WindowKey],
        area: LayoutRect,
    ) {
        let labels: std::collections::BTreeMap<_, _> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| (*k, format!("Window {}", i)))
            .collect();
        p.area = area;
        p.active = true;
        p.process_action(&ComponentAction::SetWindowLabels(labels));
        p.focus_current = keys.first().copied();
        p.display_order = keys.to_vec();
    }

    fn make_keys(n: usize) -> Vec<WindowKey> {
        use std::collections::HashMap;
        use std::sync::Arc;
        use term_wm_core::app_context::AppContext;
        use term_wm_core::components::NoopComponent;
        use term_wm_core::window::{LayerManager, WindowManager};
        use term_wm_core::wm_config::WmConfig;

        let mut wm = WindowManager::<NoopComponent>::with_config(
            WmConfig::default(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            LayerManager::new(),
            HashMap::new(),
        );
        (0..n)
            .map(|_| wm.create_window(NoopComponent))
            .collect()
    }

    fn render_panel(p: &mut WmTopPanelComponent) {
        let area = p.area;
        let mut backend = make_backend(area.width, area.height);
        let order = p.display_order.clone();
        p.render_inner(
            &mut backend,
            true,
            p.focus_current.unwrap(),
            &order,
            None,
            false,
            &NOIR,
        );
    }

    #[test]
    fn overflow_clamps_scroll_and_shows_indicators() {
        let keys = make_keys(8);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 30,
            height: 1,
        };
        let mut p = WmTopPanelComponent::new("test-app");
        push_windows(&mut p, &keys, area);

        // Content overflows the 30-col viewport (assert the premise), and an
        // extreme scroll clamps to max_scroll — no u16 underflow/panic.
        p.focus_current = keys.last().copied();
        p.h_scroll = u16::MAX;
        render_panel(&mut p);
        assert!(p.max_scroll > 0, "content must overflow the viewport");
        assert_eq!(
            p.h_scroll,
            p.max_scroll,
            "h_scroll must clamp to max_scroll without underflow/panic"
        );
        assert!(
            p.left_indicator_rect.is_some(),
            "left ◀ indicator expected when scrolled right"
        );
        assert!(
            p.right_indicator_rect.is_none(),
            "right ▶ indicator absent at the far-right scroll"
        );

        // Auto-scroll pulls the (now leftmost-focused) window back into view.
        p.focus_current = keys.first().copied();
        p.h_scroll = p.max_scroll;
        render_panel(&mut p);
        assert_eq!(p.h_scroll, 0, "auto-scroll brings the focused window into view");
        assert!(
            p.left_indicator_rect.is_none(),
            "left ◀ absent at scroll origin"
        );
        assert!(
            p.right_indicator_rect.is_some(),
            "right ▶ expected when content remains off-screen"
        );
    }

    #[test]
    fn scroll_events_adjust_h_scroll() {
        let keys = make_keys(8);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 30,
            height: 1,
        };
        let mut p = WmTopPanelComponent::new("test-app");
        push_windows(&mut p, &keys, area);
        render_panel(&mut p);
        let before = p.h_scroll;

        let ctx = ComponentContext::new(false);
        let scroll_right = Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollRight,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let res = p.handle_events(&scroll_right, &ctx);
        assert!(matches!(res, EventResult::Consumed), "ScrollRight consumed");
        assert!(p.h_scroll > before, "ScrollRight must increase h_scroll");

        let scroll_left = Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollLeft,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let res = p.handle_events(&scroll_left, &ctx);
        assert!(matches!(res, EventResult::Consumed), "ScrollLeft consumed");
        assert!(p.h_scroll <= before + SCROLL_STEP, "ScrollLeft decreases h_scroll");
    }

    #[test]
    fn drag_to_reorder_dispatches_reorder_window() {
        let keys = make_keys(3);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let mut p = WmTopPanelComponent::new("test-app");
        push_windows(&mut p, &keys, area);
        render_panel(&mut p);

        // Press the LAST entry (keys[2]).
        let (_key, lx, _width) = p.entry_geometry[2];
        let press_col = (p.entries_start_x + lx) as u16;
        let ctx = ComponentContext::new(false);
        let press = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: press_col,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let res = p.handle_events(&press, &ctx);
        assert!(
            matches!(res, EventResult::Action(TermWmAction::FocusWindow(k)) if k == keys[2]),
            "pressing an entry focuses it"
        );
        assert_eq!(p.drag_source, Some(keys[2]));

        // Drag to the far left edge of the entries viewport -> index 0.
        let drag_col = p.entries_start_x as u16;
        let drag = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: drag_col,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let res = p.handle_events(&drag, &ctx);
        assert!(matches!(res, EventResult::Consumed), "drag consumed");
        assert_eq!(p.drop_index, Some(0));

        // Release commits the reorder.
        let release = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Release(MouseButton::Left),
            column: drag_col,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let res = p.handle_events(&release, &ctx);
        assert!(
            matches!(res, EventResult::Action(TermWmAction::ReorderWindow { key, index }) if key == keys[2] && index == 0),
            "release must dispatch ReorderWindow with the computed index"
        );
        assert!(p.drag_source.is_none(), "drag state cleared on release");
    }

    #[test]
    fn plain_click_does_not_emit_reorder() {
        let keys = make_keys(3);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let mut p = WmTopPanelComponent::new("test-app");
        push_windows(&mut p, &keys, area);
        render_panel(&mut p);

        // Press + release without a Drag event = a plain click.
        let press_col = (p.entries_start_x + p.entry_geometry[1].1) as u16;
        let ctx = ComponentContext::new(false);
        let press = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: press_col,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        p.handle_events(&press, &ctx);
        let release = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Release(MouseButton::Left),
            column: press_col,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let res = p.handle_events(&release, &ctx);
        assert!(
            matches!(res, EventResult::Consumed),
            "a plain click must not emit a ReorderWindow action"
        );
    }

    #[test]
    fn drop_index_clamps_off_viewport() {
        let keys = make_keys(3);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 30,
            height: 1,
        };
        let mut p = WmTopPanelComponent::new("test-app");
        push_windows(&mut p, &keys, area);
        render_panel(&mut p);
        p.drag_source = Some(keys[0]);

        // Drag far beyond the right edge -> clamp to the end (index == len).
        let far_right = (p.entries_start_x + p.scroll_viewport_width + 50) as u16;
        let ctx = ComponentContext::new(false);
        let drag = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: far_right,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        p.handle_events(&drag, &ctx);
        assert_eq!(
            p.drop_index,
            Some(keys.len()),
            "dragging off the right edge clamps drop_index to the list length"
        );
    }
}
