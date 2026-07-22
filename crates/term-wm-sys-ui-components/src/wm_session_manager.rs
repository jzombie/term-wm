use ratatui::style::{Color, Modifier, Style};
use term_wm_core::events::MouseButton;
use term_wm_layout_engine::LayoutRect;

use term_wm_core::{
    actions::{EventResult, TermWmAction},
    components::{Component, ComponentContext},
    hitbox_registry::{HitboxId, HitboxRegistry},
    window::WindowKey,
};
use term_wm_ui_components::helpers::{downcast_ratatui, layout_rect_to_clipped_rect};

/// Entry in the session manager list.
#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub key: WindowKey,
    pub title: String,
    pub working_dir: String,
    pub is_active: bool,
}

/// Session Manager overlay component.
/// Shows a list of all open windows/sessions with tap targets for switching.
#[derive(Debug)]
pub struct WmSessionManagerComponent {
    visible: bool,
    sessions: Vec<SessionEntry>,
    window_key: Option<WindowKey>,
    hitbox_id: HitboxId,
}

impl WmSessionManagerComponent {
    pub fn new() -> Self {
        Self {
            visible: false,
            sessions: Vec::new(),
            window_key: None,
            hitbox_id: HitboxId::new(),
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn set_sessions(&mut self, sessions: Vec<SessionEntry>) {
        self.sessions = sessions;
    }

    pub fn sessions(&self) -> &[SessionEntry] {
        &self.sessions
    }
}

impl Default for WmSessionManagerComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component<TermWmAction> for WmSessionManagerComponent {
    fn on_mount(&mut self, key: WindowKey, _app: &term_wm_core::app_context::AppContext) {
        self.window_key = Some(key);
    }

    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut HitboxRegistry,
    ) {
        if !self.visible || self.sessions.is_empty() {
            return;
        }

        let screen_area = ctx.screen_area().unwrap_or(area);
        let ratatui_backend = downcast_ratatui(backend);
        let buffer = &mut ratatui_backend.buffer;
        let ratatui_area = layout_rect_to_clipped_rect(screen_area);
        let bounds = ratatui_area.intersection(buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }

        // Clear background
        for yy in bounds.y..bounds.y.saturating_add(bounds.height) {
            for xx in bounds.x..bounds.x.saturating_add(bounds.width) {
                if let Some(cell) = buffer.cell_mut((xx, yy)) {
                    cell.set_symbol(" ")
                        .set_style(Style::default().bg(Color::Black));
                }
            }
        }

        // Render session list with tap targets
        for (i, entry) in self.sessions.iter().enumerate() {
            let row_y = bounds.y + i as u16;
            if row_y >= bounds.y + bounds.height {
                break;
            }

            let row_rect = LayoutRect {
                x: i32::from(bounds.x),
                y: i32::from(row_y),
                width: bounds.width,
                height: 1,
            };

            // Register each row for hit-testing
            if let Some(key) = self.window_key {
                registry.register(
                    self.hitbox_id,
                    term_wm_core::hitbox_registry::ComponentOwner::Window(key),
                    row_rect,
                );
            }

            // Render title with activity indicator
            let indicator = if entry.is_active { "●" } else { "○" };
            let style = if entry.is_active {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let title = format!("{} {}", indicator, entry.title);
            let title_width = title.chars().count() as u16;
            let max_width = bounds.width.saturating_sub(2);
            let display_title = if title_width > max_width {
                format!("{}...", &title[..max_width.saturating_sub(3) as usize])
            } else {
                title
            };

            for (j, ch) in display_title.chars().enumerate() {
                let xx = bounds.x + j as u16;
                if xx >= bounds.x + bounds.width {
                    break;
                }
                if let Some(cell) = buffer.cell_mut((xx, row_y)) {
                    let mut buf = [0u8; 4];
                    let sym = ch.encode_utf8(&mut buf);
                    cell.set_symbol(sym).set_style(style);
                }
            }
        }
    }

    fn on_mouse_press(
        &mut self,
        _local_x: u16,
        local_y: u16,
        _button: MouseButton,
        _modifiers: term_wm_core::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        // Calculate which session was tapped based on local_y
        let index = local_y as usize;
        if index < self.sessions.len() {
            let entry = &self.sessions[index];
            return EventResult::Action(TermWmAction::FocusWindow(entry.key));
        }
        EventResult::Ignored
    }

    fn update(
        &mut self,
        _action: TermWmAction,
        _ctx: &ComponentContext,
        _actions: &mut std::collections::VecDeque<(WindowKey, TermWmAction)>,
    ) {
    }

    fn hitbox_id(&self) -> Option<HitboxId> {
        Some(self.hitbox_id)
    }

    fn destroy(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use term_wm_core::components::Component;
    use term_wm_core::events::{KeyModifiers, MouseButton};

    fn make_backend(w: u16, h: u16) -> term_wm_console::RatatuiBackend {
        let buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, w, h));
        term_wm_console::RatatuiBackend::new(buf, ratatui::layout::Rect::new(0, 0, w, h))
    }

    #[test]
    fn session_manager_new_is_not_visible() {
        let sm = WmSessionManagerComponent::new();
        assert!(!sm.visible());
    }

    #[test]
    fn session_manager_set_visible_toggles() {
        let mut sm = WmSessionManagerComponent::new();
        sm.set_visible(true);
        assert!(sm.visible());
        sm.set_visible(false);
        assert!(!sm.visible());
    }

    #[test]
    fn session_manager_default_is_not_visible() {
        let sm = WmSessionManagerComponent::default();
        assert!(!sm.visible());
    }

    #[test]
    fn session_manager_set_sessions() {
        let mut sm = WmSessionManagerComponent::new();
        let sessions = vec![
            SessionEntry {
                key: WindowKey::default(),
                title: "bash".to_string(),
                working_dir: "/home".to_string(),
                is_active: true,
            },
            SessionEntry {
                key: WindowKey::default(),
                title: "vim".to_string(),
                working_dir: "/tmp".to_string(),
                is_active: false,
            },
        ];
        sm.set_sessions(sessions);
        assert_eq!(sm.sessions().len(), 2);
    }

    #[test]
    fn render_when_not_visible_does_nothing() {
        let mut sm = WmSessionManagerComponent::new();
        sm.set_sessions(vec![SessionEntry {
            key: WindowKey::default(),
            title: "bash".to_string(),
            working_dir: "/home".to_string(),
            is_active: true,
        }]);
        let mut backend = make_backend(40, 10);
        let mut reg = HitboxRegistry::new();
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        };
        sm.render(&mut backend, area, &ComponentContext::default(), &mut reg);
        assert!(reg.is_empty());
    }

    #[test]
    fn render_when_empty_sessions_does_nothing() {
        let mut sm = WmSessionManagerComponent::new();
        sm.set_visible(true);
        let mut backend = make_backend(40, 10);
        let mut reg = HitboxRegistry::new();
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        };
        sm.render(&mut backend, area, &ComponentContext::default(), &mut reg);
        assert!(reg.is_empty());
    }

    #[test]
    fn render_visible_with_sessions_registers_hitbox() {
        use term_wm_core::hitbox_registry::ComponentOwner;
        let mut sm = WmSessionManagerComponent::new();
        sm.set_visible(true);
        let key = WindowKey::default();
        sm.set_sessions(vec![SessionEntry {
            key,
            title: "bash".to_string(),
            working_dir: "/home".to_string(),
            is_active: true,
        }]);
        let mut backend = make_backend(40, 10);
        let mut reg = HitboxRegistry::with_owner(ComponentOwner::Window(key));
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        };
        // Need to mount first so window_key is set
        sm.on_mount(
            key,
            &term_wm_core::app_context::AppContext::new("test", "0.1"),
        );
        sm.render(&mut backend, area, &ComponentContext::default(), &mut reg);
        assert!(!reg.is_empty());
    }

    #[test]
    fn on_mouse_press_within_bounds_returns_focus_action() {
        let mut sm = WmSessionManagerComponent::new();
        let key = WindowKey::default();
        sm.set_sessions(vec![SessionEntry {
            key,
            title: "bash".to_string(),
            working_dir: "/home".to_string(),
            is_active: true,
        }]);
        let ctx = ComponentContext::default();
        let result = sm.on_mouse_press(0, 0, MouseButton::Left, KeyModifiers::NONE, &ctx);
        assert!(matches!(result, EventResult::Action(TermWmAction::FocusWindow(k)) if k == key));
    }

    #[test]
    fn on_mouse_press_out_of_bounds_ignores() {
        let mut sm = WmSessionManagerComponent::new();
        sm.set_sessions(vec![SessionEntry {
            key: WindowKey::default(),
            title: "bash".to_string(),
            working_dir: "/home".to_string(),
            is_active: true,
        }]);
        let ctx = ComponentContext::default();
        let result = sm.on_mouse_press(0, 5, MouseButton::Left, KeyModifiers::NONE, &ctx);
        assert!(matches!(result, EventResult::Ignored));
    }

    #[test]
    fn hitbox_id_returns_some() {
        let sm = WmSessionManagerComponent::new();
        assert!(sm.hitbox_id().is_some());
    }
}
