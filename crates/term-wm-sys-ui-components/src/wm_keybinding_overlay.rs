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
use term_wm_ui_components::{DialogOverlayComponent, ListComponent, ScrollViewComponent};

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
        overlay.content.set_keyboard_enabled(true);
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
