use std::cell::Cell;
use std::collections::VecDeque;
use std::str;
use std::sync::Arc;

use crossterm::event::Event;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Clear};

use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::app_context::AppContext;
use term_wm_core::components::{Component, ComponentContext, Overlay, SelectionStatus};
use term_wm_core::keybindings::KeyBindings;
use term_wm_core::ui::UiFrame;
use term_wm_core::window::WindowKey;
use term_wm_ui_components::{
    DialogOverlayComponent, MarkdownViewerComponent, ScrollKeyMode, ScrollViewComponent,
};

const HELP_CONTENT_BYTES: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/help.md"));

#[derive(Debug)]
pub struct WmHelpOverlayComponent {
    dialog: DialogOverlayComponent,
    content: ScrollViewComponent<MarkdownViewerComponent>,
    area: Cell<Rect>,
    keybindings: KeyBindings,
    app_ctx: Arc<AppContext>,
}

impl Component<TermWmAction> for WmHelpOverlayComponent {
    fn render(
        &self,
        frame: &mut UiFrame<'_>,
        area: Rect,
        _ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        self.area.set(area);
        if !self.dialog.visible() || area.width == 0 || area.height == 0 {
            return;
        }
        let title = format!("{} — About / Help", self.app_ctx.app_name);
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
                let area = self.area.get();
                let rect = self.dialog.rect_for(area);
                let inner = ratatui::layout::Rect {
                    x: rect.x.saturating_add(1),
                    y: rect.y.saturating_add(1),
                    width: rect.width.saturating_sub(2),
                    height: rect.height.saturating_sub(2),
                };
                let ctx = ctx.with_screen_area(inner);
                self.content.handle_events(event, &ctx)
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

    fn selection_status(&self) -> SelectionStatus {
        self.content.selection_status()
    }

    fn selection_text(&self) -> Option<String> {
        self.content.selection_text()
    }
}

impl Overlay<TermWmAction> for WmHelpOverlayComponent {
    fn visible(&self) -> bool {
        self.dialog.visible()
    }

    fn shadow_rect(&self, area: Rect) -> Option<Rect> {
        if !self.dialog.visible() {
            return None;
        }
        Some(self.dialog.rect_for(area))
    }
}

impl WmHelpOverlayComponent {
    pub fn new(app_ctx: &Arc<AppContext>, keybindings: KeyBindings) -> Self {
        let mut dialog = DialogOverlayComponent::new();
        dialog.set_dim_backdrop(true);
        dialog.set_auto_close_on_outside_click(true);
        dialog.set_bg(term_wm_core::theme::NOIR.dialog_bg);
        dialog.set_size(70, 20);
        let viewer = ScrollViewComponent::new(MarkdownViewerComponent::new());
        let mut overlay = Self {
            dialog,
            content: viewer,
            area: Cell::new(Rect::default()),
            keybindings,
            app_ctx: Arc::clone(app_ctx),
        };
        if let Ok(raw) = str::from_utf8(HELP_CONTENT_BYTES) {
            let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
            let kb = &overlay.keybindings;
            let mut s = raw
                .replace("%PACKAGE%", &overlay.app_ctx.app_name)
                .replace("%VERSION%", &overlay.app_ctx.app_version)
                .replace("%PLATFORM%", &platform)
                .replace("%REPOSITORY%", env!("CARGO_PKG_REPOSITORY"));

            let focus_next = kb.combos_for(TermWmAction::FocusNext).join(" / ");
            let focus_prev = kb.combos_for(TermWmAction::FocusPrev).join(" / ");
            let new_win = kb.combos_for(TermWmAction::NewWindow).join(" / ");
            let menu_nav = {
                let a = kb.combos_for(TermWmAction::MenuNext).join(" / ");
                let b = kb.combos_for(TermWmAction::MenuPrev).join(" / ");
                format!("{} / {}", a, b)
            };
            let menu_alt = {
                let a = kb.combos_for(TermWmAction::MenuUp).join(" / ");
                let b = kb.combos_for(TermWmAction::MenuDown).join(" / ");
                format!("{} / {}", a, b)
            };
            let select = kb.combos_for(TermWmAction::MenuSelect).join(" / ");
            let super_key = kb.combos_for(TermWmAction::WmToggleOverlay).join(" / ");
            let help_combo = kb.combos_for(TermWmAction::OpenHelp).join(" / ");
            let help_label = if help_combo.is_empty() {
                "Help menu".to_string()
            } else {
                help_combo
            };

            s = s
                .replace("%FOCUS_NEXT%", &focus_next)
                .replace("%FOCUS_PREV%", &focus_prev)
                .replace("%NEW_WINDOW%", &new_win)
                .replace("%MENU_NAV%", &menu_nav)
                .replace("%MENU_ALT%", &menu_alt)
                .replace("%MENU_SELECT%", &select)
                .replace("%SUPER%", &super_key)
                .replace("%HELP_MENU%", &help_label);
            overlay
                .content
                .content
                .borrow_mut()
                .set_markdown(&s, &term_wm_core::theme::NOIR);
        }
        overlay
            .content
            .content
            .borrow_mut()
            .set_link_handler_fn(|url| {
                let _ = webbrowser::open(url);
                true
            });
        overlay.content.set_keyboard_mode(ScrollKeyMode::Full);
        overlay
    }

    pub fn show(&mut self) {
        self.dialog.set_visible(true);
    }

    pub fn close(&mut self) {
        self.dialog.set_visible(false);
        self.content.content.borrow_mut().reset();
    }

    pub fn visible(&self) -> bool {
        self.dialog.visible()
    }

    pub fn set_keyboard_mode(&mut self, mode: ScrollKeyMode) {
        self.content.set_keyboard_mode(mode);
    }

    pub fn set_selection_enabled(&mut self, enabled: bool) {
        self.content
            .content
            .borrow_mut()
            .set_selection_enabled(enabled);
    }
}

impl Default for WmHelpOverlayComponent {
    fn default() -> Self {
        Self::new(
            &Arc::new(AppContext::new("unknown", "0.0.0")),
            KeyBindings::default(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;

    #[test]
    fn help_constructs() {
        let h = WmHelpOverlayComponent::new(
            &Arc::new(AppContext::new("test", "0.0.0")),
            KeyBindings::default(),
        );
        let _ = h;
    }

    #[test]
    fn placeholders_are_replaced_in_markdown() {
        let overlay = WmHelpOverlayComponent::new(
            &Arc::new(AppContext::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            )),
            KeyBindings::default(),
        );
        use ratatui::buffer::Buffer;

        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let mut buffer = Buffer::empty(area);
        {
            let mut frame = term_wm_core::ui::UiFrame::from_parts(area, &mut buffer);
            overlay.content.render(
                &mut frame,
                area,
                &ComponentContext::new(true).with_overlay(true),
                &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
            );
        }

        let mut joined = String::new();
        for y in 0..area.height {
            let mut row = String::new();
            for x in 0..area.width {
                if let Some(cell) = buffer.cell((x, y)) {
                    row.push_str(cell.symbol());
                }
            }
            joined.push_str(&row);
            joined.push('\n');
        }
        let joined = joined.to_lowercase();

        let pkg = env!("CARGO_PKG_NAME").to_lowercase();
        assert!(
            joined.contains(&pkg),
            "markdown should include package name"
        );
        let ver = env!("CARGO_PKG_VERSION").to_lowercase();
        assert!(
            joined.contains(&ver),
            "markdown should include package version"
        );
    }

    #[test]
    fn show_and_close_toggle_visibility() {
        let mut overlay = WmHelpOverlayComponent::new(
            &Arc::new(AppContext::new("test", "0.0.0")),
            KeyBindings::default(),
        );
        assert!(!overlay.visible(), "initially hidden");

        overlay.show();
        assert!(overlay.visible(), "visible after show");

        overlay.close();
        assert!(!overlay.visible(), "hidden after close");
    }

    #[test]
    fn handle_help_event_closes_on_close_key() {
        let mut overlay = WmHelpOverlayComponent::new(
            &Arc::new(AppContext::new("test", "0.0.0")),
            KeyBindings::default(),
        );
        overlay.show();
        let ev = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let result = overlay.handle_events(&ev, &ComponentContext::new(true));
        assert!(!result.is_ignored(), "close key should be handled");
        assert!(!overlay.visible(), "overlay should be closed by key");
    }

    #[test]
    fn clicking_outside_auto_closes_when_enabled() {
        let mut overlay = WmHelpOverlayComponent::new(
            &Arc::new(AppContext::new("test", "0.0.0")),
            KeyBindings::default(),
        );
        overlay.dialog.set_auto_close_on_outside_click(true);
        overlay.show();

        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        overlay.area.set(area);

        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });

        let result = overlay.handle_events(&ev, &ComponentContext::new(true));
        assert!(
            !result.is_ignored(),
            "outside click should be handled when auto-close enabled"
        );
        assert!(
            !overlay.visible(),
            "overlay should be closed by outside click"
        );
    }
}
