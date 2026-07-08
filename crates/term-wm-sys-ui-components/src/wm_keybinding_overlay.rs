use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use ratatui::widgets::{Block, Borders, Clear};
use term_wm_core::events::Event;
use term_wm_layout_engine::LayoutRect;

use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::app_context::AppContext;
use term_wm_core::components::{Component, ComponentContext, Overlay};
use term_wm_core::keybindings::{Category, KeyBindings};
use term_wm_core::window::WindowKey;
use term_wm_ui_components::helpers::layout_rect_to_rect;
use term_wm_ui_components::{
    DialogOverlayComponent, ListComponent, ScrollKeyMode, ScrollViewComponent,
};

pub struct WmKeybindingOverlayComponent {
    dialog: DialogOverlayComponent,
    content: ScrollViewComponent<ListComponent>,
    area: Cell<LayoutRect>,
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
            area: Cell::new(LayoutRect::default()),
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
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        _ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        if !self.dialog.visible() || area.width == 0 || area.height == 0 {
            return;
        }
        let title = format!("{} \u{2014} Keybindings", self.app_ctx.app_name);
        self.dialog.render_backdrop(backend, area, None);
        let ratatui_area = layout_rect_to_rect(area);
        let rect = self.dialog.rect_for(ratatui_area);
        {
            let backend = term_wm_ui_components::helpers::downcast_ratatui(backend);
            let buffer = &mut backend.buffer;
            use ratatui::widgets::Widget;
            Clear.render(rect, buffer);
            let block = Block::default().title(title.as_str()).borders(Borders::ALL);
            block.render(rect, buffer);
        }
        let inner_layout = LayoutRect {
            x: i32::from(rect.x).saturating_add(1),
            y: i32::from(rect.y).saturating_add(1),
            width: rect.width.saturating_sub(2),
            height: rect.height.saturating_sub(2),
        };
        let ctx = ComponentContext::new(true).with_overlay(true);
        self.content.render(backend, inner_layout, &ctx, registry);
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
                let area = self.area.get();
                let ratatui_area = layout_rect_to_rect(area);
                if self.dialog.handle_click_outside(event, ratatui_area) {
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
        let mut overlay = make_overlay();
        let buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 80, 24));
        let mut backend =
            term_wm_console::RatatuiBackend::new(buffer, ratatui::layout::Rect::new(0, 0, 80, 24));
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        overlay.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
            &ctx,
            &mut registry,
        );
    }

    #[test]
    fn keybinding_overlay_handle_events_when_hidden_returns_ignored() {
        let mut overlay = make_overlay();
        let ctx = ComponentContext::new(true);
        let key = Event::Key(term_wm_core::events::KeyEvent {
            code: term_wm_core::events::KeyCode::Esc,
            modifiers: term_wm_core::events::KeyModifiers::NONE,
            kind: term_wm_core::events::KeyKind::Press,
        });
        let result = overlay.handle_events(&key, &ctx);
        assert!(result.is_ignored());
    }

    #[test]
    fn keybinding_overlay_close_on_esc() {
        let mut overlay = make_overlay();
        overlay.show();
        assert!(overlay.visible());
        let ctx = ComponentContext::new(true);
        let key = Event::Key(term_wm_core::events::KeyEvent {
            code: term_wm_core::events::KeyCode::Esc,
            modifiers: term_wm_core::events::KeyModifiers::NONE,
            kind: term_wm_core::events::KeyKind::Press,
        });
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
