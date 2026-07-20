// TODO: Look into https://crates.io/crates/tui-logger

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock};

use ratatui::text::{Line, Text};
use term_wm_core::events::Event;
use term_wm_layout_engine::LayoutRect;

use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext, SelectionStatus};
use term_wm_core::debug_event_flags;
use term_wm_core::utils::ansi::strip_ansi_escapes;
use term_wm_core::window::WindowKey;
use term_wm_ui_components::{ScrollViewComponent, TextRendererComponent};

const DEFAULT_MAX_LINES: usize = 2000;
static GLOBAL_LOG: OnceLock<DebugLogHandle> = OnceLock::new();
static PANIC_HOOK_INSTALLED: OnceLock<()> = OnceLock::new();

pub fn set_global_debug_log(handle: DebugLogHandle) -> bool {
    GLOBAL_LOG.set(handle).is_ok()
}

pub fn global_debug_log() -> Option<DebugLogHandle> {
    GLOBAL_LOG.get().cloned()
}

pub fn trigger_error() {
    debug_event_flags::trigger_error_pending();
}

pub fn install_panic_hook() {
    if PANIC_HOOK_INSTALLED.get().is_some() {
        return;
    }
    let _ = PANIC_HOOK_INSTALLED.set(());
    let prev = std::panic::take_hook();
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
        if let Some(handle) = GLOBAL_LOG.get() {
            for line in &lines {
                handle.push(line.to_string());
            }
        }

        debug_event_flags::trigger_panic_pending();
        prev(info);
    }));
}

#[derive(Debug)]
struct DebugLogBuffer {
    lines: VecDeque<String>,
    max_lines: usize,
}

impl DebugLogBuffer {
    fn new(max_lines: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            max_lines: max_lines.max(1),
        }
    }

    fn push_line(&mut self, line: String) {
        self.lines.push_back(line);
        while self.lines.len() > self.max_lines {
            self.lines.pop_front();
        }
    }
}

#[derive(Clone, Debug)]
pub struct DebugLogHandle {
    inner: Arc<Mutex<DebugLogBuffer>>,
}

impl DebugLogHandle {
    pub fn push(&self, line: impl Into<String>) {
        if let Ok(mut buffer) = self.inner.lock() {
            buffer.push_line(line.into());
        }
    }

    pub fn writer(&self) -> DebugLogWriter {
        DebugLogWriter::new(self.clone())
    }

    /// Read back all log lines (clones the internal buffer).
    pub fn lines(&self) -> Vec<String> {
        self.inner
            .lock()
            .map(|buf| buf.lines.iter().cloned().collect())
            .unwrap_or_default()
    }
}

#[derive(Debug)]
pub struct DebugLogWriter {
    handle: DebugLogHandle,
    pending: Vec<u8>,
}

impl DebugLogWriter {
    pub fn new(handle: DebugLogHandle) -> Self {
        Self {
            handle,
            pending: Vec::new(),
        }
    }

    fn flush_pending(&mut self, force: bool) {
        if self.pending.is_empty() {
            return;
        }
        if force {
            let text = strip_ansi_escapes(&String::from_utf8_lossy(&self.pending));
            self.pending.clear();
            for line in text.split('\n') {
                if !line.is_empty() || force {
                    self.handle.push(line.to_string());
                }
            }
            return;
        }
        let Some(pos) = self.pending.iter().rposition(|b| *b == b'\n') else {
            return;
        };
        let drained: Vec<u8> = self.pending.drain(..=pos).collect();
        let text = strip_ansi_escapes(&String::from_utf8_lossy(&drained));
        for line in text.split('\n') {
            if !line.is_empty() {
                self.handle.push(line.to_string());
            }
        }
    }
}

impl Write for DebugLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        self.flush_pending(false);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_pending(true);
        Ok(())
    }
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
        let lines = if let Ok(buffer) = self.handle.inner.lock() {
            buffer.lines.iter().cloned().collect::<Vec<_>>()
        } else {
            Vec::new()
        };

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
        let handle = DebugLogHandle {
            inner: Arc::new(Mutex::new(DebugLogBuffer::new(max_lines))),
        };
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
        Self::new(DEFAULT_MAX_LINES)
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
        if let Ok(buf) = handle.inner.lock() {
            assert_eq!(buf.lines.len(), 3);
            assert_eq!(buf.lines.front().unwrap().as_str(), "two");
        } else {
            panic!("lock failed");
        }
    }

    #[test]
    fn debug_log_writer_flushes_lines() {
        let (_comp, handle) = WmDebugLogComponent::new(10);
        let mut writer = handle.writer();
        let _ = writer.write(b"first line\nsecond line\npartial");
        writer.flush().unwrap();
        if let Ok(buf) = handle.inner.lock() {
            assert!(buf.lines.iter().any(|s| s == "first line"));
            assert!(buf.lines.iter().any(|s| s == "second line"));
            assert!(buf.lines.iter().any(|s| s == "partial"));
        }
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
}
