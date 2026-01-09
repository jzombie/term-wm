use std::io;
use std::time::Duration;

use crossterm::event::{Event, KeyEventKind};
use ratatui::prelude::{Constraint, Direction};
use ratatui::style::Style;

use crate::components::ConfirmAction;
use crate::drivers::{InputDriver, OutputDriver};
use crate::event_loop::{ControlFlow, EventLoop};
use crate::layout::{LayoutNode, TilingLayout};
use crate::ui::UiFrame;
use crate::window::{AppWindowDraw, LayoutContract, WindowDrawTask, WindowManager, WmMenuAction};

pub trait HasWindowManager<W: Copy + Eq + Ord, R: Copy + Eq + Ord> {
    fn windows(&mut self) -> &mut WindowManager<W, R>;
    fn wm_new_window(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn wm_close_window(&mut self, _id: R) -> std::io::Result<()> {
        Ok(())
    }
}

pub trait WindowApp<W: Copy + Eq + Ord, R: Copy + Eq + Ord>: HasWindowManager<W, R> {
    fn enumerate_windows(&mut self) -> Vec<R>;
    fn render_window(&mut self, frame: &mut UiFrame<'_>, window: AppWindowDraw<R>);

    fn empty_window_message(&self) -> &str {
        "No windows"
    }

    fn layout_for_windows(&mut self, windows: &[R]) -> Option<TilingLayout<R>> {
        auto_layout_for_windows(windows)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_app<O, D, A, W, R, FDraw, FDispatch, FQuit, FMap, FFocus>(
    output: &mut O,
    driver: &mut D,
    app: &mut A,
    focus_regions: &[R],
    map_region: FMap,
    _map_focus: FFocus,
    poll_interval: Duration,
    mut draw: FDraw,
    mut dispatch: FDispatch,
    mut should_quit: FQuit,
) -> io::Result<()>
where
    O: OutputDriver,
    D: InputDriver,
    A: HasWindowManager<W, R>,
    W: Copy + Eq + Ord,
    R: Copy + Eq + Ord + PartialEq<W> + std::fmt::Debug,
    FDraw: for<'frame> FnMut(UiFrame<'frame>, &mut A),
    FDispatch: FnMut(&Event, &mut A) -> bool,
    FQuit: FnMut(Option<&Event>, &mut A) -> bool,
    FMap: Fn(R) -> W + Copy,
    FFocus: Fn(W) -> Option<R>,
{
    let mut event_loop = EventLoop::new(driver, poll_interval);
    event_loop
        .driver()
        .set_mouse_capture(app.windows().mouse_capture_enabled())?;

    // The WindowManager now provides `take_closed_app_windows()` to drain app ids
    // whose windows were closed; we'll poll that each loop and call `app.wm_close_window`.
    // No additional setup required here.

    event_loop.run(|driver, event| {
        let handler = || -> io::Result<ControlFlow> {
            // Check if a panic occurred (e.g. in a background thread or previous iteration)
            // and force the debug window open if so.
            if crate::components::sys::debug_log::take_panic_pending() {
                app.windows().open_debug_window();
            }
            // Also check for reported non-fatal errors that should pop the debug log
            if crate::components::sys::debug_log::take_error_pending() {
                app.windows().open_debug_window();
            }

            // Drain any pending closed app ids recorded by the WindowManager and invoke app cleanup.
            for id in app.windows().take_closed_app_windows() {
                app.wm_close_window(id)?;
            }
            let mut flush_mouse_capture = |app: &mut A, flow: ControlFlow| {
                if let Some(enabled) = app.windows().take_mouse_capture_change() {
                    let _ = driver.set_mouse_capture(enabled);
                }
                Ok(flow)
            };
            if let Some(evt) = event {
                // Map key events to high-level `Action`s once to prefer action-based handling
                let mapped_action = match &evt {
                    Event::Key(key) => {
                        crate::keybindings::KeyBindings::default().action_for_key(key)
                    }
                    _ => None,
                };
                if app.windows().exit_confirm_visible() {
                    if let Some(action) = app.windows().handle_exit_confirm_event(&evt) {
                        match action {
                            ConfirmAction::Confirm => return Ok(ControlFlow::Quit),
                            ConfirmAction::Cancel => app.windows().close_exit_confirm(),
                        }
                    }
                    return flush_mouse_capture(app, ControlFlow::Continue);
                }

                if app.windows().help_overlay_visible() {
                    let _ = app.windows().handle_help_event(&evt);
                    return flush_mouse_capture(app, ControlFlow::Continue);
                }
                let wm_mode = app.windows().layout_contract() == LayoutContract::WindowManaged;
                if wm_mode
                    && let Event::Key(key) = &evt
                    && key.kind == KeyEventKind::Press
                    && crate::keybindings::KeyBindings::default()
                        .matches(crate::keybindings::Action::WmToggleOverlay, key)
                {
                    if app.windows().wm_overlay_visible() {
                        let passthrough = app.windows().esc_passthrough_active();
                        app.windows().close_wm_overlay();
                        if passthrough {
                            let _ = dispatch(&Event::Key(*key), app);
                        }
                    } else {
                        app.windows().open_wm_overlay();
                    }
                    return flush_mouse_capture(app, ControlFlow::Continue);
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
                                app.windows().open_help_overlay();
                                app.windows().close_wm_overlay();
                            }
                            WmMenuAction::BringFloatingFront => {
                                app.windows().bring_all_floating_to_front();
                                app.windows().close_wm_overlay();
                            }
                            WmMenuAction::ExitUi => {
                                app.windows().close_wm_overlay();
                                app.windows().open_exit_confirm();
                                return flush_mouse_capture(app, ControlFlow::Continue);
                            }
                        }
                        return flush_mouse_capture(app, ControlFlow::Continue);
                    }
                    if app.windows().wm_menu_consumes_event(&evt) {
                        return flush_mouse_capture(app, ControlFlow::Continue);
                    }
                    if let Event::Key(_key) = &evt
                        && mapped_action == Some(crate::keybindings::Action::NewWindow)
                    {
                        app.wm_new_window()?;
                        app.windows().close_wm_overlay();
                        return flush_mouse_capture(app, ControlFlow::Continue);
                    }
                    if let Event::Key(_key) = &evt
                        && (mapped_action == Some(crate::keybindings::Action::FocusNext)
                            || mapped_action == Some(crate::keybindings::Action::FocusPrev))
                    {
                        let _ = app.windows().handle_focus_event(
                            &evt,
                            focus_regions,
                            &map_region,
                            &_map_focus,
                        );
                        return flush_mouse_capture(app, ControlFlow::Continue);
                    }
                    if let Event::Key(_) = &evt {
                        return flush_mouse_capture(app, ControlFlow::Continue);
                    }
                }

                if matches!(evt, Event::Mouse(_)) && !app.windows().mouse_capture_enabled() {
                    return flush_mouse_capture(app, ControlFlow::Continue);
                }
                match &evt {
                    Event::Key(_) if app.windows().capture_active() => {
                        app.windows().clear_capture();
                        let _ = dispatch(&evt, app);
                    }
                    _ => {
                        let _ = dispatch(&evt, app);
                    }
                }
            } else {
                if should_quit(None, app) {
                    return flush_mouse_capture(app, ControlFlow::Quit);
                }
                app.windows().begin_frame();
                output.draw(|frame| draw(frame, app))?;
            }
            flush_mouse_capture(app, ControlFlow::Continue)
        };

        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(handler)) {
            Ok(result) => result,
            Err(_) => {
                // TODO: This needs to be improved; currently requires resizing the terminal window to
                // "stabilize" the messages, to produce them in a debug log window. Also, directly setting
                // the mouse capture here bypasses the state, and the UI is not reflected. It might be better
                // to just turn off mouse capturing and crash the app naturally if this cannot be improved.

                // A panic occurred; stop mouse capture to avoid terminal spam
                let _ = driver.set_mouse_capture(false);
                // Attempt to immediately redraw the UI so the debug log (populated by the panic hook)
                // is visible to the user without waiting for another input event like a resize.
                let mut redraw = || -> io::Result<()> {
                    app.windows().begin_frame();
                    output.draw(|frame| draw(frame, app))
                };
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _ = redraw();
                }));
                // Let the panic hook have recorded details into the debug log; continue event loop.
                Ok(ControlFlow::Continue)
            }
        }
    })?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_window_app<O, D, A, W, R, FDispatch, FQuit, FMap, FFocus>(
    output: &mut O,
    driver: &mut D,
    app: &mut A,
    focus_regions: &[R],
    map_region: FMap,
    _map_focus: FFocus,
    poll_interval: Duration,
    dispatch: FDispatch,
    should_quit: FQuit,
) -> io::Result<()>
where
    O: OutputDriver,
    D: InputDriver,
    A: WindowApp<W, R>,
    W: Copy + Eq + Ord,
    R: Copy + Eq + Ord + PartialEq<W> + std::fmt::Debug,
    FDispatch: FnMut(&Event, &mut A) -> bool,
    FQuit: FnMut(Option<&Event>, &mut A) -> bool,
    FMap: Fn(R) -> W + Copy,
    FFocus: Fn(W) -> Option<R>,
{
    let draw_map = map_region;
    let mut draw_state = WindowDrawState::default();
    run_app(
        output,
        driver,
        app,
        focus_regions,
        map_region,
        _map_focus,
        poll_interval,
        move |frame, app| {
            let mut frame = frame;
            draw_window_app(&mut frame, app, &mut draw_state, draw_map);
        },
        dispatch,
        should_quit,
    )
}

struct WindowDrawState<R> {
    known: Vec<R>,
}

impl<R> Default for WindowDrawState<R> {
    fn default() -> Self {
        Self { known: Vec::new() }
    }
}

impl<R: Copy + Eq> WindowDrawState<R> {
    fn update(&mut self, windows: &[R]) -> bool {
        if self.known == windows {
            false
        } else {
            self.known = windows.to_vec();
            true
        }
    }
}

fn draw_window_app<A, W, R, FMap>(
    frame: &mut UiFrame<'_>,
    app: &mut A,
    state: &mut WindowDrawState<R>,
    map_region: FMap,
) where
    A: WindowApp<W, R>,
    W: Copy + Eq + Ord,
    R: Copy + Eq + Ord + PartialEq<W> + std::fmt::Debug,
    FMap: Fn(R) -> W,
{
    let area = frame.area();
    let windows = app.enumerate_windows();
    let windows_changed = state.update(&windows);
    // If no application windows, we still might need to draw system windows (e.g. debug log).
    // The previous check `if windows.is_empty() { return }` prevented this.
    // We now allow proceeding, ensuring the layout is handled gracefully.

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

    let focus_order: Vec<W> = windows.iter().copied().map(map_region).collect();
    if !focus_order.is_empty() {
        app.windows().set_focus_order(focus_order);
    }
    app.windows().register_managed_layout(area);
    let plan = app.windows().window_draw_plan(frame);
    for task in plan {
        match task {
            WindowDrawTask::App(window) => app.render_window(frame, window),
            WindowDrawTask::System(window) => app.windows().render_system_window(frame, window),
        }
    }
    app.windows().render_overlays(frame);
}

fn auto_layout_for_windows<R: Copy + Eq + Ord>(windows: &[R]) -> Option<TilingLayout<R>> {
    let node = match windows.len() {
        0 => return None,
        1 => LayoutNode::leaf(windows[0]),
        2 => LayoutNode::split(
            Direction::Horizontal,
            vec![Constraint::Percentage(50), Constraint::Percentage(50)],
            vec![LayoutNode::leaf(windows[0]), LayoutNode::leaf(windows[1])],
        ),
        len => {
            let mut constraints = Vec::with_capacity(len);
            let base = (100 / len as u16).max(1);
            for idx in 0..len {
                if idx == len - 1 {
                    let used = base.saturating_mul((len - 1) as u16);
                    constraints.push(Constraint::Percentage(100u16.saturating_sub(used)));
                } else {
                    constraints.push(Constraint::Percentage(base));
                }
            }
            let children = windows.iter().map(|&id| LayoutNode::leaf(id)).collect();
            LayoutNode::split(Direction::Vertical, constraints, children)
        }
    };
    Some(TilingLayout::new(node))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auto_layout_empty_and_multiple() {
        let empty: Vec<u8> = vec![];
        assert!(auto_layout_for_windows(&empty).is_none());

        let one = vec![1u8];
        let layout = auto_layout_for_windows(&one).unwrap();
        // single node should be a leaf
        assert!(matches!(layout.root(), crate::layout::LayoutNode::Leaf(_)));

        let many = vec![1u8, 2, 3, 4];
        let layout2 = auto_layout_for_windows(&many).unwrap();
        // for many windows the top-level node should be a split
        assert!(matches!(
            layout2.root(),
            crate::layout::LayoutNode::Split { .. }
        ));
    }

    #[test]
    fn window_draw_state_update_changes() {
        let mut s: WindowDrawState<u8> = WindowDrawState::default();
        assert!(!s.update(&[]));
        assert!(s.update(&[1, 2]));
        assert!(!s.update(&[1, 2]));
    }
}
