The multi-phase component lifecycle is an architectural design required to bridge the gap between **immediate-mode drawing** (where the entire UI is rebuilt from scratch on every single tick) and **retained-mode component state** (similar to React) within Rust's strict memory rules. 

To achieve this without fighting the borrow checker, the architecture strictly decouples state mutation from rendering by implementing a `Component` trait with five highly orchestrated phases:

1. **Initialization (`init`):** This executes exactly once when the component is mounted into the window tree, allowing it to load configurations, instantiate internal channels, or fetch initial data streams.
2. **Event Routing (`handle_events`):** Instead of mutating state directly, this phase evaluates raw hardware inputs (like keystrokes or mouse clicks), filtering out irrelevant events and returning a rigidly typed `Action` enum (e.g., `Action::CloseWindow`).
3. **State Mutation (`update`):** This is the exclusive location where the component's internal data structures can be modified, reacting strictly to the `Action` messages generated in the previous step. 
4. **Rendering (`render`):** Operating conceptually as a pure function, this phase translates the current frozen state of the component into drawing instructions applied to the `ratatui` frame.
5. **Destruction (`destroy`):** A formalized teardown hook used to safely close network sockets, flush localized log buffers, or save state right before the component is unmounted from the layout tree.

This strict **Command Query Separation** is heavily inspired by The Elm Architecture and is mathematically critical for navigating Rust's borrow checker. Because the central window manager collects all `Action` events into a centralized queue and applies them linearly, **only one specific component is borrowed mutably at any exact microsecond**. This completely sidesteps the overlapping mutable reference errors that plague complex UI graphs, ensuring total memory safety and fearless concurrency without needing to litter the codebase with expensive `Rc<RefCell<T>>` wrappers.
