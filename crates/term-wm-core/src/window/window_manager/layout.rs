use crossterm::event::{Event, MouseEvent};
use ratatui::prelude::Rect;
use ratatui::widgets::Clear;

use super::WindowManager;
use crate::keybindings::ActionLayer;
use crate::layout::floating::*;
use crate::layout::{LayoutNode, LayoutPlan, RegionMap, SplitHandle, TilingLayout};
use crate::window::{FloatRectSpec, WindowKey};

impl WindowManager {
    pub fn scroll(&self, key: WindowKey) -> super::ScrollState {
        self.scroll.get(&key).copied().unwrap_or_default()
    }

    pub fn scroll_mut(&mut self, key: WindowKey) -> &mut super::ScrollState {
        self.scroll.entry(key).or_default()
    }

    pub fn scroll_offset(&self, key: WindowKey) -> usize {
        self.scroll(key).offset
    }

    pub fn reset_scroll(&mut self, key: WindowKey) {
        self.scroll_mut(key).reset();
    }

    pub fn apply_scroll(&mut self, key: WindowKey, total: usize, view: usize) {
        self.scroll_mut(key).apply(total, view);
    }

    pub fn set_region(&mut self, key: WindowKey, rect: Rect) {
        self.regions.set(key, rect);
    }

    pub fn full_region(&self, key: WindowKey) -> Rect {
        self.full_region_for_key(key)
    }

    pub fn region(&self, key: WindowKey) -> Rect {
        self.region_for_key(key)
    }

    pub(super) fn window_content_offset(&self, key: WindowKey) -> (u16, u16) {
        let full = self.full_region_for_key(key);
        let content = self.region_for_key(key);
        (
            content.x.saturating_sub(full.x),
            content.y.saturating_sub(full.y),
        )
    }

    pub(super) fn adjust_event_for_window(&self, key: WindowKey, event: &Event) -> Event {
        if let Event::Mouse(mut mouse) = event.clone() {
            let (offset_x, offset_y) = self.window_content_offset(key);
            mouse.column = mouse.column.saturating_add(offset_x);
            mouse.row = mouse.row.saturating_add(offset_y);
            Event::Mouse(mouse)
        } else {
            event.clone()
        }
    }

    pub fn localize_event_to_app(&self, key: WindowKey, event: &Event) -> Option<Event> {
        self.localize_event_content(key, event)
    }

    pub fn localize_event(&self, key: WindowKey, event: &Event) -> Option<Event> {
        match event {
            Event::Mouse(mouse) => {
                let dest = self.window_dest(key, self.full_region_for_key(key));
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

    pub(super) fn localize_event_content(&self, key: WindowKey, event: &Event) -> Option<Event> {
        match event {
            Event::Mouse(mouse) => {
                let dest = self.window_dest(key, self.full_region_for_key(key));
                let (offset_x, offset_y) = self.window_content_offset(key);
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

    pub fn full_region_for_key(&self, key: WindowKey) -> Rect {
        self.regions.get(key).unwrap_or_default()
    }

    pub(super) fn region_for_key(&self, key: WindowKey) -> Rect {
        let rect = self.regions.get(key).unwrap_or_default();
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

    pub fn set_regions_from_layout(&mut self, layout: &LayoutNode<WindowKey>, area: Rect) {
        self.regions = RegionMap::default();
        for (key, rect) in layout.layout(area) {
            self.regions.set(key, rect);
        }
    }

    pub fn register_tiling_layout(&mut self, layout: &TilingLayout<WindowKey>, area: Rect) {
        let (regions, handles) = layout.root().layout_with_handles(area);
        for (key, rect) in regions {
            self.regions.set(key, rect);
        }
        self.handles.extend(handles);
    }

    pub fn set_managed_layout(&mut self, layout: TilingLayout<WindowKey>) {
        self.managed_layout = Some(TilingLayout::new(super::map_layout_node(layout.root())));
        self.clear_all_floating();
    }

    pub fn set_managed_layout_none(&mut self) {
        if self.managed_layout.is_none() {
            return;
        }
        self.managed_layout = None;
    }

    pub fn set_panel_visible(&mut self, visible: bool) {
        if let Some(p) = &mut self.top_panel {
            p.set_visible(visible);
        }
    }

    pub fn set_panel_height(&mut self, height: u16) {
        if let Some(p) = &mut self.top_panel {
            p.set_height(height);
        }
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
                        .keybindings()
                        .bottom_hints_for_layer(crate::constants::MAX_BOTTOM_HINTS, active_layer);
                    if let Some(p) = &mut self.bottom_panel {
                        p.set_keybinding_hints(hints);
                    }
                } else {
                    // Embedded mode: no overlay, all actions are always dispatchable.
                    let hints = self
                        .keybindings()
                        .bottom_hints(crate::constants::MAX_BOTTOM_HINTS);
                    if let Some(p) = &mut self.bottom_panel {
                        p.set_keybinding_hints(hints);
                    }
                }
            }
            _ => {
                if let Some(p) = &mut self.bottom_panel {
                    p.set_keybinding_hints(Vec::new());
                }
            }
        }
        let active = self.panel_active();
        let has_hints = self
            .bottom_panel
            .as_ref()
            .is_some_and(|p| !p.keybinding_hints().is_empty());
        let bottom_h = if has_hints || active { 1u16 } else { 0 };
        let after_top = if let Some(p) = &mut self.top_panel {
            let (_, after) = p.split_area(active, area);
            after
        } else {
            area
        };
        let managed_area = if let Some(p) = &mut self.bottom_panel {
            let (_, managed) = p.split_bottom_area(after_top, bottom_h);
            managed
        } else {
            after_top
        };
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
        let z_snapshot = self.z_order.clone();
        let mut active_keys: Vec<WindowKey> = Vec::new();

        if let Some(layout) = self.managed_layout.as_ref() {
            let (regions, handles) = layout.root().layout_with_handles(self.managed_area);
            for (key, rect) in &regions {
                if self.is_window_floating(*key) {
                    continue;
                }
                if self.is_minimized(*key) {
                    continue;
                }
                self.regions.set(*key, *rect);
                if let Some(header) = floating_header_for_region(*key, *rect, self.managed_area) {
                    self.floating_headers.push(header);
                }
                active_keys.push(*key);
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
                    left.is_some_and(|node| node.subtree_any(|key| !self.is_window_floating(key)))
                        || right.is_some_and(|node| {
                            node.subtree_any(|key| !self.is_window_floating(key))
                        })
                })
                .collect();
            self.handles.extend(filtered_handles);
        }
        let mut floating_keys: Vec<WindowKey> = self
            .windows
            .iter()
            .filter_map(|(key, window)| {
                if window.is_floating() && !window.minimized {
                    Some(key)
                } else {
                    None
                }
            })
            .collect();
        floating_keys.sort_by_key(|key| {
            z_snapshot
                .iter()
                .position(|existing| existing == key)
                .unwrap_or(usize::MAX)
        });
        for floating_key in floating_keys {
            let Some(spec) = self.floating_rect(floating_key) else {
                continue;
            };
            let rect = spec.resolve(self.managed_area);
            self.regions.set(floating_key, rect);
            let visible = self.visible_rect_from_spec(spec);
            if visible.width > 0 && visible.height > 0 {
                self.resize_handles.extend(resize_handles_for_region(
                    floating_key,
                    visible,
                    self.managed_area,
                ));
                if let Some(header) =
                    floating_header_for_region(floating_key, visible, self.managed_area)
                {
                    self.floating_headers.push(header);
                }
            }
            active_keys.push(floating_key);
        }

        self.z_order.retain(|key| active_keys.contains(key));
        for &key in &active_keys {
            if !self.z_order.contains(&key) {
                self.z_order.push(key);
            }
        }
        self.managed_draw_order = self.z_order.clone();
        self.rebuild_focus_ring(&active_keys);
        let focused = *self.focus.current();
        if self.z_order.last().copied() != Some(focused) {
            self.focus_window_key(focused);
        }
    }

    /// Returns the full draw order including both app and system windows.
    pub fn managed_draw_order_all(&self) -> &[WindowKey] {
        &self.managed_draw_order
    }

    pub fn build_display_order(&self) -> Vec<WindowKey> {
        let mut ordered: Vec<(WindowKey, &super::Window)> = self.windows.iter().collect();
        ordered.sort_by_key(|(_, window)| window.creation_order);

        let mut out: Vec<WindowKey> = Vec::new();
        for (key, window) in ordered {
            if self.managed_draw_order.contains(&key) || window.minimized {
                out.push(key);
            }
        }
        for key in &self.managed_draw_order {
            if !out.contains(key) {
                out.push(*key);
            }
        }
        out
    }

    pub fn set_window_title(&mut self, key: WindowKey, title: impl Into<String>) {
        let title = title.into();
        let prev = self
            .window(key)
            .and_then(|w| w.title.as_deref().map(|t| t.to_string()));
        if prev.as_deref() != Some(&title) {
            let seq = self.next_title_seq;
            self.next_title_seq += 1;
            let window = self.window_mut(key);
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
        crate::layout::render_handles(frame, &self.handles, hovered.as_ref(), &self.config.theme);
    }

    pub fn set_regions_from_plan(&mut self, plan: &LayoutPlan<WindowKey>, area: Rect) {
        let plan_regions = plan.regions(area);
        self.regions = RegionMap::default();
        for key in plan_regions.ids() {
            if let Some(rect) = plan_regions.get(key) {
                self.regions.set(key, rect);
            }
        }
    }

    pub fn hit_test_region(&self, column: u16, row: u16, ids: &[WindowKey]) -> Option<WindowKey> {
        for key in ids {
            let rect = self.visible_region_for_key(*key);
            if rect.width > 0 && rect.height > 0 && crate::layout::rect_contains(rect, column, row)
            {
                return Some(*key);
            }
        }
        None
    }

    pub(super) fn hit_test_region_topmost(
        &self,
        column: u16,
        row: u16,
        ids: &[WindowKey],
    ) -> Option<WindowKey> {
        for key in ids.iter().rev() {
            let rect = self.visible_region_for_key(*key);
            if rect.width > 0 && rect.height > 0 && crate::layout::rect_contains(rect, column, row)
            {
                return Some(*key);
            }
        }
        None
    }

    pub fn clear_window_backgrounds(&self, frame: &mut crate::ui::UiFrame<'_>) {
        for key in self.regions.ids() {
            let rect = self.full_region_for_key(key);
            frame.render_widget(Clear, rect);
        }
    }

    pub fn window_draw_plan(
        &mut self,
        _frame: &mut crate::ui::UiFrame<'_>,
    ) -> Vec<super::DrawTask> {
        let mut plan = Vec::new();
        let focused_window = self.focus.current();
        let decorator = self.decorator();
        let _total = self.managed_draw_order.len() as f32;
        let num_app = self.managed_draw_order.len();
        for (i, &key) in self.managed_draw_order.iter().enumerate() {
            let full = self.full_region_for_key(key);
            if full.width == 0 || full.height == 0 {
                continue;
            }
            let dest = self.window_dest(key, full);
            let inner = decorator.content_area(Rect {
                x: 0,
                y: 0,
                width: full.width,
                height: full.height,
            });
            if inner.width == 0 || inner.height == 0 {
                continue;
            }
            let z = super::WindowManager::compute_z_depth(i, num_app);
            plan.push(super::DrawTask::App(super::WindowDrawContext {
                key,
                surface: super::WindowSurface {
                    full,
                    inner,
                    dest,
                    draw_shadow: self.is_window_floating(key) && self.config.shadow_enabled,
                    z_depth: z,
                },
                focused: *focused_window == key,
            }));
        }

        plan
    }

    pub(super) fn hover_targets(&self) -> (Option<&SplitHandle>, Option<&ResizeHandle<WindowKey>>) {
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
            crate::layout::rect_contains(handle.rect, column, row) && topmost == Some(handle.key)
        });
        (hovered, hovered_resize)
    }

    pub(super) fn window_dest(&self, key: WindowKey, fallback: Rect) -> crate::window::FloatRect {
        if let Some(spec) = self.floating_rect(key) {
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

    pub(super) fn visible_region_for_key(&self, key: WindowKey) -> Rect {
        if let Some(spec) = self.floating_rect(key) {
            self.visible_rect_from_spec(spec)
        } else {
            self.full_region_for_key(key)
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
        let mut updates: Vec<(WindowKey, FloatRectSpec)> = Vec::new();
        let floating_keys: Vec<WindowKey> = self
            .windows
            .iter()
            .filter_map(|(key, window)| window.floating_rect.as_ref().map(|_| key))
            .collect();
        for key in floating_keys {
            let Some(FloatRectSpec::Absolute(fr)) = self.floating_rect(key) else {
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
                key,
                FloatRectSpec::Absolute(crate::window::FloatRect {
                    x,
                    y,
                    width,
                    height,
                }),
            ));
        }
        for (key, spec) in updates {
            self.set_floating_rect(key, Some(spec));
        }
    }

    pub fn bring_to_front(&mut self, key: WindowKey) {
        self.bring_to_front_key(key);
    }

    pub(super) fn bring_to_front_key(&mut self, key: WindowKey) {
        if let Some(pos) = self.z_order.iter().position(|&x| x == key) {
            let item = self.z_order.remove(pos);
            self.z_order.push(item);
        }
    }

    pub fn bring_all_floating_to_front(&mut self) {
        let keys: Vec<WindowKey> = self
            .z_order
            .iter()
            .copied()
            .filter(|key| self.is_window_floating(*key))
            .collect();
        for key in keys {
            self.bring_to_front_key(key);
        }
    }

    pub(super) fn bring_floating_to_front_key(&mut self, key: WindowKey) {
        self.bring_to_front_key(key);
    }

    #[expect(dead_code)]
    pub(super) fn bring_floating_to_front(&mut self, key: WindowKey) {
        self.bring_floating_to_front_key(key);
    }
}
