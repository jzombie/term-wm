use std::collections::BTreeMap;

use crossterm::event::Event;
use ratatui::layout::Rect;

use term_wm_core::components::{Component, ComponentContext, Overlay};
use term_wm_core::keybindings::{Action, Category, KeyBindings};
use term_wm_core::ui::UiFrame;
use term_wm_ui_components::{ListComponent, ScrollViewComponent};

use crate::WmDialogOverlayComponent;

pub struct WmKeybindingOverlayComponent {
    dialog: WmDialogOverlayComponent<ListComponent>,
    keybindings: KeyBindings,
}

impl WmKeybindingOverlayComponent {
    pub fn new(keybindings: KeyBindings) -> Self {
        let list = ScrollViewComponent::new(ListComponent::new(String::new()));
        let mut dialog =
            WmDialogOverlayComponent::new(list, keybindings.clone(), Action::CloseHelp);
        dialog.dialog_mut().set_size(60, 80);
        let mut overlay = Self {
            dialog,
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
                Category::System => "System",
                Category::Navigation => "Navigation",
                Category::Windows => "Windows & Panes",
                Category::Scrolling => "Scrolling",
                Category::Menu => "Menus",
                Category::Dialogs => "Dialogs",
                Category::Selection => "Selection",
            };
            lines.push(header.to_string());
            lines.push("".to_string());
            for entry in entries {
                lines.push(entry.clone());
            }
            lines.push("".to_string());
        }
        self.dialog.content_mut().content.set_items(lines);
    }

    pub fn show(&mut self) {
        self.dialog.show();
        self.build_entries();
    }

    pub fn close(&mut self) {
        self.dialog.close();
    }

    pub fn visible(&self) -> bool {
        self.dialog.visible()
    }
}

impl std::fmt::Debug for WmKeybindingOverlayComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WmKeybindingOverlayComponent")
            .field("visible", &self.visible())
            .finish()
    }
}

impl Component for WmKeybindingOverlayComponent {
    fn resize(&mut self, area: Rect, _ctx: &ComponentContext) {
        self.dialog.dialog_mut().resize(area, _ctx);
    }

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, _ctx: &ComponentContext) {
        let title = format!("{} — Keybindings", env!("CARGO_PKG_NAME"));
        self.dialog.render(frame, area, &title);
    }

    fn handle_event(&mut self, event: &Event, ctx: &ComponentContext) -> bool {
        self.dialog.handle_event(event, ctx)
    }
}

impl Overlay for WmKeybindingOverlayComponent {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn visible(&self) -> bool {
        self.dialog.visible()
    }
}
