use crossterm::event::{Event, MouseEventKind};
use ratatui::prelude::Rect;

use super::{OverlayId, WindowId, WindowManager};
use crate::components::{ComponentContext, ConfirmAction, Overlay};
use crate::layout::{FloatingPane, rect_contains, render_handles_masked};
use crate::window::FloatRectSpec;

impl<Id: Copy + Eq + Ord + std::fmt::Debug + 'static> WindowManager<Id> {
    pub fn open_wm_overlay(&mut self) {
        self.overlay_visible = true;
        self.wm_overlay_opened_at = Some(std::time::Instant::now());
        self.wm_menu_selected = 0;
    }

    pub fn close_wm_overlay(&mut self) {
        self.overlay_visible = false;
        self.wm_overlay_opened_at = None;
        self.wm_menu_selected = 0;
    }

    pub fn wm_overlay_visible(&self) -> bool {
        self.overlay_visible
    }

    pub fn close_exit_confirm(&mut self) {
        self.overlays.remove(&OverlayId::ExitConfirm);
    }

    pub fn exit_confirm_visible(&self) -> bool {
        self.overlays.contains_key(&OverlayId::ExitConfirm)
    }

    pub fn help_overlay_visible(&self) -> bool {
        self.overlays.contains_key(&OverlayId::Help)
    }

    pub fn close_help_overlay(&mut self) {
        self.overlays.remove(&OverlayId::Help);
    }

    pub fn selection_preview_visible(&self) -> bool {
        self.overlays.contains_key(&OverlayId::SelectionPreview)
    }

    pub fn close_selection_preview(&mut self) {
        self.overlays.remove(&OverlayId::SelectionPreview);
        if let Some(prev) = self.selection_preview_restore_mouse.take() {
            self.set_mouse_capture_enabled(prev);
        }
    }

    pub fn handle_help_event(&mut self, event: &Event) -> bool {
        let Some(boxed) = self.overlays.get_mut(&OverlayId::Help) else {
            return false;
        };
        boxed.resize(
            self.last_frame_area,
            &ComponentContext::new(true).with_overlay(true),
        );
        let handled = boxed.handle_event(event, &ComponentContext::new(true).with_overlay(true));
        if !boxed.visible() {
            self.overlays.remove(&OverlayId::Help);
        }
        handled
    }

    pub fn handle_selection_preview_event(&mut self, event: &Event) -> bool {
        let Some(boxed) = self.overlays.get_mut(&OverlayId::SelectionPreview) else {
            return false;
        };
        boxed.resize(
            self.last_frame_area,
            &ComponentContext::new(true).with_overlay(true),
        );
        let handled = boxed.handle_event(event, &ComponentContext::new(true).with_overlay(true));
        if !boxed.visible() {
            self.close_selection_preview();
        }
        handled
    }

    pub fn handle_wm_menu_event(&mut self, event: &Event) -> Option<super::WmMenuAction> {
        if !self.wm_overlay_visible() {
            return None;
        }
        let items = super::wm_menu_items(
            self.mouse_capture_enabled(),
            self.clipboard_enabled(),
            self.clipboard_available(),
        );
        if let Event::Mouse(mouse) = event
            && matches!(mouse.kind, MouseEventKind::Down(_))
        {
            if let Some(index) = self.panel.hit_test_menu_item(event) {
                let selected = index.min(items.len().saturating_sub(1));
                self.wm_menu_selected = selected;
                return items.get(selected).map(|item| item.action);
            }
            if self.panel.menu_icon_contains_point(mouse.column, mouse.row) {
                return Some(super::WmMenuAction::CloseMenu);
            }
            if !self.panel.menu_contains_point(mouse.column, mouse.row) {
                return Some(super::WmMenuAction::CloseMenu);
            }
        }
        let Event::Key(key) = event else {
            return None;
        };
        let kb = &self.keybindings;
        if kb.matches(crate::keybindings::Action::MenuUp, key)
            || kb.matches(crate::keybindings::Action::MenuPrev, key)
        {
            let total = items.len();
            if total > 0 {
                let current = self.wm_menu_selected;
                if current == 0 {
                    self.wm_menu_selected = total - 1;
                } else {
                    self.wm_menu_selected = current - 1;
                }
            }
            None
        } else if kb.matches(crate::keybindings::Action::MenuDown, key)
            || kb.matches(crate::keybindings::Action::MenuNext, key)
        {
            let total = items.len();
            if total > 0 {
                let current = self.wm_menu_selected;
                self.wm_menu_selected = (current + 1) % total;
            }
            None
        } else if kb.matches(crate::keybindings::Action::MenuSelect, key) {
            items.get(self.wm_menu_selected).map(|item| item.action)
        } else {
            None
        }
    }

    pub fn handle_exit_confirm_event(&mut self, event: &Event) -> Option<ConfirmAction> {
        let comp = self.overlays.get_mut(&OverlayId::ExitConfirm)?;
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
        let kb = &self.keybindings;
        kb.matches(crate::keybindings::Action::MenuUp, key)
            || kb.matches(crate::keybindings::Action::MenuDown, key)
            || kb.matches(crate::keybindings::Action::MenuSelect, key)
            || kb.matches(crate::keybindings::Action::MenuNext, key)
            || kb.matches(crate::keybindings::Action::MenuPrev, key)
    }

    pub fn render_overlays(&mut self, frame: &mut crate::ui::UiFrame<'_>) {
        use crate::components::ComponentContext;

        let (hovered, hovered_resize) = self.hover_targets();
        let obscuring: Vec<Rect> = self
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
                        rect: crate::layout::RectSpec::Absolute(Rect {
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

        let menu_items = super::wm_menu_items(
            self.mouse_capture_enabled(),
            self.clipboard_enabled(),
            self.clipboard_available(),
        );
        let menu_labels = menu_items
            .iter()
            .map(|item| (item.icon, item.label))
            .collect::<Vec<_>>();
        let bounds = frame.area();
        self.panel.render_menu(
            frame,
            self.wm_overlay_visible(),
            bounds,
            &menu_labels,
            self.wm_menu_selected,
        );
        self.panel.render_menu_backdrop(
            frame,
            self.wm_overlay_visible(),
            self.managed_area,
            self.panel.area(),
        );

        if let Some(confirm) = self.overlays.get_mut(&OverlayId::ExitConfirm) {
            confirm.render(
                frame,
                frame.area(),
                &ComponentContext::new(false).with_overlay(true),
            );
        }
        if let Some(help) = self.overlays.get_mut(&OverlayId::Help) {
            help.render(
                frame,
                frame.area(),
                &ComponentContext::new(false).with_overlay(true),
            );
        }
        if let Some(preview) = self.overlays.get_mut(&OverlayId::SelectionPreview) {
            preview.render(
                frame,
                frame.area(),
                &ComponentContext::new(false).with_overlay(true),
            );
        }
    }
}
