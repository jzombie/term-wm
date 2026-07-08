# Double-Buffered View Model Projection Architecture (Revised)

## Executive Summary

This plan introduces a double-buffered view model projection architecture to `term-wm`, separating layout computation from rendering while maintaining zero-allocation, 60+ FPS performance. The core engine projects only spatial geometry (a `DrawPlan`), and the app layer invokes `Component::render()` with an opaque `RenderBackend` that UI crates downcast to concrete backends (like Ratatui's `Buffer`).

**Key Achievement**: After this refactor, `term-wm-core` will have zero dependencies on Ratatui, Crossterm, or portable-pty. The core compiles without any graphical libraries or terminal abstractions. UI crates downcast the opaque backend when they need to render, and the app layer translates crossterm events to core-owned event types at the boundary.

## Root Problem (from TODO)

> "I don't think that runner.rs or core should be rendering directly, nor have duplicated rendering logic. Somehow the core needs to be able to signal to the app that we are going to render something, not directly render itself."

This TODO identifies the fundamental architectural issue: **the core engine currently owns rendering logic**, creating tight coupling between the mathematical layout engine and the ratatui rendering backend. The solution:

- **Core engine** → produces a `DrawPlan` describing *where* to render (spatial geometry only)
- **App layer** → consumes the `DrawPlan`, retrieves components, and invokes *how* to render

## Architecture Principles

1. **Core never touches ratatui/crossterm/portable-pty types** — only `WindowKey`, `Rect` (from layout engine), z-index, and core-owned event types
2. **Components own their rendering and input handling** — `Component::render()` receives an opaque `RenderBackend` + `Rect`; `Component::handle_event()` receives core-owned `Event`
3. **Opaque backend with downcasting** — `RenderBackend: Any` enables UI crates to downcast to concrete backends
4. **Abstract event pipeline** — Core defines `EventSource` trait; app layer implements concrete `CrosstermEventSource`
5. **Trait objects at compositor boundary** — `dyn RenderTarget` for runtime flexibility
6. **Native Vec amortization** — `Vec::with_capacity(256)` + `clear()` for zero-allocation steady-state
7. **True backend independence** — Core compiles without Ratatui/Crossterm/portable-pty; UI crates downcast when needed
8. **PTY encapsulation** — `TerminalComponent` encapsulates PTY internally; core has zero knowledge of PTYs
9. **Correct dependency graph** — `term-wm-console` defines `RatatuiBackend`; both UI components and app depend on it
10. **No crossterm in UI components** — UI components use only core-owned `Event` types, never crossterm directly

---

## Current Architecture Analysis

### Crate Structure
```
term-wm (binary + lib)
├── term-wm-core              # Core engine, Component trait, WindowManager, IO, layout
│   ├── term-wm-layout-engine # Pure math (BSP tiling, floating rects, regions)
│   ├── term-wm-pty-engine    # PTY abstraction, vt100, clipboard
│   └── term-wm-render        # Opaque RenderBackend trait (no concrete implementations)
├── term-wm-console           # Concrete RatatuiBackend (implements RenderBackend)
│   ├── term-wm-core          # For Rect (from layout engine), RenderBackend trait
│   └── ratatui               # Buffer, Rect, Frame, Widget traits
├── term-wm-ui-components     # Reusable UI components (Terminal, ScrollView, etc.)
│   ├── term-wm-core          # For Component trait, Rect (from layout engine), Event types
│   ├── term-wm-console       # For downcasting to RatatuiBackend
│   └── ratatui               # For Buffer, Rect (used after downcast in render methods)
├── term-wm-sys-ui-components # WM chrome (panels, debug log, help, menu overlay)
│   ├── term-wm-core
│   ├── term-wm-ui-components
│   ├── term-wm-console
│   └── ratatui               # For Buffer, Rect (used after downcast in render methods)
└── term-wm (root)            # Application binary, CrosstermEventSource, TermWmApp
    ├── term-wm-core
    ├── term-wm-ui-components
    ├── term-wm-sys-ui-components
    ├── term-wm-console
    └── crossterm              # For CrosstermEventSource translation
```

**Key Insight**: `term-wm-core` has ZERO dependencies on ratatui, crossterm, or portable-pty. UI components DO have ratatui as a dependency — they need it to perform buffer mutations after downcasting `RenderBackend` to `RatatuiBackend`. The difference is that UI components don't import crossterm (they use core-owned `Event` types), and they don't define backend traits (they use `term-wm-render`'s `RenderBackend`).

### Key Findings
1. **Pure math already exists**: `term-wm-layout-engine` has zero ratatui/crossterm dependencies
2. **Core has heavy ratatui ties**: `term-wm-core` directly uses `ratatui::buffer::Buffer`, `ratatui::layout::Rect`, `ratatui::Frame`, and widget traits
3. **Per-window offscreen compositing exists**: `composite_window` in `runner.rs` already creates scratch `Buffer` per window
4. **Component trait is render-safe**: `Component::render(&self, ...)` takes immutable reference - ideal for double-buffering
5. **HitboxRegistry is per-frame**: Already rebuilt every frame, not persistent

### How This Architecture Solves the TODO Problem

The TODO identifies that `runner.rs` and `WindowManager` currently contain rendering logic (e.g., `render_empty_message()`, `render_panel()`, `render_overlays()`). This creates:

1. **Duplicated rendering logic** - rendering logic exists in both `runner.rs` and `WindowManager`
2. **Tight coupling** - core depends on ratatui/crossterm types, making it impossible to render or handle events without a terminal
3. **No headless testing** - tests must mock `UiFrame` because rendering is baked into core

The view model projection architecture solves this by:

| Current State | After Refactor |
|---------------|----------------|
| `WindowManager::render_empty_message()` writes directly to `UiFrame` | `CoreEngine::project_draw_plan()` returns `RenderRegion` with `WindowKey` |
| `runner.rs::draw_window_app()` orchestrates rendering | `runner.rs::draw_window_app()` queries draw plan, passes to `DrawPlanRenderer` |
| `composite_window()` creates scratch `Buffer` | `DrawPlanRenderer` uses swap-based rendering (zero-allocation) |
| `Component::render(&self, frame: &mut UiFrame, area: Rect, ...)` | `Component::render(&self, backend: &mut dyn RenderBackend, area: Rect, ...)` |
| `Component::handle_event(&mut self, event: &crossterm::event::Event, ...)` | `Component::handle_event(&mut self, event: &Event, ...)` (core-owned types) |
| `term-wm-core` depends on ratatui/crossterm | `term-wm-core` has zero ratatui/crossterm dependencies |
| Tests need `Buffer::empty() + UiFrame::from_parts` | Tests assert on `Rect` coordinates (no UI dependencies) |

**Correct Dependency Graph**:
```
term-wm-core
  ├── term-wm-layout-engine (pure math)
  └── term-wm-render (opaque traits)

term-wm-console
  ├── term-wm-core (for Rect from layout engine, RenderBackend)
  └── ratatui (for Buffer, Rect)

term-wm-ui-components
  ├── term-wm-core (for Component, Event types)
  └── term-wm-console (for downcasting to RatatuiBackend)

term-wm-app
  ├── term-wm-core
  ├── term-wm-ui-components
  ├── term-wm-console
  └── crossterm (for CrosstermEventSource)
```

The key insight: **the core never touches ratatui, crossterm, or portable-pty types**. It produces `DrawPlan` with `RenderRegion` structs and consumes core-owned `Event` types. The app layer translates crossterm events to core events at the boundary, and `term-wm-console` provides the concrete backend accessible to both UI components and the app.

### Current Rendering Flow
```
EventLoop::run()
  └── idle tick → DRAW PATH
      ├── wm.begin_frame()           # Clear per-frame state
      ├── wm.prepare_draw()          # Reset RegionMap, handles, hitbox registry
      └── output.draw(|frame|)       # ratatui::Terminal::draw()
          └── draw_window_app(frame)
              ├── wm.register_managed_layout(area)  # Compute tiled regions
              ├── wm.window_draw_plan(frame)        # Build Vec<DrawTask>
              ├── FOR EACH DrawTask::App(window):
              │   └── composite_window(frame, ...)
              │       ├── Allocate scratch Buffer (reused)
              │       ├── UiFrame::from_parts(local_area, &mut buffer)
              │       ├── decorator.render_window(&mut offscreen)  # Chrome
              │       ├── render_content(&mut offscreen)           # Component
              │       ├── frame.blit_from_signed(&buffer, dest)   # Blit to main
              │       └── Return scratch cells
              ├── wm.render_panel(frame)      # Top + bottom panels
              └── wm.render_overlays(frame)   # Menus, help, confirm
```

---

## Implementation Plan

### Phase 1: Workspace and Dependency Segregation

**Goal**: Sever all graphical library ties from the core mathematical engine.

#### 1.0 Abstract the Event Pipeline (Input Boundary)

**New file**: `crates/term-wm-core/src/events.rs`

**Purpose**: Core-owned event types that are independent of crossterm. App layer translates crossterm events to these core types.

```rust
// crates/term-wm-core/src/events.rs

/// Core-owned keyboard event (no crossterm dependency)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
    pub kind: KeyKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Esc,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
    // ... other keys
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Press,
    Repeat,
    Release,
}

/// Core-owned mouse event (no crossterm dependency)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub column: u16,
    pub row: u16,
    pub modifiers: KeyModifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    Press(MouseButton),
    Release(MouseButton),
    Drag(MouseButton),
    Moved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Core-owned event enum (no crossterm dependency)
#[derive(Debug, Clone)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),
    FocusGained,
    FocusLost,
    // ... other events
}

/// EventResult from component handling
#[derive(Debug)]
pub enum EventResult<Msg> {
    /// No action needed
    None,
    /// Component produced a message
    Msg(Msg),
    /// Request focus change
    Focus(WindowKey),
    /// Request action
    Action(ComponentAction),
}
```

**New file**: `crates/term-wm-core/src/io/event_source.rs`

**Purpose**: Core-owned `EventSource` trait (no crossterm dependency).

```rust
// crates/term-wm-core/src/io/event_source.rs

use crate::events::Event;
use std::io;

/// Abstraction over input sources.
/// Core defines this trait; app layer implements concrete sources.
pub trait EventSource {
    /// Poll for events without blocking.
    fn poll(&mut self, timeout: std::time::Duration) -> io::Result<bool>;
    
    /// Read the next event (blocking).
    fn read(&mut self) -> io::Result<Event>;
    
    /// Enable/disable mouse capture.
    fn set_mouse_capture(&mut self, enable: bool) -> io::Result<()>;
    
    /// Get current input profile.
    fn current_profile(&self) -> &str;
}
```

**Files to modify**:
- `crates/term-wm-core/src/components/mod.rs` — Update `Component::handle_events` signature
- `crates/term-wm-core/src/window/window_manager/mod.rs` — Update `dispatch_mouse`, `handle_key_event`
- `crates/term-wm-core/src/event_loop.rs` — Update event loop to use core types

**New file**: `crates/term-wm-app/src/crossterm_event_source.rs`

**Purpose**: Concrete `EventSource` implementation that translates crossterm events to core events. Lives in the app crate, NOT in core.

```rust
// crates/term-wm-app/src/crossterm_event_source.rs

use term_wm_core::events::{Event, KeyEvent, MouseEvent, KeyCode, KeyModifiers, KeyKind, MouseEventKind, MouseButton};
use term_wm_core::io::EventSource;
use std::io;

/// Concrete EventSource implementation for crossterm.
/// This is the ONLY place where crossterm events are translated to core events.
pub struct CrosstermEventSource {
    // crossterm-specific state
}

impl EventSource for CrosstermEventSource {
    fn poll(&mut self, timeout: std::time::Duration) -> io::Result<bool> {
        crossterm::event::poll(timeout)
    }
    
    fn read(&mut self) -> io::Result<Event> {
        let crossterm_event = crossterm::event::read()?;
        Ok(self.translate_event(crossterm_event))
    }
    
    fn set_mouse_capture(&mut self, enable: bool) -> io::Result<()> {
        if enable {
            crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
        } else {
            crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)?;
        }
        Ok(())
    }
    
    fn current_profile(&self) -> &str {
        "crossterm"
    }
}

impl CrosstermEventSource {
    fn translate_event(&self, event: crossterm::event::Event) -> Event {
        match event {
            crossterm::event::Event::Key(key) => {
                Event::Key(self.translate_key(key))
            }
            crossterm::event::Event::Mouse(mouse) => {
                Event::Mouse(self.translate_mouse(mouse))
            }
            crossterm::event::Event::Resize(w, h) => {
                Event::Resize(w, h)
            }
            crossterm::event::Event::FocusGained => Event::FocusGained,
            crossterm::event::Event::FocusLost => Event::FocusLost,
            _ => Event::Resize(0, 0), // Default for unknown events
        }
    }
    
    fn translate_key(&self, key: crossterm::event::KeyEvent) -> KeyEvent {
        KeyEvent {
            code: self.translate_key_code(key.code),
            modifiers: KeyModifiers {
                shift: key.modifiers.contains(crossterm::event::KeyModifiers::SHIFT),
                control: key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL),
                alt: key.modifiers.contains(crossterm::event::KeyModifiers::ALT),
            },
            kind: match key.kind {
                crossterm::event::KeyEventKind::Press => KeyKind::Press,
                crossterm::event::KeyEventKind::Repeat => KeyKind::Repeat,
                crossterm::event::KeyEventKind::Release => KeyKind::Release,
            },
        }
    }
    
    fn translate_key_code(&self, code: crossterm::event::KeyCode) -> KeyCode {
        match code {
            crossterm::event::KeyCode::Char(c) => KeyCode::Char(c),
            crossterm::event::KeyCode::Enter => KeyCode::Enter,
            crossterm::event::KeyCode::Tab => KeyCode::Tab,
            crossterm::event::KeyCode::Backspace => KeyCode::Backspace,
            crossterm::event::KeyCode::Esc => KeyCode::Esc,
            crossterm::event::KeyCode::Left => KeyCode::Left,
            crossterm::event::KeyCode::Right => KeyCode::Right,
            crossterm::event::KeyCode::Up => KeyCode::Up,
            crossterm::event::KeyCode::Down => KeyCode::Down,
            crossterm::event::KeyCode::Home => KeyCode::Home,
            crossterm::event::KeyCode::End => KeyCode::End,
            crossterm::event::KeyCode::PageUp => KeyCode::PageUp,
            crossterm::event::KeyCode::PageDown => KeyCode::PageDown,
            crossterm::event::KeyCode::F(n) => KeyCode::F(n),
            // ... other keys
        }
    }
    
    fn translate_mouse(&self, mouse: crossterm::event::MouseEvent) -> MouseEvent {
        MouseEvent {
            kind: match mouse.kind {
                crossterm::event::MouseEventKind::Down(btn) => {
                    MouseEventKind::Press(self.translate_button(btn))
                }
                crossterm::event::MouseEventKind::Up(btn) => {
                    MouseEventKind::Release(self.translate_button(btn))
                }
                crossterm::event::MouseEventKind::Drag(btn) => {
                    MouseEventKind::Drag(self.translate_button(btn))
                }
                crossterm::event::MouseEventKind::Moved => MouseEventKind::Moved,
            },
            column: mouse.column,
            row: mouse.row,
            modifiers: KeyModifiers {
                shift: mouse.modifiers.contains(crossterm::event::KeyModifiers::SHIFT),
                control: mouse.modifiers.contains(crossterm::event::KeyModifiers::CONTROL),
                alt: mouse.modifiers.contains(crossterm::event::KeyModifiers::ALT),
            },
        }
    }
    
    fn translate_button(&self, btn: crossterm::event::MouseButton) -> MouseButton {
        match btn {
            crossterm::event::MouseButton::Left => MouseButton::Left,
            crossterm::event::MouseButton::Right => MouseButton::Right,
            crossterm::event::MouseButton::Middle => MouseButton::Middle,
        }
    }
}
```

#### 1.1 Audit and Clean `term-wm-core` Dependencies

**Files to modify**:
- `crates/term-wm-core/Cargo.toml`
- `crates/term-wm-core/src/ui.rs`
- `crates/term-wm-core/src/hitbox_registry.rs`
- `crates/term-wm-core/src/io/render_target.rs`
- `crates/term-wm-core/src/io/console_render_target.rs`

**Actions**:
1. Remove direct `ratatui` dependency from `term-wm-core/Cargo.toml`
2. Remove `crossterm` dependency from `term-wm-core/Cargo.toml`
3. Move `UiFrame` wrapper to a new `term-wm-render` crate
4. Move `RenderTarget` trait to `term-wm-render`
5. Update `HitboxRegistry` to use `term_wm_layout_engine::rect::Rect` and `WindowKey`
6. Update `WindowManager` to use `Rect` from layout engine instead of ratatui types

**Critical**: `term-wm-core` must operate exclusively on pure integers and discrete fractions from the layout engine.

#### 1.1b Isolate PTY Dependencies

**Goal**: Ensure `term-wm-core` does not depend on `portable-pty` at all.

**Current State**: `term-wm-pty-engine` is already a separate crate, but `term-wm-core` may directly use PTY types.

**Actions**:
1. Audit `term-wm-core/Cargo.toml` for `portable-pty` dependency
2. If present, remove it entirely — no feature gates, no optional dependencies
3. Ensure `TerminalComponent` (in `term-wm-ui-components`) encapsulates all PTY interactions
4. Remove any `PtyHandle` or PTY-related fields from core `Window` struct

**Design Principle**: The core engine is a mathematical layout compositor. It doesn't know what a PTY is. Components like `TerminalComponent` handle PTY interactions internally, while the core orchestrates their spatial placement.

**TerminalComponent Encapsulates PTY** (in `term-wm-ui-components`):
```rust
// crates/term-wm-ui-components/src/terminal.rs

use term_wm_pty_engine::Pty;  // PTY dependency only in UI components

pub struct TerminalComponent {
    /// PTY handle encapsulated within the component
    pty: Pty,
    /// Internal state (scrollback, selection, etc.)
    state: TerminalState,
    // ... other fields
}

impl TerminalComponent {
    pub fn new(pty: Pty) -> Self {
        Self {
            pty,
            state: TerminalState::new(),
            // ... other initialization
        }
    }
    
    /// Read from PTY (internal to component)
    fn read_from_pty(&mut self) -> io::Result<()> {
        // PTY interaction happens here, not in core
        self.pty.read_output(&mut self.state.buffer)
    }
    
    /// Write to PTY (internal to component)
    fn write_to_pty(&mut self, data: &[u8]) -> io::Result<()> {
        // PTY interaction happens here, not in core
        self.pty.write_input(data)
    }
}

impl Component<TermWmAction> for TerminalComponent {
    fn handle_event(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        // Process events using core-owned types
        // PTY interactions happen internally, not exposed to core
        match event {
            Event::Key(key) => {
                // Translate key to bytes and write to PTY
                let bytes = self.translate_key_to_bytes(key);
                self.write_to_pty(&bytes).ok();
                EventResult::None
            }
            // ... other event handling
        }
    }
    
    fn render(
        &self,
        backend: &mut dyn RenderBackend,
        area: Rect,  // Reuse layout engine's Rect
        ctx: &ComponentContext,
    ) {
        // Read from PTY and render to backend
        // PTY interaction happens internally, not exposed to core
        let ratatui_backend = backend
            .as_any_mut()
            .downcast_mut::<RatatuiBackend>()
            .expect("TerminalComponent requires RatatuiBackend");
        
        self.render_terminal_content(&mut ratatui_backend.buffer, &area);
    }
}
```

**Why This Works**:
1. `term-wm-core` has ZERO knowledge of PTYs — no `portable-pty` dependency
2. `TerminalComponent` encapsulates all PTY interactions internally
3. Core only sees `Component::handle_event()` and `Component::render()` — abstract interfaces
4. Future backends (WebGL, HTML Canvas) can implement `TerminalComponent` without PTYs

#### 1.2 Establish `term-wm-render` and `term-wm-console` Crates

**New crate**: `crates/term-wm-render/`

**Purpose**: Generic platform rendering traits serving as the contract bridge between core's pure data and OS-specific terminal backends. Uses **trait objects** at the compositor boundary for runtime flexibility.

**Contents**:
```rust
// crates/term-wm-render/src/lib.rs

/// Opaque render backend trait with downcasting capability.
/// Core crate defines this trait; UI crates downcast to concrete implementations.
/// This enables true backend independence — core compiles without Ratatui.
pub trait RenderBackend: std::any::Any {
    /// Downcast to concrete backend type.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

/// Abstraction over the terminal output backend.
/// Uses trait objects (dyn) at the compositor boundary for runtime flexibility.
/// Performance: vtable indirection is dispatched once per frame/window, not per cell.
pub trait RenderTarget {
    fn draw(&mut self, backend: &mut dyn RenderBackend, closure: impl FnOnce());
}
```

**New crate**: `crates/term-wm-console/`

**Purpose**: Concrete Ratatui backend implementation. Lives in a library crate accessible to both `term-wm-ui-components` and `term-wm-app`.

**Contents**:
```rust
// crates/term-wm-console/src/lib.rs

use ratatui::buffer::Buffer;
use ratatui::layout::Rect as RatatuiRect;
use term_wm_render::RenderBackend;
use term_wm_layout_engine::rect::Rect;

/// Concrete Ratatui backend implementation.
/// Owns the Buffer by value (satisfying 'static for Any downcasting).
/// Swap-based rendering preserves buffer capacity without allocation.
pub struct RatatuiBackend {
    pub buffer: Buffer,
    pub area: RatatuiRect,
}

impl RenderBackend for RatatuiBackend {
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl RatatuiBackend {
    /// Create a backend owning the given buffer.
    pub fn new(buffer: Buffer, area: RatatuiRect) -> Self {
        Self { buffer, area }
    }
    
    /// Get mutable reference to the underlying Ratatui buffer.
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }
    
    /// Convert layout engine's Rect to Ratatui's Rect.
    /// This is the SINGLE conversion point between spatial types.
    pub fn layout_rect_to_ratatui_rect(rect: &Rect) -> RatatuiRect {
        RatatuiRect {
            x: rect.x as u16,
            y: rect.y as u16,
            width: rect.width as u16,
            height: rect.height as u16,
        }
    }
}
```

**Key Design Decision**: `RatatuiBackend` owns `Buffer` by value to satisfy the `'static` bound required by `std::any::Any`. Swap-based rendering via `std::mem::replace` preserves buffer capacity without allocation.

**Key Design Decision**: `term-wm-console` is a library crate that both `term-wm-ui-components` and `term-wm-app` can depend on. This resolves the dependency graph inversion — UI components can downcast to `RatatuiBackend` without depending on the binary crate.

#### 1.3 Isolate `term-wm-app`

**Files to modify**:
- `src/main.rs`
- `src/term_wm_app.rs`
- `src/widget_adapter.rs`

**Actions**:
1. Move `WidgetAdapter` and `StatefulWidgetAdapter` to `term-wm-app`
2. Ensure `ratatui` and `crossterm` are only directly invoked in `term-wm-app`
3. Update imports to use `term-wm-render` traits

---

### Phase 2: Defining the Spatial IR (DrawPlan)

**Goal**: Core engine projects only spatial geometry — no semantic awareness of component types.

#### 2.1 Create Spatial IR Types in `term-wm-core`

**New file**: `crates/term-wm-core/src/draw_plan.rs`

**Key Design Decision**: Reuse the existing `Rect` type from `term-wm-layout-engine` — do NOT define any new spatial primitives. The layout engine is the single source of truth for all geometry.

```rust
// crates/term-wm-core/src/draw_plan.rs

use term_wm_layout_engine::rect::Rect;  // Reuse existing spatial primitive
use crate::window::WindowKey;

/// A single render region in the draw plan.
/// Contains ONLY spatial geometry — no semantic awareness of component types.
#[derive(Debug, Clone)]
pub struct RenderRegion {
    /// The window key identifying which component to render
    pub key: WindowKey,
    /// Bounding box in screen coordinates (reuses layout engine's Rect)
    pub bounds: Rect,
    /// Z-ordering for layering (higher = rendered on top)
    pub z_index: usize,
    /// Whether this region should be dimmed (unfocused windows)
    pub dimmed: bool,
}

/// The complete draw plan for a frame.
/// Core engine produces this; app layer consumes it.
#[derive(Debug, Clone)]
pub struct DrawPlan {
    /// Render regions sorted by z-index (low to high)
    regions: Vec<RenderRegion>,
    /// Pre-allocated capacity for regions
    capacity: usize,
}

impl DrawPlan {
    /// Create a new draw plan with pre-allocated capacity.
    /// Uses native Vec amortization — clear() retains capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            regions: Vec::with_capacity(capacity),
            capacity,
        }
    }
    
    /// Clear the plan for reuse (retains allocated capacity).
    pub fn clear(&mut self) {
        self.regions.clear();
    }
    
    /// Add a render region to the plan.
    pub fn push(&mut self, region: RenderRegion) {
        self.regions.push(region);
    }
    
    /// Get regions sorted by z-index.
    pub fn regions(&self) -> &[RenderRegion] {
        &self.regions
    }
    
    /// Get mutable access for sorting.
    pub fn regions_mut(&mut self) -> &mut [RenderRegion] {
        &mut self.regions
    }
    
    /// Sort regions by z-index (stable sort).
    pub fn sort_by_z_index(&mut self) {
        self.regions.sort_by_key(|r| r.z_index);
    }
    
    /// Current number of regions.
    pub fn len(&self) -> usize {
        self.regions.len()
    }
    
    /// Check if plan is empty.
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
}
```

**NO `BoundingBox` struct defined here.** The `RenderRegion` struct uses `Rect` directly from `term-wm-layout-engine`.

**Why This Works**:
1. No spatial type duplication — reuses `Rect` from `term-wm-layout-engine`
2. Single source of truth for spatial primitives
3. No wasteful data mapping between layout engine output and compositor input

**Key Design Decisions**:
1. **No `ViewState` enum** — core doesn't know what components exist
2. **`WindowKey` is the only identifier** — app layer resolves to `Component`
3. **`Rect` uses `i16`** — pure integers from layout engine, no ratatui dependency
4. **`dimmed` flag** — simple modifier without semantic coupling
5. **Native Vec amortization** — `clear()` retains capacity, no overflow buffer

#### 2.2 Update Core Types to Use Spatial IR

**Files to modify**:
- `crates/term-wm-core/src/component_context.rs`
- `crates/term-wm-core/src/hitbox_registry.rs`
- `crates/term-wm-core/src/window/window_manager/mod.rs`

**Actions**:
1. Replace `ratatui::layout::Rect` with `term_wm_layout_engine::rect::Rect` in `ComponentContext::screen_area`
2. Update `HitboxRegistry` to use `term_wm_layout_engine::rect::Rect` and `WindowKey`
3. Update `WindowManager` layout methods to output `term_wm_layout_engine::rect::Rect`

**Critical**: All spatial types must come from `term-wm-layout-engine`. No custom bounding box types.

---

### Phase 3: Implementing the Pre-Allocated Draw Plan Buffer

**Goal**: Core engine manages mathematical generation of the DrawPlan without heap fragmentation.

#### 3.1 Add Draw Plan to CoreEngine

**File to modify**: `crates/term-wm-core/src/engine.rs` (new file or extend existing)

```rust
// crates/term-wm-core/src/engine.rs

use crate::draw_plan::{DrawPlan, RenderRegion};
use crate::window::WindowKey;

const INITIAL_DRAW_PLAN_CAPACITY: usize = 256;

pub struct CoreEngine {
    // ... existing fields ...
    
    /// Pre-allocated draw plan buffer (cleared, not deallocated, each frame)
    draw_plan: DrawPlan,
    
    /// Dirty flag for fast path
    is_dirty: bool,
}

impl CoreEngine {
    pub fn new() -> Self {
        Self {
            // ... existing initialization ...
            draw_plan: DrawPlan::with_capacity(INITIAL_DRAW_PLAN_CAPACITY),
            is_dirty: true,
        }
    }
    
    /// Project the current draw plan without causing heap allocation.
    /// Returns a reference to the draw plan struct (not the inner slice).
    pub fn project_draw_plan(&mut self, width: u32, height: u32) -> &DrawPlan {
        if !self.is_dirty {
            self.draw_plan.sort_by_z_index();
            return &self.draw_plan;
        }
        
        // Clear plan (retains capacity, no allocation)
        self.draw_plan.clear();
        
        // Generate new regions from layout state
        self.generate_regions(width, height);
        
        // Sort by z-index for correct layering
        self.draw_plan.sort_by_z_index();
        
        // Mark as clean
        self.is_dirty = false;
        
        &self.draw_plan
    }
    
    /// Generate render regions from current layout state.
    /// Core engine delegates ALL spatial calculations to the layout engine.
    /// No hardcoded arithmetic — the layout engine is the sole arbiter of coordinates.
    fn generate_regions(&mut self, width: u32, height: u32) {
        // Ask the layout engine to compute the full screen partition
        let screen_rect = Rect::new(0, 0, width as i16, height as i16);
        let layout = self.layout_engine.compute_layout(screen_rect);
        
        // 1. Generate terminal window regions from layout engine output
        for (window_key, window_rect) in layout.window_regions() {
            let is_focused = self.window_manager.is_focused(window_key);
            
            self.draw_plan.push(RenderRegion {
                key: window_key,
                bounds: window_rect,  // Directly from layout engine
                z_index: 0,  // Windows at base layer
                dimmed: !is_focused,
            });
        }
        
        // 2. Generate panel regions from layout engine output
        if let Some(top_panel_rect) = layout.top_panel_region() {
            self.draw_plan.push(RenderRegion {
                key: self.window_manager.top_panel_key(),
                bounds: top_panel_rect,  // Directly from layout engine
                z_index: 10,  // Panels above windows
                dimmed: false,
            });
        }
        
        if let Some(bottom_panel_rect) = layout.bottom_panel_region() {
            self.draw_plan.push(RenderRegion {
                key: self.window_manager.bottom_panel_key(),
                bounds: bottom_panel_rect,  // Directly from layout engine
                z_index: 10,
                dimmed: false,
            });
        }
        
        // 3. Generate overlay regions from layout engine output
        if let Some(overlay_key) = self.window_manager.active_overlay_key() {
            if let Some(overlay_rect) = layout.overlay_region(overlay_key) {
                self.draw_plan.push(RenderRegion {
                    key: overlay_key,
                    bounds: overlay_rect,  // Directly from layout engine
                    z_index: 20,  // Overlays above everything
                    dimmed: false,
                });
            }
        }
    }
    
    /// Mark the engine as needing re-projection
    pub fn mark_dirty(&mut self) {
        self.is_dirty = true;
    }
}
```

#### 3.2 Integrate with WindowManager

**Files to modify**:
- `crates/term-wm-core/src/window/window_manager/mod.rs`
- `crates/term-wm-core/src/runner.rs`

**Actions**:
1. Add `mark_dirty()` calls whenever window state changes (focus, resize, close, open)
2. Add `mark_dirty()` calls whenever layout changes (tiling, floating)
3. Update `draw_window_app` to use `engine.project_draw_plan()` instead of direct rendering

---

### Phase 4: The Presentation Layer Refactor (`term-wm-app`)

**Goal**: App layer consumes DrawPlan, retrieves components, and invokes rendering.

#### 4.1 Create Draw Plan Renderer

**New file**: `crates/term-wm-app/src/draw_plan_renderer.rs`

```rust
// crates/term-wm-app/src/draw_plan_renderer.rs

use term_wm_core::draw_plan::{DrawPlan, RenderRegion};
use term_wm_core::components::Component;
use term_wm_core::window::WindowKey;
use term_wm_render::RenderBackend;
use term_wm_console::RatatuiBackend;
use term_wm_layout_engine::rect::Rect;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect as RatatuiRect;

pub struct DrawPlanRenderer<'a> {
    /// Reference to the window manager for component lookup
    window_manager: &'a WindowManager,
    /// Persistent scratch buffer for offscreen compositing (swapped, not reallocated)
    scratch_buffer: Buffer,
    /// Persistent buffer for direct rendering (swapped, not reallocated)
    direct_buffer: Buffer,
}

impl<'a> DrawPlanRenderer<'a> {
    pub fn new(window_manager: &'a WindowManager) -> Self {
        Self {
            window_manager,
            scratch_buffer: Buffer::empty(RatatuiRect::zero()),
            direct_buffer: Buffer::empty(RatatuiRect::zero()),
        }
    }
    
    /// Render the draw plan to the terminal frame.
    /// This is the ONLY place where Ratatui types are used for rendering.
    pub fn render(
        &mut self,
        frame: &mut ratatui::Frame,
        draw_plan: &DrawPlan,
    ) {
        for region in draw_plan.regions() {
            let area = RatatuiBackend::layout_rect_to_ratatui_rect(&region.bounds);
            
            // Look up the component for this window key
            if let Some(component) = self.window_manager.get_component(region.key) {
                // For window content, use offscreen compositing
                if region.z_index < 10 {
                    self.render_window_composite(frame, area, component, region);
                } else {
                    // For panels/overlays, render directly to frame
                    self.render_direct(frame, area, component, region);
                }
            }
        }
    }
    
    /// Render a window with offscreen compositing (swap-based, zero-allocation).
    /// Uses std::mem::replace to temporarily move buffer into RatatuiBackend.
    fn render_window_composite(
        &mut self,
        frame: &mut ratatui::Frame,
        area: RatatuiRect,
        component: &dyn Component<TermWmAction>,
        region: &RenderRegion,
    ) {
        // Swap persistent buffer out (leaves empty buffer in place)
        let mut buffer = std::mem::replace(
            &mut self.scratch_buffer,
            Buffer::empty(RatatuiRect::zero())
        );
        
        // Resize and clear the swapped buffer (no allocation after warmup)
        buffer.resize(area);
        buffer.reset();
        
        // Create backend owning the buffer (satisfies 'static for Any)
        let mut backend = RatatuiBackend::new(buffer, area);
        
        // Create component context with screen area
        let ctx = ComponentContext::new()
            .with_screen_area(region.bounds)
            .with_dimmed(region.dimmed);
        
        // Component renders itself into the backend
        component.render(&mut backend, region.bounds, &ctx);
        
        // Apply dim modifier if needed
        if region.dimmed {
            self.apply_dim_modifier(&mut backend.buffer);
        }
        
        // Blit to main frame
        frame.blit_from_signed(&backend.buffer, area);
        
        // Swap buffer back to preserve capacity (zero-allocation)
        self.scratch_buffer = backend.buffer;
    }
    
    /// Render directly to frame (panels, overlays).
    /// Uses swap-based rendering to mutate frame's buffer without allocation.
    fn render_direct(
        &mut self,
        frame: &mut ratatui::Frame,
        area: RatatuiRect,
        component: &dyn Component<TermWmAction>,
        region: &RenderRegion,
    ) {
        let ctx = ComponentContext::new()
            .with_screen_area(region.bounds);
        
        // Swap direct buffer out
        let mut buffer = std::mem::replace(
            &mut self.direct_buffer,
            Buffer::empty(RatatuiRect::zero())
        );
        
        // Resize and clear (no allocation after warmup)
        buffer.resize(area);
        buffer.reset();
        
        // Create backend owning the buffer
        let mut backend = RatatuiBackend::new(buffer, area);
        
        // Component renders into the swapped buffer
        component.render(&mut backend, region.bounds, &ctx);
        
        // Blit to frame (component can't write directly due to 'static constraint)
        frame.blit_from_signed(&backend.buffer, area);
        
        // Swap buffer back to preserve capacity
        self.direct_buffer = backend.buffer;
    }
}
```

#### 4.2 Update Application Event Loop

**File to modify**: `crates/term-wm-core/src/runner.rs`

**Changes**:
1. Before draw phase, query core engine for draw plan:
```rust
// In draw_window_app()
let draw_plan = engine.project_draw_plan(width, height);
let mut renderer = DrawPlanRenderer::new(&wm);

// In the draw closure
renderer.render(frame, draw_plan);
```

2. After render, update hitbox registry from draw plan:
```rust
// After rendering
for region in draw_plan.regions() {
    hitbox_registry.register(
        region.bounds,
        Some(region.key),
        HitboxType::WindowContent,
    );
}
```

#### 4.3 Component Rendering Contract

**Files to modify**:
- `crates/term-wm-core/src/components/mod.rs`
- `crates/term-wm-core/src/events.rs`
- `crates/term-wm-ui-components/src/terminal.rs`
- `crates/term-wm-ui-components/src/scroll_view.rs`
- `crates/term-wm-sys-ui-components/src/wm_top_panel.rs`
- `crates/term-wm-sys-ui-components/src/wm_bottom_panel.rs`
- `crates/term-wm-sys-ui-components/src/wm_menu_overlay.rs`
- `crates/term-wm-sys-ui-components/src/wm_debug_log.rs`
- `crates/term-wm-sys-ui-components/src/wm_help_overlay.rs`

**Key Insight**: The Component trait must use opaque types for both input (events) and output (rendering).

**Updated Contract** (in `term-wm-core`):
```rust
use term_wm_render::RenderBackend;
use term_wm_layout_engine::rect::Rect;  // Reuse existing spatial primitive
use crate::events::{Event, EventResult};

pub trait Component<Msg> {
    /// Handle an input event.
    /// Returns an EventResult indicating what action to take.
    fn handle_event(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<Msg>;
    
    /// Render the component into the backend.
    /// The backend is opaque — downcast to concrete type when needed.
    /// The area is the local bounding box for this component (uses layout engine's Rect).
    /// The ctx contains screen coordinates and state.
    fn render(
        &self,
        backend: &mut dyn RenderBackend,
        area: Rect,  // From layout engine
        ctx: &ComponentContext,
    );
    
    // ... other methods unchanged ...
}
```

**UI Component Implementation** (in `term-wm-ui-components`):
```rust
// crates/term-wm-ui-components/src/terminal.rs

use term_wm_render::RenderBackend;
use term_wm_console::RatatuiBackend;
use term_wm_layout_engine::rect::Rect;  // Reuse layout engine's Rect
use term_wm_core::events::{Event, EventResult};

impl Component<TermWmAction> for TerminalComponent {
    fn handle_event(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        // Process core-owned event types
        match event {
            Event::Key(key) => {
                // Handle keyboard input using core types
                self.handle_key_input(key, ctx)
            }
            Event::Mouse(mouse) => {
                // Handle mouse input using core types
                self.handle_mouse_input(mouse, ctx)
            }
            _ => EventResult::None,
        }
    }
    
    fn render(
        &self,
        backend: &mut dyn RenderBackend,
        area: Rect,  // Reuse layout engine's Rect — matches trait signature
        ctx: &ComponentContext,
    ) {
        // Downcast to concrete Ratatui backend (once per frame, per window)
        let ratatui_backend = backend
            .as_any_mut()
            .downcast_mut::<RatatuiBackend>()
            .expect("TerminalComponent requires RatatuiBackend");
        
        let buffer = &mut ratatui_backend.buffer;
        let rect = RatatuiBackend::layout_rect_to_ratatui_rect(&area);
        
        // Execute Ratatui-specific cell mutations here
        self.render_content(buffer, rect, ctx);
    }
}

impl TerminalComponent {
    fn handle_key_input(
        &mut self,
        key: &KeyEvent,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        // Handle keyboard input using core-owned types
        // (no crossterm dependency)
        match key.code {
            KeyCode::Char(c) => {
                // Process character input
                EventResult::None
            }
            KeyCode::Enter => {
                // Process enter key
                EventResult::None
            }
            // ... other key handling
        }
    }
    
    fn handle_mouse_input(
        &mut self,
        mouse: &MouseEvent,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        // Handle mouse input using core-owned types
        // (no crossterm dependency)
        match mouse.kind {
            MouseEventKind::Press(MouseButton::Left) => {
                // Handle left click
                EventResult::None
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Handle drag
                EventResult::None
            }
            // ... other mouse handling
        }
    }
    
    fn render_content(
        &self,
        buffer: &mut ratatui::buffer::Buffer,
        area: ratatui::layout::Rect,
        ctx: &ComponentContext,
    ) {
        // Existing rendering logic using Ratatui Buffer directly
        // ... unchanged ...
    }
}
```

**Performance Note**: The downcast (`as_any_mut().downcast_mut::<RatatuiBackend>()`) costs nanoseconds per window per frame (one vtable lookup + type check). This is negligible compared to the rendering work.

**Why This Works**:
1. `term-wm-core` defines `Component` trait with `&mut dyn RenderBackend` and core-owned `Event` — no Ratatui/Crossterm dependency
2. `term-wm-ui-components` implements `Component` — downcasts to `RatatuiBackend` when rendering, uses core events for input
3. `term-wm-app` defines `RatatuiBackend` — the only place Ratatui types are defined as a backend
4. `term-wm-core/src/io/unified_event_source.rs` — translates crossterm events to core events at the boundary
5. Future backends (WebGL, HTML Canvas) can implement `RenderBackend` and `EventSource` without modifying core

---

### Phase 5: Headless Test Suite Migration

**Goal**: Rewrite layout testing suite to use purely mathematical structs.

#### 5.1 Create IR-Based Test Utilities

**New file**: `crates/term-wm-core/src/draw_plan/test_utils.rs`

```rust
// crates/term-wm-core/src/draw_plan/test_utils.rs

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::engine::CoreEngine;
    
    /// Test helper: create engine and project draw plan
    fn setup_engine_and_project(
        window_count: usize,
        width: u32,
        height: u32,
    ) -> (CoreEngine, DrawPlan) {
        let mut engine = CoreEngine::new();
        
        // Add test windows
        for i in 0..window_count {
            engine.add_test_window(format!("Window {}", i));
        }
        
        let draw_plan = engine.project_draw_plan(width, height).clone();
        (engine, draw_plan)
    }
    
    #[test]
    fn test_single_window_fills_area() {
        let (_, draw_plan) = setup_engine_and_project(1, 80, 24);
        
        assert_eq!(draw_plan.len(), 1);
        let region = &draw_plan.regions()[0];
        
        assert_eq!(region.bounds.x, 0);
        assert_eq!(region.bounds.y, 0);
        assert_eq!(region.bounds.width, 80);
        assert_eq!(region.bounds.height, 24);
        assert!(!region.dimmed);  // Focused window
    }
    
    #[test]
    fn test_two_windows_split_horizontally() {
        let (_, draw_plan) = setup_engine_and_project(2, 80, 24);
        
        // Should have 2 window regions
        assert_eq!(draw_plan.len(), 2);
        
        // Check split: first window gets left half, second gets right half
        assert_eq!(draw_plan.regions()[0].bounds.width, 40);
        assert_eq!(draw_plan.regions()[1].bounds.x, 40);
        assert_eq!(draw_plan.regions()[1].bounds.width, 40);
    }
    
    #[test]
    fn test_z_index_ordering() {
        let (_, draw_plan) = setup_engine_and_project(2, 80, 24);
        
        // Windows should have z_index 0
        for region in draw_plan.regions() {
            assert_eq!(region.z_index, 0);
        }
    }
    
    #[test]
    fn test_dimmed_flag() {
        let (_, draw_plan) = setup_engine_and_project(2, 80, 24);
        
        // First window focused (not dimmed), second unfocused (dimmed)
        assert!(!draw_plan.regions()[0].dimmed);
        assert!(draw_plan.regions()[1].dimmed);
    }
    
    #[test]
    fn test_draw_plan_capacity_reuse() {
        let mut engine = CoreEngine::new();
        engine.add_test_window("Window 1".to_string());
        
        // First projection
        let draw_plan1 = engine.project_draw_plan(80, 24).clone();
        let capacity1 = engine.draw_plan.capacity();
        
        // Second projection without state change
        let draw_plan2 = engine.project_draw_plan(80, 24).clone();
        
        // Should return same buffer (no reallocation)
        assert_eq!(draw_plan1.len(), draw_plan2.len());
        assert_eq!(engine.draw_plan.capacity(), capacity1);
        
        // Mark dirty
        engine.mark_dirty();
        let draw_plan3 = engine.project_draw_plan(80, 24).clone();
        
        // Should regenerate but reuse capacity
        assert_eq!(draw_plan1.len(), draw_plan3.len());
        assert_eq!(engine.draw_plan.capacity(), capacity1);
    }
}
```

#### 5.2 Migrate Existing Tests

**Files to modify**:
- `crates/term-wm-core/src/window/window_manager/tests.rs`
- `crates/term-wm-ui-components/src/terminal.rs` (test module)
- `crates/term-wm-ui-components/src/scroll_view.rs` (test module)

**Actions**:
1. Replace `Buffer::empty(area) + UiFrame::from_parts` pattern with `DrawPlan` assertions
2. Add coordinate validation tests for all layout scenarios
3. Add damage detection tests (verifying `is_dirty` optimization)
4. Add buffer reuse tests (verifying no heap allocation during normal operation)

#### 5.3 Add Property-Based Tests

**New file**: `crates/term-wm-core/src/draw_plan/property_tests.rs`

```rust
// crates/term-wm-core/src/draw_plan/property_tests.rs

#[cfg(test)]
mod proptest_tests {
    use super::*;
    use proptest::prelude::*;
    
    proptest! {
        #[test]
        fn test_window_bounds_within_screen(
            window_count in 1..10usize,
            width in 1i16..200i16,
            height in 1i16..100i16,
        ) {
            let (_, draw_plan) = setup_engine_and_project(window_count, width as u32, height as u32);
            
            for region in draw_plan.regions() {
                prop_assert!(region.bounds.x >= 0);
                prop_assert!(region.bounds.y >= 0);
                prop_assert!(region.bounds.width <= width);
                prop_assert!(region.bounds.height <= height);
                prop_assert!(region.bounds.x + region.bounds.width <= width);
                prop_assert!(region.bounds.y + region.bounds.height <= height);
            }
        }
        
        #[test]
        fn test_no_overlapping_windows(
            window_count in 2..10usize,
            width in 10i16..200i16,
            height in 10i16..100i16,
        ) {
            let (_, draw_plan) = setup_engine_and_project(window_count, width as u32, height as u32);
            
            // Check no two window regions overlap
            for i in 0..draw_plan.regions().len() {
                for j in (i+1)..draw_plan.regions().len() {
                    let a = &draw_plan.regions()[i].bounds;
                    let b = &draw_plan.regions()[j].bounds;
                    
                    // No overlap if: a is completely left/right/above/below b
                    let no_overlap = 
                        a.x + a.width <= b.x ||
                        b.x + b.width <= a.x ||
                        a.y + a.height <= b.y ||
                        b.y + b.height <= a.y;
                    
                    prop_assert!(no_overlap, 
                        "Regions {} and {} overlap", i, j);
                }
            }
        }
        
        #[test]
        fn test_z_index_sorted(
            window_count in 1..10usize,
            width in 10i16..200i16,
            height in 10i16..100i16,
        ) {
            let (_, draw_plan) = setup_engine_and_project(window_count, width as u32, height as u32);
            
            // After project_draw_plan(), regions should be sorted by z_index
            for window in draw_plan.regions().windows(2) {
                prop_assert!(window[0].z_index <= window[1].z_index,
                    "z_index not sorted: {} > {}", 
                    window[0].z_index, window[1].z_index);
            }
        }
    }
}
```

---

## Critical Files Summary

### New Files to Create
1. `crates/term-wm-render/` - New crate for rendering traits (opaque `RenderBackend`)
2. `crates/term-wm-console/` - New crate for concrete `RatatuiBackend` implementation
3. `crates/term-wm-core/src/events.rs` - Core-owned event types (no crossterm dependency)
4. `crates/term-wm-core/src/draw_plan.rs` - Spatial IR type definitions
5. `crates/term-wm-core/src/draw_plan/test_utils.rs` - Test utilities
6. `crates/term-wm-core/src/draw_plan/property_tests.rs` - Property-based tests
7. `crates/term-wm-core/src/engine.rs` - Core engine with draw plan buffer
8. `crates/term-wm-app/src/crossterm_event_source.rs` - Crossterm → core event translation (in app crate)
9. `crates/term-wm-app/src/draw_plan_renderer.rs` - Presentation layer

### Files to Modify
1. `crates/term-wm-core/Cargo.toml` - Remove ratatui/crossterm dependencies
2. `crates/term-wm-core/src/ui.rs` - Move to term-wm-render
3. `crates/term-wm-core/src/hitbox_registry.rs` - Use Rect from layout engine
4. `crates/term-wm-core/src/component_context.rs` - Use Rect from layout engine
5. `crates/term-wm-core/src/io/render_target.rs` - Move to term-wm-render
6. `crates/term-wm-core/src/io/console_render_target.rs` - Update imports
7. `crates/term-wm-core/src/window/window_manager/mod.rs` - Mark dirty on state changes
8. `crates/term-wm-core/src/runner.rs` - Use draw plan
9. `src/main.rs` - Update to use new architecture
10. `src/widget_adapter.rs` - Move to term-wm-app

---

## Verification Plan

### Step 1: Run Existing Tests
```bash
cargo test --workspace
```
All existing tests must pass after each phase.

### Step 2: Run Clippy
```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
```
No new warnings allowed.

### Step 3: Benchmark Rendering Performance
```bash
cargo run --release -p term-bench -- --duration 10 --windows 4
```
Verify 60+ FPS with 4 windows.

### Step 4: Verify Zero-Allocation Rendering
Add allocation tracking in debug mode:
```rust
#[cfg(debug_assertions)]
pub fn project_draw_plan(&mut self, width: u32, height: u32) -> &DrawPlan {
    let before = std::alloc::GlobalAlloc::stats(&std::alloc::System);
    // ... projection logic ...
    let after = std::alloc::GlobalAlloc::stats(&std::alloc::System);
    assert_eq!(before.bytes_alloc, after.bytes_alloc, 
        "Allocation detected during projection!");
    &self.draw_plan
}
```

### Step 5: Visual Regression Testing
Compare screenshots before and after refactor:
```bash
cargo run --release -- --screenshot-before
# Apply changes
cargo run --release -- --screenshot-after
diff before.png after.png
```

### Step 6: Memory Profiling
```bash
valgrind --tool=massif ./target/release/term-wm
ms_print massif.out.*
```
Verify no memory growth during 10-minute stress test.

---

## Risk Mitigation

### Risk 1: Breaking Existing Components
**Mitigation**: Phase 1 keeps all existing rendering working via adapter layer. Only remove adapters after all components are migrated.

### Risk 2: Performance Regression
**Mitigation**: Benchmark before and after each phase. If FPS drops below 60, optimize draw plan buffer before proceeding.

### Risk 3: Borrow Checker Issues
**Mitigation**: Use `Rc<RefCell<>>` sparingly for shared state. Prefer message-passing between engine and renderer.

### Risk 4: Test Coverage Gaps
**Mitigation**: Require 100% coverage for DrawPlan types before merging. Use property-based tests for edge cases.

---

## Success Criteria

1. ✅ `term-wm-core` has zero ratatui/crossterm/portable-pty dependencies
2. ✅ Core engine produces only spatial IR (`DrawPlan` with `RenderRegion`)
3. ✅ `project_draw_plan()` returns `&DrawPlan` (not `&[RenderRegion]`)
4. ✅ `DrawPlan` and `RenderRegion` use `Rect` from `term-wm-layout-engine` (no custom spatial types)
5. ✅ `Component::render()` signature uses `&mut dyn RenderBackend` + `Rect` (from layout engine)
6. ✅ `TerminalComponent` implements `Component` with `area: Rect` (matches trait signature)
7. ✅ `Component::handle_event()` signature uses core-owned `Event` types (no crossterm types)
8. ✅ `term-wm-console` crate defines `RatatuiBackend` (owns `Buffer` by value, satisfies `'static` for `Any`)
9. ✅ `term-wm-app` crate defines `CrosstermEventSource` (translates crossterm events to core events)
10. ✅ UI crates downcast `RenderBackend` to `RatatuiBackend` when rendering
11. ✅ UI crates have `ratatui` dependency (for buffer mutations after downcast)
12. ✅ UI crates have NO crossterm dependency — use only core-owned `Event` types
13. ✅ `TerminalComponent` encapsulates PTY internally — core has zero knowledge of PTYs
14. ✅ Layout engine is sole arbiter of spatial coordinates — no hardcoded arithmetic in core
15. ✅ All spatial types consistent — `Rect` from layout engine used throughout
16. ✅ Zero heap allocation during steady-state rendering (swap-based buffer management)
17. ✅ `RatatuiBackend` satisfies `'static` bound for `Any` downcasting
18. ✅ 60+ FPS with 4 windows and overlays
19. ✅ All existing tests pass
20. ✅ New DrawPlan-based tests provide 100% coverage
21. ✅ No borrow checker friction between engine and renderer
22. ✅ Headless testing works without virtual terminal
23. ✅ Correct dependency graph (no inversions, no phantom references, no type mismatches)
24. ✅ No zombie spatial type artifacts in documentation
