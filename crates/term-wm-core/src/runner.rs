use std::io;

use crossterm::event::{Event, KeyEventKind};
use ratatui::buffer::Buffer;
use ratatui::prelude::{Constraint, Direction, Rect};
use ratatui::style::Style;

use crate::components::{Component, ComponentContext, ConfirmAction};
use crate::event_loop::{ControlFlow, EventLoop};
use crate::io::{EventSource, RenderTarget};
use crate::layout::{LayoutNode, TilingLayout};
use crate::ui::UiFrame;
use crate::window::decorator::WindowDecorator;
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

    fn focus_regions(&mut self) -> Vec<Id> {
        self.enumerate_windows()
    }
}

fn handle_focused_app_event<A, Id>(event: &Event, app: &mut A) -> bool
where
    A: WindowProvider<Id>,
    Id: Copy + Eq + Ord + std::fmt::Debug + 'static,
{
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
            let ctx = ComponentContext::new(true);
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
    let mut event_loop = EventLoop::new(driver);
    event_loop
        .driver()
        .set_mouse_capture(app.windows().mouse_capture_enabled())?;

    event_loop.run(|driver, event| {
        let handler = || -> io::Result<ControlFlow> {
            if crate::components::sys::debug_log::take_panic_pending() {
                app.windows().open_debug_window();
            }
            if crate::components::sys::debug_log::take_error_pending() {
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
                Ok(flow)
            };
            if let Some(evt) = event {
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
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue);
                }

                if app.windows().selection_preview_visible() {
                    let _ = app.windows().handle_selection_preview_event(&evt);
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue);
                }
                if app.windows().help_overlay_visible() {
                    let _ = app.windows().handle_help_event(&evt);
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Continue);
                }
                let wm_mode = app.windows().config().wm_overlay_enabled;
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
                    if let Event::Key(_key) = &evt
                        && mapped_action == Some(crate::keybindings::Action::NewWindow)
                    {
                        app.wm_new_window()?;
                        app.windows().close_wm_overlay();
                        update_selection_snapshot(app);
                        return flush_state_changes(app, ControlFlow::Continue);
                    }
                    if let Event::Key(_key) = &evt
                        && (mapped_action == Some(crate::keybindings::Action::FocusNext)
                            || mapped_action == Some(crate::keybindings::Action::FocusPrev))
                    {
                        let _ = app
                            .windows()
                            .handle_focus_event(&evt, focus_regions, &map_region);
                        update_selection_snapshot(app);
                        return flush_state_changes(app, ControlFlow::Continue);
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
                if let Event::Key(key) = &evt
                    && key.kind == KeyEventKind::Press
                    && crate::keybindings::KeyBindings::default()
                        .matches(crate::keybindings::Action::Quit, key)
                {
                    update_selection_snapshot(app);
                    return flush_state_changes(app, ControlFlow::Quit);
                }
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
                update_selection_snapshot(app);
                app.windows().begin_frame();
                output.draw(|frame| draw(frame, app))?;
            }
            flush_state_changes(app, ControlFlow::Continue)
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

fn update_selection_snapshot<A, Id>(app: &mut A)
where
    A: WindowProvider<Id>,
    Id: Copy + Eq + Ord + std::fmt::Debug + 'static,
{
    let focus_id = app.windows().wm_focus_app();
    if let Some(id) = focus_id
        && let Some(component) = app.window_component(id)
    {
        let status = component.selection_status();
        let text = if status.active || status.dragging {
            component.selection_text()
        } else {
            None
        };
        app.windows()
            .set_selection_snapshot(status.active, status.dragging, text);
    } else {
        app.windows().set_selection_snapshot(false, false, None);
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
    app.windows().register_managed_layout(area);
    let plan = app.windows().window_draw_plan(frame);
    for task in plan {
        match task {
            DrawTask::App(window) => {
                let (title, decorator) = {
                    let wm = app.windows();
                    let title = wm.window_title(WindowId::App(window.id));
                    let decorator = wm.decorator();
                    (title, decorator)
                };
                composite_window(
                    frame,
                    &window.surface,
                    window.focused,
                    &title,
                    decorator.as_ref(),
                    |subframe| {
                        app.render_window(subframe, window);
                    },
                );
            }
            DrawTask::System(window) => {
                let (title, decorator) = {
                    let wm = app.windows();
                    let title = wm.window_title(WindowId::System(window.id));
                    let decorator = wm.decorator();
                    (title, decorator)
                };
                composite_window(
                    frame,
                    &window.surface,
                    window.focused,
                    &title,
                    decorator.as_ref(),
                    |subframe| {
                        app.windows().render_system_window(subframe, window);
                    },
                );
            }
        }
    }
    app.windows().render_overlays(frame);
}

fn composite_window<F>(
    frame: &mut UiFrame<'_>,
    surface: &WindowSurface,
    focused: bool,
    title: &str,
    decorator: &dyn WindowDecorator,
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
    let mut buffer = Buffer::empty(local_area);
    {
        let mut offscreen = UiFrame::from_parts(local_area, &mut buffer);
        decorator.render_window(&mut offscreen, local_area, title, focused);
        render_content(&mut offscreen);
    }
    frame.blit_from_signed(&buffer, surface.dest);
}

fn auto_layout_for_windows<Id: Copy + Eq + Ord>(windows: &[Id]) -> Option<TilingLayout<Id>> {
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
    }

    #[test]
    fn runner_does_not_quit_when_app_reports_windows_but_wm_has_no_active_regions() {
        use crate::window::WindowManager;

        // Create an empty WindowManager (no active regions/z-order).
        let wm: WindowManager<usize> = WindowManager::new_managed(0);
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
}
