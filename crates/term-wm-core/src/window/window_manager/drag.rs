use crate::Rect;
use crate::actions::TermWmAction;
use crate::components::{Component, Overlay, WmComponent};
use term_wm_layout_engine::{EdgeResistance, LayoutRect, detect_corner_snap, detect_edge_snap};

use super::{SnapPreviewState, WindowManager};
use crate::layout::{InsertPosition, LayoutNode, TilingLayout};
use crate::window::{WindowKey, WindowState};

/// Cells from screen edge that triggers edge-snap preview.
const EDGE_SNAP_THRESHOLD: u16 = 3;

/// Cells from screen corner that triggers corner-snap preview.
const CORNER_SNAP_THRESHOLD: u16 = 6;

impl<C: Component<TermWmAction>, L: WmComponent, O: Overlay<TermWmAction>> WindowManager<C, L, O> {
    pub(super) fn focus_window_at(&mut self, column: u16, row: u16) -> bool {
        if !self.config.wm_command_menu_enabled || self.managed_draw_order.is_empty() {
            return false;
        }
        let Some(hit) = self.hit_test_region_topmost(column, row, &self.managed_draw_order) else {
            return false;
        };
        if !matches!(hit, _) {
            return false;
        }
        self.focus_window_key(hit);
        true
    }

    #[allow(dead_code)]
    pub(super) fn detach_to_floating(&mut self, key: WindowKey, rect: Rect) -> bool {
        if self.is_window_floating(key) {
            return true;
        }
        if self.managed_layout.is_none() {
            return false;
        }

        // Purge the window from the tiling tree before marking it floating
        self.detach_from_tiling_layout(key);

        let width = rect.width.max(1);
        let height = rect.height.max(1);
        let x = rect.x;
        let y = rect.y;
        self.set_floating_rect(
            key,
            Some(crate::window::FloatRectSpec::Absolute(
                crate::window::FloatRect {
                    x,
                    y,
                    width,
                    height,
                },
            )),
        );
        self.bring_to_front_key(key);
        self.mark_layout_dirty();
        true
    }

    pub(super) fn layout_contains(&self, key: WindowKey) -> bool {
        self.managed_layout
            .as_ref()
            .is_some_and(|layout| layout.root().subtree_any(|node_key| node_key == key))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn move_floating(
        &mut self,
        key: WindowKey,
        column: u16,
        row: u16,
        start_mouse_x: u16,
        start_mouse_y: u16,
        initial_x: i32,
        initial_y: i32,
        velocity_exceeded: bool,
        resistance: &mut EdgeResistance,
    ) {
        let panel_active = self.panel_active();
        let bounds = self.managed_area;
        let Some(crate::window::FloatRectSpec::Absolute(fr)) = self.floating_rect(key) else {
            return;
        };
        let width = fr.width.max(1);
        let height = fr.height.max(1);
        let dx = column as i32 - start_mouse_x as i32;
        let dy = row as i32 - start_mouse_y as i32;
        let x = initial_x + dx;
        let mut y = initial_y + dy;
        let bounds_y = bounds.y;
        if panel_active && y < bounds_y {
            y = bounds_y;
        }

        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        let bounds_layout = LayoutRect {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
        };

        // Skip magnetic snapping if velocity threshold exceeded
        let x = if velocity_exceeded {
            x
        } else {
            resistance.apply_x(x, bounds_layout, now_ns)
        };

        let y = if velocity_exceeded {
            y
        } else {
            resistance.apply_y(y, bounds_layout, now_ns)
        };

        self.set_floating_rect(
            key,
            Some(crate::window::FloatRectSpec::Absolute(
                crate::window::FloatRect {
                    x,
                    y,
                    width,
                    height,
                },
            )),
        );
        self.mark_layout_dirty();
    }

    /// Check the BSP projection cache, and if missing, perform a dry-run
    /// insert into a cloned layout tree and cache the result.
    /// Returns the exact `Rect` the inserted leaf would occupy.
    fn get_projected_preview(
        &mut self,
        dragging_key: WindowKey,
        state: SnapPreviewState,
        area: Rect,
    ) -> Option<Rect> {
        if let Some((s, a, r)) = &self.snap_projection_cache
            && *s == state
            && *a == area
        {
            return *r;
        }
        let rect = match state {
            SnapPreviewState::Corner(pos) | SnapPreviewState::Edge(pos) => self
                .managed_layout
                .as_ref()
                .and_then(|layout| layout.project_insert(None, dragging_key, pos, area)),
            SnapPreviewState::TiledInsert(target_key, pos) => {
                self.managed_layout.as_ref().and_then(|layout| {
                    layout.project_insert(Some(target_key), dragging_key, pos, area)
                })
            }
            SnapPreviewState::VoidInsert(void_id) => self
                .managed_layout
                .as_ref()
                .and_then(|layout| layout.project_insert_void(dragging_key, void_id, area)),
            SnapPreviewState::Maximize => None,
        };
        self.snap_projection_cache = Some((state, area, rect));
        rect
    }

    /// Update the snap preview state during a drag operation.
    ///
    /// Spatial priority order (smallest region first):
    /// 1. Corner snap (TopLeft/TopRight/BottomLeft/BottomRight)
    /// 2. Sacred top edge maximize (y=0, deferred to release)
    /// 3. Edge snap (Left/Right/Top/Bottom half-screen) — checked first
    ///    when cursor is near a screen edge, even if inside a tiled window
    /// 4. Tiled insert (quadrant-based) — when inside a tiled window
    ///    but NOT near a screen edge
    /// 5. Void region (Snap Assist receptacle)
    /// 6. Edge snap fallback — when not inside any window
    ///
    /// `detach_coordinate` is passed separately for post-decouple suppression
    /// (self.mouse_capture is `None` during the extract-operate-restore cycle).
    pub(super) fn update_snap_preview(
        &mut self,
        dragging_key: WindowKey,
        mouse_x: u16,
        mouse_y: u16,
        detach_coordinate: &mut Option<(u16, u16)>,
    ) {
        let area = self.managed_area;

        // Post-decouple suppression: if detach_coordinate is set, suppress all
        // snap previews until the cursor moves 2+ cells from the decouple point.
        const SUPPRESS_THRESHOLD_SQ: u32 = 2 * 2;
        if let Some((decouple_x, decouple_y)) = *detach_coordinate {
            let dx = u32::from(mouse_x.abs_diff(decouple_x));
            let dy = u32::from(mouse_y.abs_diff(decouple_y)).saturating_mul(2);
            let dist_sq = dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy));
            if dist_sq < SUPPRESS_THRESHOLD_SQ {
                // Keep existing snap state visible — don't clear
                return;
            }
            *detach_coordinate = None;
        }

        let managed_layout_rect = LayoutRect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: area.height,
        };

        // Priority 1: Corner snap (smallest spatial region)
        if let Some(corner_pos) =
            detect_corner_snap(mouse_x, mouse_y, managed_layout_rect, CORNER_SNAP_THRESHOLD)
        {
            // Compute position-based layout cache for this snap position
            if self
                .snap_preview_cache
                .needs_recalc(mouse_x, mouse_y, dragging_key)
            {
                let floating: Vec<_> = self
                    .mapped_windows()
                    .into_iter()
                    .filter(|key| self.is_window_floating(*key))
                    .map(|key| (key, self.region(key)))
                    .collect();
                if !floating.is_empty() {
                    let positions =
                        simulate_position_based_layout(floating, dragging_key, corner_pos, area);
                    let dragged_rect = self.region(dragging_key);
                    self.snap_preview_cache.update(
                        mouse_x,
                        mouse_y,
                        dragged_rect,
                        dragging_key,
                        positions,
                    );
                } else {
                    self.snap_preview_cache.clear();
                }
            }
            let preview = self
                .snap_preview_cache
                .positions
                .iter()
                .find(|(k, _)| *k == dragging_key)
                // Only use cache for multi-window tile-all preview
                .filter(|_| self.snap_preview_cache.positions.len() > 1)
                .map(|(_, r)| *r)
                .or_else(|| {
                    self.get_projected_preview(
                        dragging_key,
                        SnapPreviewState::Corner(corner_pos),
                        area,
                    )
                })
                .unwrap_or_else(|| {
                    let ep =
                        term_wm_layout_engine::corner_preview_rect(managed_layout_rect, corner_pos);
                    Rect {
                        x: ep.x,
                        y: ep.y,
                        width: ep.width,
                        height: ep.height,
                    }
                });
            self.drag_snap = Some((None, corner_pos, preview));
            self.snap_preview = Some(SnapPreviewState::Corner(corner_pos));
            return;
        }

        // Priority 2: Sacred top edge — full-screen maximize (deferred to release)
        if mouse_y == 0 {
            self.drag_snap = Some((None, InsertPosition::Top, area));
            self.snap_preview = Some(SnapPreviewState::Maximize);
            return;
        }

        // Priority 3: Edge snap
        if let Some(pos) =
            detect_edge_snap(mouse_x, mouse_y, managed_layout_rect, EDGE_SNAP_THRESHOLD)
        {
            // Compute position-based layout cache for this snap position
            if self
                .snap_preview_cache
                .needs_recalc(mouse_x, mouse_y, dragging_key)
            {
                let floating: Vec<_> = self
                    .mapped_windows()
                    .into_iter()
                    .filter(|key| self.is_window_floating(*key))
                    .map(|key| (key, self.region(key)))
                    .collect();
                if !floating.is_empty() {
                    let positions =
                        simulate_position_based_layout(floating, dragging_key, pos, area);
                    let dragged_rect = self.region(dragging_key);
                    self.snap_preview_cache.update(
                        mouse_x,
                        mouse_y,
                        dragged_rect,
                        dragging_key,
                        positions,
                    );
                } else {
                    self.snap_preview_cache.clear();
                }
            }
            let preview = self
                .snap_preview_cache
                .positions
                .iter()
                .find(|(k, _)| *k == dragging_key)
                // Only use cache for multi-window tile-all preview
                .filter(|_| self.snap_preview_cache.positions.len() > 1)
                .map(|(_, r)| *r)
                .or_else(|| {
                    self.get_projected_preview(dragging_key, SnapPreviewState::Edge(pos), area)
                })
                .unwrap_or_else(|| {
                    let ep = term_wm_layout_engine::edge_preview_rect(managed_layout_rect, pos);
                    Rect {
                        x: ep.x,
                        y: ep.y,
                        width: ep.width,
                        height: ep.height,
                    }
                });
            self.drag_snap = Some((None, pos, preview));
            self.snap_preview = Some(SnapPreviewState::Edge(pos));
            return;
        }

        // No snap target — clear preview
        self.drag_snap = None;
        self.snap_preview = None;
    }

    #[allow(clippy::collapsible_if)]
    pub(super) fn apply_snap(&mut self, key: WindowKey) {
        use crate::layout::LayoutNode;
        if let Some((_target, position, _preview)) = self.drag_snap.take() {
            // Void snap: replace the void placeholder in the BSP tree
            if let Some(SnapPreviewState::VoidInsert(void_id)) = self.snap_preview {
                if self.is_window_floating(key) {
                    self.clear_floating_rect(key);
                }
                if let Some(layout) = &mut self.managed_layout {
                    layout.root_mut().remove_leaf(key);
                    layout.root_mut().cleanup_after_removal();
                    layout
                        .root_mut()
                        .replace_void_by_id(void_id, LayoutNode::leaf(key));
                }
                if let Some(pos) = self.z_order.iter().position(|&z_key| z_key == key) {
                    self.z_order.remove(pos);
                }
                self.z_order.push(key);
                self.bifurcate_draw_order();
                self.snap_projection_cache = None;
                return;
            }

            // Remove key from existing tree before insertion (prevents duplicates)
            if self.layout_contains(key)
                && let Some(layout) = &mut self.managed_layout
            {
                layout.remove_window(key);
                if layout.is_empty() {
                    self.managed_layout = None;
                }
            }

            // Branch based on layout state
            self.snap_projection_cache = None;
            if self.managed_layout.is_some() {
                self.clear_floating_rect(key);
                if let Some(ref mut layout) = self.managed_layout {
                    layout.split_root(key, position);
                }
            } else {
                self.apply_position_based_layout(key, position, self.managed_area);
            }
        }
    }

    pub fn tile_window(&mut self, key: WindowKey) -> bool {
        self.tile_window_key(key)
    }

    /// Try to spawn the window as floating with a sensible default size.
    /// Returns true if the window was set floating.
    ///
    /// Succeeds when:
    /// - No active tiling layout AND all existing windows are floating (or workspace empty)
    pub fn try_spawn_floating_default(&mut self, key: WindowKey) -> bool {
        let existing: Vec<_> = self
            .mapped_windows()
            .into_iter()
            .filter(|k| *k != key)
            .collect();
        if existing.is_empty() || !existing.iter().all(|k| self.is_window_floating(*k)) {
            return false;
        }
        self.managed_layout = None;
        let rect = self.default_cascading_rect(existing.len());
        self.set_floating_rect(
            key,
            Some(crate::window::FloatRectSpec::Absolute(
                crate::window::FloatRect {
                    x: rect.x,
                    y: rect.y,
                    width: rect.width,
                    height: rect.height,
                },
            )),
        );
        self.focus_window_key(key);
        self.bifurcate_draw_order();
        true
    }

    pub(super) fn tile_window_key(&mut self, key: WindowKey) -> bool {
        use crate::layout::LayoutNode;

        // Capture floating rect center BEFORE clearing it
        let floating_info = self
            .floating_rect(key)
            .map(|spec| spec.resolve(self.managed_area))
            .map(|r| r.center());

        self.clear_floating_rect(key);

        if self.layout_contains(key)
            && let Some(layout) = &mut self.managed_layout
        {
            layout.remove_window(key);
            if layout.is_empty() {
                self.managed_layout = None;
            }
        }

        if let Some(ref mut layout) = self.managed_layout {
            if let Some((cx, cy)) = floating_info {
                let regions = layout.regions(self.managed_area);
                let weight = crate::constants::CELL_ASPECT_RATIO;
                if let Some((target_key, target_rect)) =
                    term_wm_layout_engine::resolve_target(cx, cy, &regions, weight)
                {
                    let quad =
                        term_wm_layout_engine::detect_quadrant(cx as u16, cy as u16, &target_rect);
                    let pos = quad.to_insert_position();
                    let inserted = layout.root_mut().insert_leaf(target_key, key, pos);
                    if !inserted {
                        layout.split_root(key, pos);
                    }
                } else {
                    layout.insert_window_balanced(key, self.managed_area);
                }
            } else {
                layout.insert_window_balanced(key, self.managed_area);
            }
        } else {
            self.managed_layout = Some(crate::layout::TilingLayout::new(LayoutNode::leaf(key)));
        }
        self.focus_window_key(key);
        self.bifurcate_draw_order();
        true
    }

    /// Float all tiled windows, preserving their current screen positions.
    /// Atomically clears the layout tree and sets floating rects for all.
    pub(super) fn float_all_windows(&mut self) {
        // Collect tiled windows from regions AND the layout tree
        let mut tiled_keys = Vec::new();

        // Add windows from the layout tree (authoritative for tiled windows)
        if let Some(layout) = &self.managed_layout {
            for key in layout.root().collect_leaves() {
                if !self.is_window_floating(key) && !tiled_keys.contains(&key) {
                    tiled_keys.push(key);
                }
            }
        }

        // Add windows from regions not already captured
        for key in self.regions.ids() {
            if !self.is_window_floating(key) && !tiled_keys.contains(&key) {
                tiled_keys.push(key);
            }
        }

        if tiled_keys.is_empty() {
            return;
        }

        let regions: Vec<_> = tiled_keys
            .iter()
            .enumerate()
            .map(|(idx, key)| (*key, self.region_or_fallback(*key, idx)))
            .collect();

        self.managed_layout = None;

        for (key, rect) in regions {
            self.set_floating_rect(
                key,
                Some(crate::window::FloatRectSpec::Absolute(
                    crate::window::FloatRect {
                        x: rect.x,
                        y: rect.y,
                        width: rect.width.max(1),
                        height: rect.height.max(1),
                    },
                )),
            );
        }
    }

    /// Float all windows. Called when a window decouples from tiling.
    pub(super) fn execute_float_all(&mut self, _key: WindowKey) {
        self.float_all_windows();
    }

    /// Toggle between tiled and floating layout.
    /// If any windows are tiled, float all. Otherwise tile all floating windows.
    pub fn toggle_tiling(&mut self) {
        if self.managed_layout.is_some() {
            self.float_all_windows();
        } else {
            use crate::layout::LayoutNode;

            let keys: Vec<WindowKey> = self
                .z_order
                .iter()
                .copied()
                .filter(|&k| self.window_state(k) == Some(WindowState::Mapped))
                .collect();
            if keys.is_empty() {
                return;
            }

            let mut with_rects = Vec::new();
            let mut without_rects = Vec::new();

            for &k in &keys {
                if let Some(spec) = self.floating_rect(k) {
                    with_rects.push((k, spec.resolve(self.managed_area)));
                } else {
                    without_rects.push(k);
                }
                self.clear_floating_rect(k);
            }

            if !with_rects.is_empty() {
                let root_node = LayoutNode::from_rects(&with_rects);
                let mut layout = TilingLayout::new(root_node);
                for key in without_rects {
                    layout.insert_window_balanced(key, self.managed_area);
                }
                self.managed_layout = Some(layout);
            } else if !without_rects.is_empty() {
                let mut layout = TilingLayout::new(LayoutNode::leaf(without_rects[0]));
                for &key in &without_rects[1..] {
                    layout.insert_window_balanced(key, self.managed_area);
                }
                self.managed_layout = Some(layout);
            }

            // Synchronize draw order and layout projection
            self.bifurcate_draw_order();
            self.mark_layout_dirty();
        }
    }

    /// Tile all windows using position-based layout.
    /// Non-anchor windows are sorted by Euclidean proximity to the anchor,
    /// then the anchor is inserted at the snap position.
    pub(super) fn apply_position_based_layout(
        &mut self,
        anchor_key: WindowKey,
        snap_position: InsertPosition,
        area: Rect,
    ) {
        let mapped = self.mapped_windows();
        let mut all_windows: Vec<_> = mapped
            .iter()
            .copied()
            .filter(|key| self.is_window_floating(*key))
            .enumerate()
            .map(|(idx, key)| (key, self.region_or_fallback(key, idx)))
            .collect();

        // Also include windows from regions that might not be mapped (e.g., test windows)
        for key in self.regions.ids() {
            if !all_windows.iter().any(|(k, _)| *k == key) {
                let full = self.regions.get(key).unwrap_or_else(|| self.region(key));
                all_windows.push((key, full));
            }
        }

        // Ensure anchor is included even if not in regions (e.g., floating-only window)
        if !all_windows.iter().any(|(k, _)| *k == anchor_key) {
            let rect = self
                .floating_rect(anchor_key)
                .map(|spec| spec.resolve(area))
                .unwrap_or_else(|| self.region(anchor_key));
            all_windows.push((anchor_key, rect));
        }

        if all_windows.is_empty() {
            return;
        }

        let target_rect = all_windows
            .iter()
            .find(|(k, _)| *k == anchor_key)
            .map(|(_, r)| *r)
            .unwrap();

        let others: Vec<_> = all_windows
            .iter()
            .filter(|(k, _)| *k != anchor_key)
            .copied()
            .collect();

        for (key, _) in &all_windows {
            self.clear_floating_rect(*key);
        }

        if others.is_empty() {
            // Create a void split to preserve snap geometry
            let mut layout = TilingLayout::new_void();
            layout.split_root(anchor_key, snap_position);
            self.managed_layout = Some(layout);
            return;
        }

        let sorted = calculate_tiling_order(others, target_rect);
        let mut layout = TilingLayout::new(LayoutNode::leaf(sorted[0].0));
        for (key, _) in &sorted[1..] {
            layout.insert_window_balanced(*key, area);
        }
        layout.split_root(anchor_key, snap_position);
        self.managed_layout = Some(layout);
    }
}

/// Euclidean distance sort: sorts windows by squared distance from
/// the target window's center. Y-axis distances are doubled to
/// account for terminal cell aspect ratio (~2:1 height:width).
fn calculate_tiling_order(
    mut floating_rects: Vec<(WindowKey, Rect)>,
    target_rect: Rect,
) -> Vec<(WindowKey, Rect)> {
    let target_center_y = target_rect.y + (target_rect.height as i32) / 2;
    let target_center_x = target_rect.x + (target_rect.width as i32) / 2;

    floating_rects.sort_by(|a, b| {
        let a_dy = ((a.1.y + (a.1.height as i32) / 2) - target_center_y) * 2;
        let a_dx = (a.1.x + (a.1.width as i32) / 2) - target_center_x;
        let b_dy = ((b.1.y + (b.1.height as i32) / 2) - target_center_y) * 2;
        let b_dx = (b.1.x + (b.1.width as i32) / 2) - target_center_x;

        (a_dx * a_dx + a_dy * a_dy).cmp(&(b_dx * b_dx + b_dy * b_dy))
    });
    floating_rects
}

/// Pure simulation of the position-based layout for preview caching.
/// Builds an ephemeral `TilingLayout`, projects against workspace bounds,
/// and returns projected rects in sorted key order. Never mutates state.
fn simulate_position_based_layout(
    windows: Vec<(WindowKey, Rect)>,
    anchor_key: WindowKey,
    snap_position: InsertPosition,
    workspace_bounds: Rect,
) -> Vec<(WindowKey, Rect)> {
    if windows.is_empty() {
        return Vec::new();
    }

    let target_rect = windows
        .iter()
        .find(|(k, _)| *k == anchor_key)
        .map(|(_, r)| *r)
        .expect("anchor_key must be in windows");

    let others: Vec<_> = windows
        .iter()
        .filter(|(k, _)| *k != anchor_key)
        .copied()
        .collect();

    if others.is_empty() {
        let mut layout = TilingLayout::new_void();
        layout.split_root(anchor_key, snap_position);
        return layout
            .regions(workspace_bounds)
            .into_iter()
            .filter(|(k, _)| *k == anchor_key)
            .collect();
    }

    let sorted = calculate_tiling_order(others, target_rect);
    let mut layout = TilingLayout::new(LayoutNode::leaf(sorted[0].0));
    for (key, _) in &sorted[1..] {
        layout.insert_window_balanced(*key, workspace_bounds);
    }
    layout.split_root(anchor_key, snap_position);

    let mut regions = layout.regions(workspace_bounds);
    let mut result = Vec::with_capacity(sorted.len() + 1);
    if let Some(r) = regions
        .iter()
        .find(|(k, _)| *k == anchor_key)
        .map(|(_, r)| *r)
    {
        result.push((anchor_key, r));
    }
    for (k, _) in &sorted {
        if let Some(idx) = regions.iter().position(|(rk, _)| rk == k) {
            result.push((*k, regions.swap_remove(idx).1));
        }
    }
    result
}
