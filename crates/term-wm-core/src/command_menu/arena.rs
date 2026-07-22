use super::context::ContextMask;
use crate::actions::TermWmAction;
use slotmap::new_key_type;

new_key_type! {
    pub struct CommandNodeId;
}

pub type ComponentId = u64;

/// Declarative command name: supports passive state binding without mutation.
/// During the data_dirty cache rebuild, `Toggle` variants are evaluated against
/// the current ContextMask and formatted into the display name (e.g. "ON"/"OFF").
/// The arena payload itself is never mutated.
pub enum CommandName {
    Static(String),
    Toggle {
        base: String,
        active_flag: ContextMask,
        on_suffix: &'static str,
        off_suffix: &'static str,
    },
}

impl CommandName {
    pub fn format(&self, current_mask: ContextMask) -> String {
        match self {
            CommandName::Static(s) => s.clone(),
            CommandName::Toggle {
                base,
                active_flag,
                on_suffix,
                off_suffix,
            } => {
                let suffix = if current_mask.contains(*active_flag) {
                    on_suffix
                } else {
                    off_suffix
                };
                format!("{base}{suffix}")
            }
        }
    }
}

/// Flat command action — no submenus, no closures; pure data only.
/// Dynamic child panes should use `TermWmAction::ComponentDispatch`
/// and route through the central event loop.
pub enum CommandAction {
    AppAction(TermWmAction),
}

pub struct CommandNode {
    /// Permanent semantic ID, e.g. "core:split_pane", "git_pane:commit".
    /// Independent of allocation lifecycle. MRU keys on this.
    pub stable_id: String,
    pub name: CommandName,
    pub description: Option<String>,
    pub action: CommandAction,
    pub icon: Option<&'static str>,
    pub required_context: ContextMask,
    pub owner_id: Option<ComponentId>,
    pub disabled: bool,
}

impl CommandNode {
    pub fn into_action(self) -> TermWmAction {
        match self.action {
            CommandAction::AppAction(action) => action,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_name_formats_to_string() {
        let name = CommandName::Static("New Window".to_string());
        assert_eq!(name.format(ContextMask::NONE), "New Window");
    }

    #[test]
    fn toggle_name_on_when_flag_set() {
        let name = CommandName::Toggle {
            base: "Mouse Capture: ".to_string(),
            active_flag: ContextMask::MOUSE_CAPTURE,
            on_suffix: "On",
            off_suffix: "Off",
        };
        assert_eq!(name.format(ContextMask::MOUSE_CAPTURE), "Mouse Capture: On");
    }

    #[test]
    fn toggle_name_off_when_flag_unset() {
        let name = CommandName::Toggle {
            base: "Mouse Capture: ".to_string(),
            active_flag: ContextMask::MOUSE_CAPTURE,
            on_suffix: "On",
            off_suffix: "Off",
        };
        assert_eq!(name.format(ContextMask::NONE), "Mouse Capture: Off");
    }

    #[test]
    fn command_node_into_action_returns_inner() {
        let node = CommandNode {
            stable_id: "test:a".to_string(),
            name: CommandName::Static("A".to_string()),
            description: None,
            action: CommandAction::AppAction(TermWmAction::CloseMenu),
            icon: None,
            required_context: ContextMask::NONE,
            owner_id: None,
            disabled: false,
        };
        let action = node.into_action();
        assert!(matches!(action, TermWmAction::CloseMenu));
    }
}
