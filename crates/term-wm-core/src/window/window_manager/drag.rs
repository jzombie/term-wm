use ratatui::prelude::Rect;
use term_wm_layout_engine::{EdgeResistance, LayoutRect, detect_quadrant};

use super::WindowManager;
use crate::layout::InsertPosition;
use crate::window::WindowKey;

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

    pub(super) fn detach_to_floating(&mut self, key: WindowKey, rect: Rect) -> bool {
        if self.is_window_floating(key) {
            return true;
        }
        if self.managed_layout.is_none() {
            return false;
        }

        let width = rect.width.max(1);
        let height = rect.height.max(1);
        let x = rect.x;
        let y = rect.y;
        self.set_floating_rect(
            key,
            Some(crate::window::FloatRectSpec::Absolute(
                crate::window::FloatRect {
                    x: x as i32,
                    y: y as i32,
                    width,
                    height,
                },
            )),
        );
        self.bring_to_front_key(key);
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
        let bounds_y = bounds.y as i32;
        if panel_active && y < bounds_y {
            y = bounds_y;
        }

        let mut resistance = EdgeResistance::default_tui();
        let bounds_layout = LayoutRect {
            x: bounds.x as i32,
            y: bounds.y as i32,
            width: bounds.width,
            height: bounds.height,
        };
        let x = resistance.apply_x(x, bounds_layout);

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
    }

    pub(super) fn update_snap_preview(
        &mut self,
        dragging_key: WindowKey,
        mouse_x: u16,
        mouse_y: u16,
    ) {
        self.drag_snap = None;
        let area = self.managed_area;

        let target = self.z_order.iter().rev().find_map(|&key| {
            if key == dragging_key {
                return None;
            }
            if self.managed_layout.is_some() && self.is_window_floating(key) {
                return None;
            }
            let rect = self.regions.get(key)?;
            if crate::layout::rect_contains(rect, mouse_x, mouse_y) {
                Some((key, rect))
            } else {
                None
            }
        });

        if let Some((target_key, rect)) = target {
            let target_layout = LayoutRect {
                x: rect.x as i32,
                y: rect.y as i32,
                width: rect.width,
                height: rect.height,
            };

            let quadrant = detect_quadrant(mouse_x, mouse_y, &target_layout);
            let pos = match quadrant {
                term_wm_layout_engine::Quadrant::East => InsertPosition::Right,
                term_wm_layout_engine::Quadrant::West => InsertPosition::Left,
                term_wm_layout_engine::Quadrant::North => InsertPosition::Top,
                term_wm_layout_engine::Quadrant::South => InsertPosition::Bottom,
            };

            let engine_preview = term_wm_layout_engine::tiled_preview_rect(target_layout, pos);
            let preview = Rect {
                x: engine_preview.x as u16,
                y: engine_preview.y as u16,
                width: engine_preview.width,
                height: engine_preview.height,
            };

            self.drag_snap = Some((Some(target_key), pos, preview));
            return;
        }

        let managed_layout = LayoutRect {
            x: area.x as i32,
            y: area.y as i32,
            width: area.width,
            height: area.height,
        };

        let position = term_wm_layout_engine::detect_edge_snap(mouse_x, mouse_y, managed_layout, 2);

        if let Some(pos) = position {
            let engine_preview = term_wm_layout_engine::edge_preview_rect(managed_layout, pos);
            let mut preview = Rect {
                x: engine_preview.x as u16,
                y: engine_preview.y as u16,
                width: engine_preview.width,
                height: engine_preview.height,
            };

            if self.managed_layout.is_none() {
                preview = area;
            }

            self.drag_snap = Some((None, pos, preview));
        }
    }

    pub(super) fn apply_snap(&mut self, key: WindowKey) {
        use crate::layout::LayoutNode;
        if let Some((target, position, preview)) = self.drag_snap.take() {
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
                                x: preview.x as i32,
                                y: preview.y as i32,
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
