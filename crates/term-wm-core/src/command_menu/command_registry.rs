use super::arena::{CommandNode, CommandNodeId, ComponentId};
use super::context::ContextMask;
use slotmap::SlotMap;

pub struct CommandRegistry {
    arena: SlotMap<CommandNodeId, CommandNode>,
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self {
            arena: SlotMap::with_key(),
        }
    }

    pub fn register(&mut self, node: CommandNode) -> CommandNodeId {
        self.arena.insert(node)
    }

    /// Register multiple nodes owned by the same component.
    /// Returns the assigned NodeIds.
    pub fn register_batch(&mut self, nodes: Vec<CommandNode>) -> Vec<CommandNodeId> {
        nodes.into_iter().map(|n| self.arena.insert(n)).collect()
    }

    /// Drop all nodes owned by the given ComponentId.
    /// Returns the removed NodeIds for caller to use (e.g. for MRU pruning if needed).
    pub fn drop_owner(&mut self, owner_id: ComponentId) -> Vec<CommandNodeId> {
        let mut removed = Vec::new();
        self.arena.retain(|id, node| {
            if node.owner_id == Some(owner_id) {
                removed.push(id);
                false
            } else {
                true
            }
        });
        removed
    }

    /// Get a reference to the underlying arena (for looking up stable_id during reorder).
    pub fn arena(&self) -> &SlotMap<CommandNodeId, CommandNode> {
        &self.arena
    }

    /// Get a node by its CommandNodeId.
    pub fn get(&self, id: CommandNodeId) -> Option<&CommandNode> {
        self.arena.get(id)
    }

    /// Flatten valid nodes into a display list, filtered by context_mask.
    pub fn build_item_list(&self, context_mask: ContextMask) -> Vec<CommandNodeId> {
        self.arena
            .iter()
            .filter(|(_, node)| (context_mask & node.required_context) == node.required_context)
            .map(|(id, _)| id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::arena::{CommandAction, CommandName};
    use super::*;
    use crate::actions::TermWmAction;

    fn make_node(stable_id: &str, required_context: ContextMask) -> CommandNode {
        CommandNode {
            stable_id: stable_id.to_string(),
            name: CommandName::Static(stable_id.to_string()),
            description: None,
            action: CommandAction::AppAction(TermWmAction::CloseMenu),
            icon: None,
            required_context,
            owner_id: None,
            disabled: false,
        }
    }

    #[test]
    fn register_and_get() {
        let mut reg = CommandRegistry::new();
        let id = reg.register(make_node("test:node", ContextMask::NONE));
        assert!(reg.get(id).is_some());
        assert_eq!(reg.get(id).unwrap().stable_id, "test:node");
    }

    #[test]
    fn drop_owner_removes_owned_nodes() {
        let mut reg = CommandRegistry::new();
        let mut n1 = make_node("core:a", ContextMask::NONE);
        n1.owner_id = Some(42);
        let mut n2 = make_node("core:b", ContextMask::NONE);
        n2.owner_id = Some(42);
        let n3 = make_node("core:c", ContextMask::NONE); // owner_id = None

        reg.register(n1);
        reg.register(n2);
        reg.register(n3);

        let removed = reg.drop_owner(42);
        assert_eq!(removed.len(), 2);
        assert_eq!(reg.arena.len(), 1);
    }

    #[test]
    fn build_item_list_filters_by_context() {
        let mut reg = CommandRegistry::new();
        reg.register(make_node("always", ContextMask::NONE));
        reg.register(make_node("needs_focus", ContextMask::HAS_FOCUS));
        reg.register(make_node("needs_split", ContextMask::CAN_SPLIT));

        let items = reg.build_item_list(ContextMask::HAS_FOCUS | ContextMask::CAN_SPLIT);
        assert_eq!(items.len(), 3);

        let items = reg.build_item_list(ContextMask::HAS_FOCUS);
        assert_eq!(items.len(), 2);
    }
}
