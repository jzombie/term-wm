use std::str;

use crossterm::event::Event;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Clear};

use crate::components::{Component, DialogOverlayComponent, MarkdownViewerComponent};
use crate::keybindings::{Action, KeyBindings};
use crate::ui::UiFrame;

static HELP_MD: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/help.md"));

#[derive(Debug)]
pub struct HelpOverlayComponent {
    dialog: DialogOverlayComponent,
    visible: bool,
    viewer: MarkdownViewerComponent,
    area: Rect,
}

impl Component for HelpOverlayComponent {
    fn resize(&mut self, area: Rect) {
        self.area = area;
    }

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, _focused: bool) {
        self.area = area;
        if !self.visible || area.width == 0 || area.height == 0 {
            return;
        }
        // If the dialog requests a dim backdrop, apply it across the full frame
        // before clearing and drawing the help dialog contents.
        self.dialog.render_backdrop(frame, area);
        let rect = self.dialog.rect_for(area);
        frame.render_widget(Clear, rect);
        let title = format!("{} — About / Help", env!("CARGO_PKG_NAME"));
        let block = Block::default().title(title).borders(Borders::ALL);
        let inner = Rect {
            x: rect.x.saturating_add(1),
            y: rect.y.saturating_add(1),
            width: rect.width.saturating_sub(2),
            height: rect.height.saturating_sub(2),
        };
        frame.render_widget(block, rect);
        self.viewer.render_content(frame, inner);
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        self.handle_help_event_in_area(event, self.area)
    }
}

impl HelpOverlayComponent {
    pub fn handle_help_event_in_area(&mut self, event: &Event, area: Rect) -> bool {
        if !self.visible {
            return false;
        }
        match event {
            Event::Key(key) => {
                let kb = KeyBindings::default();
                if kb.matches(crate::keybindings::Action::CloseHelp, &key) {
                    self.close();
                    true
                } else {
                    self.viewer.handle_key_event(key)
                }
            }
            Event::Mouse(_) => {
                // If configured, allow clicking outside the dialog to auto-close it.
                if self.dialog.handle_click_outside(event, area) {
                    self.close();
                    return true;
                }
                let rect = self.dialog.rect_for(area);
                let inner = Rect {
                    x: rect.x.saturating_add(1),
                    y: rect.y.saturating_add(1),
                    width: rect.width.saturating_sub(2),
                    height: rect.height.saturating_sub(2),
                };
                self.viewer.handle_pointer_event_in_area(event, inner)
            }
            _ => false,
        }
    }
}

impl HelpOverlayComponent {
    pub fn new() -> Self {
        let mut overlay = Self {
            dialog: DialogOverlayComponent::new(),
            visible: false,
            viewer: MarkdownViewerComponent::new(),
            area: Rect::default(),
        };
        overlay.dialog.set_size(70, 20);
        overlay.dialog.set_dim_backdrop(true);
        // allow clicking outside the help dialog to auto-close it
        overlay.dialog.set_auto_close_on_outside_click(true);
        overlay.dialog.set_bg(crate::theme::dialog_bg());
        // substitute package/version placeholders and set markdown
        if let Ok(raw) = str::from_utf8(HELP_MD) {
            // Build a compile-time platform string (OS/ARCH) to indicate the
            // target the binary was built for.
            // Use std::env::consts (which reflect the compilation target) to
            // build a concise platform identifier.
            let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
            let mut s = raw
                .replace("%PACKAGE%", env!("CARGO_PKG_NAME"))
                .replace("%VERSION%", env!("CARGO_PKG_VERSION"))
                .replace("%PLATFORM%", &platform)
                .replace("%REPOSITORY%", env!("CARGO_PKG_REPOSITORY"));
            // Replace placeholder tokens that allow `assets/help.md` to
            // contain the descriptive text while only key combo strings are
            // produced here. This keeps the markdown authoritative and
            // avoids hardcoding user-visible sentences in code.
            let kb = KeyBindings::default();
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
            let help_combo = kb.combos_for(Action::OpenHelp).join(" / ");
            // If no combo is configured for `OpenHelp` we prefer the
            // literal 'Help menu' label in the markdown so no empty
            // placeholder appears in the rendered help.
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
                .replace("%HELP_MENU%", &help_label);
            overlay.viewer.set_markdown(&s);
        }
        overlay.viewer.set_link_handler_fn(|url| {
            let _ = webbrowser::open(url);
            true
        });
        overlay
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.viewer.set_keyboard_enabled(true);
        self.dialog.set_visible(true);
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.viewer.set_keyboard_enabled(false);
        self.dialog.set_visible(false);
        self.viewer.reset();
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn handle_help_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(key) => {
                let kb = KeyBindings::default();
                if kb.matches(crate::keybindings::Action::CloseHelp, &key) {
                    self.close();
                    true
                } else {
                    self.viewer.handle_key_event(key)
                }
            }
            Event::Mouse(_) => self.viewer.handle_pointer_event(event),
            _ => false,
        }
    }

    /// Manually set keyboard handling for the underlying viewer.
    pub fn set_keyboard_enabled(&mut self, enabled: bool) {
        self.viewer.set_keyboard_enabled(enabled);
    }
}

impl Default for HelpOverlayComponent {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_constructs() {
        let h = HelpOverlayComponent::new();
        // should create without panic
        let _ = h;
    }
}
