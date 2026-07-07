use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use crossterm::event::Event;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Clear};

use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::app_context::AppContext;
use term_wm_core::components::{Component, ComponentContext, Overlay};
use term_wm_core::keybindings::{Category, KeyBindings};
use term_wm_core::ui::UiFrame;
use term_wm_core::window::WindowKey;
use term_wm_ui_components::{
    DialogOverlayComponent, ListComponent, ScrollKeyMode, ScrollViewComponent,
};

pub struct WmKeybindingOverlayComponent {
    dialog: DialogOverlayComponent,
    content: ScrollViewComponent<ListComponent>,
    area: Cell<Rect>,
    keybindings: KeyBindings,
    app_ctx: Arc<AppContext>,
}

impl WmKeybindingOverlayComponent {
    pub fn new(app_ctx: &Arc<AppContext>, keybindings: KeyBindings) -> Self {
        let mut dialog = DialogOverlayComponent::new();
        dialog.set_dim_backdrop(true);
        dialog.set_auto_close_on_outside_click(true);
        dialog.set_bg(term_wm_core::theme::NOIR.dialog_bg);
        dialog.set_size(60, 80);
        let list = ScrollViewComponent::new(ListComponent::new(String::new()));
        let mut overlay = Self {
            dialog,
            content: list,
            area: Cell::new(Rect::default()),
            keybindings,
            app_ctx: Arc::clone(app_ctx),
        };
        overlay.build_entries();
        overlay.content.set_keyboard_mode(ScrollKeyMode::Full);
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
        self.content.content.borrow_mut().set_items(lines);
    }

    pub fn show(&mut self) {
        self.dialog.set_visible(true);
        self.build_entries();
    }

    pub fn close(&mut self) {
        self.dialog.set_visible(false);
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

impl Component<TermWmAction> for WmKeybindingOverlayComponent {
    fn render(
        &self,
        frame: &mut UiFrame<'_>,
        area: Rect,
        _ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        if !self.dialog.visible() || area.width == 0 || area.height == 0 {
            return;
        }
        let title = format!("{} — Keybindings", self.app_ctx.app_name);
        self.dialog.render_backdrop(frame, area, None);
        let rect = self.dialog.rect_for(area);
        frame.render_widget(Clear, rect);
        let block = Block::default().title(title.as_str()).borders(Borders::ALL);
        let inner = Rect {
            x: rect.x.saturating_add(1),
            y: rect.y.saturating_add(1),
            width: rect.width.saturating_sub(2),
            height: rect.height.saturating_sub(2),
        };
        frame.render_widget(block, rect);
        let ctx = ComponentContext::new(true).with_overlay(true);
        self.content.render(frame, inner, &ctx, registry);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        if !self.dialog.visible() {
            return EventResult::Ignored;
        }
        match event {
            Event::Key(key) => {
                if self.keybindings.matches(TermWmAction::CloseHelp, key) {
                    self.close();
                    EventResult::Consumed
                } else {
                    self.content.handle_events(event, ctx)
                }
            }
            Event::Mouse(_) => {
                if self.dialog.handle_click_outside(event, self.area.get()) {
                    self.close();
                    return EventResult::Consumed;
                }
                self.content.handle_events(event, ctx)
            }
            _ => EventResult::Ignored,
        }
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        self.content.update(action, ctx, actions);
    }

    fn destroy(&mut self) {}
}

impl Overlay<TermWmAction> for WmKeybindingOverlayComponent {
    fn visible(&self) -> bool {
        self.dialog.visible()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;

    fn make_overlay() -> WmKeybindingOverlayComponent {
        let app_ctx = Arc::new(AppContext::new("test", "0.0.0"));
        let kb = KeyBindings::default();
        WmKeybindingOverlayComponent::new(&app_ctx, kb)
    }

    #[test]
    fn keybinding_overlay_initially_hidden() {
        let overlay = make_overlay();
        assert!(!overlay.visible());
    }

    #[test]
    fn keybinding_overlay_show_hides() {
        let mut overlay = make_overlay();
        overlay.show();
        assert!(overlay.visible());
        overlay.close();
        assert!(!overlay.visible());
    }

    #[test]
    fn keybinding_overlay_render_when_hidden_is_noop() {
        let overlay = make_overlay();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 24));
        let mut frame = UiFrame::from_parts(Rect::new(0, 0, 80, 24), &mut buffer);
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        overlay.render(&mut frame, Rect::new(0, 0, 80, 24), &ctx, &mut registry);
    }

    #[test]
    fn keybinding_overlay_handle_events_when_hidden_returns_ignored() {
        let mut overlay = make_overlay();
        let ctx = ComponentContext::new(true);
        let key = Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        let result = overlay.handle_events(&key, &ctx);
        assert!(result.is_ignored());
    }

    #[test]
    fn keybinding_overlay_close_on_esc() {
        let mut overlay = make_overlay();
        overlay.show();
        assert!(overlay.visible());
        let ctx = ComponentContext::new(true);
        let key = Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        let result = overlay.handle_events(&key, &ctx);
        assert!(result.is_consumed());
        assert!(!overlay.visible());
    }

    #[test]
    fn keybinding_overlay_debug_fmt() {
        let overlay = make_overlay();
        let fmt = format!("{:?}", overlay);
        assert!(fmt.contains("WmKeybindingOverlayComponent"));
    }

    #[test]
    fn keybinding_overlay_build_entries_populates_content() {
        let mut overlay = make_overlay();
        overlay.show();
        let text = overlay.content.content.borrow();
        // The list should have entries from default keybindings
        assert!(!text.items().is_empty());
    }
}
