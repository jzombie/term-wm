use crossterm::event::{Event, MouseEventKind};

use super::{WmMenuAction, WindowId, WindowManager};
use crate::components::{ConfirmAction, Overlay};
use crate::layout::{FloatingPane, rect_contains, render_handles_masked};
use crate::window::FloatRectSpec;

impl<Id: Copy + Eq + Ord + std::fmt::Debug + 'static> WindowManager<Id> {
    pub fn open_wm_overlay(&mut self) {
        self.overlay_visible = true;
        self.overlay_opened_at = Some(std::time::Instant::now());
        self.menu_overlay.restore();
    }

    pub fn open_wm_overlay_no_passthrough(&mut self) {
        self.overlay_visible = true;
        self.overlay_opened_at = None;
        self.menu_overlay.restore();
    }

    pub fn close_wm_overlay(&mut self) {
        self.overlay_visible = false;
        self.overlay_opened_at = None;
        self.menu_overlay.restore();
    }

    pub fn wm_overlay_visible(&self) -> bool {
        self.overlay_visible
    }

    pub fn fold_menu(&mut self) {
        self.menu_overlay.outline();
    }

    pub fn close_exit_confirm(&mut self) {
        self.overlays.remove(&super::OverlayId::ExitConfirm);
    }

    pub fn exit_confirm_visible(&self) -> bool {
        self.overlays.contains_key(&super::OverlayId::ExitConfirm)
    }

    pub fn help_overlay_visible(&self) -> bool {
        self.overlays.contains_key(&super::OverlayId::Help)
    }

    pub fn close_help_overlay(&mut self) {
        self.overlays.remove(&super::OverlayId::Help);
    }

    pub fn handle_help_event(&mut self, event: &Event) -> bool {
        let ctx = self.component_context(true).with_overlay(true);
        let Some(boxed) = self.overlays.get_mut(&super::OverlayId::Help) else {
            return false;
        };
        boxed.resize(self.last_frame_area, &ctx);
        let handled = boxed.handle_event(event, &ctx);
        if !boxed.visible() {
            self.overlays.remove(&super::OverlayId::Help);
        }
        handled
    }

    pub fn handle_wm_menu_event(&mut self, event: &Event) -> Option<WmMenuAction> {
        if !self.wm_overlay_visible() {
            return None;
        }
        if let Event::Mouse(mouse) = event
            && matches!(mouse.kind, MouseEventKind::Down(_))
        {
            if self.panel.menu_icon_contains_point(mouse.column, mouse.row) {
                return Some(WmMenuAction::CloseMenu);
            }
            let result = self.menu_overlay.handle_event(event);
            if result.is_none() {
                return Some(WmMenuAction::CloseMenu);
            }
            return result;
        }
        self.menu_overlay.handle_event(event)
    }

    pub fn handle_exit_confirm_event(&mut self, event: &Event) -> Option<ConfirmAction> {
        let comp = self.overlays.get_mut(&super::OverlayId::ExitConfirm)?;
        let overlay: &mut dyn Overlay = &mut **comp;
        overlay.handle_confirm_event(event)
    }

    pub fn wm_menu_consumes_event(&self, event: &Event) -> bool {
        if !self.wm_overlay_visible() {
            return false;
        }
        self.menu_overlay.consumes_event(event)
    }

    pub fn render_overlays(&mut self, frame: &mut crate::ui::UiFrame<'_>) {
        let (hovered, hovered_resize) = self.hover_targets();
        let obscuring: Vec<ratatui::prelude::Rect> = self
            .managed_draw_order
            .iter()
            .filter_map(|&id| self.regions.get(id))
            .collect();
        let is_obscured =
            |x: u16, y: u16| -> bool { obscuring.iter().any(|r| rect_contains(*r, x, y)) };
        render_handles_masked(frame, &self.handles, hovered, is_obscured);
        let floating_panes: Vec<FloatingPane<WindowId<Id>>> = self
            .windows
            .iter()
            .filter_map(|(&id, window)| {
                window.floating_rect.map(|rect| match rect {
                    FloatRectSpec::Absolute(fr) => FloatingPane {
                        id,
                        rect: crate::layout::RectSpec::Absolute(ratatui::prelude::Rect {
                            x: fr.x.max(0) as u16,
                            y: fr.y.max(0) as u16,
                            width: fr.width,
                            height: fr.height,
                        }),
                    },
                    FloatRectSpec::Percent {
                        x,
                        y,
                        width,
                        height,
                    } => FloatingPane {
                        id,
                        rect: crate::layout::RectSpec::Percent {
                            x,
                            y,
                            width,
                            height,
                        },
                    },
                })
            })
            .collect();

        let mut visible_regions = crate::layout::RegionMap::default();
        for id in self.regions.ids() {
            visible_regions.set(id, self.visible_region_for_id(id));
        }

        crate::layout::floating::render_resize_outline(
            frame,
            hovered_resize.copied(),
            self.drag_resize,
            &visible_regions,
            self.managed_area,
            &floating_panes,
            &self.managed_draw_order,
        );

        if let Some((_, _, rect)) = self.drag_snap {
            let buffer = frame.buffer_mut();
            let color = crate::theme::accent();
            let clip = rect.intersection(buffer.area);
            if clip.width > 0 && clip.height > 0 {
                for y in clip.y..clip.y.saturating_add(clip.height) {
                    for x in clip.x..clip.x.saturating_add(clip.width) {
                        if let Some(cell) = buffer.cell_mut((x, y)) {
                            let mut style = cell.style();
                            style.bg = Some(color);
                            cell.set_style(style);
                        }
                    }
                }
            }
        }

        if self.wm_overlay_visible() {
            let menu_items = super::wm_menu_items(
                self.mouse_capture_enabled(),
                self.clipboard_enabled(),
                self.window_selection_enabled(),
            );
            self.menu_overlay.set_items(menu_items);
            let anchor = self.panel.menu_icon_rect().map(|r| (r.x, r.y.saturating_add(r.height)));
            self.menu_overlay.set_hover_pos(self.hover);
            self.menu_overlay.render(frame, anchor, self.managed_area);
        }

        let confirm_ctx = self.component_context(false).with_overlay(true);
        let help_ctx = self.component_context(false).with_overlay(true);
        if let Some(confirm) = self.overlays.get_mut(&super::OverlayId::ExitConfirm) {
            confirm.render(frame, frame.area(), &confirm_ctx);
        }
        if let Some(help) = self.overlays.get_mut(&super::OverlayId::Help) {
            help.render(frame, frame.area(), &help_ctx);
        }
    }
}
