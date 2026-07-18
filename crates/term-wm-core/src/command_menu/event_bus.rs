use super::arena::{CommandNode, ComponentId};
use crossbeam_channel::{Receiver, Sender, bounded};

pub enum CommandMenuEvent {
    RegisterCommands {
        owner: ComponentId,
        nodes: Vec<CommandNode>,
    },
    /// Drop all nodes owned by a terminated component.
    UnregisterOwner(ComponentId),
    /// Signal that the filtered/scored list is stale and must be rebuilt.
    InvalidateCache,
}

pub struct CommandMenuEventBus {
    tx: Sender<CommandMenuEvent>,
    rx: Receiver<CommandMenuEvent>,
}

impl Default for CommandMenuEventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandMenuEventBus {
    pub fn new() -> Self {
        let (tx, rx) = bounded(256);
        Self { tx, rx }
    }

    pub fn sender(&self) -> Sender<CommandMenuEvent> {
        self.tx.clone()
    }

    pub fn drain(&self) -> Vec<CommandMenuEvent> {
        self.rx.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::arena::{CommandAction, CommandName};
    use super::super::context::ContextMask;
    use super::*;
    use crate::actions::TermWmAction;

    fn make_test_node(stable_id: &str) -> CommandNode {
        CommandNode {
            stable_id: stable_id.to_string(),
            name: CommandName::Static(stable_id.to_string()),
            description: None,
            action: CommandAction::AppAction(TermWmAction::CloseMenu),
            icon: None,
            required_context: ContextMask::NONE,
            owner_id: Some(1),
        }
    }

    #[test]
    fn send_and_receive() {
        let bus = CommandMenuEventBus::new();
        let tx = bus.sender();
        tx.send(CommandMenuEvent::RegisterCommands {
            owner: 1,
            nodes: vec![make_test_node("test:a")],
        })
        .unwrap();

        let events = bus.drain();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn drain_returns_all_pending() {
        let bus = CommandMenuEventBus::new();
        let tx = bus.sender();
        tx.send(CommandMenuEvent::InvalidateCache).unwrap();
        tx.send(CommandMenuEvent::UnregisterOwner(42)).unwrap();

        let events = bus.drain();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn drain_empty_returns_nothing() {
        let bus = CommandMenuEventBus::new();
        assert!(bus.drain().is_empty());
    }
}
