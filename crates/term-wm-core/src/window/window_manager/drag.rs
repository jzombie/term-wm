use crate::Rect;
use term_wm_layout_engine::{EdgeResistance, LayoutRect, detect_corner_snap, detect_edge_snap};

use super::{SnapPreviewState, WindowManager};
use crate::layout::InsertPosition;
use crate::window::WindowKey;

/// Cells from screen edge that triggers edge-snap preview.
const EDGE_SNAP_THRESHOLD: u16 = 3;

/// Cells from screen corner that triggers corner-snap preview.
const CORNER_SNAP_THRESHOLD: u16 = 6;

impl WindowManager {
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
            let preview = self
                .get_projected_preview(dragging_key, SnapPreviewState::Corner(corner_pos), area)
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

        // Priority 3: Edge snap — checked BEFORE tiled insert when cursor
        // is near a screen edge.  This ensures that dragging to the right
        // edge always shows a right-half preview, even if the cursor is
        // inside a tiled window near that edge.
        if let Some(pos) =
            detect_edge_snap(mouse_x, mouse_y, managed_layout_rect, EDGE_SNAP_THRESHOLD)
        {
            let preview = self
                .get_projected_preview(dragging_key, SnapPreviewState::Edge(pos), area)
                .unwrap_or_else(|| {
                    let ep = term_wm_layout_engine::edge_preview_rect(managed_layout_rect, pos);
                    Rect {
                        x: ep.x,
                        y: ep.y,
                        width: ep.width,
                        height: ep.height,
                    }
                });
            let preview = if self.managed_layout.is_none() {
                area
            } else {
                preview
            };
            self.drag_snap = Some((None, pos, preview));
            self.snap_preview = Some(SnapPreviewState::Edge(pos));
            return;
        }

        // No snap target — clear preview
        self.drag_snap = None;
        self.snap_preview = None;
    }

    pub(super) fn apply_snap(&mut self, key: WindowKey) {
        use crate::layout::LayoutNode;
        if let Some((target, position, preview)) = self.drag_snap.take() {
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
                self.managed_draw_order = self.z_order.clone();
                self.snap_projection_cache = None;
                return;
            }

            let other_windows_exist = if let Some(layout) = &self.managed_layout {
                !layout.regions(self.managed_area).is_empty()
            } else {
                false
            };

            if target.is_none() && !other_windows_exist {
                if self.is_window_floating(key) {
                    self.set_floating_rect(
                        key,
                        Some(crate::window::FloatRectSpec::Absolute(
                            crate::window::FloatRect {
                                x: preview.x,
                                y: preview.y,
                                width: preview.width,
                                height: preview.height,
                            },
                        )),
                    );
                }
                return;
            }

            if self.is_window_floating(key) {
                self.clear_floating_rect(key);
            }

            if self.layout_contains(key)
                && let Some(layout) = &mut self.managed_layout
            {
                let should_retile = match target {
                    Some(target_key) => target_key != key,
                    None => true,
                };
                if should_retile {
                    layout.root_mut().remove_leaf(key);
                    layout.root_mut().cleanup_after_removal();
                } else {
                    self.bring_to_front_key(key);
                    return;
                }
            }

            if let Some(target_key) = target
                && self.is_window_floating(target_key)
            {
                self.clear_floating_rect(target_key);
                if self.managed_layout.is_none() {
                    self.managed_layout = Some(crate::layout::TilingLayout::new(LayoutNode::leaf(
                        target_key,
                    )));
                }
            }

            if let Some(layout) = &mut self.managed_layout {
                let success = if let Some(target_key) = target {
                    layout.root_mut().insert_leaf(target_key, key, position)
                } else {
                    false
                };

                if !success {
                    layout.split_root(key, position);
                }

                if let Some(pos) = self.z_order.iter().position(|&z_key| z_key == key) {
                    self.z_order.remove(pos);
                }
                self.z_order.push(key);
                self.managed_draw_order = self.z_order.clone();
            } else {
                self.managed_layout = Some(crate::layout::TilingLayout::new(LayoutNode::leaf(key)));
            }
            self.snap_projection_cache = None;

            let mut pending_snap = Vec::new();
            for r_key in self.regions.ids() {
                if r_key != key && self.is_window_floating(r_key) {
                    pending_snap.push(r_key);
                }
            }
            for float_key in pending_snap {
                self.tile_window_key(float_key);
            }
        }
    }

    pub fn tile_window(&mut self, key: WindowKey) -> bool {
        self.tile_window_key(key)
    }

    pub(super) fn tile_window_key(&mut self, key: WindowKey) -> bool {
        use crate::layout::LayoutNode;
        if self.layout_contains(key) {
            if self.is_window_floating(key) {
                self.clear_floating_rect(key);
            }
            self.focus_window_key(key);
            return true;
        }
        if self.managed_layout.is_none() {
            self.managed_layout = Some(crate::layout::TilingLayout::new(LayoutNode::leaf(key)));
            self.focus_window_key(key);
            return true;
        }

        let current_focus = *self.focus.current();

        let Some(layout) = self.managed_layout.as_mut() else {
            return false;
        };

        let target = self
            .regions
            .ids()
            .iter()
            .find(|r_key| **r_key == current_focus)
            .copied();

        if let Some(target) = target
            && layout
                .root_mut()
                .insert_leaf(target, key, InsertPosition::Right)
        {
            self.focus_window_key(key);
            return true;
        }

        layout.split_root(key, InsertPosition::Right);
        self.focus_window_key(key);
        true
    }
}
