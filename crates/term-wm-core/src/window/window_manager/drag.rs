use std::time::{Duration, Instant};

use crossterm::event::{Event, MouseEventKind};
use ratatui::prelude::Rect;

use super::WindowManager;
use crate::layout::InsertPosition;
use crate::layout::floating::*;
use crate::window::WindowKey;
use term_wm_layout_engine::{
    EdgeResistance, LayoutRect, apply_resize_drag_signed, detect_quadrant,
};

impl WindowManager {
    pub(super) fn handle_header_drag_event(&mut self, event: &Event) -> bool {
        use crate::window::decorator::HeaderAction;
        let Event::Mouse(mouse) = event else {
            return false;
        };
        match mouse.kind {
            MouseEventKind::Down(_) => {
                let topmost_hit = if self.config.chrome_enabled
                    && !self.managed_draw_order.is_empty()
                {
                    self.hit_test_region_topmost(mouse.column, mouse.row, &self.managed_draw_order)
                } else {
                    None
                };

                if let Some(header) = self
                    .floating_headers
                    .iter()
                    .rev()
                    .find(|handle| {
                        crate::layout::rect_contains(handle.rect, mouse.column, mouse.row)
                    })
                    .copied()
                {
                    if let Some(hit_key) = topmost_hit
                        && hit_key != header.key
                    {
                        return false;
                    }

                    let rect = self.full_region_for_key(header.key);
                    match self.decorator().hit_test(rect, mouse.column, mouse.row) {
                        HeaderAction::Minimize => {
                            self.minimize_window(header.key);
                            self.last_header_click = None;
                            return true;
                        }
                        HeaderAction::Maximize => {
                            self.toggle_maximize(header.key);
                            self.last_header_click = None;
                            return true;
                        }
                        HeaderAction::Close => {
                            self.close_window(header.key);
                            self.last_header_click = None;
                            return true;
                        }
                        HeaderAction::ToggleDirectMode => {
                            self.toggle_direct_mode(header.key);
                            self.last_header_click = None;
                            return true;
                        }
                        HeaderAction::Drag => {
                            let now = Instant::now();
                            if let Some((prev_key, prev)) = self.last_header_click
                                && prev_key == header.key
                                && now.duration_since(prev) <= Duration::from_millis(500)
                            {
                                self.toggle_maximize(header.key);
                                self.last_header_click = None;
                                return true;
                            }
                            self.last_header_click = Some((header.key, now));
                        }
                        HeaderAction::None => {}
                    }

                    if self.is_window_floating(header.key) {
                        self.bring_floating_to_front_key(header.key);
                    } else {
                        let _ = self.detach_to_floating(header.key, rect);
                    }

                    let (initial_x, initial_y) =
                        if let Some(crate::window::FloatRectSpec::Absolute(fr)) =
                            self.floating_rect(header.key)
                        {
                            (fr.x, fr.y)
                        } else {
                            (rect.x as i32, rect.y as i32)
                        };
                    self.drag_header = Some(HeaderDrag {
                        key: header.key,
                        initial_x,
                        initial_y,
                        start_x: mouse.column,
                        start_y: mouse.row,
                    });
                    self.drag_last_event = Some(Instant::now());
                    return true;
                }
            }
            MouseEventKind::Drag(_) => {
                self.drag_last_event = Some(Instant::now());
                if let Some(drag) = self.drag_header {
                    if self.is_window_floating(drag.key) {
                        self.move_floating(
                            drag.key,
                            mouse.column,
                            mouse.row,
                            drag.start_x,
                            drag.start_y,
                            drag.initial_x,
                            drag.initial_y,
                        );
                        let dx = mouse.column.abs_diff(drag.start_x);
                        let dy = mouse.row.abs_diff(drag.start_y);
                        if dx + dy > 2 {
                            self.update_snap_preview(drag.key, mouse.column, mouse.row);
                        } else {
                            self.drag_snap = None;
                        }
                    }
                    return true;
                }
            }
            MouseEventKind::Moved => {
                // Mouse re-entered the terminal after being released outside
                // during a header drag (no Up event was delivered).
                if let Some(drag) = self.drag_header.take()
                    && self.drag_snap.is_some()
                {
                    self.apply_snap(drag.key);
                    return true;
                }
            }
            MouseEventKind::Up(_) => {
                if let Some(drag) = self.drag_header.take() {
                    if self.drag_snap.is_some() {
                        self.apply_snap(drag.key);
                    }
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    pub(super) fn focus_window_at(&mut self, column: u16, row: u16) -> bool {
        if !self.config.wm_overlay_enabled || self.managed_draw_order.is_empty() {
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

    pub(super) fn handle_resize_event(&mut self, event: &Event) -> bool {
        let Event::Mouse(mouse) = event else {
            return false;
        };
        match mouse.kind {
            MouseEventKind::Down(_) => {
                let topmost_hit = if self.config.floating_windows_enabled
                    && !self.managed_draw_order.is_empty()
                {
                    self.hit_test_region_topmost(mouse.column, mouse.row, &self.managed_draw_order)
                } else {
                    None
                };

                let hit = self
                    .resize_handles
                    .iter()
                    .rev()
                    .find(|handle| {
                        crate::layout::rect_contains(handle.rect, mouse.column, mouse.row)
                    })
                    .copied();
                if let Some(handle) = hit {
                    if let Some(hit_key) = topmost_hit
                        && hit_key != handle.key
                    {
                        return false;
                    }

                    let rect = self.full_region_for_key(handle.key);
                    if !self.is_window_floating(handle.key) {
                        return false;
                    }
                    self.bring_floating_to_front_key(handle.key);
                    let (start_x, start_y, start_width, start_height) =
                        if let Some(crate::window::FloatRectSpec::Absolute(fr)) =
                            self.floating_rect(handle.key)
                        {
                            (fr.x, fr.y, fr.width, fr.height)
                        } else {
                            (rect.x as i32, rect.y as i32, rect.width, rect.height)
                        };
                    self.drag_resize = Some(ResizeDrag {
                        key: handle.key,
                        edge: handle.edge,
                        start_rect: rect,
                        start_col: mouse.column,
                        start_row: mouse.row,
                        start_x,
                        start_y,
                        start_width,
                        start_height,
                    });
                    return true;
                }
            }
            MouseEventKind::Drag(_) => {
                if let Some(drag) = self.drag_resize.as_ref()
                    && self.is_window_floating(drag.key)
                {
                    let bounds = LayoutRect {
                        x: self.managed_area.x as i32,
                        y: self.managed_area.y as i32,
                        width: self.managed_area.width,
                        height: self.managed_area.height,
                    };
                    let resized = apply_resize_drag_signed(
                        drag.start_x,
                        drag.start_y,
                        drag.start_width,
                        drag.start_height,
                        drag.edge,
                        mouse.column,
                        mouse.row,
                        drag.start_col,
                        drag.start_row,
                        bounds,
                        self.floating_resize_offscreen,
                    );
                    self.set_floating_rect(
                        drag.key,
                        Some(crate::window::FloatRectSpec::Absolute(resized)),
                    );
                    return true;
                }
            }
            MouseEventKind::Up(_) if self.drag_resize.take().is_some() => {
                return true;
            }
            _ => {}
        }
        false
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
