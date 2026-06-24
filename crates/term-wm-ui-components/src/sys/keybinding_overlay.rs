use std::collections::BTreeMap;

use crossterm::event::Event;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Clear};

use crate::{DialogOverlayComponent, ListComponent, ScrollViewComponent};
use term_wm_core::components::{Component, ComponentContext, Overlay};
use term_wm_core::keybindings::{Action, Category, KeyBindings};
use term_wm_core::ui::UiFrame;

pub struct KeybindingOverlayComponent {
    dialog: DialogOverlayComponent,
    visible: bool,
    list: ScrollViewComponent<ListComponent>,
    area: Rect,
    keybindings: KeyBindings,
}

impl KeybindingOverlayComponent {
    pub fn new(keybindings: KeyBindings) -> Self {
        let mut list = ScrollViewComponent::new(ListComponent::new(String::new()));
        list.set_keyboard_enabled(true);
        let mut dialog = DialogOverlayComponent::new();
        dialog.set_size(60, 80);
        dialog.set_dim_backdrop(true);
        dialog.set_auto_close_on_outside_click(true);
        dialog.set_bg(term_wm_core::theme::dialog_bg());
        let mut overlay = Self {
            dialog,
            visible: false,
            list,
            area: Rect::default(),
            keybindings,
        };
        overlay.build_entries();
        overlay
    }

    fn build_entries(&mut self) {
        let mut by_category: BTreeMap<Category, Vec<String>> = BTreeMap::new();
        for (action, combos) in self.keybindings.map() {
            let combo_str = combos
                .iter()
                .map(|c| c.display())
                .collect::<Vec<_>>()
                .join(" / ");
            let entry = format!("  {:30}  {}", action, combo_str);
            by_category
                .entry(action.category())
                .or_default()
                .push(entry);
        }

        let mut lines = Vec::new();
        for (cat, entries) in &by_category {
            let header = match cat {
                Category::System => " System",
                Category::Navigation => " Navigation",
                Category::Windows => " Windows & Panes",
                Category::Scrolling => " Scrolling",
                Category::Dialogs => " Dialogs & Menus",
                Category::Selection => " Selection",
            };
            lines.push(header.to_string());
            lines.push("".to_string());
            for entry in entries {
                lines.push(entry.clone());
            }
            lines.push("".to_string());
        }
        self.list.content.set_items(lines);
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.list.set_keyboard_enabled(true);
        self.dialog.set_visible(true);
        // Rebuild entries to reflect any configuration changes
        self.build_entries();
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.list.set_keyboard_enabled(false);
        self.dialog.set_visible(false);
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    fn render_content(&mut self, frame: &mut UiFrame<'_>, rect: Rect) {
        self.list
            .resize(rect, &ComponentContext::new(true).with_overlay(true));
        self.list
            .render(frame, rect, &ComponentContext::new(true).with_overlay(true));
    }
}

impl std::fmt::Debug for KeybindingOverlayComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeybindingOverlayComponent")
            .field("visible", &self.visible)
            .field("area", &self.area)
            .finish()
    }
}

impl Component for KeybindingOverlayComponent {
    fn resize(&mut self, area: Rect, _ctx: &ComponentContext) {
        self.area = area;
    }

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, _ctx: &ComponentContext) {
        self.area = area;
        if !self.visible || area.width == 0 || area.height == 0 {
            return;
        }
        self.dialog.render_backdrop(frame, area);
        let rect = self.dialog.rect_for(area);
        frame.render_widget(Clear, rect);
        let title = format!("{} — Keybindings", env!("CARGO_PKG_NAME"));
        let block = Block::default().title(title).borders(Borders::ALL);
        let inner = Rect {
            x: rect.x.saturating_add(1),
            y: rect.y.saturating_add(1),
            width: rect.width.saturating_sub(2),
            height: rect.height.saturating_sub(2),
        };
        frame.render_widget(block, rect);
        self.render_content(frame, inner);
    }

    fn handle_event(&mut self, event: &Event, ctx: &ComponentContext) -> bool {
        if !self.visible {
            return false;
        }
        match event {
            Event::Key(key) => {
                if self.keybindings.matches(Action::CloseHelp, key) {
                    self.close();
                    true
                } else {
                    self.list.handle_event(event, ctx)
                }
            }
            Event::Mouse(_) => {
                if self.dialog.handle_click_outside(event, self.area) {
                    self.close();
                    return true;
                }
                self.list.handle_event(event, ctx)
            }
            _ => false,
        }
    }
}

impl Overlay for KeybindingOverlayComponent {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn visible(&self) -> bool {
        self.visible
    }
}
