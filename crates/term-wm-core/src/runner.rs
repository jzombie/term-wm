use std::io;

use crate::events::{Event, KeyKind, MouseEventKind};
use term_wm_render::RenderTarget;

use std::collections::VecDeque;

use crate::actions::{ConfirmAction, EventResult, SystemTask, TermWmAction};
#[cfg(test)]
use crate::components::Component;
use crate::components::SelectionStatus;
use crate::debug_event_flags;
use crate::event_loop::{ControlFlow, EventLoop};
use crate::events::core_event_to_wm;
use crate::hitbox_registry::HitboxRegistry;
use crate::io::EventSource;
use crate::layout::{LayoutNode, TilingLayout};
use crate::task_scheduler::TaskScheduler;
use crate::window::{WindowKey, WindowManager};

pub trait WindowManagerHost {
    fn wm(&mut self) -> &mut WindowManager;
    fn wm_new_window(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn wm_close_window(&mut self, _key: WindowKey) -> std::io::Result<()> {
        Ok(())
    }
    fn set_clipboard_enabled(&mut self, _enabled: bool) {}
    fn set_window_selection_enabled(&mut self, _enabled: bool) {}
    fn open_help_overlay(&mut self) {
        self.wm().open_overlay(crate::window::OverlayId::Help, None);
    }
    fn open_keybindings_overlay(&mut self) {
        self.wm()
            .open_overlay(crate::window::OverlayId::Keybindings, None);
    }
    fn open_exit_confirm(&mut self) {
        self.wm().request_quit();
    }
    /// Called when a panic is detected.
    fn on_panic(&mut self) {}
    /// Toggle the debug log window visibility.
    fn toggle_debug_window(&mut self) {}
    /// Toggle the system panel window visibility.
    fn toggle_system_panel(&mut self) {}
    /// Called by the runner to check if the app wants to quit.
    /// The app sets this to `true` to exit the event loop.
    fn quit_requested(&self) -> bool {
        false
    }

    fn empty_window_message(&self) -> &str {
        "No windows"
    }

    fn layout_for_windows(&mut self, windows: &[WindowKey]) -> Option<TilingLayout<WindowKey>> {
        auto_layout_for_windows(windows)
    }

    /// Called by the runner each frame to render.
    /// The default implementation does nothing — apps override this to draw.
    fn render(&mut self, _backend: &mut dyn term_wm_render::RenderBackend) {}

    fn handle_app_event(&mut self, _event: &Event) -> bool {
        false
    }
}

fn drain_action_queue<A: WindowManagerHost>(
    app: &mut A,
    queue: &mut VecDeque<(WindowKey, TermWmAction)>,
) {
    while let Some((key, action)) = queue.pop_front() {
        match action {
            TermWmAction::SendNotification(msg) => {
                tracing::info!("drain_action_queue: SendNotification({})", msg);
                app.wm()
                    .push_notification(msg, std::time::Duration::from_secs(3));
            }
            TermWmAction::OpenCommandPalette => {
                if app.wm().command_menu_visible() {
                    app.wm().close_command_menu();
                } else {
                    app.wm().open_command_menu();
                }
            }
            action => {
                let ctx = app.wm().component_context_for(true, key);
                if let Some(comp) = app.wm().component_for_key_mut(key) {
                    comp.update(action, &ctx, queue);
                }
            }
        }
    }
}

fn handle_focused_app_event<A>(event: &Event, app: &mut A) -> bool
where
    A: WindowManagerHost,
{
    // Clear hover state when the terminal loses focus so stale
    // hover highlights do not persist on menus or buttons.
    // Do not return — allow fall-through to standard dispatch.
    if matches!(event, Event::FocusLost) {
        app.wm().clear_hover();
    }

    // Mouse events: use registry dispatch instead of tree-walk.
    // The registry is built during the render pass and provides
    // O(1) hit-testing — no coordinate mutation, no ad-hoc rect_contains.
    if matches!(event, Event::Mouse(_)) {
        if let Some(wm_event) = core_event_to_wm(event) {
            let result = app.wm().dispatch_mouse(&wm_event);
            match result {
                EventResult::Action((target_key, action)) => {
                    // Intercept global actions from system chrome (FAB has no WindowKey)
                    if matches!(action, TermWmAction::OpenCommandPalette) {
                        if app.wm().command_menu_visible() {
                            app.wm().close_command_menu();
                        } else {
                            app.wm().open_command_menu();
                        }
                        return true;
                    }
                    let key = target_key.unwrap_or_else(|| app.wm().focused_window());
                    let mut queue = VecDeque::from([(key, action)]);
                    drain_action_queue(app, &mut queue);
                    return true;
                }
                EventResult::Consumed => return true,
                EventResult::Ignored => return false,
            }
        }
        return false;
    }

    let focus_id = app.wm().focused_window();

    // Phase 1: WM-stored components (chrome, debug log, system windows)
    if let Some((_key, result)) = app.wm().dispatch_focused_event(event) {
        if let EventResult::Action(action) = result {
            let mut queue = VecDeque::from([(focus_id, action)]);
            drain_action_queue(app, &mut queue);
        }
        return true;
    }

    // Phase 2 Fallback Prep: Compute immutable state FIRST, before mutably borrowing app
    let direct_mode = app.wm().direct_mode(focus_id);
    let ctx = app
        .wm()
        .component_context_for(!direct_mode, focus_id)
        .with_direct_mode(direct_mode);
    let Some((_, localized_evt)) = app.wm().focused_window_event(event) else {
        return false;
    };
    let adjusted_evt = app.wm().adjust_event_for_window(focus_id, &localized_evt);

    // Phase 2 Dispatch: Mutably borrow app for the component
    let result = if let Some(comp) = app.wm().component_for_key_mut(focus_id) {
        comp.handle_events(&adjusted_evt, &ctx)
    } else {
        return false;
    };

    // Phase 3: Process the result
    match result {
        EventResult::Action(action) => {
            let mut queue = VecDeque::from([(focus_id, action)]);
            drain_action_queue(app, &mut queue);
            true
        }
        EventResult::Consumed => true,
        EventResult::Ignored => false,
    }
}

/// Low-level event loop. Drives rendering and input until the app quits.
///
/// Prefer [`run_with_defaults`] for typical usage. Use this directly only when
/// you need a custom draw closure or region mapping.
#[allow(clippy::too_many_arguments)]
pub fn run_event_loop<O, D, A, FDraw, FMap>(
    output: &mut O,
    driver: &mut D,
    app: &mut A,
    system_scheduler: TaskScheduler<SystemTask>,
    _map_region: FMap,
    mut draw: FDraw,
) -> io::Result<()>
where
    O: RenderTarget,
    D: EventSource,
    A: WindowManagerHost,
    FDraw: for<'frame> FnMut(&'frame mut dyn term_wm_render::RenderBackend, &mut A),
    FMap: Fn(WindowKey) -> WindowKey + Copy,
{
    let system_handle = system_scheduler.handle();
    let mut profile_tracker =
        crate::power_profile::PowerProfileTracker::new(driver.current_profile());
    let mut event_loop = EventLoop::new(driver);
    event_loop
        .driver()
        .set_mouse_capture(app.wm().mouse_capture_enabled())?;
    event_loop.run(|driver, event| {
        let handler = || -> io::Result<ControlFlow> {
            // Process expired system tasks (super-passthrough, drag-snap)
            for (_id, task) in system_handle.drain_expired() {
                match task {
                    SystemTask::DragSnap => {
                        app.wm().apply_drag_snap_if_pending();
                    }
                    SystemTask::TemporalDwellTick => {
                        app.wm().on_temporal_dwell_tick();
                    }
                    SystemTask::DismissNotification(id) => {
                        app.wm().dismiss_notification(id);
                    }
                }
            }

            if debug_event_flags::take_panic_pending() {
                app.on_panic();
            }
            if debug_event_flags::take_error_pending() {
                app.on_panic();
            }

            for id in app.wm().take_closed_windows() {
                app.wm_close_window(id)?;
            }
            // Process AppExited notifications — close windows whose PTY child
            // exited.  Regular windows are handled entirely by
            // WindowManager::close_window (destroy + remove from SlotMap).
            for key in driver.take_exited_windows() {
                app.wm().close_window(key);
            }

            // Update monocle mode on resize
            if let Some(Event::Resize(width, _height)) = &event {
                app.wm().update_monocle_mode(*width);
            }

            let mut flush_state_changes = |app: &mut A, flow: ControlFlow, consume_dirty: bool| {
                if let Some(enabled) = app.wm().take_mouse_capture_change() {
                    let _ = driver.set_mouse_capture(enabled);
                }
                if let Some(clipboard) = app.wm().take_clipboard_change() {
                    app.set_clipboard_enabled(clipboard);
                }
                if let Some(sel_enabled) = app.wm().take_window_selection_change() {
                    app.set_window_selection_enabled(sel_enabled);
                }
                if consume_dirty {
                    // Consume accumulated dirty-window keys so the power
                    // profile can drop back to PowerSaver when idle.  Only
                    // call after a successful render — on panics the dirty
                    // set is preserved so the next frame picks it up.
                    driver.take_dirty_windows();
                }
                // Clamp the driver's sleep duration to the next scheduler
                // deadline so PowerSaver's 3600s interval doesn't block
                // past a pending task timeout (e.g. SuperPassthrough).
                driver.set_max_sleep_duration(system_handle.time_until_next());
                if let Some(profile) = profile_tracker.poll(driver.current_profile()) {
                    app.wm().set_power_profile(profile);
                }
                Ok(flow)
            };
            let mut did_panic = false;
            if let Some(evt) = event {
                // Synthesized key event from bottom-panel hint click takes priority
                let evt = app.wm().take_synthetic_event().unwrap_or(evt);

                // Pre-compute the keybinding action using the configured
                // KeyBindings from WindowManager (not hardcoded defaults).
                // WmMode actions are handled when the WM overlay is open.

                // Layer 1: Active overlays (exit confirm, selection preview, help)
                if app.wm().exit_confirm_visible() {
                    if let Some(action) = app.wm().handle_exit_confirm_event(&evt) {
                        match action {
                            ConfirmAction::Confirm => return Ok(ControlFlow::Quit),
                            ConfirmAction::Cancel => app.wm().close_exit_confirm(),
                        }
                    }
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue, false);
                }

                if app.wm().help_overlay_visible() {
                    let _ = app.wm().handle_help_event(&evt);
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue, false);
                }

                // If keyboard capture is disabled for the focused window, key events
                // bypass all WM interception and go directly to the terminal,
                // except when the WM overlay is visible — overlay takes priority.
                // In direct mode, keys are forwarded immediately without WM processing.

                // Layer 2a: Command palette toggle — ALWAYS interceptable
                // This MUST happen before the direct mode check so that
                // Ctrl+Shift+Space works even when a terminal window is in direct mode.
                let wm_mode = app.wm().config().wm_command_menu_enabled;
                if wm_mode
                    && let Event::Key(key) = &evt
                    && key.kind == KeyKind::Press
                    && app
                        .wm()
                        .keybindings()
                        .matches(TermWmAction::OpenCommandPalette, key)
                {
                    if app.wm().command_menu_visible() {
                        app.wm().close_command_menu();
                    } else {
                        app.wm().open_command_menu();
                    }
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue, false);
                }

                // Layer 2b: Direct mode check — intercepts ALL other keys
                if let Event::Key(key) = &evt {
                    let focus_id = app.wm().focused_window();
                    if app.wm().direct_mode(focus_id)
                        && !app.wm().command_menu_visible()
                        && key.kind == KeyKind::Press
                    {
                        // Direct mode — forward to terminal immediately.
                        let _ = handle_focused_app_event(&evt, app);
                        update_selection_snapshot(app);
                        return flush_state_changes(app, ControlFlow::Continue, false);
                    }
                }

                // Layer 2c: App-level event handler (before WM actions, after overlays)
                if app.handle_app_event(&evt) {
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue, false);
                }

                // Pre-compute WmMode-layer action for use inside the overlay section.
                let mapped_action_wm_mode = match &evt {
                    Event::Key(key) => app
                        .wm()
                        .keybindings()
                        .action_for_key_in_layer(key, crate::keybindings::ActionLayer::WmMode),
                    _ => None,
                };
                if wm_mode && app.wm().command_menu_visible() {
                    if let Some(action) = app.wm().handle_wm_menu_event(&evt) {
                        match action {
                            TermWmAction::CloseMenu => {
                                app.wm().close_command_menu();
                            }
                            TermWmAction::ToggleMouseCapture => {
                                app.wm().toggle_mouse_capture();
                            }
                            TermWmAction::ToggleClipboardMode => {
                                app.wm().toggle_clipboard_enabled();
                            }
                            TermWmAction::ToggleWindowSelection => {
                                app.wm().toggle_window_selection();
                            }
                            TermWmAction::MinimizeWindow => {
                                let id = app.wm().focused_window();
                                app.wm().minimize_window(id);
                                app.wm().close_command_menu();
                            }
                            TermWmAction::MaximizeWindow => {
                                let id = app.wm().focused_window();
                                app.wm().toggle_maximize(id);
                                app.wm().close_command_menu();
                            }
                            TermWmAction::ToggleDirectMode => {
                                let id = app.wm().focused_window();
                                app.wm().toggle_direct_mode(id);
                                app.wm().close_command_menu();
                            }
                            TermWmAction::CloseWindow => {
                                let id = app.wm().focused_window();
                                app.wm().close_window(id);
                                app.wm().close_command_menu();
                                // System windows queued by close_window are
                                // cleaned up by take_closed_windows below.
                            }
                            TermWmAction::NewWindow => {
                                app.wm_new_window()?;
                                app.wm().close_command_menu();
                            }
                            TermWmAction::ToggleDebugWindow => {
                                app.toggle_debug_window();
                                app.wm().close_command_menu();
                            }
                            TermWmAction::ToggleSystemPanel => {
                                app.toggle_system_panel();
                                app.wm().close_command_menu();
                            }
                            TermWmAction::SendNotification(msg) => {
                                app.wm()
                                    .push_notification(msg, std::time::Duration::from_secs(3));
                                app.wm().close_command_menu();
                            }
                            TermWmAction::Help => {
                                app.open_help_overlay();
                                app.wm().close_command_menu();
                            }
                            TermWmAction::ExitUi => {
                                app.wm().close_command_menu();
                                app.open_exit_confirm();
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue, false);
                            }
                            _ => {}
                        }
                        update_selection_snapshot(app);
                        return flush_state_changes(app, ControlFlow::Continue, false);
                    }
                    if app.wm().wm_menu_consumes_event(&evt) {
                        update_selection_snapshot(app);
                        return flush_state_changes(app, ControlFlow::Continue, false);
                    }
                    // Focus routing in WM mode (Tab/Shift+Tab)
                    // Fold menu to outline so user can see the window they focused.
                    if app.wm().handle_focus_event(&evt) && matches!(&evt, Event::Key(_)) {
                        app.wm().fold_menu();
                        update_selection_snapshot(app);
                        return flush_state_changes(app, ControlFlow::Continue, false);
                    }
                    // Dispatch remaining WmMode actions (Quit, OpenHelp, etc.)
                    // while the WM overlay is open.
                    if let Some(action) = mapped_action_wm_mode {
                        match action {
                            TermWmAction::Quit => {
                                app.open_exit_confirm();
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue, false);
                            }
                            TermWmAction::OpenHelp => {
                                app.open_help_overlay();
                                app.wm().close_command_menu();
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue, false);
                            }
                            TermWmAction::OpenKeybindings => {
                                app.open_keybindings_overlay();
                                app.wm().close_command_menu();
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue, false);
                            }
                            TermWmAction::CycleNextWindow => {
                                app.wm().advance_focus(true);
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue, false);
                            }
                            TermWmAction::CyclePrevWindow => {
                                app.wm().advance_focus(false);
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue, false);
                            }
                            TermWmAction::HintToggle => {
                                let current = app.wm().hint_visibility();
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
                                app.wm().set_hint_visibility(next);
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue, false);
                            }
                            _ => {}
                        }
                    }
                    if let Event::Key(_) = &evt {
                        update_selection_snapshot(app);
                        return flush_state_changes(app, ControlFlow::Continue, false);
                    }
                }

                if matches!(evt, Event::Mouse(_)) && !app.wm().mouse_capture_enabled() {
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue, false);
                }
                // Direct focus switching for mouse clicks.  Uses the live window
                // set from managed_draw_order (repopulated every draw) instead of
                // the static focus_regions snapshot captured at startup.
                if app.wm().mouse_focus_click_enabled()
                    && let Event::Mouse(mouse) = &evt
                    && matches!(mouse.kind, MouseEventKind::Press(_))
                {
                    let targets = app.wm().managed_draw_order_all().to_vec();
                    // managed_draw_order is bottom-to-top; iterate in reverse
                    // to find the topmost window under the cursor.
                    for &key in targets.iter().rev() {
                        let rect = app.wm().full_region_for_key(key);
                        if rect.width > 0
                            && rect.height > 0
                            && crate::layout::rect_contains(rect, mouse.column, mouse.row)
                        {
                            app.wm().focus_app_window(key);
                            break;
                        }
                    }
                }
                // Route Tab/Shift+Tab through focus routing for embedded mode only.
                // In standalone mode without the open overlay, Tab passes through.
                if !wm_mode
                    && let Event::Key(key) = &evt
                    && key.kind == KeyKind::Press
                    && app.wm().keybindings().matches(TermWmAction::Quit, key)
                {
                    app.open_exit_confirm();
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue, false);
                }
                if !wm_mode && matches!(evt, Event::Key(_)) && app.wm().handle_focus_event(&evt) {
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue, false);
                }

                // Layer 3: Pass-through to focused component
                match &evt {
                    Event::Key(_) if app.wm().capture_active() => {
                        app.wm().clear_capture();
                        let _ = handle_focused_app_event(&evt, app);
                        update_selection_snapshot(app);
                    }
                    _ => {
                        let _ = handle_focused_app_event(&evt, app);
                        update_selection_snapshot(app);
                    }
                }
            } else {
                if app.quit_requested() || app.wm().quit_requested() {
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Quit, false);
                }
                update_selection_snapshot(app);
                app.wm().begin_frame();
                app.wm().prepare_draw();
                // Catch render panics (e.g. u16 subtraction overflow with a
                // tiny viewport, or a component panic) so they don't take
                // down the event loop.  The panic hook records details in
                // the debug log.  I/O errors from the draw are propagated.
                // After a panic, repair the terminal so the next draw starts
                // from a clean slate (partial escape sequences, wrong cursor
                // position, etc. are reset).
                did_panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    output.draw(|frame| {
                        // TODO: Get area from DrawPlan or terminal size, not from frame
                        draw(frame, app)
                    })
                }))
                .is_err();
                if did_panic {
                    output.repair()?;
                }
            }
            flush_state_changes(app, ControlFlow::Continue, !did_panic)
        };

        let handler_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(handler));

        system_handle.set_keep_awake(app.wm().visible_overlay_count() > 0);
        driver.set_pending_work(system_handle.is_keep_awake_active());
        match handler_result {
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

/// Run a window manager app with default draw and region mapping.
///
/// This is the standard entry point for event-loop execution. It wires up the
/// default draw closure (`app.render(backend)`) and passes through to
/// [`run_event_loop`].
///
/// # Hierarchy
///
/// ```text
/// app.run()                         // high-level: sets up console I/O
///   └─ run_with(output, driver)     // accepts custom I/O backends
///       └─ run_with_defaults(...)   // ← you are here
///           └─ run_event_loop(...)  // low-level: the actual loop
/// ```
pub fn run_with_defaults<O, D, A>(output: &mut O, driver: &mut D, app: &mut A) -> io::Result<()>
where
    O: RenderTarget,
    D: EventSource,
    A: WindowManagerHost,
{
    let system_scheduler = TaskScheduler::<SystemTask>::new();
    let system_handle = system_scheduler.handle();
    app.wm().set_system_task_handle(system_handle);

    run_event_loop(
        output,
        driver,
        app,
        system_scheduler,
        |key| key,
        |backend, app| {
            app.render(backend);
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

fn update_selection_snapshot<A>(app: &mut A)
where
    A: WindowManagerHost,
{
    let was_dragging = app.wm().selection_dragging();
    let focus = app.wm().focused_window();
    let (status, text) = app
        .wm()
        .component_for_key_mut(focus)
        .map(|c| selection_snapshot_from(c.selection_status(), c.selection_text()))
        .unwrap_or_default();
    app.wm()
        .set_selection_snapshot(status.active, status.dragging, text);
    if was_dragging && !status.dragging && status.active {
        app.wm().copy_selection_to_clipboard();
    }
}

#[derive(Default)]
#[allow(dead_code)]
struct WindowDrawState {
    known: Vec<WindowKey>,
    hitbox_registry: HitboxRegistry,
}

#[allow(dead_code)]
impl WindowDrawState {
    fn new() -> Self {
        Self {
            known: Vec::new(),
            hitbox_registry: HitboxRegistry::new(),
        }
    }

    fn update(&mut self, windows: &[WindowKey]) -> bool {
        if self.known == windows {
            false
        } else {
            self.known = windows.to_vec();
            true
        }
    }
}

pub fn auto_layout_for_windows(windows: &[WindowKey]) -> Option<TilingLayout<WindowKey>> {
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
    let mut root: BspNode<WindowKey> = BspNode::leaf(first);

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

    let layout_node: LayoutNode<WindowKey> = LayoutNode::from(root);
    Some(TilingLayout::new(layout_node))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{KeyCode, KeyEvent, KeyKind, KeyModifiers};
    use term_wm_layout_engine::LayoutRect;

    #[test]
    fn auto_layout_empty_and_multiple() {
        let empty: Vec<WindowKey> = vec![];
        assert!(auto_layout_for_windows(&empty).is_none());

        let mut wm = crate::window::WindowManager::with_config(
            crate::wm_config::WmConfig::standalone(),
            std::sync::Arc::new(crate::AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
            None,
        );
        let key = wm.create_window(Box::new(crate::components::NoopComponent));
        let one = vec![key];
        let layout = auto_layout_for_windows(&one).unwrap();
        assert!(matches!(layout.root(), crate::layout::LayoutNode::Leaf(_)));
    }

    #[test]
    fn runner_does_not_quit_when_app_reports_windows_but_wm_has_no_active_regions() {
        use crate::window::WindowManager;

        struct FakeApp {
            wm: WindowManager,
        }
        impl WindowManagerHost for FakeApp {
            fn wm(&mut self) -> &mut WindowManager {
                &mut self.wm
            }
        }

        let mut wm = WindowManager::with_config(
            crate::wm_config::WmConfig::standalone(),
            std::sync::Arc::new(crate::AppContext::new("test", "0.0.0")),
            None,
            None,
            Some(Box::new(TestMenu)),
            None,
            None,
        );
        let key = wm.create_window(Box::new(crate::components::NoopComponent));
        wm.transition_window(key, crate::window::WindowState::Mapped);

        let mut app = FakeApp { wm };
        assert!(!app.wm().mapped_windows().is_empty());

        let quit_if_no_windows = app.wm().mapped_windows().is_empty();
        assert!(
            !quit_if_no_windows,
            "Runner would quit even though app reports windows"
        );
    }

    #[test]
    fn handle_focused_app_event_routes_key_to_window_component() {
        use crate::window::WindowManager;

        struct KeyRecorder {
            received_key: bool,
        }
        impl Component<TermWmAction> for KeyRecorder {
            fn render(
                &mut self,
                _backend: &mut dyn term_wm_render::RenderBackend,
                _area: LayoutRect,
                _ctx: &crate::components::ComponentContext,
                _registry: &mut crate::hitbox_registry::HitboxRegistry,
            ) {
            }
            fn handle_events(
                &mut self,
                event: &Event,
                _ctx: &crate::components::ComponentContext,
            ) -> crate::actions::EventResult<TermWmAction> {
                if matches!(event, Event::Key(_)) {
                    self.received_key = true;
                }
                crate::actions::EventResult::Consumed
            }
            fn update(
                &mut self,
                _action: TermWmAction,
                _ctx: &crate::components::ComponentContext,
                _queue: &mut VecDeque<(crate::window::WindowKey, TermWmAction)>,
            ) {
            }
            fn destroy(&mut self) {}
        }

        struct FakeApp {
            wm: WindowManager,
        }
        impl WindowManagerHost for FakeApp {
            fn wm(&mut self) -> &mut WindowManager {
                &mut self.wm
            }
        }

        let mut app = FakeApp {
            wm: WindowManager::with_config(
                crate::wm_config::WmConfig::standalone(),
                std::sync::Arc::new(crate::AppContext::new("test", "0.0.0")),
                None,
                None,
                Some(Box::new(TestMenu)),
                None,
                None,
            ),
        };
        // Store the KeyRecorder directly in the WindowManager — no sidecar.
        let key = app.wm.create_window(Box::new(KeyRecorder {
            received_key: false,
        }));
        app.wm
            .transition_window(key, crate::window::WindowState::Mapped);
        app.wm.regions.set(
            key,
            LayoutRect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );
        app.wm.focus_app_window(key);

        let evt = Event::Key(KeyEvent {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        });

        let consumed = handle_focused_app_event(&evt, &mut app);
        assert!(
            consumed,
            "handle_focused_app_event must route key to component"
        );
        assert!(
            app.wm
                .component_for_key_mut(key)
                .and_then(|c| {
                    use crate::components::component_downcast_mut;
                    component_downcast_mut::<KeyRecorder>(c).map(|r| r.received_key)
                })
                .unwrap_or(false),
            "component must receive the key event"
        );
    }

    #[test]
    fn handle_focused_app_event_with_direct_mode_still_routes() {
        use crate::window::WindowManager;

        struct KeyRecorder {
            received_key: bool,
        }
        impl Component<TermWmAction> for KeyRecorder {
            fn render(
                &mut self,
                _backend: &mut dyn term_wm_render::RenderBackend,
                _area: LayoutRect,
                _ctx: &crate::components::ComponentContext,
                _registry: &mut crate::hitbox_registry::HitboxRegistry,
            ) {
            }
            fn handle_events(
                &mut self,
                event: &Event,
                _ctx: &crate::components::ComponentContext,
            ) -> crate::actions::EventResult<TermWmAction> {
                if matches!(event, Event::Key(_)) {
                    self.received_key = true;
                }
                crate::actions::EventResult::Consumed
            }
            fn update(
                &mut self,
                _action: TermWmAction,
                _ctx: &crate::components::ComponentContext,
                _queue: &mut VecDeque<(crate::window::WindowKey, TermWmAction)>,
            ) {
            }
            fn destroy(&mut self) {}
        }

        struct FakeApp {
            wm: WindowManager,
        }
        impl WindowManagerHost for FakeApp {
            fn wm(&mut self) -> &mut WindowManager {
                &mut self.wm
            }
        }

        let mut app = FakeApp {
            wm: WindowManager::with_config(
                crate::wm_config::WmConfig::standalone(),
                std::sync::Arc::new(crate::AppContext::new("test", "0.0.0")),
                None,
                None,
                Some(Box::new(TestMenu)),
                None,
                None,
            ),
        };
        let key = app.wm.create_window(Box::new(KeyRecorder {
            received_key: false,
        }));
        app.wm
            .transition_window(key, crate::window::WindowState::Mapped);
        app.wm.regions.set(
            key,
            LayoutRect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );
        app.wm.focus_app_window(key);

        let focus_id = app.wm.focused_window();
        app.wm.set_direct_mode(focus_id, true);
        assert!(app.wm.direct_mode(focus_id));

        let evt = Event::Key(KeyEvent {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        });

        let consumed = handle_focused_app_event(&evt, &mut app);
        assert!(consumed, "event must route even when direct_mode is true");
        // Verify through the WindowManager that the component received the key
        assert!(
            app.wm
                .component_for_key_mut(key)
                .and_then(|c| {
                    use crate::components::component_downcast_mut;
                    component_downcast_mut::<KeyRecorder>(c).map(|r| r.received_key)
                })
                .unwrap_or(false),
            "component must receive the key"
        );
    }

    // ── selection_snapshot_from ──────────────────────────────────────

    #[test]
    fn selection_snapshot_from_active_returns_text() {
        let status = SelectionStatus {
            active: true,
            dragging: false,
        };
        let (s, text) = selection_snapshot_from(status, Some("hello".into()));
        assert!(s.active);
        assert_eq!(text, Some("hello".into()));
    }

    #[test]
    fn selection_snapshot_from_dragging_returns_text() {
        let status = SelectionStatus {
            active: false,
            dragging: true,
        };
        let (s, text) = selection_snapshot_from(status, Some("dragging".into()));
        assert!(s.dragging);
        assert_eq!(text, Some("dragging".into()));
    }

    #[test]
    fn selection_snapshot_from_active_and_dragging_returns_text() {
        let status = SelectionStatus {
            active: true,
            dragging: true,
        };
        let (s, text) = selection_snapshot_from(status, Some("both".into()));
        assert!(s.active);
        assert!(s.dragging);
        assert_eq!(text, Some("both".into()));
    }

    #[test]
    fn selection_snapshot_from_inactive_returns_none_text() {
        let status = SelectionStatus {
            active: false,
            dragging: false,
        };
        let (s, text) = selection_snapshot_from(status, Some("ignored".into()));
        assert!(!s.active);
        assert!(!s.dragging);
        assert_eq!(text, None);
    }

    #[test]
    fn selection_snapshot_from_inactive_none_text() {
        let status = SelectionStatus {
            active: false,
            dragging: false,
        };
        let (_s, text) = selection_snapshot_from(status, None);
        assert_eq!(text, None);
    }

    #[test]
    fn selection_snapshot_from_active_none_text() {
        let status = SelectionStatus {
            active: true,
            dragging: false,
        };
        let (s, text) = selection_snapshot_from(status, None);
        assert!(s.active);
        assert_eq!(text, None);
    }

    // ── WindowDrawState ──────────────────────────────────────────────

    fn make_keys(n: usize) -> Vec<WindowKey> {
        let mut wm = crate::window::WindowManager::with_config(
            crate::wm_config::WmConfig::standalone(),
            std::sync::Arc::new(crate::AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
            None,
        );
        (0..n)
            .map(|_| wm.create_window(Box::new(crate::components::NoopComponent)))
            .collect()
    }

    #[test]
    fn window_draw_state_update_first_call_returns_true() {
        let mut state = WindowDrawState::default();
        let keys = make_keys(3);
        assert!(state.update(&keys));
        assert_eq!(state.known, keys);
    }

    #[test]
    fn window_draw_state_update_same_list_returns_false() {
        let mut state = WindowDrawState::default();
        let keys = make_keys(3);
        state.update(&keys);
        assert!(!state.update(&keys));
    }

    #[test]
    fn window_draw_state_update_different_list_returns_true() {
        let mut state = WindowDrawState::default();
        let a = make_keys(3);
        let b = make_keys(2);
        state.update(&a);
        assert!(state.update(&b));
        assert_eq!(state.known, b);
    }

    #[test]
    fn window_draw_state_update_empty_lists() {
        let mut state = WindowDrawState::default();
        // First update from empty to empty — no change, returns false
        assert!(!state.update(&[]));
        // Second update — same, still false
        assert!(!state.update(&[]));
    }

    #[test]
    fn window_draw_state_default_is_empty() {
        let state = WindowDrawState::default();
        assert!(state.known.is_empty());
    }

    // ── auto_layout_for_windows ──────────────────────────────────────

    #[test]
    fn auto_layout_two_windows_creates_split() {
        let mut wm = crate::window::WindowManager::with_config(
            crate::wm_config::WmConfig::standalone(),
            std::sync::Arc::new(crate::AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
            None,
        );
        let k1 = wm.create_window(Box::new(crate::components::NoopComponent));
        let k2 = wm.create_window(Box::new(crate::components::NoopComponent));
        let layout = auto_layout_for_windows(&[k1, k2]).unwrap();
        let node = layout.root();
        match node {
            crate::layout::LayoutNode::Split { .. } => {
                assert!(node.subtree_any(|id| id == k1));
                assert!(node.subtree_any(|id| id == k2));
            }
            _ => panic!("expected Split for two windows"),
        }
    }

    #[test]
    fn auto_layout_three_windows_all_present() {
        let mut wm = crate::window::WindowManager::with_config(
            crate::wm_config::WmConfig::standalone(),
            std::sync::Arc::new(crate::AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
            None,
        );
        let k1 = wm.create_window(Box::new(crate::components::NoopComponent));
        let k2 = wm.create_window(Box::new(crate::components::NoopComponent));
        let k3 = wm.create_window(Box::new(crate::components::NoopComponent));
        let layout = auto_layout_for_windows(&[k1, k2, k3]).unwrap();
        let node = layout.root();
        assert!(node.subtree_any(|id| id == k1));
        assert!(node.subtree_any(|id| id == k2));
        assert!(node.subtree_any(|id| id == k3));
    }

    #[test]
    fn auto_layout_multiple_windows_uses_all() {
        let mut wm = crate::window::WindowManager::with_config(
            crate::wm_config::WmConfig::standalone(),
            std::sync::Arc::new(crate::AppContext::new("test", "0.0.0")),
            None,
            None,
            None,
            None,
            None,
        );
        // Use 4 windows — this reliably works within 80x24 default area
        let keys: Vec<WindowKey> = (0..4)
            .map(|_| wm.create_window(Box::new(crate::components::NoopComponent)))
            .collect();
        let layout = auto_layout_for_windows(&keys).unwrap();
        let node = layout.root();
        for k in &keys {
            assert!(
                node.subtree_any(|id| id == *k),
                "window must appear in layout"
            );
        }
    }

    use crate::components::Overlay;

    #[derive(Debug)]
    struct TestMenu;
    impl Component<TermWmAction> for TestMenu {
        fn render(
            &mut self,
            _backend: &mut dyn term_wm_render::RenderBackend,
            _area: LayoutRect,
            _ctx: &crate::components::ComponentContext,
            _registry: &mut crate::hitbox_registry::HitboxRegistry,
        ) {
        }
        fn handle_events(
            &mut self,
            _event: &Event,
            _ctx: &crate::components::ComponentContext,
        ) -> crate::actions::EventResult<TermWmAction> {
            crate::actions::EventResult::Consumed
        }
        fn update(
            &mut self,
            _action: TermWmAction,
            _ctx: &crate::components::ComponentContext,
            _queue: &mut VecDeque<(crate::window::WindowKey, TermWmAction)>,
        ) {
        }
        fn destroy(&mut self) {}
    }
    impl Overlay<TermWmAction> for TestMenu {
        fn handle_confirm_event(
            &mut self,
            _event: &Event,
        ) -> Option<crate::actions::ConfirmAction> {
            None
        }
        fn visible(&self) -> bool {
            false
        }
    }
    impl crate::components::WmComponent for TestMenu {}
}

#[cfg(test)]
mod power_calibration_tests {
    use super::*;
    use std::collections::HashSet;
    use std::time::Duration;

    struct SpyEventSource {
        captured_max_sleep: Option<Option<Duration>>,
        mock_dirty_windows: HashSet<WindowKey>,
    }

    impl EventSource for SpyEventSource {
        fn poll(&mut self, _: Duration) -> io::Result<bool> {
            Ok(false)
        }
        fn read(&mut self) -> io::Result<Event> {
            unreachable!()
        }
        fn next_key(&mut self) -> io::Result<crate::events::KeyEvent> {
            unreachable!()
        }
        fn next_mouse(&mut self) -> io::Result<crate::events::MouseEvent> {
            unreachable!()
        }
        fn set_max_sleep_duration(&mut self, d: Option<Duration>) {
            self.captured_max_sleep = Some(d);
        }
        fn take_dirty_windows(&mut self) -> HashSet<WindowKey> {
            std::mem::take(&mut self.mock_dirty_windows)
        }
    }

    #[test]
    fn scheduler_deadline_propagates_to_driver() {
        let mut driver = SpyEventSource {
            captured_max_sleep: None,
            mock_dirty_windows: HashSet::new(),
        };
        let sched = crate::task_scheduler::TaskScheduler::<crate::actions::SystemTask>::new();
        let handle = sched.handle();
        handle.schedule_once(
            Duration::from_millis(250),
            crate::actions::SystemTask::DragSnap,
        );

        let time_left = handle.time_until_next();
        driver.set_max_sleep_duration(time_left);

        let captured = driver.captured_max_sleep.unwrap().unwrap();
        assert!(captured <= Duration::from_millis(250));
        assert!(captured > Duration::from_millis(200));
    }

    #[test]
    fn dirty_keys_are_drained_cleanly_on_frame_completion() {
        let mut key_set = HashSet::new();
        key_set.insert(WindowKey::default());
        let mut driver = SpyEventSource {
            captured_max_sleep: None,
            mock_dirty_windows: key_set,
        };

        let first = EventSource::take_dirty_windows(&mut driver);
        assert_eq!(first.len(), 1);

        let second = EventSource::take_dirty_windows(&mut driver);
        assert!(second.is_empty(), "dirty keys must drain between cycles");
    }
}
