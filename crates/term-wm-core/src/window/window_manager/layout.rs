use std::sync::Arc;

use crossterm::event::{Event, MouseEvent};
use ratatui::prelude::Rect;
use ratatui::widgets::Clear;

use super::{SystemWindowId, WindowId, WindowManager};
use crate::keybindings::ActionLayer;
use crate::layout::floating::*;
use crate::layout::{LayoutNode, LayoutPlan, RegionMap, SplitHandle, TilingLayout};
use crate::window::FloatRectSpec;

impl<Id: Copy + Eq + Ord + std::fmt::Debug + 'static> WindowManager<Id> {
    pub fn scroll(&self, id: Id) -> super::ScrollState {
        self.scroll.get(&id).copied().unwrap_or_default()
    }

    pub fn scroll_mut(&mut self, id: Id) -> &mut super::ScrollState {
        self.scroll.entry(id).or_default()
    }

    pub fn scroll_offset(&self, id: Id) -> usize {
        self.scroll(id).offset
    }

    pub fn reset_scroll(&mut self, id: Id) {
        self.scroll_mut(id).reset();
    }

    pub fn apply_scroll(&mut self, id: Id, total: usize, view: usize) {
        self.scroll_mut(id).apply(total, view);
    }

    pub fn set_region(&mut self, id: Id, rect: Rect) {
        self.regions.set(WindowId::app(id), rect);
    }

    pub fn full_region(&self, id: Id) -> Rect {
        self.full_region_for_id(WindowId::app(id))
    }

    pub fn region(&self, id: Id) -> Rect {
        self.region_for_id(WindowId::app(id))
    }

    pub(super) fn window_content_offset(&self, id: WindowId<Id>) -> (u16, u16) {
        let full = self.full_region_for_id(id);
        let content = self.region_for_id(id);
        (
            content.x.saturating_sub(full.x),
            content.y.saturating_sub(full.y),
        )
    }

    pub(super) fn adjust_event_for_window(&self, id: WindowId<Id>, event: &Event) -> Event {
        if let Event::Mouse(mut mouse) = event.clone() {
            let (offset_x, offset_y) = self.window_content_offset(id);
            mouse.column = mouse.column.saturating_add(offset_x);
            mouse.row = mouse.row.saturating_add(offset_y);
            Event::Mouse(mouse)
        } else {
            event.clone()
        }
    }

    pub fn localize_event_to_app(&self, id: Id, event: &Event) -> Option<Event> {
        self.localize_event_content(WindowId::app(id), event)
    }

    pub fn localize_event(&self, id: WindowId<Id>, event: &Event) -> Option<Event> {
        match event {
            Event::Mouse(mouse) => {
                let dest = self.window_dest(id, self.full_region_for_id(id));
                let column =
                    (i32::from(mouse.column) - dest.x).clamp(0, i32::from(u16::MAX)) as u16;
                let row = (i32::from(mouse.row) - dest.y).clamp(0, i32::from(u16::MAX)) as u16;
                Some(Event::Mouse(MouseEvent {
                    column,
                    row,
                    kind: mouse.kind,
                    modifiers: mouse.modifiers,
                }))
            }
            _ => None,
        }
    }

    pub(super) fn localize_event_content(&self, id: WindowId<Id>, event: &Event) -> Option<Event> {
        match event {
            Event::Mouse(mouse) => {
                let dest = self.window_dest(id, self.full_region_for_id(id));
                let (offset_x, offset_y) = self.window_content_offset(id);
                let content_x = dest.x + i32::from(offset_x);
                let content_y = dest.y + i32::from(offset_y);
                let column =
                    (i32::from(mouse.column) - content_x).clamp(0, i32::from(u16::MAX)) as u16;
                let row = (i32::from(mouse.row) - content_y).clamp(0, i32::from(u16::MAX)) as u16;
                Some(Event::Mouse(MouseEvent {
                    column,
                    row,
                    kind: mouse.kind,
                    modifiers: mouse.modifiers,
                }))
            }
            _ => None,
        }
    }

    pub fn full_region_for_id(&self, id: WindowId<Id>) -> Rect {
        self.regions.get(id).unwrap_or_default()
    }

    pub(super) fn region_for_id(&self, id: WindowId<Id>) -> Rect {
        let rect = self.regions.get(id).unwrap_or_default();
        if self.config.chrome_enabled {
            let area = if self.floating_resize_offscreen {
                rect
            } else {
                super::clamp_rect(rect, self.managed_area)
            };
            if area.width < 3 || area.height < 4 {
                return Rect::default();
            }
            Rect {
                x: area.x + 1,
                y: area.y + 2,
                width: area.width.saturating_sub(2),
                height: area.height.saturating_sub(3),
            }
        } else {
            rect
        }
    }

    pub fn set_regions_from_layout(&mut self, layout: &LayoutNode<Id>, area: Rect) {
        self.regions = RegionMap::default();
        for (id, rect) in layout.layout(area) {
            self.regions.set(WindowId::app(id), rect);
        }
    }

    pub fn register_tiling_layout(&mut self, layout: &TilingLayout<Id>, area: Rect) {
        let (regions, handles) = layout.root().layout_with_handles(area);
        for (id, rect) in regions {
            self.regions.set(WindowId::app(id), rect);
        }
        self.handles.extend(handles);
    }

    pub fn set_managed_layout(&mut self, layout: TilingLayout<Id>) {
        self.managed_layout = Some(TilingLayout::new(super::map_layout_node(layout.root())));
        self.clear_all_floating();
        if self.system_window_visible(SystemWindowId::DebugLog) {
            self.ensure_system_window_in_layout(WindowId::system(SystemWindowId::DebugLog));
        }
    }

    pub fn set_managed_layout_none(&mut self) {
        if self.managed_layout.is_none() {
            return;
        }
        self.managed_layout = None;
        if self.system_window_visible(SystemWindowId::DebugLog) {
            self.ensure_system_window_in_layout(WindowId::system(SystemWindowId::DebugLog));
        }
    }

    pub fn set_panel_visible(&mut self, visible: bool) {
        self.top_panel.set_visible(visible);
    }

    pub fn set_panel_height(&mut self, height: u16) {
        self.top_panel.set_height(height);
    }

    pub fn register_managed_layout(&mut self, area: Rect) {
        self.last_frame_area = area;
        // Compute hints before layout so split_area can reserve space for them.
        // Show Global-layer hints (WmToggleOverlay only) when overlay closed,
        // WmMode hints when overlay is open.
        // In embedded mode (wm_overlay_enabled = false) there is no overlay
        // distinction, so show all hints unconditionally.
        let active_layer = if self.config.wm_overlay_enabled && self.wm_overlay_visible() {
            ActionLayer::WmMode
        } else {
            ActionLayer::Global
        };
        match self.hint_visibility {
            crate::wm_config::HintVisibility::Always => {
                if self.config.wm_overlay_enabled {
                    let hints = self
                        .keybindings
                        .bottom_hints_for_layer(crate::constants::MAX_BOTTOM_HINTS, active_layer);
                    self.bottom_panel.set_keybinding_hints(hints);
                } else {
                    // Embedded mode: no overlay, all actions are always dispatchable.
                    let hints = self
                        .keybindings
                        .bottom_hints(crate::constants::MAX_BOTTOM_HINTS);
                    self.bottom_panel.set_keybinding_hints(hints);
                }
            }
            _ => {
                self.bottom_panel.set_keybinding_hints(Vec::new());
            }
        }
        let active = self.panel_active();
        let has_hints = !self.bottom_panel.keybinding_hints().is_empty();
        let bottom_h = if has_hints || active {
            1u16
        } else {
            0
        };
        let (_, after_top) = self.top_panel.split_area(active, area);
        let (_, managed_area) = self.bottom_panel.split_bottom_area(after_top, bottom_h);
        let prev_managed = self.managed_area;
        self.managed_area = managed_area;
        if prev_managed.width > 0 && prev_managed.height > 0 {
            let prev_full = FloatRectSpec::Absolute(crate::window::FloatRect {
                x: prev_managed.x as i32,
                y: prev_managed.y as i32,
                width: prev_managed.width,
                height: prev_managed.height,
            });
            let new_full = FloatRectSpec::Absolute(crate::window::FloatRect {
                x: self.managed_area.x as i32,
                y: self.managed_area.y as i32,
                width: self.managed_area.width,
                height: self.managed_area.height,
            });
            for (_id, window) in self.windows.iter_mut() {
                if window.floating_rect == Some(prev_full) {
                    window.floating_rect = Some(new_full);
                }
            }
        }
        self.clamp_floating_to_bounds();
        if self.system_window_visible(SystemWindowId::DebugLog) {
            self.ensure_system_window_in_layout(WindowId::system(SystemWindowId::DebugLog));
        }
        let z_snapshot = self.z_order.clone();
        let mut active_ids: Vec<WindowId<Id>> = Vec::new();

        if let Some(layout) = self.managed_layout.as_ref() {
            let (regions, handles) = layout.root().layout_with_handles(self.managed_area);
            for (id, rect) in &regions {
                if self.is_window_floating(*id) {
                    continue;
                }
                if self.is_minimized(*id) {
                    continue;
                }
                self.regions.set(*id, *rect);
                if let Some(header) = floating_header_for_region(*id, *rect, self.managed_area) {
                    self.floating_headers.push(header);
                }
                active_ids.push(*id);
            }
            let filtered_handles: Vec<SplitHandle> = handles
                .into_iter()
                .filter(|handle| {
                    let Some(LayoutNode::Split { children, .. }) =
                        layout.root().node_at_path(&handle.path)
                    else {
                        return false;
                    };
                    let left = children.get(handle.index);
                    let right = children.get(handle.index + 1);
                    left.is_some_and(|node| node.subtree_any(|id| !self.is_window_floating(id)))
                        || right
                            .is_some_and(|node| node.subtree_any(|id| !self.is_window_floating(id)))
                })
                .collect();
            self.handles.extend(filtered_handles);
        }
        let mut floating_ids: Vec<WindowId<Id>> = self
            .windows
            .iter()
            .filter_map(|(&id, window)| {
                if window.is_floating() && !window.minimized {
                    Some(id)
                } else {
                    None
                }
            })
            .collect();
        floating_ids.sort_by_key(|id| {
            z_snapshot
                .iter()
                .position(|existing| existing == id)
                .unwrap_or(usize::MAX)
        });
        for floating_id in floating_ids {
            let Some(spec) = self.floating_rect(floating_id) else {
                continue;
            };
            let rect = spec.resolve(self.managed_area);
            self.regions.set(floating_id, rect);
            let visible = self.visible_rect_from_spec(spec);
            if visible.width > 0 && visible.height > 0 {
                self.resize_handles.extend(resize_handles_for_region(
                    floating_id,
                    visible,
                    self.managed_area,
                ));
                if let Some(header) =
                    floating_header_for_region(floating_id, visible, self.managed_area)
                {
                    self.floating_headers.push(header);
                }
            }
            active_ids.push(floating_id);
        }

        self.z_order.retain(|id| active_ids.contains(id));
        for &id in &active_ids {
            if !self.z_order.contains(&id) {
                self.z_order.push(id);
            }
        }
        self.managed_draw_order = self.z_order.clone();
        self.managed_draw_order_app = self
            .managed_draw_order
            .iter()
            .filter_map(|id| id.as_app())
            .collect();
        self.rebuild_wm_focus_ring(&active_ids);
        let focused = self.wm_focus.current();
        if self.z_order.last().copied() != Some(focused) {
            self.focus_window_id(focused);
        }
    }

    pub fn managed_draw_order(&self) -> &[Id] {
        &self.managed_draw_order_app
    }

    /// Returns the full draw order including both app and system windows.
    pub fn managed_draw_order_all(&self) -> &[WindowId<Id>] {
        &self.managed_draw_order
    }

    pub fn build_display_order(&self) -> Vec<WindowId<Id>> {
        let mut ordered: Vec<(WindowId<Id>, &super::Window)> = self
            .windows
            .iter()
            .map(|(id, window)| (*id, window))
            .collect();
        ordered.sort_by_key(|(_, window)| window.creation_order);

        let mut out: Vec<WindowId<Id>> = Vec::new();
        for (id, window) in ordered {
            if self.managed_draw_order.contains(&id) || window.minimized {
                out.push(id);
            }
        }
        for id in &self.managed_draw_order {
            if !out.contains(id) {
                out.push(*id);
            }
        }
        out
    }

    pub fn set_window_title(&mut self, id: Id, title: impl Into<String>) {
        let title = title.into();
        let prev = self
            .window(WindowId::app(id))
            .and_then(|w| w.title.as_deref().map(|t| t.to_string()));
        if prev.as_deref() != Some(&title) {
            let seq = self.next_title_seq;
            self.next_title_seq += 1;
            let window = self.window_mut(WindowId::app(id));
            window.title = Some(title);
            window.title_set_order = Some(seq);
        }
    }

    pub fn render_split_handles(&self, frame: &mut crate::ui::UiFrame<'_>) {
        let hovered = self.hover.and_then(|(col, row)| {
            self.handles
                .iter()
                .find(|h| crate::layout::rect_contains(h.rect, col, row))
                .cloned()
        });
        crate::layout::render_handles(frame, &self.handles, hovered.as_ref());
    }

    pub fn set_regions_from_plan(&mut self, plan: &LayoutPlan<Id>, area: Rect) {
        let plan_regions = plan.regions(area);
        self.regions = RegionMap::default();
        for id in plan_regions.ids() {
            if let Some(rect) = plan_regions.get(id) {
                self.regions.set(WindowId::app(id), rect);
            }
        }
    }

    pub fn hit_test_region(&self, column: u16, row: u16, ids: &[Id]) -> Option<Id> {
        for id in ids {
            let rect = self.visible_region_for_id(WindowId::app(*id));
            if rect.width > 0 && rect.height > 0 && crate::layout::rect_contains(rect, column, row)
            {
                return Some(*id);
            }
        }
        None
    }

    pub(super) fn hit_test_region_topmost(
        &self,
        column: u16,
        row: u16,
        ids: &[WindowId<Id>],
    ) -> Option<WindowId<Id>> {
        for id in ids.iter().rev() {
            let rect = self.visible_region_for_id(*id);
            if rect.width > 0 && rect.height > 0 && crate::layout::rect_contains(rect, column, row)
            {
                return Some(*id);
            }
        }
        None
    }

    pub fn clear_window_backgrounds(&self, frame: &mut crate::ui::UiFrame<'_>) {
        for id in self.regions.ids() {
            let rect = self.full_region_for_id(id);
            frame.render_widget(Clear, rect);
        }
    }

    pub fn window_draw_plan(
        &mut self,
        _frame: &mut crate::ui::UiFrame<'_>,
    ) -> Vec<super::DrawTask<Id>> {
        let mut plan = Vec::new();
        let focused_window = self.wm_focus.current();
        let decorator = Arc::clone(&self.decorator);
        for &id in &self.managed_draw_order {
            let full = self.full_region_for_id(id);
            if full.width == 0 || full.height == 0 {
                continue;
            }
            let visible_full = self.visible_region_for_id(id);
            if visible_full.width == 0 || visible_full.height == 0 {
                continue;
            }
            let dest = self.window_dest(id, full);
            let inner = decorator.content_area(Rect {
                x: 0,
                y: 0,
                width: full.width,
                height: full.height,
            });
            if inner.width == 0 || inner.height == 0 {
                continue;
            }
            match id {
                WindowId::System(system_id) => {
                    if !self.system_window_visible(system_id) {
                        continue;
                    }
                    plan.push(super::DrawTask::System(super::SystemWindowDraw {
                        id: system_id,
                        surface: super::WindowSurface { full, inner, dest },
                        focused: focused_window == id,
                    }));
                }
                WindowId::App(app_id) => {
                    plan.push(super::DrawTask::App(super::WindowDrawContext {
                        id: app_id,
                        surface: super::WindowSurface { full, inner, dest },
                        focused: focused_window == WindowId::app(app_id),
                    }));
                }
            }
        }
        plan
    }

    pub fn render_system_window(
        &mut self,
        frame: &mut crate::ui::UiFrame<'_>,
        window: super::SystemWindowDraw,
    ) {
        if window.surface.inner.width == 0 || window.surface.inner.height == 0 {
            return;
        }
        self.render_system_window_entry(frame, window);
    }

    pub(super) fn hover_targets(
        &self,
    ) -> (Option<&SplitHandle>, Option<&ResizeHandle<WindowId<Id>>>) {
        let Some((column, row)) = self.hover else {
            return (None, None);
        };
        let topmost = self.hit_test_region_topmost(column, row, &self.managed_draw_order);
        let hovered = if topmost.is_none() {
            self.handles
                .iter()
                .find(|handle| crate::layout::rect_contains(handle.rect, column, row))
        } else {
            None
        };
        let hovered_resize = self.resize_handles.iter().find(|handle| {
            crate::layout::rect_contains(handle.rect, column, row) && topmost == Some(handle.id)
        });
        (hovered, hovered_resize)
    }

    pub(super) fn window_dest(&self, id: WindowId<Id>, fallback: Rect) -> crate::window::FloatRect {
        if let Some(spec) = self.floating_rect(id) {
            spec.resolve_signed(self.managed_area)
        } else {
            crate::window::FloatRect {
                x: fallback.x as i32,
                y: fallback.y as i32,
                width: fallback.width,
                height: fallback.height,
            }
        }
    }

    pub(super) fn visible_rect_from_spec(&self, spec: FloatRectSpec) -> Rect {
        super::float_rect_visible(spec.resolve_signed(self.managed_area), self.managed_area)
    }

    pub(super) fn visible_region_for_id(&self, id: WindowId<Id>) -> Rect {
        if let Some(spec) = self.floating_rect(id) {
            self.visible_rect_from_spec(spec)
        } else {
            self.full_region_for_id(id)
        }
    }

    pub(super) fn clamp_floating_to_bounds(&mut self) {
        use crate::constants::MIN_FLOATING_VISIBLE_MARGIN;
        use crate::layout::floating::FLOATING_MIN_HEIGHT;
        use crate::layout::floating::FLOATING_MIN_WIDTH;

        let bounds = self.managed_area;
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        let mut updates: Vec<(WindowId<Id>, FloatRectSpec)> = Vec::new();
        let floating_ids: Vec<WindowId<Id>> = self
            .windows
            .iter()
            .filter_map(|(&id, window)| window.floating_rect.as_ref().map(|_| id))
            .collect();
        for id in floating_ids {
            let Some(FloatRectSpec::Absolute(fr)) = self.floating_rect(id) else {
                continue;
            };

            let rect_left = fr.x;
            let rect_top = fr.y;
            let rect_right = fr.x.saturating_add(fr.width as i32);
            let rect_bottom = fr.y.saturating_add(fr.height as i32);
            let bounds_left = bounds.x as i32;
            let bounds_top = bounds.y as i32;
            let bounds_right = bounds_left.saturating_add(bounds.width as i32);
            let bounds_bottom = bounds_top.saturating_add(bounds.height as i32);

            let min_w = FLOATING_MIN_WIDTH.min(bounds.width.max(1));
            let min_h = FLOATING_MIN_HEIGHT.min(bounds.height.max(1));

            let min_visible_margin = MIN_FLOATING_VISIBLE_MARGIN;

            let width = if self.floating_resize_offscreen {
                fr.width.max(min_w)
            } else {
                fr.width.max(min_w).min(bounds.width)
            };
            let height = if self.floating_resize_offscreen {
                fr.height.max(min_h)
            } else {
                fr.height.max(min_h).min(bounds.height)
            };

            let max_x = if self.floating_resize_offscreen {
                (bounds
                    .x
                    .saturating_add(bounds.width)
                    .saturating_sub(min_visible_margin.min(width))) as i32
            } else {
                bounds.x.saturating_add(bounds.width.saturating_sub(width)) as i32
            };

            let max_y = if self.floating_resize_offscreen {
                (bounds
                    .y
                    .saturating_add(bounds.height)
                    .saturating_sub(min_visible_margin.min(height))) as i32
            } else {
                bounds
                    .y
                    .saturating_add(bounds.height.saturating_sub(height)) as i32
            };

            let out_x = rect_right <= bounds_left || rect_left >= bounds_right;
            let out_y = rect_bottom <= bounds_top || rect_top >= bounds_bottom;

            let x = if out_x || !self.floating_resize_offscreen {
                fr.x.clamp(bounds_left, max_x)
            } else {
                let left_allowed =
                    bounds_left.saturating_sub(width as i32 - min_visible_margin.min(width) as i32);
                let left_allowed = left_allowed.min(max_x);
                fr.x.clamp(left_allowed, max_x)
            };

            let y = if out_y || !self.floating_resize_offscreen {
                fr.y.clamp(bounds_top, max_y)
            } else {
                let visible_height = min_visible_margin.min(height) as i32;
                let top_allowed = bounds_top.saturating_sub(height as i32 - visible_height);
                let top_allowed = top_allowed.min(max_y);
                fr.y.clamp(top_allowed, max_y)
            };

            updates.push((
                id,
                FloatRectSpec::Absolute(crate::window::FloatRect {
                    x,
                    y,
                    width,
                    height,
                }),
            ));
        }
        for (id, spec) in updates {
            self.set_floating_rect(id, Some(spec));
        }
    }

    pub fn bring_to_front(&mut self, id: Id) {
        self.bring_to_front_id(WindowId::app(id));
    }

    pub(super) fn bring_to_front_id(&mut self, id: WindowId<Id>) {
        if let Some(pos) = self.z_order.iter().position(|&x| x == id) {
            let item = self.z_order.remove(pos);
            self.z_order.push(item);
        }
    }

    pub fn bring_all_floating_to_front(&mut self) {
        let ids: Vec<WindowId<Id>> = self
            .z_order
            .iter()
            .copied()
            .filter(|id| self.is_window_floating(*id))
            .collect();
        for id in ids {
            self.bring_to_front_id(id);
        }
    }

    pub(super) fn bring_floating_to_front_id(&mut self, id: WindowId<Id>) {
        self.bring_to_front_id(id);
    }

    pub(super) fn bring_floating_to_front(&mut self, id: Id) {
        self.bring_floating_to_front_id(WindowId::app(id));
    }
}
