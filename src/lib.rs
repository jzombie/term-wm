// TODO: Include README in Rust docs

pub use term_wm_core::*;
pub use term_wm_ui_components::*;
pub mod prelude;
pub mod term_wm_app;
pub mod tracing_sub;
pub mod unified_event_source;
pub use term_wm_console::widget_adapter::{StatefulWidgetAdapter, WidgetAdapter};

use std::sync::Arc;
use term_wm_console::RatatuiBackend;
use term_wm_console::draw_plan_renderer::{
    ColorConvert, DrawPlanRenderer, composite_window, overlay_shadow_data, render_cursor_overlay,
    render_drop_shadow, render_ghost_preview, render_handles_masked, render_overlays,
    render_panels, render_resize_outline,
};
use term_wm_core::hitbox_registry::HitboxRegistry;
use term_wm_core::window::{WindowManager, WindowSurface};

/// Default rendering implementation for the window manager.
/// Shared by all apps so they don't need to reimplement rendering.
pub fn render_app(
    backend: &mut dyn term_wm_render::RenderBackend,
    wm: &mut term_wm_core::window::WindowManager,
    engine: &mut term_wm_core::engine::CoreEngine,
    renderer: &mut DrawPlanRenderer,
) {
    let Some(ratatui_backend) = backend.as_any_mut().downcast_mut::<RatatuiBackend>() else {
        return;
    };

    // Clear per-frame draw state (regions, floating headers, hitbox registry)
    // that was populated during the previous frame's render pass.
    wm.prepare_draw();

    let area = term_wm_layout_engine::LayoutRect {
        x: 0,
        y: 0,
        width: ratatui_backend.area.width,
        height: ratatui_backend.area.height,
    };

    // Initialize monocle state on every render pass (not just resize).
    // This ensures the very first frame evaluates terminal width against
    // the monocle threshold without waiting for a resize event.
    wm.update_monocle_mode(area.width);

    // Update window titles from process names
    let windows: Vec<_> = wm.mapped_windows();
    for &key in &windows {
        if let Some(title) = wm.window_pane_title(key) {
            wm.set_window_title(key, title);
        }
    }

    wm.register_managed_layout(area);
    let draw_plan = engine.project_draw_plan(area.width as u32, area.height as u32, wm);
    let all_titles: std::collections::BTreeMap<_, _> = wm.window_titles().into_iter().collect();
    let num_windows = draw_plan.len();
    let total = num_windows + wm.visible_overlay_count();

    // Register panel hitboxes BEFORE the window loop (lowest Z-order)
    wm.register_panel_hitboxes();

    // Register tiling split handle hitboxes below windows
    if !wm.is_monocle() {
        wm.register_layout_handle_hitboxes();
    }

    let decorator = wm.decorator();
    // Take the renderer's persistent scratch buffer — resized per window,
    // returned to the renderer after the loop.  No Buffer::empty allocations
    // in steady state.
    let mut scratch_buf = renderer.take_scratch();
    let plan_regions = draw_plan.regions();
    let num_windows = plan_regions.len();
    for (i, region) in plan_regions.iter().enumerate() {
        // Skip hidden regions (used for monocle mode culling)
        if region.hidden {
            continue;
        }

        match &region.region_type {
            term_wm_core::draw_plan::RegionType::Window(key) => {
                let full = region.bounds;
                if full.width == 0 || full.height == 0 {
                    continue;
                }
                let is_monocle = wm.is_monocle();
                let dest = if is_monocle {
                    term_wm_core::window::FloatRect {
                        x: full.x,
                        y: full.y,
                        width: full.width,
                        height: full.height,
                    }
                } else {
                    wm.window_dest(*key, full)
                };
                let inner = decorator.content_area(Rect {
                    x: 0,
                    y: 0,
                    width: full.width,
                    height: full.height,
                });
                if inner.width == 0 || inner.height == 0 {
                    continue;
                }
                let floating = if is_monocle {
                    false
                } else {
                    wm.is_window_floating(*key)
                };
                let focused = wm.focused_window() == *key;
                let draw_shadow = floating && wm.config().shadow_enabled;
                let z_depth = WindowManager::compute_z_depth(i, total);
                let surface = WindowSurface {
                    full,
                    inner,
                    dest,
                    draw_shadow,
                    z_depth,
                };

                // Register window content hitbox
                let decorator_ref = wm.decorator();
                let screen_inner = decorator_ref.content_area(Rect {
                    x: surface.dest.x,
                    y: surface.dest.y,
                    width: surface.dest.width,
                    height: surface.dest.height,
                });
                let content_hitbox_id = wm.window_content_hitbox_id(*key).unwrap_or_default();
                wm.hitbox_registry_mut()
                    .register(content_hitbox_id, screen_inner);

                // Register chrome hitboxes (resize handles + header)
                wm.register_window_chrome_hitboxes(*key);

                let title = all_titles.get(key).map(String::as_str).unwrap_or("");
                let win_ctx = term_wm_core::window::decorator::WindowRenderCtx {
                    title,
                    focused,
                    floating,
                    direct_mode: wm.direct_mode(*key),
                    hover_pos: wm.hover_pos(),
                    theme: wm.config().theme,
                };
                let decorator_arc = Arc::clone(&decorator);
                composite_window(
                    backend,
                    &surface,
                    decorator_arc.as_ref(),
                    win_ctx,
                    |backend, _registry| {
                        let ctx = wm
                            .component_context_for(focused, *key)
                            .with_screen_area(screen_inner);
                        if let Some(component) = wm.component_for_key_mut(*key) {
                            component.render(
                                backend,
                                surface.inner,
                                &ctx,
                                &mut HitboxRegistry::new(),
                            );
                        }
                    },
                    &mut scratch_buf,
                );
            }
            // Notification rendering deferred to after tiling handles
            term_wm_core::draw_plan::RegionType::Notification(_) => {}
            term_wm_core::draw_plan::RegionType::FloatingWindow(_) => {
                // Floating windows are rendered like regular windows
                // This is a placeholder for now
            }
            term_wm_core::draw_plan::RegionType::Panel(_) => {
                // Panels are rendered by the WindowManager
                // This is a placeholder for now
            }
            term_wm_core::draw_plan::RegionType::Overlay => {
                // Overlays are rendered by the WindowManager
                // This is a placeholder for now
            }
            term_wm_core::draw_plan::RegionType::TargetHighlight(_) => {
                // Target highlight is a pulsing border overlay
                // This is a placeholder for now
            }
        }
    }
    renderer.put_scratch(scratch_buf);

    // Render panels AFTER windows
    render_panels(backend, wm);

    // Render FAB only in monocle/mobile mode — sole mobile navigation mechanism.
    if wm.is_monocle()
        && let Some(fab) =
            wm.get_semantic_component_mut(term_wm_core::window::ComponentTag::FloatingActionButton)
    {
        let mut local_hb = HitboxRegistry::new();
        let ctx = term_wm_core::components::ComponentContext::new(true).with_screen_area(area);
        fab.render(backend, area, &ctx, &mut local_hb);
        wm.hitbox_registry_mut().merge(local_hb);
    }

    // Render tiling split handles
    {
        use term_wm_console::RatatuiBackend;
        if let Some(rb) = backend.as_any_mut().downcast_mut::<RatatuiBackend>() {
            let buf = &mut rb.buffer;
            let handles = wm.tiling_handles();
            let hovered = wm.hovered_tiling_handle();
            let managed = wm.managed_draw_order_all().to_vec();
            let regions = wm.regions();
            let obscuring: Vec<term_wm_layout_engine::LayoutRect> =
                managed.iter().filter_map(|&key| regions.get(key)).collect();
            let is_obscured = |x: u16, y: u16| -> bool {
                obscuring
                    .iter()
                    .any(|r| term_wm_core::layout::rect_contains(*r, x, y))
            };
            if !wm.is_monocle() {
                render_handles_masked(
                    buf,
                    handles,
                    hovered.as_ref(),
                    &is_obscured,
                    &wm.config().theme,
                );
            }

            // Floating resize outlines
            let hovered_resize = wm.hovered_resize_handle();
            let draw_order = wm.managed_draw_order_all();
            let floating_panes: Vec<
                term_wm_core::layout::FloatingPane<term_wm_core::window::WindowKey>,
            > = if wm.is_monocle() {
                Vec::new()
            } else {
                wm.floating_panes()
                    .into_iter()
                    .map(|(key, rect)| match rect {
                        term_wm_core::window::FloatRectSpec::Absolute(fr) => {
                            term_wm_core::layout::FloatingPane {
                                key,
                                rect: term_wm_core::layout::RectSpec::Absolute(
                                    term_wm_layout_engine::LayoutRect {
                                        x: fr.x,
                                        y: fr.y,
                                        width: fr.width,
                                        height: fr.height,
                                    },
                                ),
                            }
                        }
                        term_wm_core::window::FloatRectSpec::Percent {
                            x,
                            y,
                            width,
                            height,
                        } => term_wm_core::layout::FloatingPane {
                            key,
                            rect: term_wm_core::layout::RectSpec::Percent {
                                x,
                                y,
                                width,
                                height,
                            },
                        },
                    })
                    .collect()
            };
            render_resize_outline(
                buf,
                hovered_resize.copied(),
                None,
                wm.regions(),
                area,
                &floating_panes,
                draw_order,
                &wm.config().theme,
            );

            // Snap preview (dashed border + shade fill + countdown text)
            if let Some((_, _, snap_rect)) = wm.drag_snap_rect_data() {
                use ratatui::layout::Alignment;
                use ratatui::style::{Color, Style};
                use ratatui::widgets::Paragraph;
                let rat_snap = ratatui::prelude::Rect {
                    x: snap_rect.x.max(0) as u16,
                    y: snap_rect.y.max(0) as u16,
                    width: snap_rect.width,
                    height: snap_rect.height,
                };
                render_ghost_preview(buf, *snap_rect, &wm.config().theme);
                if let Some(remaining) = wm.drag_snap_remaining() {
                    const GRACE: std::time::Duration = std::time::Duration::from_millis(500);
                    let timeout = wm.config().drag_snap_timeout.unwrap();
                    if timeout.saturating_sub(remaining) >= GRACE {
                        let action = wm.snap_preview_action_label().unwrap_or("snap");
                        let text = if remaining == std::time::Duration::ZERO {
                            format!("Mouse left — {}...", action)
                        } else {
                            format!("Mouse left — {} in {}s", action, remaining.as_secs().max(1))
                        };
                        let text_len = text.len() as u16;
                        let text_x = rat_snap.x + (rat_snap.width.saturating_sub(text_len)) / 2;
                        let text_y = rat_snap.y + rat_snap.height / 2;
                        if text_x >= rat_snap.x && text_y >= rat_snap.y {
                            let text_area = ratatui::prelude::Rect {
                                x: text_x,
                                y: text_y,
                                width: text_len,
                                height: 1,
                            };
                            let paragraph = Paragraph::new(text)
                                .style(
                                    Style::default()
                                        .fg(wm.config().theme.accent_alt.to_ratatui())
                                        .bg(Color::Black),
                                )
                                .alignment(Alignment::Center);
                            ratatui::widgets::Widget::render(paragraph, text_area, buf);
                        }
                    }
                }
            }

            // Dim target tile border during tiled-insert snap preview
            if let Some(target_key) = wm.snap_preview_target_key()
                && let Some(target_rect) = wm.regions().get(target_key)
            {
                let dim = ratatui::style::Modifier::DIM;
                let rx = target_rect.x.max(0) as u16;
                let ry = target_rect.y.max(0) as u16;
                let right = rx.saturating_add(target_rect.width).saturating_sub(1);
                let bottom = ry.saturating_add(target_rect.height).saturating_sub(1);
                for x in rx..=right {
                    if let Some(cell) = buf.cell_mut((x, ry)) {
                        cell.set_style(cell.style().add_modifier(dim));
                    }
                    if bottom != ry
                        && let Some(cell) = buf.cell_mut((x, bottom))
                    {
                        cell.set_style(cell.style().add_modifier(dim));
                    }
                }
                for y in (ry + 1)..bottom {
                    if let Some(cell) = buf.cell_mut((rx, y)) {
                        cell.set_style(cell.style().add_modifier(dim));
                    }
                    if right != rx
                        && let Some(cell) = buf.cell_mut((right, y))
                    {
                        cell.set_style(cell.style().add_modifier(dim));
                    }
                }
            }
        }
    }

    // Render notification toasts (after tiling handles, before overlays)
    {
        for region in draw_plan.regions() {
            if let term_wm_core::draw_plan::RegionType::Notification(msg) = &region.region_type {
                let area = term_wm_ui_components::helpers::layout_rect_to_rect(region.bounds);
                renderer.render_notification(backend, area, msg);
            }
        }
    }

    // Render overlay drop shadows before overlays themselves
    {
        use term_wm_console::RatatuiBackend;
        if let Some(rb) = backend.as_any_mut().downcast_mut::<RatatuiBackend>() {
            let theme = wm.config().theme;
            for (rect, z) in overlay_shadow_data(wm, area, num_windows, total) {
                render_drop_shadow(&mut rb.buffer, rect, 1.0 - z, &theme);
            }
        }
    }
    // Render overlays (command menu, help, exit confirm)
    render_overlays(backend, wm);

    // Register notification hitboxes — swallows mouse events over toast area
    // Uses the notification component's persistent hitbox_id via render.
    if let Some(nc) = wm.notification_component_mut() {
        let ctx = term_wm_core::components::ComponentContext::new(false);
        let mut local_hb = HitboxRegistry::new();
        for region in draw_plan.regions() {
            if matches!(
                region.region_type,
                term_wm_core::draw_plan::RegionType::Notification(_)
            ) {
                nc.render(backend, region.bounds, &ctx, &mut local_hb);
            }
        }
        wm.hitbox_registry_mut().merge(local_hb);
    }

    // Cursor overlay — MUST be last (highest Z-order) so it paints over
    // all previously rendered content including overlays and chrome.
    if let Some(rb) = backend.as_any_mut().downcast_mut::<RatatuiBackend>() {
        render_cursor_overlay(&mut rb.buffer, wm, &wm.config().theme);
    }
}
