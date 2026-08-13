#![doc = include_str!("../README.md")]

extern crate self as term_wm;

pub use term_wm_core::*;
pub use term_wm_render::RenderBackend;
pub use term_wm_ui_components::*;
pub use term_wm_view::view;
// Root-level re-exports of the core types the generated `view!` code and
// consumer apps reference (the root `components` module is app-specific and
// shadows `term_wm_core::components`, so these are hoisted to the root). The
// remainder are re-exported by promoting the existing `use` imports below.
pub use term_wm_core::actions::EventResult;
pub use term_wm_core::component_context::ComponentContext;
pub use term_wm_core::events::Event;
pub mod components;
pub mod logging;
pub mod prelude;
pub mod term_wm_app;
pub mod unified_event_source;
pub use term_wm_console::widget_adapter::{StatefulWidgetAdapter, WidgetAdapter};

use ratatui::prelude::Widget;
use term_wm_console::RatatuiBackend;
use term_wm_console::draw_plan_renderer::{
    ColorConvert, DrawPlanRenderer, composite_window, overlay_shadow_data, render_cursor_overlay,
    render_drop_shadow, render_ghost_preview, render_handles_masked, render_overlays,
    render_panels, render_resize_outline, row_has_content_in_range,
};
pub use term_wm_core::actions::TermWmAction;
pub use term_wm_core::components::{Component, NoopComponent, Overlay, SelectionStatus, WmComponent};
pub use term_wm_core::hitbox_registry::{ComponentOwner, HitboxId, HitboxRegistry};
pub use term_wm_core::window::{WindowKey, WindowManager, WindowSurface};

/// Default rendering implementation for the window manager.
/// Shared by all apps so they don't need to reimplement rendering.
pub fn render_app<C: Component<TermWmAction> + 'static, L: WmComponent, O: Overlay<TermWmAction>>(
    backend: &mut dyn term_wm_render::RenderBackend,
    wm: &mut WindowManager<C, L, O>,
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
        let _ = wm.take_alternate_screen_transition(key);
    }

    wm.register_managed_layout(area);
    let draw_plan = engine.project_draw_plan(area.width as u32, area.height as u32, wm);
    let all_titles: std::collections::BTreeMap<_, _> = wm.window_titles().into_iter().collect();
    let num_windows = draw_plan.len();
    let total = num_windows + wm.overlays().len();

    // Register panel hitboxes BEFORE the window loop (lowest Z-order)
    let top_panel_owner = wm
        .semantic_registry
        .get(&term_wm_core::window::ComponentTag::TopPanel)
        .map(|&id| ComponentOwner::Layer(id))
        .unwrap_or(ComponentOwner::Test);
    let bottom_panel_owner = wm
        .semantic_registry
        .get(&term_wm_core::window::ComponentTag::BottomPanel)
        .map(|&id| ComponentOwner::Layer(id))
        .unwrap_or(ComponentOwner::Test);
    wm.register_panel_hitboxes(top_panel_owner, bottom_panel_owner);

    // Register tiling split handle hitboxes below windows
    if !wm.is_monocle() {
        wm.register_layout_handle_hitboxes();
    }

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
                let inner = full;
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
                let z_depth = WindowManager::<C>::compute_z_depth(i, total);
                let surface = WindowSurface {
                    full,
                    inner,
                    dest,
                    draw_shadow,
                    z_depth,
                };

                let title = all_titles.get(key).map(String::as_str).unwrap_or("");
                let borders_enabled = wm.window_borders_enabled(*key);
                let header_enabled = wm.window_header_enabled(*key);
                let win_ctx = term_wm_console::draw_plan_renderer::ChromeCtx {
                    title,
                    focused,
                    floating,
                    hover_pos: wm.hover_pos(),
                    theme: wm.config().theme,
                    wm_buttons: wm.window_management_buttons_for(*key),
                    borders_enabled,
                    header_enabled,
                };
                let content_hitbox_id = HitboxId::new();
                let (_chrome_return, chrome_hb) = composite_window(
                    backend,
                    &surface,
                    *key,
                    content_hitbox_id,
                    win_ctx,
                    |backend, content_bounds| {
                        let screen_area = Rect {
                            x: content_bounds.x + surface.dest.x,
                            y: content_bounds.y + surface.dest.y,
                            width: content_bounds.width,
                            height: content_bounds.height,
                        };
                        let ctx = wm
                            .component_context_for(focused, *key)
                            .with_screen_area(screen_area);
                        if let Some(component) = wm.component_for_key_mut(*key) {
                            let mut local_hb =
                                HitboxRegistry::with_owner(ComponentOwner::Window(*key));
                            // Isolate per-window render panics so a crashing
                            // terminal does not prevent the debug-log overlay
                            // (or any other window) from rendering.
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                component.render(backend, content_bounds, &ctx, &mut local_hb);
                            }));
                            wm.hitbox_registry_mut().merge(local_hb);
                        }
                    },
                    &mut scratch_buf,
                );
                // Merge chrome hitboxes (including content hitbox) into main registry
                wm.hitbox_registry_mut().merge(chrome_hb);
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

    // Render empty state if no mapped windows — clickable link opens command palette
    if wm.mapped_windows().is_empty() {
        use ratatui::style::{Modifier, Style};
        use ratatui::widgets::Paragraph;
        let empty_msg = wm
            .keybindings()
            .combos_for(term_wm_core::actions::TermWmAction::OpenCommandPalette)
            .first()
            .cloned()
            .map_or_else(
                || "No opened windows.".to_string(),
                |hint| format!("Press {hint} to open Command Palette."),
            );
        if let Some(rb) = backend.as_any_mut().downcast_mut::<RatatuiBackend>() {
            let buf = &mut rb.buffer;
            let msg_width = empty_msg.len() as u16;
            let x = area.width.saturating_sub(msg_width) / 2;
            let y = area.height as i32 / 2;
            let text_area = ratatui::layout::Rect {
                x,
                y: y.max(0) as u16,
                width: msg_width,
                height: 1,
            };
            Paragraph::new(empty_msg.as_str())
                .style(
                    Style::default()
                        .fg(wm.config().theme.link_color.to_ratatui())
                        .add_modifier(Modifier::UNDERLINED),
                )
                .render(text_area, buf);

            // Register hitbox so click opens command palette
            let hitbox_id = term_wm_core::hitbox_registry::HitboxId::new();
            wm.hitbox_registry_mut().register(
                hitbox_id,
                term_wm_core::hitbox_registry::ComponentOwner::Chrome(
                    term_wm_core::chrome::ChromeTarget::EmptyStatePlaceholder,
                ),
                term_wm_layout_engine::LayoutRect {
                    x: i32::from(x),
                    y: y.max(0),
                    width: msg_width,
                    height: 1,
                },
            );
        }
    }

    // Render panels AFTER windows
    render_panels(backend, wm);

    // Detect whether the app draws content under the FAB's footprint on its
    // bottom row, so the layout pass can reserve the FAB's row next frame in
    // cramped monocle. Runs before the FAB draws (no self-trigger; the buffer is
    // recreated each frame). The flag is cleared whenever cramped monocle is
    // inactive so a stale value can't falsely pad the first frame after
    // re-entering cramped monocle.
    if wm.is_monocle_cramped() {
        // The FAB's label is the shared menu icon; its DISPLAY width (not char count)
        // is the footprint the app must not collide with. Wide glyphs (Nerd Font,
        // emoji) span 2 terminal columns but a single char, so `.chars().count()`
        // would under-size the footprint and miss collisions under the label's left
        // edge. Matches the FAB component's own width (also display-based).
        let fab_width = {
            let ctx = wm.component_context(true);
            let icon = term_wm_ui_components::helpers::menu_icon(ctx.app_name());
            unicode_width::UnicodeWidthStr::width(icon.as_str()) as u16
        };
        let has = if fab_width == 0 {
            false
        } else {
            let wa = wm.managed_area();
            let bottom_y = wa.y + i32::from(wa.height).saturating_sub(1);
            // The FAB is right-aligned to the screen; recompute its footprint from
            // the CURRENT frame's geometry (a stored x would be stale after a resize
            // and could clamp the range to empty).
            let x_start = wa.x + i32::from(wa.width).saturating_sub(i32::from(fab_width));
            let x_end = wa.x + i32::from(wa.width);
            // Downcast locally (matching the empty-state block) to avoid holding a
            // &mut borrow of `backend` across render_panels / the FAB render below.
            if let Some(rb) = backend.as_any_mut().downcast_mut::<RatatuiBackend>() {
                row_has_content_in_range(&rb.buffer, x_start, x_end, bottom_y)
            } else {
                false
            }
        };
        wm.set_bottom_content_flag(has);
    } else {
        wm.set_bottom_content_flag(false);
    }

    // Render FAB only in cramped monocle mode.
    // Hidden when command palette is open; the bottom panel overlay fills the row.
    let fab_layer_id = wm
        .semantic_registry
        .get(&term_wm_core::window::ComponentTag::FloatingActionButton)
        .copied();
    let fab_ctx = wm.component_context(true).with_screen_area(area);
    if wm.is_monocle_cramped()
        && !wm.command_menu_visible()
        && let Some(fab) =
            wm.get_semantic_component_mut(term_wm_core::window::ComponentTag::FloatingActionButton)
        && let Some(layer_id) = fab_layer_id
    {
        let mut local_hb = HitboxRegistry::with_owner(ComponentOwner::Layer(layer_id));
        fab.render(backend, area, &fab_ctx, &mut local_hb);
        wm.hitbox_registry_mut().merge(local_hb);
    }

    // Render tiling split handles
    {
        use term_wm_console::RatatuiBackend;
        if let Some(rb) = backend.as_any_mut().downcast_mut::<RatatuiBackend>() {
            let buf = &mut rb.buffer;
            let handles = wm.tiling_handles().to_vec();
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
                    &handles,
                    hovered.as_ref(),
                    &is_obscured,
                    &wm.config().theme,
                );
                // Register split handle hitboxes for mouse dispatch
                for handle in &handles {
                    wm.hitbox_registry_mut().register(
                        handle.hitbox_id,
                        ComponentOwner::Chrome(term_wm_core::chrome::ChromeTarget::SplitHandle(
                            handle.hitbox_id,
                        )),
                        handle.rect,
                    );
                }
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
            // Extra occluders for the resize outline, using the same
            // is_obscured masking the tiling drag handles use. Panels mask their
            // claimed rows. Modal invariant: every overlay (help, command
            // palette, exit confirm) dims the full managed area, so mask the
            // whole area while any is open. If a partial overlay is ever added,
            // iterate its bounds instead of the full area.
            let mut extra: [term_wm_layout_engine::LayoutRect; 3] =
                [term_wm_layout_engine::LayoutRect::default(); 3];
            let mut n = 0;
            let top = wm.top_claimed_area();
            if !top.is_empty() {
                extra[n] = top;
                n += 1;
            }
            let bottom = wm.bottom_claimed_area();
            if !bottom.is_empty() {
                extra[n] = bottom;
                n += 1;
            }
            if !wm.overlay_keys().is_empty() {
                extra[n] = area;
                n += 1;
            }
            let extra_obscuring = &extra[..n];

            render_resize_outline(
                buf,
                hovered_resize,
                None,
                wm.regions(),
                area,
                &floating_panes,
                draw_order,
                extra_obscuring,
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
                let area =
                    term_wm_ui_components::helpers::layout_rect_to_clipped_rect(region.bounds);
                renderer.render_notification(backend, area, msg);
            }
        }
    }

    // Render overlay drop shadows before overlays themselves
    {
        use term_wm_console::RatatuiBackend;
        if let Some(rb) = backend.as_any_mut().downcast_mut::<RatatuiBackend>() {
            let theme = wm.config().theme;
            let mut tmp_mask = std::mem::take(&mut rb.mask_buffer);
            let buf_len = rb.buffer.content.len();
            if tmp_mask.len() < buf_len {
                tmp_mask.resize(buf_len, 0);
            }
            for (rect, z) in overlay_shadow_data(wm, area, num_windows, total) {
                render_drop_shadow(
                    &mut rb.buffer,
                    &mut tmp_mask[..buf_len],
                    rect,
                    1.0 - z,
                    &theme,
                );
            }
            rb.mask_buffer = tmp_mask;
        }
    }
    // Render overlays (command menu, help, exit confirm)
    render_overlays(backend, wm);

    // Register notification hitboxes — swallows mouse events over toast area
    let notif_layer_id = wm
        .semantic_registry
        .get(&term_wm_core::window::ComponentTag::NotificationArea)
        .copied();
    if let Some(nc) = wm.notification_component_mut() {
        let ctx = term_wm_core::components::ComponentContext::new(false);
        let layer_id = notif_layer_id.unwrap_or(term_wm_core::window::LayerId::new());
        let mut local_hb = HitboxRegistry::with_owner(ComponentOwner::Layer(layer_id));
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
