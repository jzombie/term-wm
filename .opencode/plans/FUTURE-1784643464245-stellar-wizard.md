# Plan: Migrate Windows to Components in `term-wm-sys-ui-components`

## Goal

Move chrome rendering (borders, title bar, buttons) and chrome event detection out of `term-wm-console` and `WindowManager` into a self-contained `WmWindowComponent<C>` in `term-wm-sys-ui-components`. The window becomes a component that wraps its content and handles its own chrome.

## Constraints from Code Review

- **SEV-1: No micro-components in root enum.** Do NOT add drag handles, borders, or buttons as variants of `CoreWmComponent` / `AppRootComponent`. The enum size would balloon to the largest variant (TerminalComponent ~500B), wasting memory for every instance.
- **Immediate-mode for stateless chrome.** Drag handles, borders, buttons are stateless per-frame — render them directly without arena tracking.
- **If arena tracking needed:** Use a separate `SlotMap` with a dedicated `UiWidgetEnum`.

## Architecture

### After migration

```
WindowManager<AppRootComponent>
  └── AppRootComponent                     ← same enum, same memory footprint
        ├── Core(CoreWmComponent)          ← same enum, same variants
        │     ├── Terminal(ScrollViewComponent<TerminalComponent>)
        │     ├── DebugLog(WmDebugLogComponent)
        │     ├── SystemPanel(WmSystemPanelComponent)
        │     ├── SessionManager(WmSessionManagerComponent)
        │     └── Noop(NoopComponent)
        └── SvgImage(SvgImageComponent)
```

The `WmWindowComponent<C>` wraps `AppRootComponent` — it is NOT a variant. Chrome rendering is handled by `WmWindowComponent::render()` via immediate-mode drawing, NOT via sub-components tracked in the SlotMap.

The `Window` metadata struct is absorbed into `WmWindowComponent` fields (title, state, floating_rect, flags). WindowManager no longer has a separate `windows: SlotMap<WindowKey, Window>`.

### New type hierarchy

```
WindowManager stores: SlotMap<ComponentKey, WmWindowComponent<AppRootComponent>>

WmWindowComponent<C> {
    inner: C,
    // Window metadata (moved from Window struct)
    title: Option<String>,
    state: WindowState,
    floating_rect: Option<FloatRectSpec>,
    prev_floating_rect: Option<FloatRectSpec>,
    is_maximized: bool,
    borders_enabled: bool,
    header_enabled: bool,
    direct_mode: bool,
    is_system_window: bool,
    active_keyboard_focus: Option<HitboxId>,
    content_hitbox_id: HitboxId,
    // Drag/resize initiation state (per-component, minimal)
    last_header_click: Option<(Instant)>,        // for double-click → maximize
}
```

### What stays in WindowManager (unchanged)
- `z_order`, `regions`, `managed_draw_order`, `managed_layout` — tiling layout
- `FocusRing`, `macro_focus` — focus management
- `MouseCaptureState` — drag/resize ongoing state machine (complex, needs tiling tree access)
- `mouse_capture`, `drag_snap`, `snap_preview` — drag/resize continuation
- `scroll` — per-window scroll state
- `layer_manager` — chrome panels, overlays
- `hitbox_registry` — per-frame hit testing

## Implementation Steps

### Step 1: Add new `TermWmAction` variants for drag/resize initiation

**File: `crates/term-wm-core/src/actions.rs`**

Add:
```rust
StartWindowDrag { key: WindowKey, col: u16, row: u16 },
StartWindowResize { key: WindowKey, edge: ResizeEdge, col: u16, row: u16 },
```

These let the WmWindowComponent initiate drag/resize by returning these actions from `on_mouse_press`. The WindowManager handles them by setting up `MouseCaptureState` (same logic as current ChromeTarget interception).

### Step 2: Create `WmWindowComponent<C>` in `term-wm-sys-ui-components`

**New file: `crates/term-wm-sys-ui-components/src/wm_window.rs`**

```rust
pub struct WmWindowComponent<C: Component<TermWmAction>> {
    inner: C,
    pub title: Option<String>,
    pub title_set_order: Option<usize>,
    pub state: WindowState,
    pub floating_rect: Option<FloatRectSpec>,
    pub prev_floating_rect: Option<FloatRectSpec>,
    pub creation_order: usize,
    pub direct_mode: bool,
    pub is_system_window: bool,
    pub is_maximized: bool,
    pub borders_enabled: bool,
    pub header_enabled: bool,
    pub active_keyboard_focus: Option<HitboxId>,
    content_hitbox_id: HitboxId,
    // Double-click detection (SimpleClickTracker or similar)
    last_header_click: Option<Instant>,
}
```

**`Component<TermWmAction>` impl:**

- `render()`:
  1. Skip if `state != WindowState::Mapped`
  2. Compute `content_rect` from area + flags (use `chrome::metrics::content_rect`)
  3. **Immediate-mode drawing** (no sub-components):
     - Draw border chars (corners, edges) using `backend`
     - Draw header row: title text (centered), close/maximize/minimize/direct-mode buttons
     - Style: focused vs unfocused palette (from `WmConfig` / `ComponentContext`)
  4. Register chrome hitboxes via `registry`:
     - Content area: `ComponentOwner::Window(key)` with `content_hitbox_id`
     - Header drag area: unique `HitboxId` → mapped to "drag"
     - Resize edges (if floating): unique `HitboxId` per edge → mapped to "resize"
     - Buttons: unique `HitboxId` each → mapped to "close"/"maximize"/"minimize"/"direct_mode"
  5. Call `inner.render(backend, content_rect, ctx, registry)`

- `handle_events()`:
  1. Check `ctx.active_hitbox()` against registered chrome hitbox IDs:
     - **Drag area hitbox** (press) → return `EventResult::Actions([TermWmAction::StartWindowDrag { key, col, row }])`
       - Also: double-click within 500ms → `MaximizeWindow` instead
     - **Resize edge hitbox** (press) → return `EventResult::Actions([TermWmAction::StartWindowResize { key, edge, col, row }])`
     - **Close button** → return `EventResult::Actions([TermWmAction::CloseWindow])`
     - **Maximize button** → return `EventResult::Actions([TermWmAction::MaximizeWindow])`
     - **Minimize button** → return `EventResult::Actions([TermWmAction::MinimizeWindow])`
     - **Direct mode button** → return `EventResult::Actions([TermWmAction::ToggleDirectMode])`
     - **Content area / other** → delegate to `inner.handle_events(event, ctx)`
  2. Keyboard events → delegate to `inner.handle_events(event, ctx)`
  3. For header double-click tracking: store `last_header_click` on press in drag area

- All other `Component` trait methods (`init`, `on_mount`, `update`, `on_key`, etc.) delegate to `inner`.

**Export `WmWindowComponent` from `crates/term-wm-sys-ui-components/src/lib.rs`.**

**Dependencies added to `term-wm-sys-ui-components/Cargo.toml`:**
- `term_wm_core` (already present) — need `chrome::metrics` and `chrome::target` access

### Step 3: Move chrome metric constants to `term-wm-core` (already there)

Already in `crates/term-wm-core/src/chrome/metrics.rs`. The `content_rect` function can be used by both console (for layout) and the new component. No move needed.

### Step 4: Refactor `WindowManager` to remove `Window` metadata struct

**File: `crates/term-wm-core/src/window/window_manager/mod.rs`**

Changes:
1. Remove `windows: SlotMap<WindowKey, Window>` field
2. Change `components: SlotMap<ComponentKey, C>` to use `WmWindowComponent<C>` as the generic
   - Actually, `WindowManager` is generic over `C: Component<TermWmAction>`, so the user of WindowManager passes `WmWindowComponent<AppRootComponent>` as `C`.
   - The `C` in `WindowManager<C>` changes meaning: it's now the window wrapper, not just the content.
   - So we have: `components: SlotMap<ComponentKey, C>` where `C = WmWindowComponent<AppRootComponent>`.
3. `create_window()` now takes the full `WmWindowComponent<C>` instead of `C`:
   ```rust
   pub fn create_window(&mut self, component: C) -> WindowKey {
       let component_key = self.components.insert(component);
       self.windows.insert(Window::new(order, component_key))
   }
   ```
   Becomes:
   ```rust
   pub fn create_window(&mut self, mut window_comp: WmWindowComponent<C>) -> WindowKey {
       let component_key = self.components.insert(window_comp);
       // No separate Window struct needed
       WindowKey(component_key) // or keep WindowKey as-is
   }
   ```

   Wait — `C` is used both as the generic param of `WindowManager<C>` and as the stored type. If we change the generic from `C: Component<TermWmAction>` to `C: Into<WmWindowComponent<Content>>` or similar, that changes the API.

   Better approach: Keep `WindowManager<C: Component<TermWmAction>>`. The caller passes `WmWindowComponent<AppRootComponent>` as `C`. Inside:
   ```rust
   components: SlotMap<ComponentKey, C>,  // C = WmWindowComponent<AppRootComponent>
   ```
   `create_window` directly inserts the component. No separate `Window` struct needed.

   But `WindowManager` needs to access window metadata that was on `Window`. The `WmWindowComponent` exposes methods:
   - `.title()`, `.state()`, `.floating_rect()`, `.is_maximized()`, etc.
   - Setters: `.set_title()`, `.set_state()`, `.set_floating_rect()`, etc.

   The WindowManager calls these through its component accessor methods (e.g., `component_for_key_mut(key).set_floating_rect(...)`).

4. Update all WindowManager methods that accessed `self.windows.get(key)` to instead use `component_for_key(key)`:
   - `close_window`, `toggle_maximize`, `minimize_window`, `toggle_direct_mode`
   - `transition_window`
   - `set_direct_mode`, `direct_mode`
   - `set_floating_rect`, `floating_rect`, `clear_floating_rect`
   - `title_or_default`, etc.

5. **Remove `ChromeTarget` dispatch from `dispatch_mouse` Phase 2.** No more `ComponentOwner::Chrome` match — chrome events go through the component's `handle_events` like any other component event. The component returns `TermWmAction::StartWindowDrag`, `TermWmAction::StartWindowResize`, etc.

6. **Handle new actions** in the action processing loop (or wherever TermWmActions are handled):
   - `StartWindowDrag` → same logic as current `init_window_drag`
   - `StartWindowResize` → same logic as current resize initiation

7. Remove `chrome/target.rs` (ChromeTarget enum) — no longer needed.

### Step 5: Remove chrome rendering from `term-wm-console`

**File: `crates/term-wm-console/src/draw_plan_renderer.rs`**

Changes:
1. Remove `composite_window()`, `render_window_chrome()`, `render_window()`, `register_window_chrome_hitboxes()` functions
2. Remove `ChromeCtx` struct
3. In the `render_app` flow (in `src/lib.rs`): instead of calling `composite_window()`, just call `component.render(backend, area, ctx, registry)` directly. The chrome is now rendered inside the component.

**File: `src/lib.rs` (render_app function)**

The window rendering loop changes from:
```rust
composite_window(backend, &surface, key, content_hitbox_id, chrome_ctx, |backend, content_bounds| {
    component.render(backend, content_bounds, &ctx, &mut local_hb);
}, &mut scratch);
```
To:
```rust
component.render(backend, full_area, &ctx, &mut local_hb);
```
The component handles chrome + content rendering internally.

### Step 6: Clean up obsolete types

1. **Remove `crates/term-wm-core/src/chrome/target.rs`** — `ChromeTarget` enum no longer needed
2. **Remove `crates/term-wm-core/src/window/entry.rs`** — `Window` struct no longer needed (fields migrated to `WmWindowComponent`)
3. **Remove `WindowState` from `term-wm-core`** — move it to `WmWindowComponent` or keep in core as shared type
   - Probably keep `WindowState` in core since `WindowManager` still references states

### Step 7: Update exports and crate interfaces

- Update `crates/term-wm-sys-ui-components/src/lib.rs` to export `WmWindowComponent`
- Update `crates/term-wm-core/src/window/mod.rs` to remove `WindowState` / `ComponentKey` / `OverlayKey` (if no longer needed) — most likely still needed
- Update `crates/term-wm-ui-facade/src/core_component.rs` — no change needed (it wraps content components, not the window wrapper)
- Update `src/components.rs` — `AppRootComponent` stays the same

### Step 8: Handle the `OverlayKey` / overlays

Overlays are already separate (`SlotMap<OverlayKey, Box<dyn Overlay<TermWmAction>>>`) and are not affected by this refactor.

## Critical Details

### How chrome hitbox routing works (new model)

1. During `render()`, `WmWindowComponent` registers hitboxes via `registry.register()` for each chrome element, all with `ComponentOwner::Window(key)`.
2. Each hitbox gets a unique `HitboxId` (stored on the component for the frame).
3. During `handle_events()`, the component checks `ctx.active_hitbox()`:
   - If it matches a chrome hitbox → handle chrome action
   - If it matches `content_hitbox_id` → delegate to `inner.handle_events()`
4. `WindowManager::dispatch_mouse()` sends all events (including chrome) to `comp.handle_events()`. No more `ComponentOwner::Chrome` path.

### Drag/resize initiation flow (new model)

```
1. User presses mouse on header
2. dispatch_mouse hit-tests → ComponentOwner::Window(key), builds ComponentContext
3. WmWindowComponent::handle_events → sees active_hitbox is drag area
4. Returns EventResult::Actions([TermWmAction::StartWindowDrag { key, col, row }])
5. WindowManager handles action → sets up MouseCaptureState::DraggingWindow
6. Subsequent Drag/Release events → Phase 1 of dispatch_mouse routes to WM's drag handler
```

### Memory safety (addressing the code review SEV-1)

- `WmWindowComponent<C>` is NOT a variant of `AppRootComponent` / `CoreWmComponent`.
- Chrome elements (borders, buttons, drag area) are rendered via immediate-mode drawing operations — no struct allocations, no arena storage.
- Chrome hitboxes are registered in the per-frame `HitboxRegistry` like any other component's hitboxes — ephemeral, rebuilt every frame.
- The memory footprint is: `WmWindowComponent<C>` = `C` + ~48 bytes of metadata fields. This is stored in the existing `SlotMap<ComponentKey, C>`, which was already storing components of various sizes.

### Edge cases

- **Min/max content sizes**: `content_rect()` returns `Rect::default()` if window too small — handled same as before.
- **Border button hover state**: Tracked per-frame via `ctx.hover_pos()` or stored temporarily during render.
- **Direct mode**: `direct_mode` flag on `WmWindowComponent` — when true, chrome is hidden (no borders/header drawn), clicks go straight to inner component.
- **Tiled vs floating**: Different border styles (rounded vs square corners), floating resize edges — both handled inside `render()` via immediate-mode drawing.
- **Offscreen rendering**: `composite_window` currently does offscreen compositing with drop shadows. In the new model, `WmWindowComponent::render()` receives the area directly. Drop shadows need to be handled differently — either:
  - The component draws shadows in its own area (if the area includes shadow margin)
  - Or the render_app function handles shadow composition (less ideal)
  - For now, preserve offscreen compositing by having the draw_plan_renderer handle the shadow pass, while chrome rendering moves into the component.

Actually, for the shadow/buffer compositing, we might want to keep `composite_window` as a thin wrapper that:
1. Creates offscreen buffer
2. Calls `component.render()` (which now includes chrome)
3. Applies drop shadow
4. Blits to main buffer

This way the component doesn't need to know about buffering/shadow — it just renders to whatever `backend` it's given.

## Files to Modify

| File | Change |
|------|--------|
| `crates/term-wm-core/src/actions.rs` | Add `StartWindowDrag`, `StartWindowResize` variants |
| `crates/term-wm-sys-ui-components/src/wm_window.rs` | **NEW** — `WmWindowComponent<C>` |
| `crates/term-wm-sys-ui-components/src/lib.rs` | Export `WmWindowComponent` |
| `crates/term-wm-core/src/window/window_manager/mod.rs` | Remove `windows` SlotMap, update all Window accesses to use component_for_key, remove ChromeTarget dispatch |
| `crates/term-wm-core/src/window/mod.rs` | Remove `WindowState`? (keep if still useful) |
| `crates/term-wm-core/src/chrome/target.rs` | Remove (ChromeTarget no longer needed) |
| `crates/term-wm-console/src/draw_plan_renderer.rs` | Remove chrome rendering functions, keep only compositing/shadow utility |
| `src/lib.rs` | Update render_app to call component.render() directly |
| `crates/term-wm-ui-facade/src/core_component.rs` | No change (content wrapper stays) |
| `src/components.rs` | No change (AppRootComponent stays) |

## Verification

1. **Build check**: `cargo check --workspace --all-features`
2. **Clippy**: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
3. **Tests**: `cargo test --workspace --all-features`
4. **Manual test**: Run `cargo run` and verify:
   - Windows render with correct borders/title/buttons
   - Clicking close/maximize/minimize buttons works
   - Dragging floating windows by header works
   - Resizing floating windows works
   - Double-click header maximizes
   - Tiled windows render correctly (square corners)
   - No regressions in content rendering (terminal, debug log, etc.)
5. **Memory**: Verify `WmWindowComponent` size is reasonable (`std::mem::size_of::<WmWindowComponent<AppRootComponent>>()`)
