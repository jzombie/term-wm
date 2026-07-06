# Plan: Typed Mouse Coordinates + Hitbox Registry

## Problem

Mouse events currently take a **tree-walk dispatch** path:

1. `runner.rs` receives `Event::Mouse`
2. Routes through overlays, chrome, focus dispatch (focus.rs), managed events (chrome.rs), resize/drag handlers
3. Each step does ad-hoc `rect_contains()` checks and coordinate subtraction (`localize_event_content`)
4. Falls through to `focused_window_event` → `adjust_event_for_window` → `component.handle_events`
5. Every component in the chain has its `handle_events` called, even if not clicked
6. Inside a window, nested components (ScrollView → List → items) each repeat the same pattern

This is fragile, involves coordinate mutation at every boundary, and forces every component to replicate the same "was I clicked?" logic.

## Solution: Flat Hitbox Registry + Component-Level Registration

Replace the tree-walk with a single flat hit-test query. The key insight: **the registry is populated during the render pass**, not discovered at event time. Components register their own clickable areas, and scroll containers clip child registrations to their visible viewport.

### 1. WmEvent — Typed Event Enum

`crates/term-wm-core/src/events.rs`

```rust
pub enum WmEvent {
    Key(KeyEvent),
    Mouse {
        kind: MouseEventKind,
        modifiers: KeyModifiers,
        position: MousePosition,   // always CoordSpace::Screen
    },
    Resize(u16, u16),
    FocusGained,
    FocusLost,
    Paste(String),
}
```

`crates/term-wm-core/src/mouse_coord.rs`

```rust
pub struct MousePosition {
    pub column: i16,
    pub row: i16,
    pub space: CoordSpace,
}

pub enum CoordSpace { Screen }

impl MousePosition {
    pub fn is_inside(&self, area: Rect) -> bool;
    /// Returns local (col, row) if point is inside area, else None.
    /// Does NOT saturate — returns None when outside. This is the
    /// replacement for ad-hoc rect_contains + subtraction.
    pub fn to_local(&self, area: Rect) -> Option<(u16, u16)>;
}
```

### 2. HitboxRegistry — Flat Data-Oriented Hit-Testing

`crates/term-wm-core/src/hitbox_registry.rs`

```rust
pub type ComponentId = u32; // monotonically increasing, unique per window per frame

#[derive(Clone, Copy)]
pub enum HitTarget {
    Window(WindowKey),
    Component(WindowKey, ComponentId),  // fine-grained dispatch to a specific component
    Overlay(OverlayId),
    TopPanel,
    BottomPanel,
    ChromeResize(WindowKey, ResizeEdge),
    ChromeHeader(WindowKey, HeaderAction),
    LayoutHandle,
}

pub struct HitboxEntry {
    pub target: HitTarget,
    pub area: Rect,       // always absolute screen coords, post-clip
}

/// Maximum depth of nested clipping containers (ScrollViews, overlay bounds,
/// etc.) that the registry supports without heap allocation.
///
/// Depth ≤ 5: In practice, no UI nests more than 4–5 *clipping* boundaries.
/// Layout-only containers (padding, margins, rows, columns) do not clip;
/// only scrollable or bounded containers create clip rects. A button inside
/// a List inside a ScrollView inside a floating Window inside an Overlay
/// is already an extreme case at depth 4. We set the inline capacity to 8
/// to leave a generous safety margin while keeping the entire struct
/// stack-allocated in the common case.
const CLIP_STACK_INLINE_CAPACITY: usize = 8;

pub struct HitboxRegistry {
    entries: Vec<HitboxEntry>,
    /// Active clip rects from scroll containers.
    /// Inline storage avoids heap allocation for the common case (depth ≤ 5).
    /// Falls back to heap only in pathological nesting > 8 levels deep.
    clip_stack: smallvec::SmallVec<[Rect; CLIP_STACK_INLINE_CAPACITY]>,
}

impl HitboxRegistry {
    pub fn new() -> Self;

    /// Reset for a new frame.
    pub fn clear(&mut self);

    /// Register a clickable area. The area is intersected with the
    /// current clip stack before storing. If the intersection is
    /// empty, the entry is skipped entirely.
    /// This means scrolled-off components simply don't appear in
    /// the registry — no rect_contains needed at event time.
    pub fn register(&mut self, target: HitTarget, area: Rect);

    /// Push a clip rect (called by ScrollView before rendering children).
    /// All subsequent register() calls intersect area with this rect.
    /// Stacks: ScrollView → child → grandchild.
    pub fn push_clip(&mut self, rect: Rect);

    /// Pop the active clip rect (called by ScrollView after children).
    pub fn pop_clip(&mut self);

    /// Query: reverse scan (front-to-back) for top-most entry
    /// containing the position. Returns the target.
    pub fn hit_test(&self, pos: MousePosition) -> Option<HitTarget>;

    /// Build a dispatch map: (WindowKey, ComponentId) → Vec index.
    /// Used by the window manager to route hits to the right component.
    pub fn build_component_map(&self) -> FxHashMap<(WindowKey, ComponentId), usize>;
}
```

### 3. Updated Component Trait

```rust
pub trait Component<A>: std::any::Any {
    fn handle_events(&mut self, event: &WmEvent, ctx: &ComponentContext) -> EventResult<A>;

    /// Receives the registry for self-registration during render.
    /// Components that want targeted hit-testing call
    /// `registry.register(HitTarget::Component(window_key, self.id), self.area)`.
    /// The registry handles clip_rect intersection automatically.
    fn render(&self, frame: &mut UiFrame, area: Rect, ctx: &ComponentContext,
              registry: &mut HitboxRegistry);
}
```

### 4. Component Self-Registration Pattern

Every interactive component that needs click handling stores a stable `ComponentId`:

```rust
struct MyButton {
    id: ComponentId,  // assigned once in constructor or first render
    // ...
}

impl Component<Action> for MyButton {
    fn render(&self, frame: &mut UiFrame, area: Rect, ctx: &ComponentContext,
              registry: &mut HitboxRegistry) {
        registry.register(HitTarget::Component(ctx.window_key, self.id), area);
        // ... draw the button
    }

    fn handle_events(&mut self, event: &WmEvent, ctx: &ComponentContext) -> EventResult<Action> {
        // Registry already resolved this component as the hit target.
        // No rect_contains needed ever.
        match event {
            WmEvent::Mouse { kind: MouseEventKind::Down(MouseButton::Left), .. } => {
                EventResult::Action(Action::Clicked)
            }
            _ => EventResult::Ignored,
        }
    }
}
```

Key property: `handle_events` is **only called when the registry determined this specific component was hit**. No `is_inside()`, no offset subtraction. The component just acts.

### 5. Clip Rect Propagation (ScrollView)

This is the mechanism that solves the "scrolled-off button receives clicks" problem:

```rust
// In ScrollViewComponent::render():
// 1. Compute the visible (clipped) area of the content
let visible_area = Rect { x: area.x, y: area.y, width: area.width, height: content_height.min(area.height) };

// 2. Push clip before rendering children
registry.push_clip(visible_area);

// 3. Render children normally — their register() calls are intersected
for child in &self.children {
    child.render(frame, child_area, ctx, registry);
}

// 4. Pop clip afterward
registry.pop_clip();
```

A button scrolled 50 rows up registers `area = (x, y+50, w, h)` but the clip rect is `(x, y, w, visible_h)`. The intersection is empty → button is not registered → click passes through to whatever is visually beneath.

No event-time computation. All clipping is resolved during the render pass.

## How It Works

### Population (during render pass)

The render pipeline gains a `&mut HitboxRegistry` parameter threaded from `WindowManager::prepare_draw()`:

```
prepare_draw(window_manager):
  1. registry.clear()
  2. register TopPanel, BottomPanel, Chrome areas
  3. for each window in managed_draw_order:
     register Window(key), visible_region(key)
  4. for focused window:
     register ChromeResize edges, ChromeHeader buttons, LayoutHandle areas
  5. for each overlay:
     registry.push_clip(overlay_rect)   // overlay clips to its own bounds
     register Overlay(id), overlay_rect(id)
     overlay.component.render(...)       // may register sub-targets
     registry.pop_clip()
  6. for each window:
     window.component.render(...)        // may push_clip for scrollviews,
                                         // register sub-targets via registry
```

DOD property: all geometry is computed upfront from layout + decorator constants. No vtable calls in the hot path. The `clip_stack` is a small Vec (depth ≤ 5).

### Dispatch (on mouse event)

```
runner receives Event::Mouse(mouse):
  1. Convert to WmEvent::Mouse { position: MousePosition { column, row, space: Screen }, ... }
  2. hit_target = app.windows().hitbox_registry.hit_test(position)
  3. Match hit_target:
     Window(key)                 → component_for_key(key).handle_events(&wm_event, ctx)
     Component(key, id)          → resolve_component(key, id).handle_events(&wm_event, ctx)
     Overlay(id)                 → overlays[id].component.handle_events(&wm_event, ctx)
     TopPanel / BottomPanel      → dispatch to panel component (which registered sub-targets)
     ChromeResize(key, edge)     → handle_resize_event(key, edge)
     ChromeHeader(key, action)   → handle_header_action(key, action)
     LayoutHandle                → layout.handle_mouse_event(position)
  4. No match → Ignored (click on empty space — no component was called at all)
```

`resolve_component(key, id)` uses a `FxHashMap<(WindowKey, ComponentId), &mut dyn Component>` built during `prepare_draw()` from the registry's entries. This is a flat lookup — O(1), no tree walk.

### What the Registry Replaces

| Current | Replaced by |
|---------|-------------|
| `focus.rs:dispatch_focused_event` mouse handling | Registry query |
| `focus.rs:hover-to-scroll` tree walk | Registry query + dispatch |
| `chrome.rs:handle_managed_event` ad-hoc rect_contains | Registry entries for chrome |
| `layout/tiling.rs:handle_event` | Registry `LayoutHandle` entry |
| `localize_event_content` | Removed (no mutation needed) |
| `adjust_event_for_window` | Removed for mouse (kept for key events if needed) |
| `ComponentContext::hover_pos` | Removed (replaced by component-local last_mouse_pos) |
| `WindowManager::hover: Option<(u16, u16)>` | Removed (registry subsumes it) |
| Component-level `rect_contains` in handle_events | Removed (registry guarantees hit) |
| ScrollView manual offset rollback in handle_events | Removed (clip_rect at render time) |

### What Stays the Same

- Key event routing (keyboard events don't need hit-testing)
- The `render()` method signature gains `registry` parameter but the rendering logic is unchanged
- The existing render pipeline (`begin_frame` → `prepare_draw` → `draw`)
- Focus tracking, z-order management
- UiFrame clipping for visual output

## Files to Create/Modify

### New Files

| File | Contents |
|------|----------|
| `crates/term-wm-core/src/mouse_coord.rs` | `MousePosition`, `CoordSpace`, `is_inside()`, `to_local()` |
| `crates/term-wm-core/src/events.rs` | `WmEvent` enum |
| `crates/term-wm-core/src/hitbox_registry.rs` | `HitTarget`, `HitboxEntry`, `HitboxRegistry` (with clip_stack, push_clip, pop_clip) |

### Core Framework Changes

| File | Change |
|------|--------|
| `lib.rs` | Re-export new modules |
| `components/mod.rs` | `handle_events(&WmEvent, ...)`; `render(..., &mut HitboxRegistry)` |
| `component_context.rs` | Remove `hover_pos` field; add `window_key: WindowKey` for component self-identification |
| `runner.rs` | Convert crossterm `Event::Mouse` → `WmEvent`; call `window_manager.dispatch_mouse()` instead of tree-dispatch |
| `window/window_manager/mod.rs` | Add `hitbox_registry: HitboxRegistry`, `component_dispatch: FxHashMap<(WindowKey, ComponentId), usize>`; implement `dispatch_mouse()` that queries registry and routes to `resolve_component` or chrome handlers |
| `window/window_manager/layout.rs` | Remove `hit_test_region`, `hit_test_region_topmost`; register `LayoutHandle` entries in registry during `prepare_draw` |
| `window/window_manager/focus.rs` | `dispatch_focused_event` simplified to handle only key events |
| `window/window_manager/chrome.rs` | `handle_managed_event` becomes registry entries + chrome handler dispatch |
| `window/window_manager/overlays.rs` | Register overlay rect in registry; push_clip before rendering overlay component |
| `window/window_manager/drag.rs` | Uses registry result instead of `hit_test_region_topmost` |
| `window/decorator.rs` | `WindowRenderCtx::hover_pos` → `Option<MousePosition>`; registers header button rects in registry |

### UI Component Changes (Per-Component)

Every component that has `handle_events` with mouse handling applies the same mechanical transformation:

1. `Event::Mouse(m)` → `WmEvent::Mouse { position, kind, modifiers }`
2. Remove all `rect_contains(last_area, mouse.column, mouse.row)` — guaranteed unnecessary
3. Replace `mouse.column - last_area.x` with `position.to_local(last_area).unwrap()`
4. `render()` signature gains `registry: &mut HitboxRegistry`
5. Components that target sub-widgets store `id: ComponentId` and call `registry.register(HitTarget::Component(window_key, self.id), area)` in `render()`
6. `ScrollViewComponent` calls `registry.push_clip(visible_area)` before and `registry.pop_clip()` after child render
7. Remove all `ComponentContext::hover_pos` references; host `last_mouse_pos: Option<MousePosition>` locally if hover tracking is needed

Affected: `terminal.rs`, `scroll_view.rs`, `list.rs`, `text_renderer.rs`, `markdown_viewer.rs`, `menu.rs`, `ascii_image.rs`, `svg_image.rs`, `wm_top_panel.rs`, `wm_bottom_panel.rs`, `wm_menu_overlay.rs`, `wm_help_overlay.rs`, `wm_keybinding_overlay.rs`, `wm_debug_log.rs`

## Implementation Order

1. **Scaffolding** — `mouse_coord.rs`, `events.rs`, `hitbox_registry.rs`. Add `smallvec` to `term-wm-core/Cargo.toml` dependencies. `ClipStack` uses `SmallVec<[Rect; CLIP_STACK_INLINE_CAPACITY]>` — zero heap allocation at depth ≤ 5.

2. **Trait + registry plumbing** — Add `&mut HitboxRegistry` param to `Component::render()`. Add `HitboxRegistry` to `WindowManager`. Implement `build_hitbox_registry()` in `prepare_draw()` — register window, chrome, panel, overlay rects. Implement `push_clip/pop_clip`. **compile check only** — dispatch still goes through old tree-walk.

3. **Implement `dispatch_mouse`** — Exchange the mouse branch in `runner.rs` to call `window_manager.dispatch_mouse(&wm_event)` instead of tree-dispatch. Old tree-walk code (`dispatch_focused_event`, `handle_managed_event`) still exists but is no longer reached for mouse. Key routing unchanged.

4. **Remove old mouse dispatch** — Delete `dispatch_focused_event` mouse handling, `handle_managed_event`, `localize_event_content`, `adjust_event_for_window` mouse portion, `hit_test_region*`, `hover_pos` from ComponentContext, `hover` field from WindowManager.

5. **Component self-registration** — Add `ComponentId` to interactive components that need targeted dispatch. Implement `registry.register(...)` calls in their `render()` methods.

6. **ScrollView clipping** — Add `push_clip/pop_clip` calls to `ScrollViewComponent::render()`.

7. **Verification**:
   ```bash
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   cargo test --workspace
   ```

## Verification Checklist

- [ ] Floating window click: registry resolves top-most window, dispatches to its component
- [ ] Overlay click: overlay clip rect + registration intercepts before window
- [ ] Chrome resize: drag starts from registered resize-edge entry
- [ ] Header button: close/max/min registered in registry, dispatched to correct handler
- [ ] Scrollview clipping: button scrolled out of view is absent from registry, click passes through
- [ ] Nested ScrollView: grandchild with double clip rect is correctly clipped
- [ ] Out-of-bounds drag: no u16 underflow (i16 coordinates in MousePosition)
- [ ] Text selection: selection logic uses `to_local()` for offset computation
- [ ] Zero clip rect: component clipped to zero area is absent from registry
- [ ] Empty space click: no registry match → Ignored event, no component dispatch
- [ ] `cargo clippy` clean with `-D warnings`
- [ ] `cargo test` all pass
