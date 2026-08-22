# term-wm-view

A declarative `view!` [proc-macro](https://doc.rust-lang.org/reference/procedural-macros.html) for building [term-wm](https://crates.io/crates/term-wm) component trees.

Expands to fully-monomorphized Rust that a developer could hand-write: container constructors,
leaf component constructors, and generated delegate enums for heterogeneous sibling sets. The
result is a value implementing `Component` — there is no runtime tree, no reactivity, no
reconciliation, no `Box`/`dyn`.

## Tiers

1. **Layout-primitive tags**: `<VStack>`/`<Column>`, `<HStack>`/`<Row>`, `<Center width height>`,
   `<Grid cols rows>` (constraint strings parsed at compile time into `GridConstraint`s).
2. **Built-in component tags**: `<Label text>`, `<Button label action|onClick>`.
3. **Expression braces** `{ expr }`: any expression yielding a `Component` (owned, or `&mut C` via
   the blanket impl) — no registry needed for third-party or fallible components.

See the main [term-wm](https://crates.io/crates/term-wm) crate for documentation.
