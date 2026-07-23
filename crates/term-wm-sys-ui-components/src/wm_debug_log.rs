// TODO: Look into https://crates.io/crates/tui-logger

use std::collections::VecDeque;
use std::sync::OnceLock;

use ratatui::text::{Line, Text};
use term_wm_core::events::Event;
use term_wm_layout_engine::LayoutRect;

use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext, SelectionStatus};
use term_wm_core::debug_event_flags;
use term_wm_core::window::WindowKey;
use term_wm_ui_components::{ScrollViewComponent, TextRendererComponent};

// Re-export from core so callers don't need two imports.
pub use term_wm_core::debug_log::{
    DebugLogHandle, DebugLogWriter, global_debug_log, set_global_debug_log,
};

static PANIC_HOOK_INSTALLED: OnceLock<()> = OnceLock::new();

pub fn trigger_error() {
    debug_event_flags::trigger_error_pending();
}

pub fn install_panic_hook() {
    if PANIC_HOOK_INSTALLED.get().is_some() {
        return;
    }
    let _ = PANIC_HOOK_INSTALLED.set(());
    std::panic::set_hook(Box::new(move |info| {
        let mut lines: Vec<String> = Vec::new();
        lines.push("=== PANIC ===".to_string());
        if let Some(location) = info.location() {
            lines.push(format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            ));
        }
        if let Some(msg) = info.payload().downcast_ref::<&str>() {
            lines.push(format!("message: {msg}"));
        } else if let Some(msg) = info.payload().downcast_ref::<String>() {
            lines.push(format!("message: {msg}"));
        } else {
            lines.push("message: <non-string panic>".to_string());
        }
        let backtrace = std::backtrace::Backtrace::force_capture();
        for line in backtrace.to_string().lines() {
            lines.push(line.to_string());
        }
        lines.push("============".to_string());

        // Write to debug log buffer (for in-app viewing).
        // The buffer lives in term-wm-core and persists across component
        // destruction/re-creation cycles.
        if let Some(handle) = global_debug_log() {
            for line in &lines {
                handle.push(line.to_string());
            }
        }

        debug_event_flags::trigger_panic_pending();
        // IMPORTANT!  Do *not* call take_hook. It blows up the terminal
        // and prevents the debug log from properly opening
        tracing::error!("{}", lines.join("\n"));
    }));
}

#[derive(Debug)]
pub struct WmDebugLogComponent {
    handle: DebugLogHandle,
    scroll_view: ScrollViewComponent<TextRendererComponent>,
}

impl Component<TermWmAction> for WmDebugLogComponent {
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let lines = self.handle.lines();
        let text = Text::from(lines.into_iter().map(Line::from).collect::<Vec<_>>());
        {
            let mut content = self.scroll_view.content.borrow_mut();
            content.set_text(text);
            content.set_wrap(false);
        }

        self.scroll_view.render(backend, area, ctx, registry);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        self.scroll_view.handle_events(event, ctx)
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        self.scroll_view.update(action, ctx, actions);
    }

    fn destroy(&mut self) {}

    fn selection_status(&self) -> SelectionStatus {
        self.scroll_view.selection_status()
    }

    fn selection_text(&self) -> Option<String> {
        self.scroll_view.selection_text()
    }
}

impl WmDebugLogComponent {
    pub fn new(max_lines: usize) -> (Self, DebugLogHandle) {
        let handle = DebugLogHandle::new(max_lines);
        let mut renderer = TextRendererComponent::new();
        renderer.set_wrap(false);
        let mut scroll_view = ScrollViewComponent::new(renderer);
        scroll_view.set_sticky_bottom(true);
        (
            Self {
                handle: handle.clone(),
                scroll_view,
            },
            handle,
        )
    }

    pub fn new_default() -> (Self, DebugLogHandle) {
        Self::new(term_wm_core::debug_log::DEFAULT_MAX_LINES)
    }

    /// Create from an existing handle (preserves log history).
    pub fn from_handle(handle: DebugLogHandle) -> Self {
        let mut renderer = TextRendererComponent::new();
        renderer.set_wrap(false);
        let mut scroll_view = ScrollViewComponent::new(renderer);
        scroll_view.set_sticky_bottom(true);
        Self {
            handle,
            scroll_view,
        }
    }

    pub fn set_selection_enabled(&mut self, enabled: bool) {
        self.scroll_view
            .content
            .borrow_mut()
            .set_selection_enabled(enabled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use term_wm_core::events::{Event, KeyCode, MouseEvent, MouseEventKind};
    use term_wm_layout_engine::LayoutRect;

    #[test]
    fn debug_log_handle_and_buffer_limits() {
        let (_comp, handle) = WmDebugLogComponent::new(3);
        handle.push("one");
        handle.push("two");
        handle.push("three");
        handle.push("four");
        let lines = handle.lines();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].as_str(), "two");
    }

    #[test]
    fn debug_log_writer_flushes_lines() {
        let (_comp, handle) = WmDebugLogComponent::new(10);
        let mut writer = handle.writer();
        let _ = writer.write(b"first line\nsecond line\npartial");
        writer.flush().unwrap();
        let lines = handle.lines();
        assert!(lines.iter().any(|s| s == "first line"));
        assert!(lines.iter().any(|s| s == "second line"));
        assert!(lines.iter().any(|s| s == "partial"));
    }

    #[test]
    fn debug_log_component_handle_event_scrolls() {
        let (mut comp, handle) = WmDebugLogComponent::new(10);
        for i in 0..20 {
            handle.push(format!("line{i}"));
        }
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 5,
        };
        let ratatui_area = term_wm_ui_components::helpers::layout_rect_to_clipped_rect(area);
        let buf = ratatui::buffer::Buffer::empty(ratatui_area);
        let mut backend = term_wm_console::RatatuiBackend::new(buf, ratatui_area);
        {
            Component::render(
                &mut comp,
                &mut backend,
                area,
                &ComponentContext::new(true),
                &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
            );
        }
        let ctx = ComponentContext::new(true);
        let vh = comp.scroll_view.scroll_handle();
        let info = vh.info();
        let max_off = info.offset_y; // after render, sticky_bottom snaps to bottom

        // PageUp: handle_events returns ScrollView action, update processes it
        let page_up = Event::Key(term_wm_core::events::KeyEvent {
            code: KeyCode::PageUp,
            modifiers: term_wm_core::events::KeyModifiers::NONE,
            kind: term_wm_core::events::KeyKind::Press,
        });
        let page_up_result = comp.handle_events(&page_up, &ctx);
        if let term_wm_core::actions::EventResult::Action(action) = page_up_result {
            comp.update(action, &ctx, &mut VecDeque::new());
        }
        assert!(vh.info().offset_y < max_off);

        // ScrollDown mouse event: handle_events returns ScrollView action, update processes it
        let before = vh.info().offset_y;
        let scroll_down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: term_wm_core::events::KeyModifiers::NONE,
        });
        let scroll_result = comp.handle_events(&scroll_down, &ctx);
        if let term_wm_core::actions::EventResult::Action(action) = scroll_result {
            comp.update(action, &ctx, &mut VecDeque::new());
        }
        assert!(vh.info().offset_y >= before);
    }

    #[test]
    fn debug_log_buffer_persists_after_component_recreation() {
        // Create a component with a shared global-like handle
        let (_comp1, handle) = WmDebugLogComponent::new(100);
        handle.push("before-destroy");
        handle.push("still-here");

        // Simulate destroying the old component and creating a new one
        // from the SAME handle (as would happen via global_debug_log())
        let comp2 = WmDebugLogComponent::from_handle(handle.clone());

        // The new component sees the same log history
        let lines = comp2.handle.lines();
        assert!(
            lines.iter().any(|l| l == "before-destroy"),
            "new component must see logs from before re-creation"
        );
        assert!(
            lines.iter().any(|l| l == "still-here"),
            "new component must see all pre-existing logs"
        );

        // Writing to the handle after re-creation is visible to the new component
        handle.push("after-recreation");
        let lines = comp2.handle.lines();
        assert!(
            lines.iter().any(|l| l == "after-recreation"),
            "new component must see logs written after re-creation"
        );
    }

    #[test]
    fn debug_log_captures_logs_while_component_not_rendered() {
        // The global buffer persists even when no WmDebugLogComponent exists.
        // Simulate this by writing to the handle, then creating a component
        // and verifying the logs are there.
        let (_comp, handle) = WmDebugLogComponent::new(100);

        // Simulate logs arriving while no component is mounted
        handle.push("log-while-unmounted-1");
        handle.push("log-while-unmounted-2");

        // Now create a new component from that handle
        let comp = WmDebugLogComponent::from_handle(handle);
        let lines = comp.handle.lines();
        assert!(
            lines.iter().any(|l| l == "log-while-unmounted-1"),
            "component must capture logs that arrived while unmounted"
        );
        assert!(
            lines.iter().any(|l| l == "log-while-unmounted-2"),
            "component must capture all logs that arrived while unmounted"
        );
    }

    #[test]
    fn debug_log_global_buffer_survives_across_recreations() {
        // Write via one handle, recreate from global, verify persistence
        let (_comp1, handle) = WmDebugLogComponent::new(50);

        // Set as global (simulating what main.rs does)
        set_global_debug_log(handle.clone());

        handle.push("first-session-line");

        // Drop the component (simulating close_window)
        drop(_comp1);

        // Re-create from the global handle (simulating toggle)
        let global = global_debug_log().expect("global should be set");
        let comp2 = WmDebugLogComponent::from_handle(global);
        let lines = comp2.handle.lines();
        assert!(
            lines.iter().any(|l| l == "first-session-line"),
            "global buffer must survive component destruction and re-creation"
        );

        // Clean up: replace global so other tests aren't affected
        let (_comp3, fresh_handle) = WmDebugLogComponent::new(50);
        set_global_debug_log(fresh_handle);
    }
}
