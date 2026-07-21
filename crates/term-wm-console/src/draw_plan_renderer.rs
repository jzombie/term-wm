use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Frame;
use term_wm_layout_engine::LayoutRect;

use crate::RatatuiBackend;
use term_wm_core::actions::TermWmAction;
use term_wm_core::component_context::ComponentContext;
use term_wm_core::components::{Component, ComponentAction, TopPanelState};
use term_wm_core::constants::{SHADOW_OFFSET_X, SHADOW_OFFSET_Y};
use term_wm_core::draw_plan::{DrawPlan, RegionType, RenderRegion, ZLayer};
use term_wm_core::hitbox_registry::{HitboxId, HitboxRegistry};
use term_wm_core::layout::floating::{ResizeEdge, ResizeHandle};
use term_wm_core::layout::rect_contains;
use term_wm_core::layout::tiling::SplitHandle;
use term_wm_core::layout::{Direction, FloatingPane, RectSpec, RegionMap};
use term_wm_core::term_color::lerp_color;
use term_wm_core::theme::{Color, Theme};
use term_wm_core::window::{ComponentTag, WindowKey, WindowManager, WindowSurface};

/// Render context for window chrome (owned by console, not core).
pub struct ChromeCtx<'a> {
    pub title: &'a str,
    pub focused: bool,
    pub floating: bool,
    pub direct_mode: bool,
    pub hover_pos: Option<(u16, u16)>,
    pub theme: term_wm_core::theme::Theme,
    pub wm_buttons: Vec<term_wm_core::window::WmButton>,
    pub borders_enabled: bool,
    pub header_enabled: bool,
}

use term_wm_core::chrome::{
    LEFT_BORDER_WIDTH, RIGHT_BORDER_WIDTH, TOP_BORDER_HEIGHT, content_rect,
};

// ── Chrome layout constants (console-specific) ──────────────
const HEADER_BUTTON_GAP: u16 = 2;
const EDGE_INDEX_ADJUST: u16 = 1;

/// Register chrome hitboxes for a window (resize, drag, close, maximize buttons).
/// Parameters for chrome hitbox registration.
struct ChromeHitboxParams {
    key: WindowKey,
    frame_size: (u16, u16),
    screen_origin: (i16, i16),
    content_hitbox_id: HitboxId,
    wm_buttons: Vec<term_wm_core::window::WmButton>,
    borders_enabled: bool,
    header_enabled: bool,
}

/// Register chrome hitboxes for a window (resize, drag, close, maximize buttons).
/// `frame_size` is (width, height) of the window frame. `screen_origin` is the screen-space
/// top-left coordinate (x, y). Also registers a content-area hitbox.
fn register_window_chrome_hitboxes(registry: &mut HitboxRegistry, params: &ChromeHitboxParams) {
    use term_wm_core::chrome::ChromeTarget;
    use term_wm_core::hitbox_registry::ComponentOwner;
    use term_wm_core::layout::floating::ResizeEdge;

    let ChromeHitboxParams {
        key,
        frame_size,
        screen_origin,
        content_hitbox_id,
        wm_buttons,
        borders_enabled,
        header_enabled,
    } = params;
    let (width, height) = *frame_size;
    let (ox, oy) = *screen_origin;

    // Build screen-space rect from local offsets — no double-translation risk
    let to_screen = |lx: u16, ly: u16, lw: u16, lh: u16| -> LayoutRect {
        LayoutRect {
            x: i32::from(ox) + i32::from(lx),
            y: i32::from(oy) + i32::from(ly),
            width: lw,
            height: lh,
        }
    };

    let outer_right = width.saturating_sub(EDGE_INDEX_ADJUST);
    let bottom_y = height.saturating_sub(EDGE_INDEX_ADJUST);

    // Resize handles at each edge (only if borders are enabled)
    if *borders_enabled {
        for (edge, lx, ly, lw, lh) in [
            (ResizeEdge::Left, 0u16, 1u16, 1u16, height.saturating_sub(2)),
            (
                ResizeEdge::Right,
                outer_right,
                1u16,
                1u16,
                height.saturating_sub(2),
            ),
            (ResizeEdge::Top, 1u16, 0u16, width.saturating_sub(2), 1u16),
            (
                ResizeEdge::Bottom,
                1u16,
                bottom_y,
                width.saturating_sub(2),
                1u16,
            ),
        ] {
            registry.register(
                HitboxId::new(),
                ComponentOwner::Chrome(ChromeTarget::Resize(*key, edge)),
                to_screen(lx, ly, lw, lh),
            );
        }
    }

    // Drag handle at the header area (only if header is enabled)
    if *header_enabled {
        let drag_y = if *borders_enabled {
            TOP_BORDER_HEIGHT
        } else {
            0
        };
        let drag_x = if *borders_enabled {
            LEFT_BORDER_WIDTH
        } else {
            0
        };
        let drag_w = if *borders_enabled {
            width.saturating_sub(LEFT_BORDER_WIDTH + RIGHT_BORDER_WIDTH)
        } else {
            width
        };
        registry.register(
            HitboxId::new(),
            ComponentOwner::Chrome(ChromeTarget::Drag(*key)),
            to_screen(drag_x, drag_y, drag_w, 1),
        );

        // Window management buttons from centralized list
        for (i, btn) in wm_buttons.iter().enumerate() {
            let bx = outer_right
                .saturating_sub(HEADER_BUTTON_GAP)
                .saturating_sub(HEADER_BUTTON_GAP * i as u16);
            let target = match btn.action {
                TermWmAction::CloseWindow => ChromeTarget::CloseButton(*key),
                TermWmAction::MaximizeWindow => ChromeTarget::MaximizeButton(*key),
                TermWmAction::MinimizeWindow => ChromeTarget::MinimizeButton(*key),
                TermWmAction::ToggleDirectMode => ChromeTarget::ToggleDirectMode(*key),
                _ => continue,
            };
            registry.register(
                HitboxId::new(),
                ComponentOwner::Chrome(target),
                to_screen(bx, drag_y, 1, 1),
            );
        }
    }

    // Corner resize hitboxes (registered after edges for LIFO priority)
    if *borders_enabled {
        for (edge, lx, ly) in [
            (ResizeEdge::TopLeft, 0u16, 0u16),
            (ResizeEdge::TopRight, outer_right, 0u16),
            (ResizeEdge::BottomLeft, 0u16, bottom_y),
            (ResizeEdge::BottomRight, outer_right, bottom_y),
        ] {
            registry.register(
                HitboxId::new(),
                ComponentOwner::Chrome(ChromeTarget::Resize(*key, edge)),
                to_screen(lx, ly, 1, 1),
            );
        }
    }

    // Content area hitbox — use shared content_rect function
    let content_full = term_wm_core::Rect {
        x: 0,
        y: 0,
        width,
        height,
    };
    let inner = content_rect(content_full, *borders_enabled, *header_enabled);
    registry.register(
        *content_hitbox_id,
        ComponentOwner::Window(*key),
        to_screen(inner.x as u16, inner.y as u16, inner.width, inner.height),
    );
}

/// Single-pass window chrome rendering: draw chrome, register hitboxes (chrome + content),
/// return inner content bounds. `frame_size` is (width, height) of the window frame.
/// `screen_origin` is the screen-space top-left coordinate (x as i16, y as i16).
pub fn render_window_chrome(
    buffer: &mut Buffer,
    registry: &mut HitboxRegistry,
    key: WindowKey,
    frame_size: (u16, u16),
    screen_origin: (i16, i16),
    content_hitbox_id: HitboxId,
    ctx: &ChromeCtx<'_>,
) -> LayoutRect {
    let (width, height) = frame_size;
    let local_bounds = LayoutRect {
        x: 0,
        y: 0,
        width,
        height,
    };

    // Draw chrome using existing renderer
    render_window(
        buffer,
        local_bounds,
        ChromeCtx {
            title: ctx.title,
            focused: ctx.focused,
            floating: ctx.floating,
            direct_mode: ctx.direct_mode,
            hover_pos: ctx.hover_pos,
            theme: ctx.theme,
            wm_buttons: ctx.wm_buttons.clone(),
            borders_enabled: ctx.borders_enabled,
            header_enabled: ctx.header_enabled,
        },
    );

    // Register chrome hitboxes + content hitbox (atomic)
    register_window_chrome_hitboxes(
        registry,
        &ChromeHitboxParams {
            key,
            frame_size,
            screen_origin,
            content_hitbox_id,
            wm_buttons: ctx.wm_buttons.clone(),
            borders_enabled: ctx.borders_enabled,
            header_enabled: ctx.header_enabled,
        },
    );

    // Return inner content bounds (single source of truth: content_rect)
    let full_area = term_wm_core::Rect {
        x: 0,
        y: 0,
        width,
        height,
    };
    let inner = content_rect(full_area, ctx.borders_enabled, ctx.header_enabled);
    LayoutRect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: inner.height,
    }
}

/// Convert LayoutRect to Ratatui Rect
fn layout_rect_to_clipped_rect(layout: LayoutRect) -> Rect {
    Rect {
        x: layout.x as u16,
        y: layout.y as u16,
        width: layout.width,
        height: layout.height,
    }
}

/// Copy cells from source buffer to destination buffer within the given area.
fn blit_buffer(src: &Buffer, dst: &mut Buffer, area: Rect) {
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = src.cell((x, y))
                && let Some(dst_cell) = dst.cell_mut((x, y))
            {
                *dst_cell = cell.clone();
            }
        }
    }
}

/// Draw plan renderer that consumes the spatial IR and renders components.
/// Uses swap-based rendering for zero-allocation steady-state.
/// Holds persistent offscreen buffers that are swapped (not reallocated)
/// each frame.  The caller takes a buffer via `take_scratch()`, resizes
/// it for the current window, renders into it, blits the result to the
/// main buffer, then returns it via `put_scratch()`.  After the first
/// frame the Buffer capacity is stable — no heap allocations in steady
/// state.
pub struct DrawPlanRenderer {
    scratch_buffer: Buffer,
    direct_buffer: Buffer,
}

impl DrawPlanRenderer {
    pub fn new() -> Self {
        Self {
            scratch_buffer: Buffer::empty(Rect::ZERO),
            direct_buffer: Buffer::empty(Rect::ZERO),
        }
    }

    /// Downcast a `&mut dyn term_wm_render::RenderBackend` to `&mut RatatuiBackend`.
    pub fn downcast_to_ratatui<'a>(
        &self,
        backend: &'a mut dyn crate::RenderBackend,
    ) -> Option<&'a mut RatatuiBackend> {
        backend.as_any_mut().downcast_mut::<RatatuiBackend>()
    }

    /// Render the draw plan directly to a buffer (no Frame needed).
    pub fn render_to_buffer(
        &mut self,
        target_buf: &mut Buffer,
        draw_plan: &DrawPlan,
        wm: &mut WindowManager,
        hitbox_registry: &mut HitboxRegistry,
    ) {
        for region in draw_plan.regions() {
            // Skip hidden regions (used for monocle mode culling)
            if region.hidden {
                continue;
            }

            let area = layout_rect_to_clipped_rect(region.bounds);

            match &region.region_type {
                RegionType::Window(key) => {
                    if let Some(component) = wm.component_for_key_mut(*key) {
                        if region.layer <= ZLayer::TiledWindow {
                            self.render_window_composite_to_buffer(
                                target_buf,
                                area,
                                component,
                                region,
                                hitbox_registry,
                            );
                        } else {
                            self.render_direct_to_buffer(target_buf, area, component, region);
                        }
                    }
                }
                RegionType::Notification(msg) => {
                    use ratatui::style::{Color, Style};
                    use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};

                    Clear.render(area, target_buf);
                    let block = Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::White))
                        .style(Style::default().bg(Color::DarkGray).fg(Color::White));
                    Paragraph::new(msg.as_ref())
                        .block(block)
                        .wrap(Wrap { trim: true })
                        .render(area, target_buf);
                }
                RegionType::FloatingWindow(key) => {
                    // Floating windows are rendered like regular windows
                    if let Some(component) = wm.component_for_key_mut(*key) {
                        self.render_direct_to_buffer(target_buf, area, component, region);
                    }
                }
                RegionType::Panel(_) => {
                    // TOOD: Address
                    // Panels are rendered by the WindowManager
                    // This is a placeholder for now
                }
                RegionType::Overlay => {
                    // TOOD: Address
                    // Overlays are rendered by the WindowManager
                    // This is a placeholder for now
                }
                RegionType::TargetHighlight(_key) => {
                    // TOOD: Address
                    // Target highlight is a pulsing border overlay
                    // This is a placeholder for now
                }
            }
        }
    }

    /// Render a window with offscreen compositing into a target buffer.
    fn render_window_composite_to_buffer(
        &mut self,
        target_buf: &mut Buffer,
        area: Rect,
        component: &mut dyn Component<TermWmAction>,
        region: &RenderRegion,
        hitbox_registry: &mut HitboxRegistry,
    ) {
        let mut buffer = std::mem::replace(&mut self.scratch_buffer, Buffer::empty(Rect::ZERO));
        buffer.resize(area);
        buffer.reset();

        let mut backend = RatatuiBackend::new(buffer, area);
        let ctx = ComponentContext::new(!region.dimmed).with_screen_area(region.bounds);
        component.render(&mut backend, region.bounds, &ctx, hitbox_registry);

        if region.dimmed {
            self.apply_dim_modifier(&mut backend.buffer);
        }

        blit_buffer(&backend.buffer, target_buf, area);
        self.scratch_buffer = backend.buffer;
    }

    /// Render directly into target buffer (panels, overlays).
    fn render_direct_to_buffer(
        &mut self,
        target_buf: &mut Buffer,
        area: Rect,
        component: &mut dyn Component<TermWmAction>,
        region: &RenderRegion,
    ) {
        let mut buffer = std::mem::replace(&mut self.direct_buffer, Buffer::empty(Rect::ZERO));
        buffer.resize(area);
        buffer.reset();

        let mut backend = RatatuiBackend::new(buffer, area);
        let ctx = ComponentContext::new(true).with_screen_area(region.bounds);
        component.render(
            &mut backend,
            region.bounds,
            &ctx,
            &mut HitboxRegistry::new(),
        );

        blit_buffer(&backend.buffer, target_buf, area);
        self.direct_buffer = backend.buffer;
    }

    /// Render a notification toast into the target buffer.
    pub fn render_notification(
        &self,
        backend: &mut dyn crate::RenderBackend,
        area: Rect,
        msg: &str,
    ) {
        use ratatui::style::{Color, Style};
        use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget, Wrap};

        let Some(rb) = backend.as_any_mut().downcast_mut::<RatatuiBackend>() else {
            return;
        };
        let buf = &mut rb.buffer;
        Clear.render(area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::White))
            .style(Style::default().bg(Color::Reset).fg(Color::White));
        Paragraph::new(msg)
            .block(block)
            .wrap(Wrap { trim: true })
            .render(area, buf);
    }

    /// Render the draw plan to the terminal frame.
    /// This is the ONLY place where Ratatui types are used for rendering.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        draw_plan: &DrawPlan,
        wm: &mut WindowManager,
        hitbox_registry: &mut HitboxRegistry,
    ) {
        for region in draw_plan.regions() {
            // Skip hidden regions (used for monocle mode culling)
            if region.hidden {
                continue;
            }

            let area = layout_rect_to_clipped_rect(region.bounds);

            match &region.region_type {
                RegionType::Window(key) => {
                    if let Some(component) = wm.component_for_key_mut(*key) {
                        if region.layer <= ZLayer::TiledWindow {
                            self.render_window_composite(
                                frame,
                                area,
                                component,
                                region,
                                hitbox_registry,
                            );
                        } else {
                            self.render_direct(frame, area, component, region);
                        }
                    }
                }
                RegionType::Notification(msg) => {
                    use ratatui::style::{Color, Style};
                    use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};

                    let buf = frame.buffer_mut();
                    Clear.render(area, buf);
                    let block = Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::White))
                        .style(Style::default().bg(Color::DarkGray).fg(Color::White));
                    Paragraph::new(msg.as_ref())
                        .block(block)
                        .wrap(Wrap { trim: true })
                        .render(area, buf);
                }
                RegionType::FloatingWindow(key) => {
                    // Floating windows are rendered like regular windows
                    if let Some(component) = wm.component_for_key_mut(*key) {
                        self.render_direct(frame, area, component, region);
                    }
                }
                RegionType::Panel(_) => {
                    // Panels are rendered by the WindowManager
                    // This is a placeholder for now
                }
                RegionType::Overlay => {
                    // Overlays are rendered by the WindowManager
                    // This is a placeholder for now
                }
                RegionType::TargetHighlight(_key) => {
                    // Target highlight is a pulsing border overlay
                    // This is a placeholder for now
                }
            }
        }
    }

    /// Render a window with offscreen compositing (swap-based, zero-allocation).
    fn render_window_composite(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        component: &mut dyn Component<TermWmAction>,
        region: &RenderRegion,
        hitbox_registry: &mut HitboxRegistry,
    ) {
        // Swap persistent buffer out (leaves empty buffer in place)
        let mut buffer = std::mem::replace(&mut self.scratch_buffer, Buffer::empty(Rect::ZERO));

        // Resize and clear the swapped buffer (no allocation after warmup)
        buffer.resize(area);
        buffer.reset();

        // Create backend owning the buffer (satisfies 'static for Any)
        let mut backend = RatatuiBackend::new(buffer, area);

        // Create component context with screen area
        let ctx = ComponentContext::new(!region.dimmed).with_screen_area(region.bounds);

        // Component renders itself into the backend
        component.render(&mut backend, region.bounds, &ctx, hitbox_registry);

        // Apply dim modifier if needed
        if region.dimmed {
            self.apply_dim_modifier(&mut backend.buffer);
        }

        // Blit to main frame
        blit_buffer(&backend.buffer, frame.buffer_mut(), area);

        // Swap buffer back to preserve capacity (zero-allocation)
        self.scratch_buffer = backend.buffer;
    }

    /// Render directly to frame (panels, overlays).
    fn render_direct(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        component: &mut dyn Component<TermWmAction>,
        region: &RenderRegion,
    ) {
        // Swap direct buffer out
        let mut buffer = std::mem::replace(&mut self.direct_buffer, Buffer::empty(Rect::ZERO));

        // Resize and clear (no allocation after warmup)
        buffer.resize(area);
        buffer.reset();

        // Create backend owning the buffer
        let mut backend = RatatuiBackend::new(buffer, area);

        // Create component context
        let ctx = ComponentContext::new(true).with_screen_area(region.bounds);

        // Component renders into the swapped buffer
        component.render(
            &mut backend,
            region.bounds,
            &ctx,
            &mut HitboxRegistry::new(),
        );

        // Blit to frame
        blit_buffer(&backend.buffer, frame.buffer_mut(), area);

        // Swap buffer back to preserve capacity
        self.direct_buffer = backend.buffer;
    }

    /// Apply DIM modifier to a buffer (for unfocused windows).
    fn apply_dim_modifier(&self, buffer: &mut Buffer) {
        let area = buffer.area;
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if let Some(cell) = buffer.cell_mut((x, y))
                    && !cell.symbol().starts_with(' ')
                {
                    cell.modifier.insert(ratatui::style::Modifier::DIM);
                }
            }
        }
    }

    /// Take the persistent scratch buffer for offscreen compositing.
    /// Leaves a zero-sized Buffer in its place so the caller can resize
    /// and fill it.  Call `put_scratch` when done to preserve the
    /// allocated capacity for the next frame.
    pub fn take_scratch(&mut self) -> Buffer {
        std::mem::replace(&mut self.scratch_buffer, Buffer::empty(Rect::ZERO))
    }

    /// Return a scratch buffer taken with `take_scratch`.
    pub fn put_scratch(&mut self, buf: Buffer) {
        self.scratch_buffer = buf;
    }
}

impl Default for DrawPlanRenderer {
    fn default() -> Self {
        Self::new()
    }
}

// ── Rendering functions (called by render_app in lib.rs) ──────────────

pub fn render_panels(backend: &mut dyn term_wm_render::RenderBackend, wm: &mut WindowManager) {
    let status_line = if wm.command_menu_visible() {
        Some("Tab/Shift-Tab: cycle windows".to_string())
    } else {
        None
    };
    let display = wm.build_display_order();
    let titles_map: std::collections::BTreeMap<WindowKey, String> =
        wm.window_titles().into_iter().collect();
    let panel_active = wm.panel_active();
    let focus_current = wm.focused_window();
    let mouse_capture_enabled = wm.mouse_capture_enabled();
    let clipboard_enabled = wm.clipboard_enabled();
    let window_selection_enabled = wm.window_selection_enabled();
    let selection_active = wm.selection_active();
    let selection_dragging = wm.selection_dragging();
    let wm_overlay_visible = wm.command_menu_visible();

    // Top panel
    {
        if let Some(p) = wm.get_semantic_component_mut(ComponentTag::TopPanel) {
            p.process_action(&ComponentAction::SetPanelActive(panel_active));
            p.process_action(&ComponentAction::SetWindowLabels(titles_map));
            p.process_action(&ComponentAction::SetTopPanelState(Box::new(
                TopPanelState {
                    focus_current: Some(focus_current),
                    display_order: display,
                    status_line,
                    mouse_capture_enabled,
                    clipboard_enabled,
                    window_selection_enabled,
                    selection_active,
                    selection_dragging,
                    menu_open: wm_overlay_visible,
                },
            )));
        }
    }
    let top_area = wm.top_claimed_area();
    let top_ctx = wm.component_context(false);
    {
        let mut local_hb = HitboxRegistry::new();
        if let Some(p) = wm.get_semantic_component_mut(ComponentTag::TopPanel) {
            p.render(backend, top_area, &top_ctx, &mut local_hb);
        }
        wm.hitbox_registry_mut().merge(local_hb);
    }
    // Bottom panel
    let bottom_area = wm.bottom_claimed_area();
    let bottom_ctx = wm.component_context(panel_active);
    {
        let mut local_hb = HitboxRegistry::new();
        if let Some(p) = wm.get_semantic_component_mut(ComponentTag::BottomPanel) {
            p.render(backend, bottom_area, &bottom_ctx, &mut local_hb);
        }
        wm.hitbox_registry_mut().merge(local_hb);
    }
}

/// Returns (shadow_rect, z_depth) pairs for all visible overlays
/// that request a drop shadow.
pub fn overlay_shadow_data(
    wm: &WindowManager,
    area: LayoutRect,
    z_base: usize,
    z_total: usize,
) -> Vec<(LayoutRect, f32)> {
    let mut data = Vec::new();
    for (idx, (_, overlay)) in wm.overlays().iter().enumerate() {
        if let Some(rect) = overlay.shadow_rect(area) {
            let z = WindowManager::compute_z_depth(z_base + idx, z_total);
            data.push((rect, z));
        }
    }
    data
}

/// Render all active overlays (command menu, help, exit confirm).
pub fn render_overlays(backend: &mut dyn term_wm_render::RenderBackend, wm: &mut WindowManager) {
    let full_area = wm.managed_area();

    // Panel overlay in monocle mode — render BEFORE command menu so the panel
    // header (including hamburger icon) appears as a visual context layer.
    // Use explicit SetPanelActive(true/false) because ComponentContext's active
    // flag does NOT control WmTopPanelComponent's internal self.active guard
    // (set at wm_top_panel.rs:462 via SetPanelActive action).
    if wm.is_monocle() && wm.command_menu_visible() {
        let display = wm.build_display_order();
        let titles_map: std::collections::BTreeMap<WindowKey, String> =
            wm.window_titles().into_iter().collect();
        let focus_current = wm.focused_window();
        let mc_enabled = wm.mouse_capture_enabled();
        let cb_enabled = wm.clipboard_enabled();
        let ws_enabled = wm.window_selection_enabled();
        let sel_active = wm.selection_active();
        let sel_dragging = wm.selection_dragging();

        let top_area = LayoutRect {
            x: 0,
            y: 0,
            width: full_area.width,
            height: 1,
        };
        let mut top_hb = HitboxRegistry::new();
        if let Some(p) = wm.get_semantic_component_mut(ComponentTag::TopPanel) {
            p.process_action(&ComponentAction::SetPanelActive(true));
            p.process_action(&ComponentAction::SetWindowLabels(titles_map));
            p.process_action(&ComponentAction::SetTopPanelState(Box::new(
                TopPanelState {
                    focus_current: Some(focus_current),
                    display_order: display,
                    status_line: Some("Tab/Shift-Tab: cycle windows".to_string()),
                    mouse_capture_enabled: mc_enabled,
                    clipboard_enabled: cb_enabled,
                    window_selection_enabled: ws_enabled,
                    selection_active: sel_active,
                    selection_dragging: sel_dragging,
                    menu_open: true,
                },
            )));

            let ctx = ComponentContext::new(false).with_screen_area(top_area);
            p.render(backend, top_area, &ctx, &mut top_hb);

            // Revert to layout-derived state — the next render_panels call will
            // set the correct active state based on panel_active().
            p.process_action(&ComponentAction::SetPanelActive(false));
        }
        wm.hitbox_registry_mut().merge(top_hb);

        // Bottom panel overlay in monocle mode — keybinding hints
        let bottom_area = LayoutRect {
            x: 0,
            y: i32::from(full_area.height.saturating_sub(1)),
            width: full_area.width,
            height: 1,
        };
        let mut bottom_hb = HitboxRegistry::new();
        if let Some(p) = wm.get_semantic_component_mut(ComponentTag::BottomPanel) {
            let ctx = ComponentContext::new(false).with_screen_area(bottom_area);
            p.render(backend, bottom_area, &ctx, &mut bottom_hb);
        }
        wm.hitbox_registry_mut().merge(bottom_hb);
    }

    let hover_pos = wm.hover_pos();

    // Overlays (help, exit confirm, command palette)
    let overlay_keys = wm.overlay_keys();
    for key in overlay_keys {
        if let Some(overlay) = wm.overlay_for_key_mut(key) {
            let mut hitbox = HitboxRegistry::new();
            let ctx = ComponentContext::new(false)
                .with_overlay(true)
                .with_screen_area(full_area)
                .with_hover_pos(hover_pos);
            overlay.render(backend, full_area, &ctx, &mut hitbox);
            wm.hitbox_registry_mut().merge(hitbox);
        }
    }
}

/// Render a drop-shadow behind a floating window or overlay.
pub fn render_drop_shadow(buf: &mut Buffer, dest: LayoutRect, z_depth: f32, theme: &Theme) {
    use ratatui::style::Modifier;

    let shadow_color = lerp_color(theme.shadow_tint, theme.shadow_bg, z_depth).to_ratatui();
    let sx = dest.x.saturating_add(SHADOW_OFFSET_X);
    let sy = dest.y.saturating_add(SHADOW_OFFSET_Y);
    let ex = sx.saturating_add(i32::from(dest.width));
    let ey = sy.saturating_add(i32::from(dest.height));
    for y in sy.max(0)..ey.min(buf.area.height as i32) {
        for x in sx.max(0)..ex.min(buf.area.width as i32) {
            if let Some(cell) = buf.cell_mut((x as u16, y as u16)) {
                if !cell.symbol().starts_with(' ') {
                    cell.modifier.insert(Modifier::DIM);
                }
                cell.set_bg(shadow_color);
            }
        }
    }
}

/// Composite a single window: chrome + content in offscreen buffer, then blit.
/// Uses the provided `scratch` buffer for offscreen compositing — callers
/// should hold a persistent buffer (e.g. from `DrawPlanRenderer::take_scratch`)
/// to avoid per-frame allocation.
/// Returns the inner content bounds (after chrome inset) and the chrome hitbox registry.
pub fn composite_window<F>(
    backend: &mut dyn term_wm_render::RenderBackend,
    surface: &WindowSurface,
    key: WindowKey,
    content_hitbox_id: HitboxId,
    mut ctx: ChromeCtx<'_>,
    mut render_content: F,
    scratch: &mut Buffer,
) -> (LayoutRect, HitboxRegistry)
where
    F: FnMut(&mut dyn term_wm_render::RenderBackend, LayoutRect),
{
    if surface.dest.width == 0 || surface.dest.height == 0 {
        return (LayoutRect::default(), HitboxRegistry::new());
    }
    let local_area = Rect {
        x: 0,
        y: 0,
        width: surface.dest.width,
        height: surface.dest.height,
    };
    ctx.hover_pos = ctx.hover_pos.map(|(cx, cy)| {
        let local_x = if surface.dest.x < 0 {
            cx.saturating_add((-surface.dest.x) as u16)
        } else {
            cx.saturating_sub(surface.dest.x as u16)
        };
        let local_y = if surface.dest.y < 0 {
            cy.saturating_add((-surface.dest.y) as u16)
        } else {
            cy.saturating_sub(surface.dest.y as u16)
        };
        (local_x, local_y)
    });
    let focused = ctx.focused;
    let theme = ctx.theme;

    // Reuse caller's scratch buffer — resize instead of allocating
    scratch.resize(local_area);
    scratch.reset();
    let mut buffer = std::mem::replace(scratch, Buffer::empty(Rect::ZERO));
    let inner_bounds: LayoutRect;
    let mut chrome_registry = HitboxRegistry::new();
    {
        let mut offscreen = RatatuiBackend::new(buffer, local_area);
        // Atomic single-pass: draw chrome + register hitboxes + get inner bounds
        inner_bounds = render_window_chrome(
            &mut offscreen.buffer,
            &mut chrome_registry,
            key,
            (surface.dest.width, surface.dest.height),
            (surface.dest.x as i16, surface.dest.y as i16),
            content_hitbox_id,
            &ctx,
        );
        render_content(&mut offscreen, inner_bounds);
        buffer = offscreen.buffer;
    }
    if !focused {
        for cell in buffer.content.iter_mut() {
            cell.modifier.insert(ratatui::style::Modifier::DIM);
        }
    }
    let Some(ratatui_backend) = backend.as_any_mut().downcast_mut::<RatatuiBackend>() else {
        // Return buffer to caller before early return
        *scratch = buffer;
        return (inner_bounds, chrome_registry);
    };
    let main_buf = &mut ratatui_backend.buffer;
    if surface.draw_shadow {
        render_drop_shadow(main_buf, surface.dest, 1.0 - surface.z_depth, &theme);
    }
    let src_off_x = u16::try_from(-surface.dest.x.min(0)).unwrap_or(0);
    let src_off_y = u16::try_from(-surface.dest.y.min(0)).unwrap_or(0);
    let dest_x = surface.dest.x.max(0) as u16;
    let dest_y = surface.dest.y.max(0) as u16;
    let copy_w = local_area.width.saturating_sub(src_off_x);
    let copy_h = local_area.height.saturating_sub(src_off_y);
    for y in 0..copy_h {
        for x in 0..copy_w {
            let dst_x = dest_x.saturating_add(x);
            let dst_y = dest_y.saturating_add(y);
            if let Some(src) = buffer.cell((x + src_off_x, y + src_off_y))
                && dst_x < main_buf.area.width
                && dst_y < main_buf.area.height
                && let Some(dst) = main_buf.cell_mut((dst_x, dst_y))
            {
                *dst = src.clone();
            }
        }
    }
    // Return the resized buffer to the caller's scratch for reuse next frame
    *scratch = buffer;
    (inner_bounds, chrome_registry)
}

/// Render window chrome (borders, title bar, hover-aware buttons, direct mode indicator).
fn render_window(buffer: &mut Buffer, rect: LayoutRect, ctx: ChromeCtx<'_>) {
    use ratatui::style::{Color, Modifier, Style};

    let ChromeCtx {
        title,
        focused,
        floating,
        direct_mode,
        hover_pos,
        theme,
        wm_buttons,
        borders_enabled,
        header_enabled,
    } = ctx;

    let focused_header_style = Style::default()
        .bg(theme.decorator_header_bg.to_ratatui())
        .fg(theme.decorator_header_fg.to_ratatui())
        .add_modifier(Modifier::BOLD);
    let normal_header_style = Style::default()
        .bg(theme.panel_bg.to_ratatui())
        .fg(theme.decorator_header_fg.to_ratatui());
    let border_style = if focused {
        Style::default()
            .fg(theme.decorator_border_active.to_ratatui())
            .bg(Color::Reset)
    } else {
        Style::default()
            .fg(theme.decorator_border.to_ratatui())
            .bg(Color::Reset)
    };

    let header_style = if focused {
        focused_header_style
    } else {
        normal_header_style
    };
    let header_bg = if focused {
        theme.decorator_header_bg
    } else {
        theme.panel_bg
    };

    let outer_left = rect.x as u16;
    let outer_top = rect.y as u16;
    let outer_right = outer_left
        .saturating_add(rect.width)
        .saturating_sub(EDGE_INDEX_ADJUST);
    let outer_bottom = outer_top
        .saturating_add(rect.height)
        .saturating_sub(EDGE_INDEX_ADJUST);
    let header_y = if borders_enabled {
        outer_top.saturating_add(TOP_BORDER_HEIGHT)
    } else {
        outer_top
    };

    if header_enabled {
        for x in outer_left.saturating_add(LEFT_BORDER_WIDTH)..outer_right {
            if let Some(cell) = buffer.cell_mut((x, header_y)) {
                cell.set_symbol(" ");
                cell.set_style(header_style);
            }
        }
        let title_len = title.len() as u16;
        let header_width = outer_right
            .saturating_sub(outer_left)
            .saturating_sub(RIGHT_BORDER_WIDTH);
        if title_len <= header_width {
            let start_x =
                outer_left.saturating_add(LEFT_BORDER_WIDTH) + (header_width - title_len) / 2;
            for (idx, ch) in title.chars().enumerate() {
                let x = start_x + idx as u16;
                if let Some(cell) = buffer.cell_mut((x, header_y)) {
                    cell.set_symbol(&ch.to_string());
                    cell.set_style(header_style);
                }
            }
        }
        {
            let contrast_fg = theme.menu_selected_fg.to_ratatui();
            // Buttons are laid out right-to-left from outer_right
            for (i, btn) in wm_buttons.iter().enumerate() {
                let bx = outer_right
                    .saturating_sub(HEADER_BUTTON_GAP)
                    .saturating_sub(HEADER_BUTTON_GAP * i as u16);
                if let Some(cell) = buffer.cell_mut((bx, header_y)) {
                    cell.set_symbol(btn.symbol);
                    let stoplight_fg = match btn.action {
                        TermWmAction::CloseWindow => theme.error.to_ratatui(),
                        TermWmAction::MinimizeWindow => theme.warning.to_ratatui(),
                        TermWmAction::MaximizeWindow => theme.accent.to_ratatui(),
                        _ => theme.decorator_header_fg.to_ratatui(),
                    };
                    let is_hovered = hover_pos == Some((bx, header_y));
                    let style = if is_hovered {
                        let (hover_bg, hover_fg) = match btn.action {
                            TermWmAction::CloseWindow => (theme.error.to_ratatui(), contrast_fg),
                            TermWmAction::MinimizeWindow => {
                                (theme.warning.to_ratatui(), contrast_fg)
                            }
                            TermWmAction::MaximizeWindow => {
                                (theme.accent.to_ratatui(), contrast_fg)
                            }
                            _ => (theme.accent_alt.to_ratatui(), contrast_fg),
                        };
                        Style::default()
                            .bg(hover_bg)
                            .fg(hover_fg)
                            .add_modifier(Modifier::BOLD)
                    } else if matches!(btn.action, TermWmAction::ToggleDirectMode)
                        && direct_mode
                        && focused
                    {
                        Style::default()
                            .bg(theme.decorator_header_fg.to_ratatui())
                            .fg(theme.decorator_header_bg.to_ratatui())
                    } else {
                        Style::default().bg(header_bg.to_ratatui()).fg(stoplight_fg)
                    };
                    cell.set_style(style);
                }
            }
        }
    }

    // Borders — rounded corners for floating windows
    if borders_enabled {
        let (tl, tr, bl, br) = if floating {
            ("╭", "╮", "╰", "╯")
        } else {
            ("┌", "┐", "└", "┘")
        };
        for x in outer_left..=outer_right {
            if let Some(cell) = buffer.cell_mut((x, outer_top)) {
                let sym = if x == outer_left {
                    tl
                } else if x == outer_right {
                    tr
                } else {
                    "─"
                };
                cell.set_symbol(sym);
                cell.set_style(border_style);
            }
        }
        for x in outer_left..=outer_right {
            if let Some(cell) = buffer.cell_mut((x, outer_bottom)) {
                let sym = if x == outer_left {
                    bl
                } else if x == outer_right {
                    br
                } else {
                    "─"
                };
                cell.set_symbol(sym);
                cell.set_style(border_style);
            }
        }
        for y in outer_top.saturating_add(TOP_BORDER_HEIGHT)..outer_bottom {
            if let Some(cell) = buffer.cell_mut((outer_left, y)) {
                cell.set_symbol("│");
                cell.set_style(border_style);
            }
            if let Some(cell) = buffer.cell_mut((outer_right, y)) {
                cell.set_symbol("│");
                cell.set_style(border_style);
            }
        }
    }
}

/// Render tiling split handles with occlusion masking.
pub fn render_handles_masked(
    buffer: &mut Buffer,
    handles: &[SplitHandle],
    hovered: Option<&SplitHandle>,
    is_obscured: &dyn Fn(u16, u16) -> bool,
    theme: &Theme,
) {
    use ratatui::style::{Modifier, Style};

    let hover_rect = hovered.map(|handle| handle.rect);
    for handle in handles {
        if handle.rect.width == 0 || handle.rect.height == 0 {
            continue;
        }
        let is_hovered = hover_rect == Some(handle.rect);
        let style = if is_hovered {
            Style::default()
                .fg(theme.menu_selected_bg.to_ratatui())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.decorator_border_active.to_ratatui())
        };
        let hr = Rect {
            x: handle.rect.x.max(0) as u16,
            y: handle.rect.y.max(0) as u16,
            width: handle.rect.width,
            height: handle.rect.height,
        };
        let clip = hr.intersection(buffer.area);
        if clip.width > 0 && clip.height > 0 {
            for y in clip.y..clip.y.saturating_add(clip.height) {
                for x in clip.x..clip.x.saturating_add(clip.width) {
                    if is_obscured(x, y) {
                        continue;
                    }
                    if let Some(cell) = buffer.cell_mut((x, y)) {
                        cell.reset();
                        cell.set_symbol("·");
                        cell.set_style(style);
                    }
                }
            }
        }
        match handle.direction {
            Direction::Horizontal => {
                let x = hr.x + hr.width / 2;
                let y_center = hr.y + hr.height / 2;
                for offset in 0..3 {
                    let y = y_center.saturating_sub(1).saturating_add(offset);
                    if y < hr.y || y >= hr.y.saturating_add(hr.height) {
                        continue;
                    }
                    if is_obscured(x, y) {
                        continue;
                    }
                    if let Some(cell) = buffer.cell_mut((x, y)) {
                        cell.set_symbol(if is_hovered { "O" } else { "o" });
                        cell.set_style(style);
                    }
                }
            }
            Direction::Vertical => {
                let y = hr.y + hr.height / 2;
                let x_center = hr.x + hr.width / 2;
                for offset in 0..3 {
                    let x = x_center.saturating_sub(1).saturating_add(offset);
                    if x < hr.x || x >= hr.x.saturating_add(hr.width) {
                        continue;
                    }
                    if is_obscured(x, y) {
                        continue;
                    }
                    if let Some(cell) = buffer.cell_mut((x, y)) {
                        cell.set_symbol(if is_hovered { "O" } else { "o" });
                        cell.set_style(style);
                    }
                }
            }
        }
        if is_hovered {
            let border_style = Style::default()
                .fg(theme.accent_alt.to_ratatui())
                .add_modifier(Modifier::BOLD);
            let max_x = hr.x.saturating_add(hr.width).saturating_sub(1);
            let max_y = hr.y.saturating_add(hr.height).saturating_sub(1);
            for x in hr.x..=max_x {
                if is_obscured(x, hr.y) {
                    continue;
                }
                if let Some(cell) = buffer.cell_mut((x, hr.y)) {
                    cell.set_symbol("-");
                    cell.set_style(border_style);
                }
                if is_obscured(x, max_y) {
                    continue;
                }
                if let Some(cell) = buffer.cell_mut((x, max_y)) {
                    cell.set_symbol("-");
                    cell.set_style(border_style);
                }
            }
            for y in hr.y..=max_y {
                if is_obscured(hr.x, y) {
                    continue;
                }
                if let Some(cell) = buffer.cell_mut((hr.x, y)) {
                    cell.set_symbol("|");
                    cell.set_style(border_style);
                }
                if is_obscured(max_x, y) {
                    continue;
                }
                if let Some(cell) = buffer.cell_mut((max_x, y)) {
                    cell.set_symbol("|");
                    cell.set_style(border_style);
                }
            }
            if !is_obscured(hr.x, hr.y)
                && let Some(cell) = buffer.cell_mut((hr.x, hr.y))
            {
                cell.set_symbol("+");
                cell.set_style(border_style);
            }
            if !is_obscured(max_x, hr.y)
                && let Some(cell) = buffer.cell_mut((max_x, hr.y))
            {
                cell.set_symbol("+");
                cell.set_style(border_style);
            }
            if !is_obscured(hr.x, max_y)
                && let Some(cell) = buffer.cell_mut((hr.x, max_y))
            {
                cell.set_symbol("+");
                cell.set_style(border_style);
            }
            if !is_obscured(max_x, max_y)
                && let Some(cell) = buffer.cell_mut((max_x, max_y))
            {
                cell.set_symbol("+");
                cell.set_style(border_style);
            }
        }
    }
}

/// Render floating resize outline (double-line box-drawing characters).
#[allow(clippy::too_many_arguments)]
pub fn render_resize_outline(
    buffer: &mut Buffer,
    hovered: Option<ResizeHandle<WindowKey>>,
    dragging: Option<term_wm_core::layout::floating::ResizeDrag<WindowKey>>,
    regions: &RegionMap<WindowKey>,
    bounds: LayoutRect,
    floating: &[FloatingPane<WindowKey>],
    draw_order: &[WindowKey],
    theme: &Theme,
) {
    use ratatui::style::{Modifier, Style};

    let target_edge = dragging.map(|d| d.edge).or_else(|| hovered.map(|h| h.edge));
    let target_key = dragging.map(|d| d.key).or_else(|| hovered.map(|h| h.key));
    let Some(key) = target_key else { return };
    // Use the floating pane's original spec to compute the visible rect.
    // The region (regions.get) has already been clamped to x>=0 by
    // FloatRectSpec::resolve, so it can't accurately clip width for
    // windows partially off the left edge.  The pane spec still has the
    // original signed coordinates.
    let Some(pane_spec) = floating.iter().find(|p| p.key == key) else {
        return;
    };
    let orig_raw = match &pane_spec.rect {
        RectSpec::Absolute(r) => *r,
        RectSpec::Percent {
            x,
            y,
            width,
            height,
        } => {
            let x = *x;
            let y = *y;
            let width = *width;
            let height = *height;
            let bw = bounds.width as i32;
            let bh = bounds.height as i32;
            LayoutRect {
                x: bounds.x + (x as i32 * bw) / 100,
                y: bounds.y + (y as i32 * bh) / 100,
                width: (width as u32 * bounds.width as u32 / 100) as u16,
                height: (height as u32 * bounds.height as u32 / 100) as u16,
            }
        }
    };
    let vx0 = orig_raw.x.max(bounds.x);
    let vy0 = orig_raw.y.max(bounds.y);
    let vx1 = orig_raw
        .x
        .saturating_add(i32::from(orig_raw.width))
        .min(bounds.x.saturating_add(i32::from(bounds.width)));
    let vy1 = orig_raw
        .y
        .saturating_add(i32::from(orig_raw.height))
        .min(bounds.y.saturating_add(i32::from(bounds.height)));
    if vx1 <= vx0 || vy1 <= vy0 || orig_raw.width < 3 || orig_raw.height < 3 {
        return;
    }
    let rect = LayoutRect {
        x: vx0,
        y: vy0,
        width: u16::try_from(vx1 - vx0).unwrap_or(0),
        height: u16::try_from(vy1 - vy0).unwrap_or(0),
    };

    let obscuring: Vec<LayoutRect> = if let Some(idx) = draw_order.iter().position(|&x| x == key) {
        draw_order[idx + 1..]
            .iter()
            .filter_map(|&above_key| regions.get(above_key))
            .collect()
    } else {
        Vec::new()
    };
    let is_obscured =
        |x: u16, y: u16| -> bool { obscuring.iter().any(|r| rect_contains(*r, x, y)) };

    let right = (rect.x + i32::from(rect.width) - 1) as u16;
    let bottom = (rect.y + i32::from(rect.height) - 1) as u16;
    let rx = rect.x as u16;
    let ry = rect.y as u16;
    let bx = bounds.x as u16;
    let by = bounds.y as u16;
    let bw = bounds.width;
    let bh = bounds.height;

    let style = Style::default()
        .fg(theme.accent_alt.to_ratatui())
        .add_modifier(Modifier::BOLD);

    let Some(edge) = target_edge else { return };
    match edge {
        ResizeEdge::Top => {
            if ry >= by && ry < by.saturating_add(bh) && rect.width > 2 {
                for x in rx.saturating_add(1)..=right.saturating_sub(1) {
                    if x >= bx
                        && x < bx.saturating_add(bw)
                        && !is_obscured(x, ry)
                        && let Some(cell) = buffer.cell_mut((x, ry))
                    {
                        cell.set_symbol("═");
                        cell.set_style(style);
                    }
                }
            }
        }
        ResizeEdge::Bottom => {
            if bottom >= by && bottom < by.saturating_add(bh) && rect.width > 2 {
                for x in rx.saturating_add(1)..=right.saturating_sub(1) {
                    if x >= bx
                        && x < bx.saturating_add(bw)
                        && !is_obscured(x, bottom)
                        && let Some(cell) = buffer.cell_mut((x, bottom))
                    {
                        cell.set_symbol("═");
                        cell.set_style(style);
                    }
                }
            }
        }
        ResizeEdge::Left => {
            if rx >= bx && rx < bx.saturating_add(bw) && rect.height > 2 {
                for y in ry.saturating_add(1)..=bottom.saturating_sub(1) {
                    if y >= by
                        && y < by.saturating_add(bh)
                        && !is_obscured(rx, y)
                        && let Some(cell) = buffer.cell_mut((rx, y))
                    {
                        cell.set_symbol("║");
                        cell.set_style(style);
                    }
                }
            }
        }
        ResizeEdge::Right => {
            if right >= bx && right < bx.saturating_add(bw) && rect.height > 2 {
                for y in ry.saturating_add(1)..=bottom.saturating_sub(1) {
                    if y >= by
                        && y < by.saturating_add(bh)
                        && !is_obscured(right, y)
                        && let Some(cell) = buffer.cell_mut((right, y))
                    {
                        cell.set_symbol("║");
                        cell.set_style(style);
                    }
                }
            }
        }
        ResizeEdge::TopLeft => {
            if rx >= bx
                && ry >= by
                && !is_obscured(rx, ry)
                && let Some(cell) = buffer.cell_mut((rx, ry))
            {
                cell.set_symbol("╔");
                cell.set_style(style);
            }
            if ry >= by
                && ry < by.saturating_add(bh)
                && let Some(cell) = buffer.cell_mut((rx.saturating_add(1), ry))
            {
                cell.set_symbol("═");
                cell.set_style(style);
            }
            if rx >= bx
                && rx < bx.saturating_add(bw)
                && let Some(cell) = buffer.cell_mut((rx, ry.saturating_add(1)))
            {
                cell.set_symbol("║");
                cell.set_style(style);
            }
        }
        ResizeEdge::TopRight => {
            if right < bx.saturating_add(bw)
                && ry >= by
                && !is_obscured(right, ry)
                && let Some(cell) = buffer.cell_mut((right, ry))
            {
                cell.set_symbol("╗");
                cell.set_style(style);
            }
            if ry >= by
                && ry < by.saturating_add(bh)
                && let Some(cell) = buffer.cell_mut((right.saturating_sub(1), ry))
            {
                cell.set_symbol("═");
                cell.set_style(style);
            }
            if right >= bx
                && right < bx.saturating_add(bw)
                && let Some(cell) = buffer.cell_mut((right, ry.saturating_add(1)))
            {
                cell.set_symbol("║");
                cell.set_style(style);
            }
        }
        ResizeEdge::BottomLeft => {
            if rx >= bx
                && bottom < by.saturating_add(bh)
                && !is_obscured(rx, bottom)
                && let Some(cell) = buffer.cell_mut((rx, bottom))
            {
                cell.set_symbol("╚");
                cell.set_style(style);
            }
            if bottom >= by
                && bottom < by.saturating_add(bh)
                && let Some(cell) = buffer.cell_mut((rx.saturating_add(1), bottom))
            {
                cell.set_symbol("═");
                cell.set_style(style);
            }
            if rx >= bx
                && rx < bx.saturating_add(bw)
                && let Some(cell) = buffer.cell_mut((rx, bottom.saturating_sub(1)))
            {
                cell.set_symbol("║");
                cell.set_style(style);
            }
        }
        ResizeEdge::BottomRight => {
            if right < bx.saturating_add(bw)
                && bottom < by.saturating_add(bh)
                && !is_obscured(right, bottom)
                && let Some(cell) = buffer.cell_mut((right, bottom))
            {
                cell.set_symbol("╝");
                cell.set_style(style);
            }
            if bottom >= by
                && bottom < by.saturating_add(bh)
                && let Some(cell) = buffer.cell_mut((right.saturating_sub(1), bottom))
            {
                cell.set_symbol("═");
                cell.set_style(style);
            }
            if right >= bx
                && right < bx.saturating_add(bw)
                && let Some(cell) = buffer.cell_mut((right, bottom.saturating_sub(1)))
            {
                cell.set_symbol("║");
                cell.set_style(style);
            }
        }
    }
}

/// Render a ghost preview rectangle with dashed borders and a light shade fill.
/// Used during drag operations to show where a window will land when released.
pub fn render_ghost_preview(buf: &mut Buffer, preview_rect: LayoutRect, theme: &Theme) {
    use ratatui::style::Modifier;
    let rect = layout_rect_to_clipped_rect(preview_rect);
    let clip = rect.intersection(buf.area);
    if clip.width < 2 || clip.height < 2 {
        return;
    }

    let fg_color = theme.accent.to_ratatui();
    let left = clip.x;
    let right = clip.x + clip.width - 1;
    let top = clip.y;
    let bottom = clip.y + clip.height - 1;

    // Corners
    for &(pos, sym) in &[
        ((left, top), "┌"),
        ((right, top), "┐"),
        ((left, bottom), "└"),
        ((right, bottom), "┘"),
    ] {
        if let Some(cell) = buf.cell_mut(pos) {
            cell.set_symbol(sym);
            cell.set_fg(fg_color);
            cell.modifier.insert(Modifier::DIM);
        }
    }

    // Top/bottom edges (horizontal dashes)
    if clip.width > 2 {
        for x in (left + 1)..right {
            for &y in &[top, bottom] {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_symbol("─");
                    cell.set_fg(fg_color);
                    cell.modifier.insert(Modifier::DIM);
                }
            }
        }
    }

    // Left/right edges (vertical dashes)
    if clip.height > 2 {
        for y in (top + 1)..bottom {
            for &x in &[left, right] {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_symbol("│");
                    cell.set_fg(fg_color);
                    cell.modifier.insert(Modifier::DIM);
                }
            }
        }
    }

    // Interior shade fill — pure background tint, preserves underlying text
    if clip.width > 2 && clip.height > 2 {
        let preview_bg = theme.accent.to_ratatui();
        for y in (top + 1)..bottom {
            for x in (left + 1)..right {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_bg(preview_bg);
                }
            }
        }
    }
}

/// Convert core `Color` to ratatui `Color`.
pub trait ColorConvert {
    fn to_ratatui(self) -> ratatui::style::Color;
}

impl ColorConvert for Color {
    fn to_ratatui(self) -> ratatui::style::Color {
        match self {
            Color::Black => ratatui::style::Color::Black,
            Color::Red => ratatui::style::Color::Red,
            Color::Green => ratatui::style::Color::Green,
            Color::Yellow => ratatui::style::Color::Yellow,
            Color::Blue => ratatui::style::Color::Blue,
            Color::Magenta => ratatui::style::Color::Magenta,
            Color::Cyan => ratatui::style::Color::Cyan,
            Color::White => ratatui::style::Color::White,
            Color::Gray => ratatui::style::Color::Gray,
            Color::DarkGray => ratatui::style::Color::DarkGray,
            Color::LightRed => ratatui::style::Color::LightRed,
            Color::LightGreen => ratatui::style::Color::LightGreen,
            Color::LightYellow => ratatui::style::Color::LightYellow,
            Color::LightBlue => ratatui::style::Color::LightBlue,
            Color::LightMagenta => ratatui::style::Color::LightMagenta,
            Color::LightCyan => ratatui::style::Color::LightCyan,
            Color::Rgb(r, g, b) => ratatui::style::Color::Rgb(r, g, b),
            Color::Indexed(i) => ratatui::style::Color::Indexed(i),
        }
    }
}

/// Apply background-color inversion (`Modifier::REVERSED`) to the cell under
/// the mouse cursor.  Must be called as the **absolute last** render step
/// (highest Z-order) so it paints over all previously rendered content.
///
/// Uses style-modifier overrides only — no character replacement — so the
/// underlying text is fully preserved.  The active state (drag/resize) also
/// inverts an adjacent cell as a visual "badge", clamped to buffer boundaries.
pub fn render_cursor_overlay(buf: &mut Buffer, wm: &WindowManager, _theme: &Theme) {
    use ratatui::style::Modifier;

    // Don't render when mouse capture is disabled — the last hover position
    // would be stale and the OS pointer already provides visual feedback.
    if !wm.mouse_capture_enabled() {
        return;
    }

    let Some((hx, hy)) = wm.hover_pos() else {
        return;
    };

    if hx >= buf.area.width || hy >= buf.area.height {
        return;
    }

    // Apply REVERSED to cell under cursor (preserves text).
    if let Some(cell) = buf.cell_mut((hx, hy)) {
        cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect as RatatuiRect;
    use ratatui::style::Modifier;
    use ratatui::style::Style;
    use std::sync::Arc;
    use term_wm_core::app_context::AppContext;
    use term_wm_core::theme::NOIR;
    use term_wm_core::window::FloatRect;
    use term_wm_core::window::WmButton;
    use term_wm_core::wm_config::WmConfig;

    fn test_wm_buttons() -> Vec<WmButton> {
        vec![
            WmButton {
                action: TermWmAction::CloseWindow,
                label: "Close Window",
                symbol: "X",
            },
            WmButton {
                action: TermWmAction::MaximizeWindow,
                label: "Maximize Window",
                symbol: "▢",
            },
            WmButton {
                action: TermWmAction::MinimizeWindow,
                label: "Minimize Window",
                symbol: "_",
            },
            WmButton {
                action: TermWmAction::ToggleDirectMode,
                label: "Toggle Direct Mode",
                symbol: "D",
            },
        ]
    }

    #[test]
    fn composite_window_skips_negative_dest_x() {
        let main_area = RatatuiRect {
            x: 0,
            y: 0,
            width: 50,
            height: 20,
        };
        let mut main_buffer = Buffer::empty(main_area);
        for cell in main_buffer.content.iter_mut() {
            cell.set_symbol(".");
        }
        let mut backend = RatatuiBackend::new(main_buffer, main_area);

        let surface = WindowSurface {
            full: term_wm_core::Rect {
                x: 0,
                y: 0,
                width: 30,
                height: 8,
            },
            inner: term_wm_core::Rect {
                x: 0,
                y: 0,
                width: 30,
                height: 8,
            },
            dest: FloatRect {
                x: -5,
                y: 0,
                width: 30,
                height: 8,
            },
            draw_shadow: false,
            z_depth: 0.5,
        };

        let ctx = ChromeCtx {
            title: "test",
            focused: false,
            floating: false,
            direct_mode: false,
            hover_pos: None,
            theme: NOIR,
            wm_buttons: test_wm_buttons(),
            borders_enabled: true,
            header_enabled: true,
        };

        let mut scratch = Buffer::empty(RatatuiRect {
            x: 0,
            y: 0,
            width: 30,
            height: 8,
        });
        let (_inner, _chrome_hb) = composite_window(
            &mut backend,
            &surface,
            term_wm_core::window::WindowKey::default(),
            HitboxId::new(),
            ctx,
            |b, _inner| {
                let rb = b.as_any_mut().downcast_mut::<RatatuiBackend>().unwrap();
                if let Some(cell) = rb.buffer.cell_mut((5, 2)) {
                    cell.set_symbol("X");
                    cell.set_style(Style::default());
                }
                if let Some(cell) = rb.buffer.cell_mut((6, 2)) {
                    cell.set_symbol("Y");
                    cell.set_style(Style::default());
                }
            },
            &mut scratch,
        );

        let buf = &backend.buffer;
        assert_eq!(
            buf.cell((0, 2)).map(|c| c.symbol()),
            Some("X"),
            "source col 5 should map to main col 0 when dest.x = -5"
        );
        assert_eq!(
            buf.cell((1, 2)).map(|c| c.symbol()),
            Some("Y"),
            "source col 6 should map to main col 1 when dest.x = -5"
        );
        assert_ne!(
            buf.cell((0, 2)).map(|c| c.symbol()),
            Some("│"),
            "left border from source col 0 should NOT appear at main col 0"
        );
    }

    #[test]
    fn content_hitbox_clipped_when_dest_x_negative() {
        // A 30-col window at dest.x=-5 should have its content hitbox
        // clipped from x=-4,width=28 to x=0,width=24 (managed_area is
        // {0,0,50,20} from screen_bounds in ChromeCtx).
        let main_area = RatatuiRect {
            x: 0,
            y: 0,
            width: 50,
            height: 20,
        };
        let main_buffer = Buffer::empty(main_area);
        let mut backend = RatatuiBackend::new(main_buffer, main_area);

        let surface = WindowSurface {
            full: term_wm_core::Rect {
                x: 0,
                y: 0,
                width: 30,
                height: 8,
            },
            inner: term_wm_core::Rect {
                x: 0,
                y: 0,
                width: 30,
                height: 8,
            },
            dest: FloatRect {
                x: -5,
                y: 0,
                width: 30,
                height: 8,
            },
            draw_shadow: false,
            z_depth: 0.5,
        };

        let ctx = ChromeCtx {
            title: "test",
            focused: false,
            floating: false,
            direct_mode: false,
            hover_pos: None,
            theme: NOIR,
            wm_buttons: test_wm_buttons(),
            borders_enabled: true,
            header_enabled: true,
        };

        let mut scratch = Buffer::empty(RatatuiRect {
            x: 0,
            y: 0,
            width: 30,
            height: 8,
        });
        let (_, chrome_hb) = composite_window(
            &mut backend,
            &surface,
            term_wm_core::window::WindowKey::default(),
            HitboxId::new(),
            ctx,
            |_, _| {},
            &mut scratch,
        );

        use term_wm_core::mouse_coord::{CoordSpace, MousePosition};
        let screen = |col, row| MousePosition {
            column: col,
            row,
            space: CoordSpace::Screen,
        };

        // Column 0 is inside the visible window content → must hit
        assert!(
            chrome_hb.hit_test(screen(0, 3)).is_some(),
            "click at col 0 should hit the floating window (visible content)"
        );

        // Column 23 is inside visible content (width=24 covers [0,24))
        assert!(
            chrome_hb.hit_test(screen(23, 3)).is_some(),
            "click at col 23 should hit the floating window (right edge of content)"
        );

        // Column 25 is right beyond the visible window edge (dest.x=-5, width=30,
        // right edge at screen col 25, content hitbox clipped to [0,24), right
        // resize edge sits at col 24). Column 25 must NOT hit the floating window.
        assert!(
            chrome_hb.hit_test(screen(25, 3)).is_none(),
            "click at col 25 should NOT hit the floating window (one past right edge)"
        );
        assert!(
            chrome_hb.hit_test(screen(30, 3)).is_none(),
            "click at col 30 should NOT hit the floating window (way past right edge)"
        );
    }

    // ── render_cursor_overlay tests ──────────────────────────────────────

    fn make_wm() -> WindowManager {
        let config = WmConfig::default();
        let app_ctx = Arc::new(AppContext::new("test", "0.1.0"));
        WindowManager::with_config(
            config,
            app_ctx,
            None,
            term_wm_core::window::LayerManager::new(),
            std::collections::HashMap::new(),
        )
    }

    fn make_buf(width: u16, height: u16) -> Buffer {
        let mut buf = Buffer::empty(RatatuiRect {
            x: 0,
            y: 0,
            width,
            height,
        });
        for cell in buf.content.iter_mut() {
            cell.set_symbol("·");
        }
        buf
    }

    #[test]
    fn cursor_overlay_no_hover_is_noop() {
        let mut buf = make_buf(10, 10);
        let wm = make_wm();
        let before = buf.clone();
        render_cursor_overlay(&mut buf, &wm, &NOIR);
        assert_eq!(buf, before, "no hover → no change");
    }

    #[test]
    fn cursor_overlay_hover_outside_bounds_is_noop() {
        let mut buf = make_buf(10, 10);
        let mut wm = make_wm();
        wm.set_hover_pos(20, 20);
        let before = buf.clone();
        render_cursor_overlay(&mut buf, &wm, &NOIR);
        assert_eq!(buf, before, "hover outside buffer → no change");
    }

    #[test]
    fn cursor_overlay_hover_applies_reversed() {
        let mut buf = make_buf(10, 10);
        let mut wm = make_wm();
        wm.set_hover_pos(3, 4);
        render_cursor_overlay(&mut buf, &wm, &NOIR);
        let cell = buf.cell((3, 4)).unwrap();
        assert!(
            cell.modifier.contains(Modifier::REVERSED),
            "cell at hover position should have REVERSED modifier"
        );
        // Symbol should be preserved
        assert_eq!(cell.symbol(), "·", "symbol must not be overwritten");
    }

    #[test]
    fn cursor_overlay_zero_buffer_is_noop() {
        let mut buf = make_buf(0, 0);
        let mut wm = make_wm();
        wm.set_hover_pos(0, 0);
        let before = buf.clone();
        render_cursor_overlay(&mut buf, &wm, &NOIR);
        assert_eq!(buf, before, "zero-size buffer → no change");
    }

    #[test]
    fn cursor_overlay_single_cell_buffer() {
        let mut buf = make_buf(1, 1);
        let mut wm = make_wm();
        wm.set_hover_pos(0, 0);
        render_cursor_overlay(&mut buf, &wm, &NOIR);
        assert!(
            buf.cell((0, 0))
                .unwrap()
                .modifier
                .contains(Modifier::REVERSED),
            "single cell should be REVERSED"
        );
        assert_eq!(buf.cell((0, 0)).unwrap().symbol(), "·");
    }
}
