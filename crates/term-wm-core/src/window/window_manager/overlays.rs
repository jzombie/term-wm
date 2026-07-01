use std::time::Duration;

use crossterm::event::{Event, MouseEventKind};
use ratatui::layout::Alignment;
use ratatui::style::{Color, Style};
use ratatui::widgets::Paragraph;

use super::{WindowId, WindowManager, WmMenuAction};
use crate::components::{Component, ConfirmAction, Overlay};
use crate::keybindings::Action;
use crate::layout::{FloatingPane, rect_contains, render_handles_masked};
use crate::window::FloatRectSpec;

impl<Id: Copy + Eq + Ord + std::fmt::Debug + 'static> WindowManager<Id> {
    pub fn open_wm_overlay(&mut self) {
        self.overlay_visible = true;
        self.overlay_opened_at = Some(std::time::Instant::now());
        if let Some(menu) = &mut self.menu_overlay {
            menu.restore();
        }
    }

    pub fn open_wm_overlay_no_passthrough(&mut self) {
        self.overlay_visible = true;
        self.overlay_opened_at = None;
        if let Some(menu) = &mut self.menu_overlay {
            menu.restore();
        }
    }

    pub fn close_wm_overlay(&mut self) {
        self.overlay_visible = false;
        self.overlay_opened_at = None;
        if let Some(menu) = &mut self.menu_overlay {
            menu.restore();
        }
    }

    pub fn wm_overlay_visible(&self) -> bool {
        self.overlay_visible
    }

    pub fn fold_menu(&mut self) {
        if let Some(menu) = &mut self.menu_overlay {
            menu.outline();
        }
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
            && self
                .top_panel
                .as_ref()
                .is_some_and(|p| p.menu_icon_contains_point(mouse.column, mouse.row))
        {
            return Some(WmMenuAction::CloseMenu);
        }

        let ctx = self.component_context(false).with_overlay(true);
        let Some(menu) = &mut self.menu_overlay else {
            return None;
        };
        let comp: &mut dyn Component = &mut **menu;
        comp.handle_event(event, &ctx);

        if let Some(action) = menu.selected_action() {
            return Some(*action);
        }

        if let Event::Mouse(mouse) = event
            && matches!(mouse.kind, MouseEventKind::Down(_))
        {
            return Some(WmMenuAction::CloseMenu);
        }

        None
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
        let Event::Key(key) = event else {
            return false;
        };
        let kb = self.keybindings();
        kb.matches(Action::MenuUp, key)
            || kb.matches(Action::MenuDown, key)
            || kb.matches(Action::MenuSelect, key)
            || kb.matches(Action::MenuNext, key)
            || kb.matches(Action::MenuPrev, key)
    }

    pub fn render_overlays(
        &mut self,
        frame: &mut crate::ui::UiFrame<'_>,
        z_base: usize,
        z_total: usize,
    ) {
        let (hovered, hovered_resize) = self.hover_targets();
        let obscuring: Vec<ratatui::prelude::Rect> = self
            .managed_draw_order
            .iter()
            .filter_map(|&id| self.regions.get(id))
            .collect();
        let is_obscured =
            |x: u16, y: u16| -> bool { obscuring.iter().any(|r| rect_contains(*r, x, y)) };
        render_handles_masked(frame, &self.handles, hovered, is_obscured, &self.config.theme);
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
            &self.config.theme,
        );

        if let Some((_, _, rect)) = self.drag_snap {
            let buffer = frame.buffer_mut();
            let color = self.config.theme.accent;
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

            if let Some(remaining) = self.drag_snap_remaining() {
                const GRACE: Duration = Duration::from_millis(500);
                let timeout = self.config.drag_snap_timeout.unwrap();
                if timeout.saturating_sub(remaining) < GRACE {
                    // Still within grace period — don't show countdown yet
                } else {
                    let text = if remaining == Duration::ZERO {
                        "Mouse left — snapping...".to_string()
                    } else {
                        format!("Mouse left — snapping in {}s", remaining.as_secs().max(1))
                    };
                    let text_len = text.len() as u16;
                    let text_x = rect.x + (rect.width.saturating_sub(text_len)) / 2;
                    let text_y = rect.y + rect.height / 2;
                    if text_x >= rect.x && text_y >= rect.y {
                        let text_area = ratatui::prelude::Rect {
                            x: text_x,
                            y: text_y,
                            width: text_len,
                            height: 1,
                        };
                        let paragraph = Paragraph::new(text)
                            .style(
                                Style::default()
                                    .fg(self.config.theme.accent_alt)
                                    .bg(Color::Black),
                            )
                            .alignment(Alignment::Center);
                        frame.render_widget(paragraph, text_area);
                    }
                }
            }
        }

        let mut oi = z_base;
        if self.wm_overlay_visible() {
            let menu_items = super::wm_menu_items(
                self.mouse_capture_enabled(),
                self.clipboard_enabled(),
                self.window_selection_enabled(),
            );
            let anchor = self
                .top_panel
                .as_ref()
                .and_then(|p| p.menu_icon_rect())
                .map(|r| (r.x, r.y.saturating_add(r.height)));
            let menu_ctx = self
                .component_context(false)
                .with_overlay(true)
                .with_hover_pos(self.hover)
                .with_keybindings(std::sync::Arc::new(self.keybindings().clone()));
            if let Some(menu) = &mut self.menu_overlay {
                menu.set_items(menu_items);
                menu.set_anchor(anchor);
                menu.set_managed_area(self.managed_area);
                let comp: &mut dyn Component = &mut **menu;
                comp.render(frame, frame.area(), &menu_ctx);
            }
            oi += 1;
        }
        for overlay_id in [super::OverlayId::ExitConfirm, super::OverlayId::Help] {
            if self.overlays.contains_key(&overlay_id) {
                let z = super::WindowManager::<Id>::compute_z_depth(oi, z_total);
                oi += 1;
                let ctx = self.component_context(false).with_overlay(true);
                if let Some(r) = self
                    .overlays
                    .get(&overlay_id)
                    .and_then(|o| o.shadow_rect(frame.area()))
                {
                    let shadow_dest = crate::window::FloatRect {
                        x: r.x as i32,
                        y: r.y as i32,
                        width: r.width,
                        height: r.height,
                    };
                    crate::ui::render_drop_shadow(frame, shadow_dest, z, &self.config.theme);
                }
                if let Some(overlay) = self.overlays.get_mut(&overlay_id) {
                    overlay.render(frame, frame.area(), &ctx);
                }
            }
        }
    }
}
