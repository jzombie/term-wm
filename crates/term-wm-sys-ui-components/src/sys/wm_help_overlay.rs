use std::str;

use crossterm::event::Event;
use ratatui::layout::Rect;

use term_wm_core::components::{Component, ComponentContext, Overlay};
use term_wm_core::keybindings::{Action, KeyBindings};
use term_wm_core::ui::UiFrame;
use term_wm_ui_components::{MarkdownViewerComponent, ScrollViewComponent};

use crate::WmDialogOverlayComponent;

const HELP_CONTENT_BYTES: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/help.md"));

#[derive(Debug)]
pub struct WmHelpOverlayComponent {
    dialog: WmDialogOverlayComponent<MarkdownViewerComponent>,
}

impl Component for WmHelpOverlayComponent {
    fn resize(&mut self, area: Rect, _ctx: &ComponentContext) {
        self.dialog.dialog_mut().resize(area, _ctx);
    }

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, _ctx: &ComponentContext) {
        let title = format!("{} — About / Help", env!("CARGO_PKG_NAME"));
        self.dialog.render(frame, area, &title);
    }

    fn handle_event(&mut self, event: &Event, ctx: &ComponentContext) -> bool {
        self.dialog.handle_event(event, ctx)
    }
}

impl Overlay for WmHelpOverlayComponent {
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

impl WmHelpOverlayComponent {
    pub fn new(keybindings: KeyBindings) -> Self {
        let viewer = ScrollViewComponent::new(MarkdownViewerComponent::new());
        let mut dialog = WmDialogOverlayComponent::new(viewer, keybindings.clone(), Action::CloseHelp);
        dialog.dialog_mut().set_size(70, 20);
        if let Ok(raw) = str::from_utf8(HELP_CONTENT_BYTES) {
            let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
            let mut s = raw
                .replace("%PACKAGE%", env!("CARGO_PKG_NAME"))
                .replace("%VERSION%", env!("CARGO_PKG_VERSION"))
                .replace("%PLATFORM%", &platform)
                .replace("%REPOSITORY%", env!("CARGO_PKG_REPOSITORY"));

            let kb = &keybindings;
            let focus_next = kb.combos_for(Action::FocusNext).join(" / ");
            let focus_prev = kb.combos_for(Action::FocusPrev).join(" / ");
            let new_win = kb.combos_for(Action::NewWindow).join(" / ");
            let menu_nav = {
                let a = kb.combos_for(Action::MenuNext).join(" / ");
                let b = kb.combos_for(Action::MenuPrev).join(" / ");
                format!("{} / {}", a, b)
            };
            let menu_alt = {
                let a = kb.combos_for(Action::MenuUp).join(" / ");
                let b = kb.combos_for(Action::MenuDown).join(" / ");
                format!("{} / {}", a, b)
            };
            let select = kb.combos_for(Action::MenuSelect).join(" / ");
            let super_key = kb.combos_for(Action::WmToggleOverlay).join(" / ");
            let help_combo = kb.combos_for(Action::OpenHelp).join(" / ");
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
            dialog.content_mut().content.set_markdown(&s);
        }
        dialog.content_mut().content.set_link_handler_fn(|url| {
            let _ = webbrowser::open(url);
            true
        });
        Self { dialog }
    }

    pub fn show(&mut self) {
        self.dialog.show();
    }

    pub fn close(&mut self) {
        self.dialog.close();
        self.dialog.content_mut().content.reset();
    }

    pub fn visible(&self) -> bool {
        self.dialog.visible()
    }

    pub fn handle_help_event(&mut self, event: &Event, ctx: &ComponentContext) -> bool {
        self.dialog.handle_event(event, ctx)
    }

    pub fn set_keyboard_enabled(&mut self, enabled: bool) {
        self.dialog.content_mut().set_keyboard_enabled(enabled);
    }

    pub fn set_selection_enabled(&mut self, enabled: bool) {
        self.dialog.set_selection_enabled(enabled);
    }
}

impl Default for WmHelpOverlayComponent {
    fn default() -> Self {
        Self::new(KeyBindings::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;

    #[test]
    fn help_constructs() {
        let h = WmHelpOverlayComponent::new(KeyBindings::default());
        let _ = h;
    }

    #[test]
    fn placeholders_are_replaced_in_markdown() {
        let mut overlay = WmHelpOverlayComponent::new(KeyBindings::default());
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
            overlay.dialog.content_mut().render(&mut frame, area, &ComponentContext::new(true).with_overlay(true));
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
        let mut overlay = WmHelpOverlayComponent::new(KeyBindings::default());
        assert!(!overlay.visible(), "initially hidden");

        overlay.show();
        assert!(overlay.visible(), "visible after show");
        assert!(overlay.dialog.dialog_mut().visible(), "dialog visible after show");

        overlay.close();
        assert!(!overlay.visible(), "hidden after close");
        assert!(!overlay.dialog.dialog_mut().visible(), "dialog hidden after close");
    }

    #[test]
    fn handle_help_event_closes_on_close_key() {
        let mut overlay = WmHelpOverlayComponent::new(KeyBindings::default());
        overlay.show();
        let ev = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let handled = overlay.handle_help_event(&ev, &ComponentContext::new(true));
        assert!(handled, "close key should be handled");
        assert!(!overlay.visible(), "overlay should be closed by key");
    }

    #[test]
    fn clicking_outside_auto_closes_when_enabled() {
        let mut overlay = WmHelpOverlayComponent::new(KeyBindings::default());
        overlay.dialog.dialog_mut().set_auto_close_on_outside_click(true);
        overlay.show();

        let _area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };

        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });

        let handled = overlay.dialog.handle_event(&ev, &ComponentContext::new(true));
        assert!(
            handled,
            "outside click should be handled when auto-close enabled"
        );
        assert!(
            !overlay.visible(),
            "overlay should be closed by outside click"
        );
    }
}
