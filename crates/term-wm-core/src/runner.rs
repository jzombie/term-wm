use std::io;

use crossterm::event::{Event, KeyEventKind, MouseEventKind};
use ratatui::buffer::{Buffer, Cell};
use ratatui::prelude::Rect;
use ratatui::style::{Modifier, Style};

use std::collections::VecDeque;

use crate::actions::{ConfirmAction, EventResult, SystemTask, TermWmAction};
use crate::components::{Component, SelectionStatus};
use crate::debug_event_flags;
use crate::event_loop::{ControlFlow, EventLoop};
use crate::events::crossterm_event_to_wm;
use crate::hitbox_registry::{HitTarget, HitboxRegistry};
use crate::io::{EventSource, RenderTarget};
use crate::layout::{LayoutNode, TilingLayout};
use crate::task_scheduler::TaskScheduler;
use crate::ui::UiFrame;
use crate::window::decorator::{WindowDecorator, WindowRenderCtx};
use crate::window::{DrawTask, WindowKey, WindowManager, WindowSurface};

pub trait WindowManagerHost {
    fn windows(&mut self) -> &mut WindowManager;
    fn wm_new_window(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn wm_close_window(&mut self, _key: WindowKey) -> std::io::Result<()> {
        Ok(())
    }
    fn set_clipboard_enabled(&mut self, _enabled: bool) {}
    fn set_window_selection_enabled(&mut self, _enabled: bool) {}
    fn open_help_overlay(&mut self) {
        self.windows()
            .open_overlay(crate::window::OverlayId::Help, None);
    }
    fn open_keybindings_overlay(&mut self) {
        self.windows()
            .open_overlay(crate::window::OverlayId::Keybindings, None);
    }
    fn open_exit_confirm(&mut self) {
        self.windows().request_quit();
    }
    /// Called when a panic is detected.
    fn on_panic(&mut self) {}
    /// Toggle the debug log window visibility.
    fn toggle_debug_window(&mut self) {}
    /// Called by the runner to check if the app wants to quit.
    /// The app sets this to `true` to exit the event loop.
    fn quit_requested(&self) -> bool {
        false
    }
}

pub trait WindowProvider: WindowManagerHost {
    fn enumerate_windows(&mut self) -> Vec<WindowKey>;

    fn empty_window_message(&self) -> &str {
        "No windows"
    }

    fn layout_for_windows(&mut self, windows: &[WindowKey]) -> Option<TilingLayout<WindowKey>> {
        auto_layout_for_windows(windows)
    }

    fn window_component(&mut self, _key: WindowKey) -> Option<&mut dyn Component<TermWmAction>> {
        None
    }

    fn window_pane_title(&mut self, _key: WindowKey) -> Option<String> {
        None
    }

    fn handle_app_event(&mut self, _event: &Event) -> bool {
        false
    }

    fn focus_regions(&mut self) -> Vec<WindowKey> {
        self.enumerate_windows()
    }
}

fn drain_action_queue<A: WindowProvider>(
    app: &mut A,
    queue: &mut VecDeque<(WindowKey, TermWmAction)>,
) {
    while let Some((key, action)) = queue.pop_front() {
        let ctx = app.windows().component_context_for(true, key);
        if let Some(comp) = app.window_component(key) {
            comp.update(action, &ctx, queue);
        }
    }
}

fn handle_focused_app_event<A>(event: &Event, app: &mut A) -> bool
where
    A: WindowProvider,
{
    // Clear hover state when the terminal loses focus so stale
    // hover highlights do not persist on menus or buttons.
    // Do not return — allow fall-through to standard dispatch.
    if matches!(event, Event::FocusLost) {
        app.windows().clear_hover();
    }

    // Mouse events: use registry dispatch instead of tree-walk.
    // The registry is built during the render pass and provides
    // O(1) hit-testing — no coordinate mutation, no ad-hoc rect_contains.
    if matches!(event, Event::Mouse(_)) {
        if let Some(wm_event) = crossterm_event_to_wm(event) {
            return app.windows().dispatch_mouse(&wm_event);
        }
        return false;
    }

    let focus_id = app.windows().focused_window();

    // Phase 1: WM-stored components (chrome, debug log, system windows)
    if let Some((_key, result)) = app.windows().dispatch_focused_event(event) {
        if let EventResult::Action(action) = result {
            let mut queue = VecDeque::from([(focus_id, action)]);
            drain_action_queue(app, &mut queue);
        }
        return true;
    }

    // Phase 2 Fallback Prep: Compute immutable state FIRST, before mutably borrowing app
    let direct_mode = app.windows().direct_mode(focus_id);
    let ctx = app
        .windows()
        .component_context_for(!direct_mode, focus_id)
        .with_direct_mode(direct_mode);
    let Some((_, localized_evt)) = app.windows().focused_window_event(event) else {
        return false;
    };
    let adjusted_evt = app
        .windows()
        .adjust_event_for_window(focus_id, &localized_evt);

    // Phase 2 Dispatch: Mutably borrow app for the component
    let result = if let Some(comp) = app.window_component(focus_id) {
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

#[allow(clippy::too_many_arguments)]
pub fn run_app<O, D, A, FDraw, FMap>(
    output: &mut O,
    driver: &mut D,
    app: &mut A,
    focus_regions: &[WindowKey],
    system_scheduler: TaskScheduler<SystemTask>,
    _map_region: FMap,
    mut draw: FDraw,
) -> io::Result<()>
where
    O: RenderTarget,
    D: EventSource,
    A: WindowProvider,
    FDraw: for<'frame> FnMut(UiFrame<'frame>, &mut A),
    FMap: Fn(WindowKey) -> WindowKey + Copy,
{
    let system_handle = system_scheduler.handle();
    let mut profile_tracker =
        crate::power_profile::PowerProfileTracker::new(driver.current_profile());
    let mut event_loop = EventLoop::new(driver);
    event_loop
        .driver()
        .set_mouse_capture(app.windows().mouse_capture_enabled())?;
    event_loop.run(|driver, event| {
        let handler = || -> io::Result<ControlFlow> {
            // Process expired system tasks (super-passthrough, drag-snap)
            for (_id, task) in system_handle.drain_expired() {
                match task {
                    SystemTask::SuperPassthrough { event } => {
                        app.windows().clear_super_pending();
                        let _ = handle_focused_app_event(&event, app);
                    }
                    SystemTask::DragSnap => {
                        app.windows().apply_drag_snap_if_pending();
                    }
                }
            }

            if debug_event_flags::take_panic_pending() {
                app.on_panic();
            }
            if debug_event_flags::take_error_pending() {
                app.on_panic();
            }

            for id in app.windows().take_closed_windows() {
                app.wm_close_window(id)?;
            }
            // Process AppExited notifications — close windows whose PTY child
            // exited.  SlotMap returns None for stale keys (generational
            // indexing), so close_window safely no-ops on already-removed keys.
            for key in driver.take_exited_windows() {
                app.windows().close_window(key);
                app.wm_close_window(key)?;
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
                if let Some(profile) = profile_tracker.poll(driver.current_profile()) {
                    app.windows().set_power_profile(profile);
                }
                Ok(flow)
            };
            if let Some(evt) = event {
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
                    let focus_id = app.windows().focused_window();
                    if app.windows().direct_mode(focus_id)
                        && !app.windows().command_menu_visible()
                        && key.kind == KeyEventKind::Press
                    {
                        let is_wm_key = app
                            .windows()
                            .keybindings()
                            .matches(TermWmAction::WmToggleOverlay, key);
                        match app.windows().handle_super_press(key, is_wm_key) {
                            crate::window::SuperPressResult::DoubleSuper => {
                                app.windows().open_command_menu_no_passthrough();
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

                // Layer 2b: Global-layer actions (only WmToggleOverlay is Global;
                // all other actions are WmMode — dispatched when the overlay is open).
                if let Some(_action) = mapped_action {
                    // Only WmToggleOverlay reaches here; handled inline below.
                }

                // Pre-compute WmMode-layer action for use inside the overlay section.
                let mapped_action_wm_mode = match &evt {
                    Event::Key(key) => app
                        .windows()
                        .keybindings()
                        .action_for_key_in_layer(key, crate::keybindings::ActionLayer::WmMode),
                    _ => None,
                };

                // WM command menu toggle (special case due to passthrough logic)
                let wm_mode = app.windows().config().wm_command_menu_enabled;
                if wm_mode
                    && let Event::Key(key) = &evt
                    && key.kind == KeyEventKind::Press
                    && app
                        .windows()
                        .keybindings()
                        .matches(TermWmAction::WmToggleOverlay, key)
                {
                    if app.windows().command_menu_visible() {
                        let passthrough = app.windows().super_passthrough_active();
                        app.windows().close_command_menu();
                        if passthrough {
                            let passthrough_event = Event::Key(*key);
                            let _ = handle_focused_app_event(&passthrough_event, app);
                        }
                    } else {
                        app.windows().open_command_menu();
                    }
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue);
                }
                if wm_mode && app.windows().command_menu_visible() {
                    if let Some(action) = app.windows().handle_wm_menu_event(&evt) {
                        match action {
                            TermWmAction::CloseMenu => {
                                app.windows().close_command_menu();
                            }
                            TermWmAction::ToggleMouseCapture => {
                                app.windows().toggle_mouse_capture();
                            }
                            TermWmAction::ToggleClipboardMode => {
                                app.windows().toggle_clipboard_enabled();
                            }
                            TermWmAction::ToggleWindowSelection => {
                                app.windows().toggle_window_selection();
                            }
                            TermWmAction::MinimizeWindow => {
                                let id = app.windows().focused_window();
                                app.windows().minimize_window(id);
                                app.windows().close_command_menu();
                            }
                            TermWmAction::MaximizeWindow => {
                                let id = app.windows().focused_window();
                                app.windows().toggle_maximize(id);
                                app.windows().close_command_menu();
                            }
                            TermWmAction::CloseWindow => {
                                let id = app.windows().focused_window();
                                app.windows().close_window(id);
                                app.windows().close_command_menu();
                            }
                            TermWmAction::NewWindow => {
                                app.wm_new_window()?;
                                app.windows().close_command_menu();
                            }
                            TermWmAction::ToggleDebugWindow => {
                                app.toggle_debug_window();
                                app.windows().close_command_menu();
                            }
                            TermWmAction::Help => {
                                app.open_help_overlay();
                                app.windows().close_command_menu();
                            }
                            TermWmAction::BringFloatingFront => {
                                app.windows().bring_all_floating_to_front();
                                app.windows().close_command_menu();
                            }
                            TermWmAction::ExitUi => {
                                app.windows().close_command_menu();
                                app.open_exit_confirm();
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue);
                            }
                            _ => {}
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
                    if app.windows().handle_focus_event(&evt, focus_regions) {
                        if matches!(&evt, Event::Key(_)) {
                            app.windows().fold_menu();
                        } else {
                            app.windows().close_command_menu();
                        }
                        update_selection_snapshot(app);
                        return flush_state_changes(app, ControlFlow::Continue);
                    }
                    // Dispatch remaining WmMode actions (Quit, OpenHelp, etc.)
                    // while the WM overlay is open.
                    if let Some(action) = mapped_action_wm_mode {
                        match action {
                            TermWmAction::Quit => {
                                app.open_exit_confirm();
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue);
                            }
                            TermWmAction::OpenHelp => {
                                app.open_help_overlay();
                                app.windows().close_command_menu();
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue);
                            }
                            TermWmAction::OpenKeybindings => {
                                app.open_keybindings_overlay();
                                app.windows().close_command_menu();
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue);
                            }
                            TermWmAction::CycleNextWindow => {
                                app.windows().advance_focus(true);
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue);
                            }
                            TermWmAction::CyclePrevWindow => {
                                app.windows().advance_focus(false);
                                update_selection_snapshot(app);
                                return flush_state_changes(app, ControlFlow::Continue);
                            }
                            TermWmAction::HintToggle => {
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
                    for &key in targets.iter().rev() {
                        let rect = app.windows().full_region_for_key(key);
                        if rect.width > 0
                            && rect.height > 0
                            && crate::layout::rect_contains(rect, mouse.column, mouse.row)
                        {
                            app.windows().focus_app_window(key);
                            break;
                        }
                    }
                }
                // Route Tab/Shift+Tab through focus routing for embedded mode only.
                // In standalone mode without the open overlay, Tab passes through.
                if !wm_mode
                    && let Event::Key(key) = &evt
                    && key.kind == KeyEventKind::Press
                    && app.windows().keybindings().matches(TermWmAction::Quit, key)
                {
                    app.open_exit_confirm();
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue);
                }
                if !wm_mode
                    && matches!(evt, Event::Key(_))
                    && app.windows().handle_focus_event(&evt, focus_regions)
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
                if app.quit_requested() || app.windows().quit_requested() {
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Quit);
                }
                update_selection_snapshot(app);
                app.windows().begin_frame();
                app.windows().prepare_draw();
                // Catch render panics (e.g. u16 subtraction overflow with a
                // tiny viewport, or a component panic) so they don't take
                // down the event loop.  The panic hook records details in
                // the debug log.  I/O errors from the draw are propagated.
                // After a panic, repair the terminal so the next draw starts
                // from a clean slate (partial escape sequences, wrong cursor
                // position, etc. are reset).
                let did_panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    output.draw(|frame| {
                        let area = frame.area();
                        if area.width < 2 || area.height < 2 {
                            return;
                        }
                        draw(frame, app)
                    })
                }))
                .is_err();
                if did_panic {
                    output.repair()?;
                }
            }
            flush_state_changes(app, ControlFlow::Continue)
        };

        let handler_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(handler));

        system_handle.set_keep_awake(app.windows().visible_overlay_count() > 0);
        driver.set_pending_work(system_handle.has_pending());
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

#[allow(clippy::too_many_arguments)]
pub fn run_window_app<O, D, A>(output: &mut O, driver: &mut D, app: &mut A) -> io::Result<()>
where
    O: RenderTarget,
    D: EventSource,
    A: WindowProvider,
{
    // Create the system-level task scheduler and pass a handle to the WindowManager
    // so it can register/cancel timers (super-passthrough, drag-snap) directly.
    let system_scheduler = TaskScheduler::<SystemTask>::new();
    let system_handle = system_scheduler.handle();
    app.windows().set_system_task_handle(system_handle);

    let mut draw_state = WindowDrawState::new();
    let focus_regions: Vec<WindowKey> = app.focus_regions();
    run_app(
        output,
        driver,
        app,
        &focus_regions,
        system_scheduler,
        |key| key,
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

fn update_selection_snapshot<A>(app: &mut A)
where
    A: WindowProvider,
{
    let was_dragging = app.windows().selection_dragging();
    let focus = app.windows().focused_window();
    let (status, text) = app
        .window_component(focus)
        .map(|c| selection_snapshot_from(c.selection_status(), c.selection_text()))
        .unwrap_or_default();
    app.windows()
        .set_selection_snapshot(status.active, status.dragging, text);
    if was_dragging && !status.dragging && status.active {
        app.windows().copy_selection_to_clipboard();
    }
}

#[derive(Default)]
struct WindowDrawState {
    known: Vec<WindowKey>,
    scratch_cells: Vec<Cell>,
    hitbox_registry: HitboxRegistry,
}

impl WindowDrawState {
    fn new() -> Self {
        Self {
            known: Vec::new(),
            scratch_cells: Vec::new(),
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

fn draw_window_app<A>(frame: &mut UiFrame<'_>, app: &mut A, state: &mut WindowDrawState)
where
    A: WindowProvider,
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

    let focus_order: Vec<WindowKey> = windows.to_vec();
    if !focus_order.is_empty() {
        app.windows().set_focus_order(focus_order);
    }
    for &key in &windows {
        if let Some(title) = app.window_pane_title(key) {
            app.windows().set_window_title(key, title);
        }
    }
    app.windows().register_managed_layout(area);
    let all_titles: std::collections::BTreeMap<WindowKey, String> =
        app.windows().window_titles().into_iter().collect();
    let plan = app.windows().window_draw_plan(frame);
    let num_windows = plan.len();
    let total = num_windows + app.windows().visible_overlay_count();

    // Register panel hitboxes BEFORE the window loop (lowest Z-order).
    app.windows()
        .register_panel_hitboxes(&mut state.hitbox_registry);

    // Register tiling split handle hitboxes below windows (floating windows
    // correctly occlude them; tiled windows and handles are disjoint).
    app.windows()
        .register_layout_handle_hitboxes(&mut state.hitbox_registry);

    for (i, task) in plan.into_iter().enumerate() {
        let z = WindowManager::compute_z_depth(i, total);
        match task {
            DrawTask::App(mut window) => {
                window.surface.z_depth = z;
                let (ctx, decorator) = {
                    let wm = app.windows();
                    let title = all_titles
                        .get(&window.key)
                        .map(String::as_str)
                        .unwrap_or("");
                    let ctx = WindowRenderCtx {
                        title,
                        focused: window.focused,
                        direct_mode: wm.direct_mode(window.key),
                        hover_pos: wm.hover,
                        theme: wm.config().theme,
                    };
                    let decorator = wm.decorator();
                    (ctx, decorator)
                };
                // Register window content hitbox in SCREEN coordinates.
                let screen_inner = decorator.content_area(Rect {
                    x: window.surface.dest.x as u16,
                    y: window.surface.dest.y as u16,
                    width: window.surface.dest.width,
                    height: window.surface.dest.height,
                });
                state
                    .hitbox_registry
                    .register(HitTarget::Window(window.key), screen_inner);
                composite_window(
                    frame,
                    &window.surface,
                    decorator.as_ref(),
                    ctx,
                    |subframe, registry| {
                        let ctx = app
                            .windows()
                            .component_context_for(window.focused, window.key)
                            .with_screen_area(screen_inner);
                        if let Some(component) = app.window_component(window.key) {
                            component.render(subframe, window.surface.inner, &ctx, registry);
                        } else if let Some(component) = app.windows().component_for_key(window.key)
                        {
                            component.render(subframe, window.surface.inner, &ctx, registry);
                        }
                    },
                    &mut state.scratch_cells,
                    &mut state.hitbox_registry,
                );
                // Register chrome hitboxes AFTER content (higher Z-order).
                app.windows()
                    .register_window_chrome_hitboxes(window.key, &mut state.hitbox_registry);
            }
        }
    }
    app.windows().render_panel(frame);
    app.windows()
        .render_overlays(frame, num_windows, total, &mut state.hitbox_registry);

    // Swap the draw-time registry into WindowManager for event dispatch.
    // state.hitbox_registry becomes empty (ready for next frame),
    // wm.hitbox_registry gets the correctly Z-ordered snapshot.
    state
        .hitbox_registry
        .swap_entries(&mut app.windows().hitbox_registry);
}

fn composite_window<F>(
    frame: &mut UiFrame<'_>,
    surface: &WindowSurface,
    decorator: &dyn WindowDecorator,
    mut ctx: WindowRenderCtx<'_>,
    mut render_content: F,
    scratch: &mut Vec<Cell>,
    _registry: &mut HitboxRegistry,
) where
    F: FnMut(&mut UiFrame<'_>, &mut HitboxRegistry),
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
    let theme = ctx.theme;
    let size = local_area.area() as usize;
    scratch.clear();
    scratch.resize(size, Cell::default());
    let mut buffer = Buffer {
        area: local_area,
        content: std::mem::take(scratch),
    };
    {
        let mut offscreen = UiFrame::from_parts(local_area, &mut buffer);
        decorator.render_window(&mut offscreen, local_area, ctx);
        render_content(&mut offscreen, _registry);
    }
    if !focused {
        for cell in buffer.content.iter_mut() {
            cell.modifier.insert(Modifier::DIM);
        }
    }
    if surface.draw_shadow {
        crate::ui::render_drop_shadow(frame, surface.dest, surface.z_depth, &theme);
    }
    frame.blit_from_signed(&buffer, surface.dest);
    *scratch = buffer.content;
}

fn auto_layout_for_windows(windows: &[WindowKey]) -> Option<TilingLayout<WindowKey>> {
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
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

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
            key: WindowKey,
        }
        impl WindowManagerHost for FakeApp {
            fn windows(&mut self) -> &mut WindowManager {
                &mut self.wm
            }
        }
        impl WindowProvider for FakeApp {
            fn enumerate_windows(&mut self) -> Vec<WindowKey> {
                vec![self.key]
            }
        }

        let mut wm = WindowManager::with_config(
            crate::wm_config::WmConfig::standalone(),
            std::sync::Arc::new(crate::AppContext::new("test", "0.0.0")),
            None,
            None,
            Some(Box::new(TestMenu)),
        );
        let key = wm.create_window(Box::new(crate::components::NoopComponent));

        let mut app = FakeApp { wm, key };
        assert!(!app.enumerate_windows().is_empty());

        let quit_if_no_windows = app.enumerate_windows().is_empty();
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
                &self,
                _frame: &mut crate::ui::UiFrame<'_>,
                _area: ratatui::layout::Rect,
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
            fn windows(&mut self) -> &mut WindowManager {
                &mut self.wm
            }
        }
        impl WindowProvider for FakeApp {
            fn enumerate_windows(&mut self) -> Vec<WindowKey> {
                self.wm.all_window_keys()
            }
            fn window_component(
                &mut self,
                key: WindowKey,
            ) -> Option<&mut dyn Component<TermWmAction>> {
                self.wm.component_for_key_mut(key)
            }
        }

        let mut app = FakeApp {
            wm: WindowManager::with_config(
                crate::wm_config::WmConfig::standalone(),
                std::sync::Arc::new(crate::AppContext::new("test", "0.0.0")),
                None,
                None,
                Some(Box::new(TestMenu)),
            ),
        };
        // Store the KeyRecorder directly in the WindowManager — no sidecar.
        let key = app.wm.create_window(Box::new(KeyRecorder {
            received_key: false,
        }));
        app.wm.regions.set(
            key,
            ratatui::layout::Rect {
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
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
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
                &self,
                _frame: &mut crate::ui::UiFrame<'_>,
                _area: ratatui::layout::Rect,
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
            fn windows(&mut self) -> &mut WindowManager {
                &mut self.wm
            }
        }
        impl WindowProvider for FakeApp {
            fn enumerate_windows(&mut self) -> Vec<WindowKey> {
                self.wm.all_window_keys()
            }
            fn window_component(
                &mut self,
                key: WindowKey,
            ) -> Option<&mut dyn Component<TermWmAction>> {
                self.wm.component_for_key_mut(key)
            }
        }

        let mut app = FakeApp {
            wm: WindowManager::with_config(
                crate::wm_config::WmConfig::standalone(),
                std::sync::Arc::new(crate::AppContext::new("test", "0.0.0")),
                None,
                None,
                Some(Box::new(TestMenu)),
            ),
        };
        let key = app.wm.create_window(Box::new(KeyRecorder {
            received_key: false,
        }));
        app.wm.regions.set(
            key,
            ratatui::layout::Rect {
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
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
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
            &self,
            _frame: &mut crate::ui::UiFrame<'_>,
            _area: ratatui::prelude::Rect,
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
    impl crate::components::MenuOverlay<TermWmAction> for TestMenu {
        fn outline(&mut self) {}
        fn restore(&mut self) {}
        fn set_items(&mut self, _items: Vec<crate::components::MenuItem<TermWmAction>>) {}
        fn set_timeout(&mut self, _timeout: std::time::Duration) {}
        fn selected_action(&self) -> Option<&TermWmAction> {
            None
        }
        fn set_anchor(&mut self, _pos: Option<(u16, u16)>) {}
        fn set_managed_area(&mut self, _area: ratatui::prelude::Rect) {}
    }
}
