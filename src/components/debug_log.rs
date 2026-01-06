use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock};

use crossterm::event::{Event, KeyCode, MouseEventKind};
use ratatui::style::Style;
use ratatui::text::{Line, Text};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, layout::Rect};

use crate::components::Component;

const DEFAULT_MAX_LINES: usize = 2000;
static GLOBAL_LOG: OnceLock<DebugLogHandle> = OnceLock::new();
static PANIC_HOOK_INSTALLED: OnceLock<()> = OnceLock::new();

pub fn set_global_debug_log(handle: DebugLogHandle) -> bool {
    GLOBAL_LOG.set(handle).is_ok()
}

pub fn global_debug_log() -> Option<DebugLogHandle> {
    GLOBAL_LOG.get().cloned()
}

pub fn log_line(line: impl Into<String>) {
    if let Some(handle) = GLOBAL_LOG.get() {
        handle.push(line);
    }
}

pub fn install_panic_hook() {
    if PANIC_HOOK_INSTALLED.get().is_some() {
        return;
    }
    let _ = PANIC_HOOK_INSTALLED.set(());
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(handle) = GLOBAL_LOG.get() {
            handle.push("".to_string());
            handle.push("=== PANIC ===".to_string());
            if let Some(location) = info.location() {
                handle.push(format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                ));
            }
            if let Some(msg) = info.payload().downcast_ref::<&str>() {
                handle.push(format!("message: {msg}"));
            } else if let Some(msg) = info.payload().downcast_ref::<String>() {
                handle.push(format!("message: {msg}"));
            } else {
                handle.push("message: <non-string panic>".to_string());
            }
            let backtrace = std::backtrace::Backtrace::force_capture();
            for line in backtrace.to_string().lines() {
                handle.push(line.to_string());
            }
            handle.push("============".to_string());
        }
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
            let text = String::from_utf8_lossy(&self.pending).to_string();
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
        let text = String::from_utf8_lossy(&drained).to_string();
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
pub struct DebugLogComponent {
    handle: DebugLogHandle,
    scroll_from_bottom: usize,
}

impl DebugLogComponent {
    pub fn new(max_lines: usize) -> (Self, DebugLogHandle) {
        let handle = DebugLogHandle {
            inner: Arc::new(Mutex::new(DebugLogBuffer::new(max_lines))),
        };
        (
            Self {
                handle: handle.clone(),
                scroll_from_bottom: 0,
            },
            handle,
        )
    }

    pub fn new_default() -> (Self, DebugLogHandle) {
        Self::new(DEFAULT_MAX_LINES)
    }

    fn clamp_scroll(&mut self, line_count: usize, area: Rect) -> usize {
        let max_scroll = line_count.saturating_sub(area.height as usize);
        if self.scroll_from_bottom > max_scroll {
            self.scroll_from_bottom = max_scroll;
        }
        max_scroll
    }
}

impl Component for DebugLogComponent {
    fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let buffer = frame.buffer_mut();
        let bounds = area.intersection(buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        for y in bounds.y..bounds.y.saturating_add(bounds.height) {
            for x in bounds.x..bounds.x.saturating_add(bounds.width) {
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.reset();
                    cell.set_symbol(" ");
                }
            }
        }
        let lines = if let Ok(buffer) = self.handle.inner.lock() {
            buffer.lines.iter().cloned().collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let max_scroll = self.clamp_scroll(lines.len(), area);
        let scroll_top = max_scroll.saturating_sub(self.scroll_from_bottom);
        let text = Text::from(lines.into_iter().map(Line::from).collect::<Vec<_>>());
        let mut paragraph = Paragraph::new(text).scroll((scroll_top as u16, 0));
        if focused {
            paragraph = paragraph.style(Style::default().fg(crate::theme::debug_highlight()));
        }
        frame.render_widget(paragraph, area);
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(key) => match key.code {
                KeyCode::PageUp => {
                    self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(5);
                    true
                }
                KeyCode::PageDown => {
                    self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(5);
                    true
                }
                KeyCode::Home => {
                    self.scroll_from_bottom = usize::MAX;
                    true
                }
                KeyCode::End => {
                    self.scroll_from_bottom = 0;
                    true
                }
                _ => false,
            },
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(2);
                    true
                }
                MouseEventKind::ScrollDown => {
                    self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(2);
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{Event, KeyCode, MouseEvent, MouseEventKind};
    use std::io::Write;

    #[test]
    fn debug_log_handle_and_buffer_limits() {
        let (_comp, handle) = DebugLogComponent::new(3);
        handle.push("one");
        handle.push("two");
        handle.push("three");
        handle.push("four");
        // internal buffer should be capped at 3
        if let Ok(buf) = handle.inner.lock() {
            assert_eq!(buf.lines.len(), 3);
            assert_eq!(buf.lines.front().unwrap().as_str(), "two");
        } else {
            panic!("lock failed");
        }
    }

    #[test]
    fn debug_log_writer_flushes_lines() {
        let (_comp, handle) = DebugLogComponent::new(10);
        let mut writer = handle.writer();
        let _ = writer.write(b"first line\nsecond line\npartial");
        // flush should push pending partial when forced
        writer.flush().unwrap();
        if let Ok(buf) = handle.inner.lock() {
            assert!(buf.lines.iter().any(|s| s == "first line"));
            assert!(buf.lines.iter().any(|s| s == "second line"));
            assert!(buf.lines.iter().any(|s| s == "partial"));
        }
    }

    #[test]
    fn debug_log_component_handle_event_scrolls() {
        let (mut comp, _handle) = DebugLogComponent::new(10);
        assert_eq!(comp.scroll_from_bottom, 0);
        comp.handle_event(&Event::Key(crossterm::event::KeyEvent::new(
            KeyCode::PageUp,
            crossterm::event::KeyModifiers::NONE,
        )));
        assert!(comp.scroll_from_bottom >= 5);
        let before = comp.scroll_from_bottom;
        comp.handle_event(&Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }));
        // scroll_from_bottom should have decreased or stayed at zero
        assert!(comp.scroll_from_bottom <= before);
    }
}
