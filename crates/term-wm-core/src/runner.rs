use std::io;

use crossterm::event::{Event, KeyEventKind, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::prelude::Rect;
use ratatui::style::{Modifier, Style};

use crate::components::{Component, ComponentContext, ConfirmAction, Overlay, SelectionStatus};
use crate::debug_event_flags;
use crate::event_loop::{ControlFlow, EventLoop};
use crate::io::{EventSource, RenderTarget};
use crate::keybindings::Action;
use crate::layout::{LayoutNode, TilingLayout};
use crate::ui::UiFrame;
use crate::window::decorator::{WindowDecorator, WindowRenderCtx};
use crate::window::{
    DrawTask, WindowDrawContext, WindowId, WindowManager, WindowSurface, WmMenuAction,
};

pub trait WindowManagerHost<Id: Copy + Eq + Ord + std::fmt::Debug + 'static> {
    fn windows(&mut self) -> &mut WindowManager<Id>;
    fn wm_new_window(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn wm_close_window(&mut self, _id: Id) -> std::io::Result<()> {
        Ok(())
    }
    fn set_clipboard_enabled(&mut self, _enabled: bool) {}
    fn set_window_selection_enabled(&mut self, _enabled: bool) {}
    fn open_help_overlay(&mut self) {
        self.windows()
            .open_overlay(crate::window::OverlayId::Help, Box::new(NoopOverlay));
    }
    fn open_keybindings_overlay(&mut self) {
        // Default noop; overridden in main.rs for real overlay.
        self.windows()
            .open_overlay(crate::window::OverlayId::Keybindings, Box::new(NoopOverlay));
    }
    fn open_exit_confirm(&mut self) {
        self.windows()
            .open_overlay(crate::window::OverlayId::ExitConfirm, Box::new(NoopOverlay));
    }
}

struct NoopOverlay;
impl Component for NoopOverlay {
    fn render(
        &mut self,
        _frame: &mut crate::ui::UiFrame<'_>,
        _area: ratatui::layout::Rect,
        _ctx: &ComponentContext,
    ) {
    }
}
impl std::fmt::Debug for NoopOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoopOverlay").finish()
    }
}
impl Overlay for NoopOverlay {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

pub trait WindowProvider<Id: Copy + Eq + Ord + std::fmt::Debug + 'static>:
    WindowManagerHost<Id>
{
    fn enumerate_windows(&mut self) -> Vec<Id>;
    fn render_window(&mut self, frame: &mut UiFrame<'_>, window: WindowDrawContext<Id>);

    fn empty_window_message(&self) -> &str {
        "No windows"
    }

    fn layout_for_windows(&mut self, windows: &[Id]) -> Option<TilingLayout<Id>> {
        auto_layout_for_windows(windows)
    }

    fn window_component(&mut self, _id: Id) -> Option<&mut dyn Component> {
        None
    }

    fn window_pane_title(&mut self, _id: Id) -> Option<String> {
        None
    }

    fn handle_app_event(&mut self, _event: &Event) -> bool {
        false
    }

    fn focus_regions(&mut self) -> Vec<Id> {
        self.enumerate_windows()
    }
}

fn handle_focused_app_event<A, Id>(event: &Event, app: &mut A) -> bool
where
    A: WindowProvider<Id>,
    Id: Copy + Eq + Ord + std::fmt::Debug + 'static,
{
    let ctx = app.windows().component_context(true);

    let mut pending_focus: Option<Id> = None;
    let mut pending_event: Option<Event> = None;
    let consumed = {
        let windows = app.windows();
        windows.dispatch_focused_event(event, |focus_id, localized| {
            pending_focus = Some(focus_id);
            pending_event = Some(localized.clone());
            false
        })
    };

    if let (Some(focus_id), Some(localized)) = (pending_focus, pending_event) {
        if let Some(component) = app.window_component(focus_id) {
            component.handle_event(&localized, &ctx)
        } else {
            false
        }
    } else {
        consumed
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_app<O, D, A, Id, FDraw, FMap>(
    output: &mut O,
    driver: &mut D,
    app: &mut A,
    focus_regions: &[Id],
    map_region: FMap,
    mut draw: FDraw,
) -> io::Result<()>
where
    O: RenderTarget,
    D: EventSource,
    A: WindowProvider<Id>,
    Id: Copy + Eq + Ord + std::fmt::Debug + 'static,
    FDraw: for<'frame> FnMut(UiFrame<'frame>, &mut A),
    FMap: Fn(Id) -> Id + Copy,
{
    let initial_profile = driver.current_profile();
    let mut last_seen_profile = initial_profile;
    let mut event_loop = EventLoop::new(driver);
    event_loop
        .driver()
        .set_mouse_capture(app.windows().mouse_capture_enabled())?;
    event_loop.run(|driver, event| {
        let handler = || -> io::Result<ControlFlow> {
            if debug_event_flags::take_panic_pending() {
                app.windows().open_debug_window();
            }
            if debug_event_flags::take_error_pending() {
                app.windows().open_debug_window();
            }

            for id in app.windows().take_closed_app_windows() {
                app.wm_close_window(id)?;
            }
            let mut flush_state_changes = |app: &mut A, flow: ControlFlow| {
                if let Some(enabled) = app.windows().take_mouse_capture_change() {
                    let _ = driver.set_mouse_capture(enabled);
                }
                if let Some(clipboard) = app.windows().take_clipboard_change() {
                    app.set_clipboard_enabled(clipboard);
                }
                if let Some(sel_enabled) = app.windows().take_window_selection_change() {
                    app.set_window_selection_enabled(sel_enabled);
                }
                let current = driver.current_profile();
                if current != last_seen_profile {
                    app.windows().set_power_profile(current);
                    last_seen_profile = current;
                }
                Ok(flow)
            };
            if let Some(evt) = event {
                app.windows().invalidate_frame();
                // Synthesized key event from bottom-panel hint click takes priority
                let evt = app.windows().take_synthetic_event().unwrap_or(evt);
                // Pre-compute the keybinding action using the configured
                // KeyBindings from WindowManager (not hardcoded defaults).
                // Only Global-layer actions are proactively dispatched;
                // WmMode actions are handled when the WM overlay is open.
                let mapped_action = match &evt {
                    Event::Key(key) => app
                        .windows()
                        .keybindings()
                        .action_for_key_in_layer(key, crate::keybindings::ActionLayer::Global),
                    _ => None,
                };

                // Layer 1: Active overlays (exit confirm, selection preview, help)
                if app.windows().exit_confirm_visible() {
                    if let Some(action) = app.windows().handle_exit_confirm_event(&evt) {
                        match action {
                            ConfirmAction::Confirm => return Ok(ControlFlow::Quit),
                            ConfirmAction::Cancel => app.windows().close_exit_confirm(),
                        }
                    }
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue);
                }

                if app.windows().help_overlay_visible() {
                    let _ = app.windows().handle_help_event(&evt);
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue);
                }

                // If keyboard capture is disabled for the focused window, key events
                // bypass all WM interception and go directly to the terminal,
                // except when the WM overlay is visible — overlay takes priority.
                // Uses the unified double-Super handler: first Super is deferred (panel
                // shows countdown), second Super within window opens overlay, timeout
                // (checked in idle path) forwards the first Super to the terminal.
                if let Event::Key(key) = &evt {
                    let focus_id = app.windows().wm_focus();
                    if app.windows().direct_mode(focus_id)
                        && !app.windows().wm_overlay_visible()
                        && key.kind == KeyEventKind::Press
                    {
                        let is_wm_key = app
                            .windows()
                            .keybindings()
                            .matches(crate::keybindings::Action::WmToggleOverlay, key);
                        match app.windows().handle_super_press(key, is_wm_key) {
                            crate::window::SuperPressResult::DoubleSuper => {
                                app.windows().open_wm_overlay_no_passthrough();
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue);
                            }
                            crate::window::SuperPressResult::Pending => {
                                // First Super of a pair — deferred. Panel shows countdown.
                                // Timeout forwarding happens in the idle path below.
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue);
                            }
                            crate::window::SuperPressResult::Forward => {
                                // Non-wm-toggle key → forward to terminal immediately.
                                let _ = handle_focused_app_event(&evt, app);
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue);
                            }
                        }
                    }
                }

                // Layer 2a: App-level event handler (before WM actions, after overlays)
                if app.handle_app_event(&evt) {
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue);
                }

                // Layer 2b: WM global actions (Global layer only — currently just Esc)
                // All other actions (FocusNext, scrolling, etc.) are WmMode and only
                // dispatched when the WM overlay is visible (see below).
                if let Some(action) = mapped_action {
                    match action {
                        Action::Quit => {
                            app.open_exit_confirm();
                            update_selection_snapshot(app);
                            return flush_state_changes(app, ControlFlow::Continue);
                        }
                        Action::OpenHelp => {
                            app.open_help_overlay();
                            update_selection_snapshot(app);
                            return flush_state_changes(app, ControlFlow::Continue);
                        }
                        Action::OpenKeybindings => {
                            app.open_keybindings_overlay();
                            update_selection_snapshot(app);
                            return flush_state_changes(app, ControlFlow::Continue);
                        }
                        _ => {}
                    }
                }

                // Pre-compute WmMode-layer action for use inside the overlay section.
                let mapped_action_wm_mode = match &evt {
                    Event::Key(key) => app
                        .windows()
                        .keybindings()
                        .action_for_key_in_layer(key, crate::keybindings::ActionLayer::WmMode),
                    _ => None,
                };

                // WM overlay toggle (special case due to passthrough logic)
                let wm_mode = app.windows().config().wm_overlay_enabled;
                if wm_mode
                    && let Event::Key(key) = &evt
                    && key.kind == KeyEventKind::Press
                    && app
                        .windows()
                        .keybindings()
                        .matches(crate::keybindings::Action::WmToggleOverlay, key)
                {
                    if app.windows().wm_overlay_visible() {
                        let passthrough = app.windows().super_passthrough_active();
                        app.windows().close_wm_overlay();
                        if passthrough {
                            let passthrough_event = Event::Key(*key);
                            let _ = handle_focused_app_event(&passthrough_event, app);
                        }
                    } else {
                        app.windows().open_wm_overlay();
                    }
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue);
                }
                if wm_mode && app.windows().wm_overlay_visible() {
                    if let Some(action) = app.windows().handle_wm_menu_event(&evt) {
                        match action {
                            WmMenuAction::CloseMenu => {
                                app.windows().close_wm_overlay();
                            }
                            WmMenuAction::ToggleMouseCapture => {
                                app.windows().toggle_mouse_capture();
                            }
                            WmMenuAction::ToggleClipboardMode => {
                                app.windows().toggle_clipboard_enabled();
                            }
                            WmMenuAction::ToggleWindowSelection => {
                                app.windows().toggle_window_selection();
                            }
                            WmMenuAction::MinimizeWindow => {
                                let id = app.windows().wm_focus();
                                app.windows().minimize_window(id);
                                app.windows().close_wm_overlay();
                            }
                            WmMenuAction::MaximizeWindow => {
                                let id = app.windows().wm_focus();
                                app.windows().toggle_maximize(id);
                                app.windows().close_wm_overlay();
                            }
                            WmMenuAction::CloseWindow => {
                                let id = app.windows().wm_focus();
                                app.windows().close_window(id);
                                app.windows().close_wm_overlay();
                            }
                            WmMenuAction::NewWindow => {
                                app.wm_new_window()?;
                                app.windows().close_wm_overlay();
                            }
                            WmMenuAction::ToggleDebugWindow => {
                                app.windows().toggle_debug_window();
                                app.windows().close_wm_overlay();
                            }
                            WmMenuAction::Help => {
                                app.open_help_overlay();
                                app.windows().close_wm_overlay();
                            }
                            WmMenuAction::BringFloatingFront => {
                                app.windows().bring_all_floating_to_front();
                                app.windows().close_wm_overlay();
                            }
                            WmMenuAction::ExitUi => {
                                app.windows().close_wm_overlay();
                                app.open_exit_confirm();
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue);
                            }
                        }
                        update_selection_snapshot(app);
                        return flush_state_changes(app, ControlFlow::Continue);
                    }
                    if app.windows().wm_menu_consumes_event(&evt) {
                        update_selection_snapshot(app);
                        return flush_state_changes(app, ControlFlow::Continue);
                    }
                    // Focus routing in WM mode (Tab/Shift+Tab)
                    // Fold menu to outline so user can see the window they focused.
                    if app
                        .windows()
                        .handle_focus_event(&evt, focus_regions, &map_region)
                    {
                        app.windows().fold_menu();
                        update_selection_snapshot(app);
                        return flush_state_changes(app, ControlFlow::Continue);
                    }
                    // Dispatch remaining WmMode actions (Quit, OpenHelp, etc.)
                    // while the WM overlay is open.
                    if let Some(action) = mapped_action_wm_mode {
                        match action {
                            Action::Quit => {
                                app.open_exit_confirm();
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue);
                            }
                            Action::OpenHelp => {
                                app.open_help_overlay();
                                app.windows().close_wm_overlay();
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue);
                            }
                            Action::OpenKeybindings => {
                                app.open_keybindings_overlay();
                                app.windows().close_wm_overlay();
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue);
                            }
                            Action::CycleNextWindow => {
                                app.windows().advance_focus(true);
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue);
                            }
                            Action::CyclePrevWindow => {
                                app.windows().advance_focus(false);
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue);
                            }
                            Action::HintToggle => {
                                let current = app.windows().hint_visibility();
                                let next = match current {
                                    crate::wm_config::HintVisibility::Always => {
                                        crate::wm_config::HintVisibility::Never
                                    }
                                    crate::wm_config::HintVisibility::OnDemand => {
                                        crate::wm_config::HintVisibility::Always
                                    }
                                    crate::wm_config::HintVisibility::Never => {
                                        crate::wm_config::HintVisibility::Always
                                    }
                                };
                                app.windows().set_hint_visibility(next);
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue);
                            }
                            _ => {}
                        }
                    }
                    if let Event::Key(_) = &evt {
                        update_selection_snapshot(app);
                        return flush_state_changes(app, ControlFlow::Continue);
                    }
                }

                if matches!(evt, Event::Mouse(_)) && !app.windows().mouse_capture_enabled() {
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue);
                }
                // Direct focus switching for mouse clicks.  Uses the live window
                // set from managed_draw_order (repopulated every draw) instead of
                // the static focus_regions snapshot captured at startup.
                if app.windows().mouse_focus_click_enabled()
                    && let Event::Mouse(mouse) = &evt
                    && matches!(mouse.kind, MouseEventKind::Down(_))
                {
                    let targets = app.windows().managed_draw_order_all().to_vec();
                    // managed_draw_order is bottom-to-top; iterate in reverse
                    // to find the topmost window under the cursor.
                    for &id in targets.iter().rev() {
                        let rect = app.windows().full_region_for_id(id);
                        if rect.width > 0
                            && rect.height > 0
                            && crate::layout::rect_contains(rect, mouse.column, mouse.row)
                        {
                            match id {
                                WindowId::App(app_id) => app.windows().focus_app_window(app_id),
                                WindowId::System(_) => app.windows().focus_window_id(id),
                            }
                            break;
                        }
                    }
                }
                // Route Tab/Shift+Tab through focus routing for embedded mode only.
                // In standalone mode without the open overlay, Tab passes through.
                if !wm_mode
                    && matches!(evt, Event::Key(_))
                    && app
                        .windows()
                        .handle_focus_event(&evt, focus_regions, &map_region)
                {
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue);
                }

                // Layer 3: Pass-through to focused component
                match &evt {
                    Event::Key(_) if app.windows().capture_active() => {
                        app.windows().clear_capture();
                        let _ = handle_focused_app_event(&evt, app);
                        update_selection_snapshot(app);
                    }
                    _ => {
                        let _ = handle_focused_app_event(&evt, app);
                        update_selection_snapshot(app);
                    }
                }
            } else {
                if app.enumerate_windows().is_empty() && !app.windows().has_active_system_windows()
                {
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Quit);
                }
                // Forward any timed-out pending Esc to the terminal.
                if let Some(super_event) = app.windows().take_expired_super_event() {
                    let _ = handle_focused_app_event(&super_event, app);
                }
                // Cancel drag snap if mouse left the viewport during a header drag.
                app.windows().take_expired_drag_snap();
                update_selection_snapshot(app);
                app.windows().begin_frame();
                // Skip the full render on idle when nothing has changed
                // and the power profile allows it (Balanced, PowerSaver).
                if !app.windows().frame_dirty()
                    && app.windows().power_profile().skip_idle_render()
                {
                    // still mark clean so the next explicit invalidation triggers a draw
                } else {
                    app.windows().prepare_draw();
                    // Catch render panics (e.g. u16 subtraction overflow with a
                    // tiny viewport) so they don't take down the event loop.
                    // The panic hook records details in the debug log.
                    // I/O errors from the draw are still propagated.
                    let io_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        output.draw(|frame| {
                            let area = frame.area();
                            if area.width < 2 || area.height < 2 {
                                return;
                            }
                            draw(frame, app)
                        })
                    }))
                    .unwrap_or(Ok(()));
                    io_result?;
                    app.windows().mark_frame_clean();
                }
            }
            flush_state_changes(app, ControlFlow::Continue)
        };

        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(handler)) {
            Ok(result) => result,
            Err(_) => {
                // A panic occurred outside the render path (e.g. in event
                // processing).  Keep mouse capture ON and don't attempt to
                // redraw — the next event will render normally.
                Ok(ControlFlow::Continue)
            }
        }
    })?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_window_app<O, D, A, Id>(output: &mut O, driver: &mut D, app: &mut A) -> io::Result<()>
where
    O: RenderTarget,
    D: EventSource,
    A: WindowProvider<Id>,
    Id: Copy + Eq + Ord + std::fmt::Debug + 'static,
{
    let mut draw_state = WindowDrawState::<Id>::default();
    let focus_regions: Vec<Id> = app.focus_regions();
    run_app(
        output,
        driver,
        app,
        &focus_regions,
        |id| id,
        move |frame, app| {
            let mut frame = frame;
            draw_window_app(&mut frame, app, &mut draw_state);
        },
    )
}

/// Helper: given a provider of selection status/text, return the tuple.
fn selection_snapshot_from(
    s: SelectionStatus,
    text: Option<String>,
) -> (SelectionStatus, Option<String>) {
    if s.active || s.dragging {
        (s, text)
    } else {
        (s, None)
    }
}

fn update_selection_snapshot<A, Id>(app: &mut A)
where
    A: WindowProvider<Id>,
    Id: Copy + Eq + Ord + std::fmt::Debug + 'static,
{
    let was_dragging = app.windows().selection_dragging();
    let focus = app.windows().wm_focus();
    let (status, text) = match focus {
        WindowId::App(id) => app
            .window_component(id)
            .map(|c| selection_snapshot_from(c.selection_status(), c.selection_text()))
            .unwrap_or_default(),
        WindowId::System(sys_id) => app
            .windows()
            .system_window_entry_mut(sys_id)
            .map(|entry| selection_snapshot_from(entry.selection_status(), entry.selection_text()))
            .unwrap_or_default(),
    };
    app.windows()
        .set_selection_snapshot(status.active, status.dragging, text);
    if was_dragging && !status.dragging && status.active {
        app.windows().copy_selection_to_clipboard();
    }
}

struct WindowDrawState<Id> {
    known: Vec<Id>,
}

impl<Id> Default for WindowDrawState<Id> {
    fn default() -> Self {
        Self { known: Vec::new() }
    }
}

impl<Id: Copy + Eq> WindowDrawState<Id> {
    fn update(&mut self, windows: &[Id]) -> bool {
        if self.known == windows {
            false
        } else {
            self.known = windows.to_vec();
            true
        }
    }
}

fn draw_window_app<A, Id>(frame: &mut UiFrame<'_>, app: &mut A, state: &mut WindowDrawState<Id>)
where
    A: WindowProvider<Id>,
    Id: Copy + Eq + Ord + std::fmt::Debug + 'static,
{
    let area = frame.area();
    let windows = app.enumerate_windows();
    let windows_changed = state.update(&windows);

    if windows_changed {
        if let Some(layout) = app.layout_for_windows(&windows) {
            app.windows().set_managed_layout(layout);
        } else if windows.is_empty() {
            // Force a layout update to reflect empty state, but don't clear system windows
            // that the WindowManager might inject. passing None usually clears the app layout.
            app.windows().set_managed_layout_none();
        }
    }

    if windows.is_empty() {
        // If app windows are empty, we might still have system windows.
        // We render the "empty" message underneath, then let the window manager draw its overlays/system windows on top.
        let message = app.empty_window_message();
        if !message.is_empty() {
            frame
                .buffer_mut()
                .set_string(area.x, area.y, message, Style::default());
        }
    }

    let focus_order: Vec<Id> = windows.to_vec();
    if !focus_order.is_empty() {
        app.windows().set_focus_order(focus_order);
    }
    for &id in &windows {
        if let Some(title) = app.window_pane_title(id) {
            app.windows().set_window_title(id, title);
        }
    }
    app.windows().register_managed_layout(area);
    let all_titles: std::collections::BTreeMap<WindowId<Id>, String> =
        app.windows().window_titles().into_iter().collect();
    let plan = app.windows().window_draw_plan(frame);
    let num_windows = plan.len();
    let total = num_windows + app.windows().visible_overlay_count();
    for (i, task) in plan.into_iter().enumerate() {
        let z = crate::window::WindowManager::<Id>::compute_z_depth(i, total);
        match task {
            DrawTask::App(mut window) => {
                window.surface.z_depth = z;
                let (ctx, decorator) = {
                    let wm = app.windows();
                    let title = all_titles
                        .get(&WindowId::App(window.id))
                        .map(String::as_str)
                        .unwrap_or("");
                    let ctx = WindowRenderCtx {
                        title,
                        focused: window.focused,
                        direct_mode: wm.direct_mode(WindowId::App(window.id)),
                        hover_pos: wm.hover,
                    };
                    let decorator = wm.decorator();
                    (ctx, decorator)
                };
                composite_window(
                    frame,
                    &window.surface,
                    decorator.as_ref(),
                    ctx,
                    |subframe| {
                        app.render_window(subframe, window);
                    },
                );
            }
            DrawTask::System(mut window) => {
                window.surface.z_depth = z;
                let (ctx, decorator) = {
                    let wm = app.windows();
                    let title = all_titles
                        .get(&WindowId::System(window.id))
                        .map(String::as_str)
                        .unwrap_or("");
                    let ctx = WindowRenderCtx {
                        title,
                        focused: window.focused,
                        direct_mode: wm.direct_mode(WindowId::System(window.id)),
                        hover_pos: wm.hover,
                    };
                    let decorator = wm.decorator();
                    (ctx, decorator)
                };
                composite_window(
                    frame,
                    &window.surface,
                    decorator.as_ref(),
                    ctx,
                    |subframe| {
                        app.windows().render_system_window(subframe, window);
                    },
                );
            }
        }
    }
    app.windows().render_panel(frame);
    app.windows().render_overlays(frame, num_windows, total);
}

fn composite_window<F>(
    frame: &mut UiFrame<'_>,
    surface: &WindowSurface,
    decorator: &dyn WindowDecorator,
    mut ctx: WindowRenderCtx<'_>,
    mut render_content: F,
) where
    F: FnMut(&mut UiFrame<'_>),
{
    if surface.dest.width == 0 || surface.dest.height == 0 {
        return;
    }
    let local_area = Rect {
        x: 0,
        y: 0,
        width: surface.dest.width,
        height: surface.dest.height,
    };
    ctx.hover_pos = ctx.hover_pos.map(|(cx, cy)| {
        (
            cx.saturating_sub(surface.dest.x.max(0) as u16),
            cy.saturating_sub(surface.dest.y.max(0) as u16),
        )
    });
    let focused = ctx.focused;
    let mut buffer = Buffer::empty(local_area);
    {
        let mut offscreen = UiFrame::from_parts(local_area, &mut buffer);
        decorator.render_window(&mut offscreen, local_area, ctx);
        render_content(&mut offscreen);
    }
    if !focused {
        for cell in buffer.content.iter_mut() {
            cell.modifier.insert(Modifier::DIM);
        }
    }
    if surface.draw_shadow {
        crate::ui::render_drop_shadow(frame, surface.dest, surface.z_depth);
    }
    frame.blit_from_signed(&buffer, surface.dest);
}

fn auto_layout_for_windows<Id: Copy + Eq + Ord + std::fmt::Debug>(
    windows: &[Id],
) -> Option<TilingLayout<Id>> {
    use term_wm_layout_engine::{BspNode, LayoutRect, LongestSide, OrientationHeuristic};

    if windows.is_empty() {
        return None;
    }

    let default_area = LayoutRect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    };

    let mut heuristic = LongestSide;
    let mut windows_iter = windows.iter();
    let first = *windows_iter.next().unwrap();
    let mut root: BspNode<Id> = BspNode::leaf(first);

    for (depth, &id) in windows_iter.enumerate() {
        let orientation = heuristic.choose(default_area, depth);
        let position = match orientation {
            term_wm_layout_engine::Orientation::Horizontal => {
                term_wm_layout_engine::InsertPosition::Right
            }
            term_wm_layout_engine::Orientation::Vertical => {
                term_wm_layout_engine::InsertPosition::Bottom
            }
        };

        let all_ids = root.all_leaf_ids();
        if let Some(&last) = all_ids.last() {
            let _ = root.insert_leaf(
                last,
                id,
                position,
                default_area,
                &term_wm_layout_engine::SizeConstraints {
                    min_width: 4,
                    min_height: 2,
                },
            );
        }
    }

    let layout_node: LayoutNode<Id> = LayoutNode::from(root);
    Some(TilingLayout::new(layout_node))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    #[derive(Debug)]
    struct TestMenu;
    impl crate::components::Component for TestMenu {
        fn render(
            &mut self,
            _frame: &mut crate::ui::UiFrame<'_>,
            _area: ratatui::prelude::Rect,
            _ctx: &crate::components::ComponentContext,
        ) {
        }
    }
    impl crate::components::Overlay for TestMenu {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }
    impl crate::components::MenuOverlay<crate::window::WmMenuAction> for TestMenu {
        fn outline(&mut self) {}
        fn restore(&mut self) {}
        fn set_items(
            &mut self,
            _items: Vec<crate::components::MenuItem<crate::window::WmMenuAction>>,
        ) {
        }
        fn set_timeout(&mut self, _timeout: std::time::Duration) {}
        fn selected_action(&self) -> Option<&crate::window::WmMenuAction> {
            None
        }
        fn set_anchor(&mut self, _pos: Option<(u16, u16)>) {}
        fn set_managed_area(&mut self, _area: ratatui::prelude::Rect) {}
    }

    #[test]
    fn auto_layout_empty_and_multiple() {
        let empty: Vec<u8> = vec![];
        assert!(auto_layout_for_windows(&empty).is_none());

        let one = vec![1u8];
        let layout = auto_layout_for_windows(&one).unwrap();
        // single node should be a leaf
        assert!(matches!(layout.root(), crate::layout::LayoutNode::Leaf(_)));
    }

    #[test]
    fn runner_does_not_quit_when_app_reports_windows_but_wm_has_no_active_regions() {
        use crate::window::WindowManager;

        // Create an empty WindowManager (no active regions/z-order).
        let wm: WindowManager<usize> = WindowManager::with_config(
            0,
            crate::wm_config::WmConfig::standalone(),
            std::sync::Arc::new(crate::AppContext::new("test", "0.0.0")),
            None,
            None,
            Box::new(TestMenu),
        );
        assert!(!wm.has_any_active_windows());

        // Create a fake app that enumerates windows (i.e., app-level windows still exist)
        // while the WM reports no active windows.
        struct FakeApp {
            wm: WindowManager<usize>,
        }
        impl super::WindowManagerHost<usize> for FakeApp {
            fn windows(&mut self) -> &mut WindowManager<usize> {
                &mut self.wm
            }
        }
        impl super::WindowProvider<usize> for FakeApp {
            fn enumerate_windows(&mut self) -> Vec<usize> {
                vec![1]
            }
            fn render_window(
                &mut self,
                _frame: &mut crate::ui::UiFrame<'_>,
                _window: WindowDrawContext<usize>,
            ) {
            }
        }

        let mut app = FakeApp { wm };

        // Sanity: the app-level enumerate shows a window, but the WM reports no active regions.
        assert!(!app.enumerate_windows().is_empty());
        assert!(!app.windows().has_active_system_windows());

        // The runner's quit condition should NOT trigger a quit here:
        // quit_if_no_windows = app.enumerate_windows().is_empty() && !app.windows().has_active_system_windows()
        let quit_if_no_windows =
            app.enumerate_windows().is_empty() && !app.windows().has_active_system_windows();
        assert!(
            !quit_if_no_windows,
            "Runner would quit even though app reports windows"
        );
    }

    #[test]
    fn handle_focused_app_event_routes_key_to_window_component() {
        use crate::components::ComponentContext;
        use crate::window::WindowManager;

        struct KeyRecorder {
            received_key: bool,
        }
        impl Component for KeyRecorder {
            fn render(
                &mut self,
                _frame: &mut crate::ui::UiFrame<'_>,
                _area: ratatui::layout::Rect,
                _ctx: &ComponentContext,
            ) {
            }
            fn handle_event(&mut self, event: &Event, _ctx: &ComponentContext) -> bool {
                if matches!(event, Event::Key(_)) {
                    self.received_key = true;
                }
                true
            }
        }

        struct FakeApp {
            wm: WindowManager<usize>,
            recorder: KeyRecorder,
        }
        impl super::WindowManagerHost<usize> for FakeApp {
            fn windows(&mut self) -> &mut WindowManager<usize> {
                &mut self.wm
            }
        }
        impl super::WindowProvider<usize> for FakeApp {
            fn enumerate_windows(&mut self) -> Vec<usize> {
                vec![1]
            }
            fn render_window(
                &mut self,
                _frame: &mut crate::ui::UiFrame<'_>,
                _window: WindowDrawContext<usize>,
            ) {
            }
            fn window_component(&mut self, _id: usize) -> Option<&mut dyn Component> {
                Some(&mut self.recorder)
            }
            fn focus_regions(&mut self) -> Vec<usize> {
                vec![1]
            }
        }

        let mut app = FakeApp {
            wm: WindowManager::<usize>::with_config(
                0,
                crate::wm_config::WmConfig::standalone(),
                std::sync::Arc::new(crate::AppContext::new("test", "0.0.0")),
                None,
                None,
                Box::new(TestMenu),
            ),
            recorder: KeyRecorder {
                received_key: false,
            },
        };
        app.wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        app.wm.regions.set(
            WindowId::App(1usize),
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );
        app.wm.focus_app_window(1usize);

        let evt = Event::Key(KeyEvent {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });

        let consumed = handle_focused_app_event(&evt, &mut app);
        assert!(
            consumed,
            "handle_focused_app_event must route key to component"
        );
        assert!(
            app.recorder.received_key,
            "component must receive the key event"
        );
    }

    #[test]
    fn handle_focused_app_event_with_direct_mode_still_routes() {
        use crate::components::ComponentContext;
        use crate::window::WindowManager;

        struct KeyRecorder {
            received_key: bool,
        }
        impl Component for KeyRecorder {
            fn render(
                &mut self,
                _frame: &mut crate::ui::UiFrame<'_>,
                _area: ratatui::layout::Rect,
                _ctx: &ComponentContext,
            ) {
            }
            fn handle_event(&mut self, event: &Event, _ctx: &ComponentContext) -> bool {
                if matches!(event, Event::Key(_)) {
                    self.received_key = true;
                }
                true
            }
        }

        struct FakeApp {
            wm: WindowManager<usize>,
            recorder: KeyRecorder,
        }
        impl super::WindowManagerHost<usize> for FakeApp {
            fn windows(&mut self) -> &mut WindowManager<usize> {
                &mut self.wm
            }
        }
        impl super::WindowProvider<usize> for FakeApp {
            fn enumerate_windows(&mut self) -> Vec<usize> {
                vec![1]
            }
            fn render_window(
                &mut self,
                _frame: &mut crate::ui::UiFrame<'_>,
                _window: WindowDrawContext<usize>,
            ) {
            }
            fn window_component(&mut self, _id: usize) -> Option<&mut dyn Component> {
                Some(&mut self.recorder)
            }
            fn focus_regions(&mut self) -> Vec<usize> {
                vec![1]
            }
        }

        let mut app = FakeApp {
            wm: WindowManager::<usize>::with_config(
                0,
                crate::wm_config::WmConfig::standalone(),
                std::sync::Arc::new(crate::AppContext::new("test", "0.0.0")),
                None,
                None,
                Box::new(TestMenu),
            ),
            recorder: KeyRecorder {
                received_key: false,
            },
        };
        app.wm.register_managed_layout(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        app.wm.regions.set(
            WindowId::App(1usize),
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );
        app.wm.focus_app_window(1usize);

        let focus_id = app.wm.wm_focus();
        app.wm.set_direct_mode(focus_id, true);
        assert!(app.wm.direct_mode(focus_id));

        let evt = Event::Key(KeyEvent {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });

        let consumed = handle_focused_app_event(&evt, &mut app);
        assert!(consumed, "event must route even when direct_mode is true");
        assert!(app.recorder.received_key, "component must receive the key");
    }
}
