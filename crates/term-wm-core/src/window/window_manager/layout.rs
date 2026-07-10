use crate::Rect;
use crate::events::{Event, MouseEvent};

use super::WindowManager;
use crate::keybindings::ActionLayer;
use crate::layout::floating::*;
use crate::layout::{LayoutNode, LayoutPlan, RegionMap, SplitHandle, TilingLayout};
use crate::window::{FloatRectSpec, WindowKey, WindowState};

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
            content.x.saturating_sub(full.x) as u16,
            content.y.saturating_sub(full.y) as u16,
        )
    }

    pub fn adjust_event_for_window(&self, key: WindowKey, event: &Event) -> Event {
        if let Event::Mouse(mut mouse) = event.clone() {
            let (offset_x, offset_y) = self.window_content_offset(key);
            mouse.column = mouse.column.saturating_add(offset_x);
            mouse.row = mouse.row.saturating_add(offset_y);
            Event::Mouse(mouse)
        } else {
            event.clone()
        }
    }

    #[cfg(test)]
    pub fn localize_event_to_app(&self, key: WindowKey, event: &Event) -> Option<Event> {
        self.localize_event_content(key, event)
    }

    #[cfg(test)]
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
        if let Some(p) = &mut self.top_component {
            p.set_visible(visible);
        }
    }

    pub fn set_panel_height(&mut self, _height: u16) {
        // Height is determined by the component's consume_area; no-op here
    }

    pub fn register_managed_layout(&mut self, area: Rect) {
        self.last_frame_area = area;
        let active_layer = if self.config.wm_command_menu_enabled && self.command_menu_visible() {
            ActionLayer::WmMode
        } else {
            ActionLayer::Global
        };
        match self.hint_visibility {
            crate::wm_config::HintVisibility::Always => {
                if self.config.wm_command_menu_enabled {
                    let hints = self
                        .keybindings()
                        .bottom_hints_for_layer(crate::constants::MAX_BOTTOM_HINTS, active_layer);
                    if let Some(p) = &mut self.bottom_component {
                        p.process_action(&crate::components::ComponentAction::SetKeybindingHints(
                            hints,
                        ));
                    }
                } else {
                    let hints = self
                        .keybindings()
                        .bottom_hints(crate::constants::MAX_BOTTOM_HINTS);
                    if let Some(p) = &mut self.bottom_component {
                        p.process_action(&crate::components::ComponentAction::SetKeybindingHints(
                            hints,
                        ));
                    }
                }
            }
            _ => {
                if let Some(p) = &mut self.bottom_component {
                    p.process_action(&crate::components::ComponentAction::SetKeybindingHints(
                        Vec::new(),
                    ));
                }
            }
        }
        // Compute whether the panel should be active from config + visibility,
        // BEFORE calling consume_area (which needs this state to claim space).
        let panel_active =
            self.config.panel_enabled && self.top_component.as_ref().is_some_and(|p| p.visible());
        // Push active state to the component so consume_area claims the right space
        if let Some(p) = &mut self.top_component {
            p.process_action(&crate::components::ComponentAction::SetPanelActive(
                panel_active,
            ));
        }
        let has_hints = if let Some(p) = self.bottom_component.as_ref() {
            if let crate::components::ComponentResponse::Hints(h) =
                p.query(&crate::components::ComponentQuery::KeybindingHints)
            {
                !h.is_empty()
            } else {
                false
            }
        } else {
            false
        };
        let bottom_h = if has_hints || panel_active { 1u16 } else { 0 };
        let (top_rect, after_top) = if let Some(p) = &mut self.top_component {
            let (claimed, rest) = p.consume_area(area);
            (claimed, rest)
        } else {
            (Rect::default(), area)
        };
        self.top_claimed = top_rect;
        let (bottom_rect, managed_area) = if let Some(p) = &mut self.bottom_component {
            let bottom = Rect {
                x: after_top.x,
                y: after_top
                    .y
                    .saturating_add(i32::from(after_top.height))
                    .saturating_sub(i32::from(bottom_h)),
                width: after_top.width,
                height: bottom_h,
            };
            let managed = Rect {
                x: after_top.x,
                y: after_top.y,
                width: after_top.width,
                height: after_top.height.saturating_sub(bottom_h),
            };
            p.consume_area(bottom);
            (bottom, managed)
        } else {
            (Rect::default(), after_top)
        };
        self.bottom_claimed = bottom_rect;
        let prev_managed = self.managed_area;
        self.managed_area = managed_area;
        if prev_managed.width > 0 && prev_managed.height > 0 {
            let prev_full = FloatRectSpec::Absolute(crate::window::FloatRect {
                x: prev_managed.x,
                y: prev_managed.y,
                width: prev_managed.width,
                height: prev_managed.height,
            });
            let new_full = FloatRectSpec::Absolute(crate::window::FloatRect {
                x: self.managed_area.x,
                y: self.managed_area.y,
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
                // Skip stale keys no longer in the SlotMap (e.g. after
                // finalize_window_removal).  These would otherwise be
                // re-added to z_order / managed_draw_order and render
                // with a "DefaultKey(NvM)" fallback title.
                if self.window(*key).is_none() {
                    continue;
                }
                if self.is_window_floating(*key) {
                    continue;
                }
                if self.window_state(*key) == Some(WindowState::Iconic) {
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
                        && right.is_some_and(|node| {
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
                if window.is_floating() && window.state != WindowState::Iconic {
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
                let is_maximized = self
                    .windows
                    .get(floating_key)
                    .is_some_and(|w| w.is_maximized);
                if !is_maximized {
                    self.resize_handles.extend(resize_handles_for_region(
                        floating_key,
                        visible,
                        self.managed_area,
                    ));
                }
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
        self.bifurcate_draw_order();
        self.rebuild_focus_ring(&active_keys);
        let focused = *self.focus.current();
        if self.z_order.last().copied() != Some(focused) {
            self.focus_window_key(focused);
        }
        // Mark layout as dirty so CoreEngine::project_draw_plan regenerates
        // the draw plan on the next frame — without this, tiling resizes and
        // other layout mutations use stale region bounds.
        self.mark_layout_dirty();
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
            if self.managed_draw_order.contains(&key) || window.state == WindowState::Iconic {
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
        if prev.as_deref() != Some(&title)
            && let Some(window) = self.windows.get_mut(key)
        {
            let seq = self.next_title_seq;
            self.next_title_seq += 1;
            window.title = Some(title);
            window.title_set_order = Some(seq);
        }
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

    pub fn window_draw_plan(
        &mut self,
        _frame: &mut dyn term_wm_render::RenderBackend,
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

    #[allow(dead_code)]
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

    pub fn window_dest(&self, key: WindowKey, fallback: Rect) -> crate::window::FloatRect {
        if let Some(spec) = self.floating_rect(key) {
            spec.resolve_signed(self.managed_area)
        } else {
            crate::window::FloatRect {
                x: fallback.x,
                y: fallback.y,
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
            let bounds_left = bounds.x;
            let bounds_top = bounds.y;
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
                bounds
                    .x
                    .saturating_add(i32::from(bounds.width))
                    .saturating_sub(i32::from(min_visible_margin.min(width)))
            } else {
                bounds
                    .x
                    .saturating_add(i32::from(bounds.width.saturating_sub(width)))
            };

            let max_y = if self.floating_resize_offscreen {
                bounds
                    .y
                    .saturating_add(i32::from(bounds.height))
                    .saturating_sub(i32::from(min_visible_margin.min(height)))
            } else {
                bounds
                    .y
                    .saturating_add(i32::from(bounds.height.saturating_sub(height)))
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

    /// Rebuild `managed_draw_order` from `z_order` with strict layer separation:
    /// all tiled windows first (back), then all floating windows (front).
    pub(super) fn bifurcate_draw_order(&mut self) {
        let mut bifurcated = Vec::with_capacity(self.z_order.len());
        for &key in &self.z_order {
            if !self.is_window_floating(key) {
                bifurcated.push(key);
            }
        }
        for &key in &self.z_order {
            if self.is_window_floating(key) {
                bifurcated.push(key);
            }
        }
        self.managed_draw_order = bifurcated;
    }

    pub fn bring_to_front(&mut self, key: WindowKey) {
        self.bring_to_front_key(key);
    }

    pub(super) fn bring_to_front_key(&mut self, key: WindowKey) {
        if !self.is_window_floating(key) {
            return;
        }
        if let Some(pos) = self.z_order.iter().position(|&x| x == key) {
            let item = self.z_order.remove(pos);
            self.z_order.push(item);
        }
        self.bifurcate_draw_order();
    }

    pub(super) fn bring_floating_to_front_key(&mut self, key: WindowKey) {
        self.bring_to_front_key(key);
    }

    #[expect(dead_code)]
    pub(super) fn bring_floating_to_front(&mut self, key: WindowKey) {
        self.bring_floating_to_front_key(key);
    }

    /// Remove a window from the tiling layout.
    /// Idempotent — safe to call even if already detached.
    pub(super) fn detach_from_tiling_layout(&mut self, key: WindowKey) {
        if let Some(ref mut layout) = self.managed_layout {
            let _ = layout.root_mut().remove_leaf(key);
            layout.root_mut().cleanup_after_removal();
            // If the tree was a single leaf matching key, remove_leaf
            // cannot remove it.  Clear it explicitly to prevent stale
            // leaves from persisting in the tree.
            layout.root_mut().clear_leaf(key);
        }
    }

    /// Re-insert a window into the tiling layout (attaches next to current focus).
    pub(super) fn reattach_to_tiling_layout(&mut self, key: WindowKey) {
        use crate::layout::LayoutNode;
        if self.layout_contains(key) {
            return;
        }
        if self.managed_layout.is_none() {
            self.managed_layout = Some(TilingLayout::new(LayoutNode::leaf(key)));
            return;
        }
        let current_focus = *self.focus.current();
        let Some(layout) = self.managed_layout.as_mut() else {
            return;
        };
        if current_focus == key {
            // Focus was previously set to this window; insert at root.
            layout.split_root(key, crate::layout::InsertPosition::Right);
            return;
        }
        let inserted =
            layout
                .root_mut()
                .insert_leaf(current_focus, key, crate::layout::InsertPosition::Right);
        if !inserted {
            layout.split_root(key, crate::layout::InsertPosition::Right);
        }
    }
}
