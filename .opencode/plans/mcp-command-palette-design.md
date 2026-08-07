# MCP Integration & Command Palette Redesign

## Design Brainstorm (July 2026)

### Overview

Integrate the [Model Context Protocol (MCP)](https://modelcontextprotocol.io) into term-wm so that AI agents can programmatically interact with the window manager — create/manipulate windows, run commands, read terminal output, etc.

**Core insight:** The Command Palette and MCP are two front-ends to the *same* action system. The Command Registry (`CommandRegistry` + `CommandNode`) becomes the single source of truth. Registering an action in the palette automatically makes it available to remote MCP agents.

### Architecture

```
Agent A ──┐
Agent B ──┼── MCP stdio ──▶ Edge Crate ──▶ muxio IPC ──▶ term-wm Core
Agent C ──┘
```

- **Edge Crate** (`term-wm-mcp`): A thin binary. Each agent spawns its own via MCP stdio (1:1 process per agent). Connects back to the core over muxio IPC. Translates MCP JSON-RPC ↔ bitcode-encoded Rust types.
- **Core-side muxio server**: An IPC endpoint hosted in the WM process that accepts connections from edge processes. Handles command dispatch and state queries.
- **No daemon**: No persistent server beyond the core's muxio listener. If the core isn't running, edges fail to connect and exit.
- **Independent from `term-session-*`**: New crate with its own muxio service definitions. No reuse of `term-session-server` or `term-session-client`. Different socket, different RPC methods.

### The Unified Command System

#### Current State

The project already has rich command infrastructure:

- `CommandNode` (`command_menu/arena.rs:53`): `stable_id`, `name`, `description`, `icon`, `context_mask`, `action` (`CommandAction::AppAction(TermWmAction)`)
- `CommandRegistry` (`command_menu/command_registry.rs`): SlotMap-based registry with `register()`, `register_batch()`, `drop_owner()`, `build_item_list()`
- `CommandMenuEventBus` (`command_menu/event_bus.rs`): Channel-based dynamic registration (exists but unused)
- `TermWmAction` (`actions.rs:37`): Flat enum with typed variants (`NewWindow`, `CloseWindow(WindowKey)`, `Scroll(isize)`, etc.)
- `dispatch_action()` (`runner.rs:70`): Single match dispatcher for all actions
- `CommandPaletteComponent` (`ui-components/src/command_palette.rs`): Fuzzy-searchable UI overlay
- `WmCommandPaletteComponent` (`sys-ui-components/src/wm_command_palette.rs`): WM wrapper with dialog backdrop, populates registry dynamically from `wm_menu_items()`

#### Target State

`CommandRegistry` becomes the single source of truth. The palette and MCP are just front-ends:

```
User input  → WmCommandPaletteComponent ──┐
                                          ├──→ CommandRegistry → dispatch_action() → WM internals
MCP Agent  → MCP Edge ────────────────────┘
```

`CommandNode` gains optional parameter schema support:

```rust
pub struct CommandNode {
    pub stable_id: String,
    pub name: CommandName,
    pub description: Option<String>,
    pub icon: Option<&'static str>,
    pub required_context: ContextMask,
    pub owner_id: Option<ComponentId>,
    pub disabled: bool,
    // NEW:
    pub param_type: Option<TypeId>,          // for MCP schema derivation
    pub action: CommandAction,               // reworked (see open questions)
}
```

The edge generates JSON Schema at the MCP boundary from the `param_type` via `#[derive(JsonSchema)]` (using `schemars`). The core never touches JSON — params are passed over muxio as bitcode-encoded Rust structs.

### Wire Protocol

**Edge ↔ Core (muxio IPC):**
- Transport: Unix socket (or Windows named pipe)
- Serialization: `bitcode`
- Service definition in a new crate (e.g., `term-wm-mcp-service-definitions`)
- RPC methods (tentative):
  - `ListCommands` → list all registered commands with their schema info
  - `ExecuteCommand { stable_id, params: Vec<u8> }` → execute a command with bitcode-encoded params
  - `ListWindows` → list windows with titles/sizes/PIDs
  - `ReadOutput { window_id, lines: u16 }` → get terminal contents
  - `SubscribeEvents` → push notification stream for state changes (window created/closed, etc.)

**Agent ↔ Edge (MCP stdio):**
- Transport: stdin/stdout (MCP stdio transport)
- Protocol: JSON-RPC 2.0 (MCP spec)
- `params` in JSON-RPC are translated to/from the `#[derive(JsonSchema)]` on the Rust param type
- Edge binary: `term-wm mcp` — spawned by the MCP client (Claude Desktop, etc.)

### The Edge Crate (`term-wm-mcp`)

Responsibilities:
1. Connect to the core's muxio IPC listener
2. Listen for MCP stdio messages from the agent
3. On agent `tools/list` → call `ListCommands` IPC, derive JSON Schema from `param_type` via `schemars`, return tool list
4. On agent `tools/call` → validate JSON params, bitcode-encode them into `Vec<u8>`, call `ExecuteCommand` IPC, return result
5. On agent `resources/read` → call `ListWindows` / `ReadOutput` IPC

No persistent state. Short-lived per-agent process.

### The Core-Side Muxio Server

Responsibilities:
1. Listen on a well-known socket path
2. Accept connections from edge processes
3. Serve `ListCommands` from the `CommandRegistry` (owned by `WindowManager`)
4. Serve `ExecuteCommand` by looking up the `CommandNode`, resolving params to a `TermWmAction`, and calling `dispatch_action()`
5. Serve `ListWindows` / `ReadOutput` by querying `WindowManager` state

Hosted in the WM process — wired up during `AppBuilder` / `main.rs` initialization, similar to how the session server is spawned.

### Open Questions / Undecided

#### 1. How do parameterized commands map to `TermWmAction`?

**Option A — Richer enum variants:** Grow `TermWmAction` variants with optional fields. E.g.:
```rust
pub enum TermWmAction {
    NewWindow(NewWindowParams),
    CloseWindow(WindowKey),
    // ...
}

#[derive(Encode, Decode, JsonSchema)]
pub struct NewWindowParams {
    command: Option<String>,
    cwd: Option<String>,
}
```
Palette adds `NewWindow` with default params. MCP can pass full params. Fully typed dispatch. Compiler catches exhaustiveness. Requires touching the enum for new parameterized commands.

**Option B — Generic params carrier:** More dynamic, less type-safe. E.g. a `HashMap<String, Vec<u8>>` alongside the action variant. Harder to maintain but avoids changing the enum.

**Current leaning: Option A.** Type safety is worth the verbosity. The number of param-bearing commands is small. Follows the existing pattern (variants already carry `WindowKey`, `isize`, `String`, etc.).

#### 2. Do existing param-bearing `TermWmAction` variants (`CloseWindow(WindowKey)`, `Scroll(isize)`, etc.) get `CommandNode` parameter schemas for MCP exposure, or only new commands?

If yes, the palette may show commands that prompt for runtime parameters. If no, there's a distinction between "palette actions" (simple, no interactive params) and "MCP tools" (parametrized, full schema).

#### 3. Where does `CommandMenuEventBus` get wired up?

The event bus exists for dynamic registration (plugins, panes registering their own commands). Unused today. MCP integration may be the impetus to wire it in — letting running panes register MCP-exposed actions dynamically.

#### 4. Return values from commands

MCP `tools/call` expects a response. The palette is fire-and-forget. What does `ExecuteCommand` return?
- `CreateWindow` → the new `WindowKey`
- `ToggleMonocle` → status `{ monocle: "on" | "off" }`
- `CloseWindow` → `{ success: true }`

The dispatch path needs to support responses, not just enqueue actions. Two approaches:
- **Sync**: `ExecuteCommand` blocks on a oneshot channel while `dispatch_action()` sends the result back
- **Async**: `ExecuteCommand` returns immediately with a `request_id`, response comes as a separate push notification

#### 5. Edge process lifecycle

How is the socket path communicated to the edge binary? (argv flag? env var? well-known path under XDG_RUNTIME_DIR?)

#### 6. How does `CommandAction` evolve?

Currently just `AppAction(TermWmAction)`. If we add handler functions or param types, does it grow to:
```rust
pub enum CommandAction {
    AppAction(TermWmAction),
    Handler(Box<dyn FnOnce(&[u8]) -> Result<TermWmAction>>),
}
```
Or do handlers live elsewhere?

### Integration Touchpoints

- **`CommandNode`** (arena.rs): Add `param_type: Option<TypeId>`
- **`CommandAction`** (arena.rs): May grow beyond `AppAction(TermWmAction)` if handler functions are needed
- **`CommandRegistry`** (command_registry.rs): Likely unchanged — already generic enough
- **`TermWmAction`** (actions.rs): Grow variants with struct-typed params (per Option A)
- **`dispatch_action()`** (runner.rs): May need to return values for MCP response
- **`WindowManager`**: Needs to host the muxio server + serve command/window queries
- **`AppBuilder`**: Wire up muxio server at startup
- **`main.rs`**: Add `mcp` subcommand for the edge binary
- **New crates**:
  - `term-wm-mcp-service-definitions` — RPC method types (bitcode)
  - `term-wm-mcp` — edge binary + core-side muxio server logic (or split further)

### Crate Structure (Tentative)

```
crates/
├── term-wm-mcp-service-definitions/     # RPC types shared by core + edge
│   └── src/lib.rs                       # service trait, request/response types
├── term-wm-mcp/                         # edge binary + optional lib
│   ├── src/
│   │   ├── main.rs                      # "term-wm mcp" entry point
│   │   ├── edge.rs                      # muxio client side (connects to core)
│   │   ├── mcp_handler.rs               # translates MCP JSON-RPC ↔ muxio IPC
│   │   └── json_schema.rs               # derives JSON Schema from TypeId
│   └── Cargo.toml
├── term-wm-core/                        # existing: gains muxio server host
│   └── src/window/window_manager/
│       └── mcp_server.rs               # new: muxio server, ListCommands, ExecuteCommand
└── term-wm-sys-ui-components/           # existing: WmCommandPaletteComponent
    └── src/wm_command_palette.rs        # gains awareness of param_type for UI
```

(Where the line falls between the new crate and term-wm-core is TBD. The muxio server could live in the new crate and call into core, or live in core — follows the same pattern as the existing session server.)

### Verification

- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test`
- Manual: spawn `term-wm mcp` as an MCP subprocess, verify `tools/list` returns the expected commands, `tools/call` executes them, `resources/read` returns data
- Multi-agent: connect two edges simultaneously, verify both can list and execute
- Edge-case: start edge without core running → clean error exit
