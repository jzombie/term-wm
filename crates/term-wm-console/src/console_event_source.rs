use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

use term_wm_core::events::{
    Event, KeyCode, KeyEvent, KeyKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use term_wm_core::io::EventSource;
use term_wm_core::power_profile::PowerProfile;
use term_wm_core::utils::KeyboardNormalizer;

/// Translate a crossterm event to a core-owned event.
fn translate_crossterm_event(evt: crossterm::event::Event) -> Option<Event> {
    match evt {
        crossterm::event::Event::Key(key) => Some(Event::Key(translate_key_event(key))),
        crossterm::event::Event::Mouse(mouse) => Some(Event::Mouse(translate_mouse_event(mouse))),
        crossterm::event::Event::Resize(w, h) => Some(Event::Resize(w, h)),
        crossterm::event::Event::FocusGained => Some(Event::FocusGained),
        crossterm::event::Event::FocusLost => Some(Event::FocusLost),
        crossterm::event::Event::Paste(text) => Some(Event::Paste(text)),
    }
}

fn translate_key_event(key: crossterm::event::KeyEvent) -> KeyEvent {
    let mut modifiers = translate_key_modifiers(key.modifiers);
    // macOS sends BackTab for Shift+Tab — set shift
    if matches!(key.code, crossterm::event::KeyCode::BackTab) {
        modifiers.shift = true;
    }
    KeyEvent {
        code: translate_key_code(key.code),
        modifiers,
        kind: match key.kind {
            crossterm::event::KeyEventKind::Press => KeyKind::Press,
            crossterm::event::KeyEventKind::Repeat => KeyKind::Repeat,
            crossterm::event::KeyEventKind::Release => KeyKind::Release,
        },
    }
}

fn translate_key_code(code: crossterm::event::KeyCode) -> KeyCode {
    match code {
        crossterm::event::KeyCode::Char(c) => KeyCode::Char(c),
        crossterm::event::KeyCode::Enter => KeyCode::Enter,
        crossterm::event::KeyCode::Tab => KeyCode::Tab,
        crossterm::event::KeyCode::BackTab => KeyCode::Tab,
        crossterm::event::KeyCode::Backspace => KeyCode::Backspace,
        crossterm::event::KeyCode::Esc => KeyCode::Esc,
        crossterm::event::KeyCode::Left => KeyCode::Left,
        crossterm::event::KeyCode::Right => KeyCode::Right,
        crossterm::event::KeyCode::Up => KeyCode::Up,
        crossterm::event::KeyCode::Down => KeyCode::Down,
        crossterm::event::KeyCode::Home => KeyCode::Home,
        crossterm::event::KeyCode::End => KeyCode::End,
        crossterm::event::KeyCode::PageUp => KeyCode::PageUp,
        crossterm::event::KeyCode::PageDown => KeyCode::PageDown,
        crossterm::event::KeyCode::Delete => KeyCode::Delete,
        crossterm::event::KeyCode::Insert => KeyCode::Insert,
        crossterm::event::KeyCode::F(n) => KeyCode::F(n),
        _ => KeyCode::Esc, // Fallback for unrecognized keys
    }
}

fn translate_key_modifiers(mods: crossterm::event::KeyModifiers) -> KeyModifiers {
    KeyModifiers {
        shift: mods.contains(crossterm::event::KeyModifiers::SHIFT),
        control: mods.contains(crossterm::event::KeyModifiers::CONTROL),
        alt: mods.contains(crossterm::event::KeyModifiers::ALT),
    }
}

fn translate_mouse_event(mouse: crossterm::event::MouseEvent) -> MouseEvent {
    MouseEvent {
        kind: match mouse.kind {
            crossterm::event::MouseEventKind::Down(btn) => {
                MouseEventKind::Press(translate_button(btn))
            }
            crossterm::event::MouseEventKind::Up(btn) => {
                MouseEventKind::Release(translate_button(btn))
            }
            crossterm::event::MouseEventKind::Drag(btn) => {
                MouseEventKind::Drag(translate_button(btn))
            }
            crossterm::event::MouseEventKind::Moved => MouseEventKind::Moved,
            crossterm::event::MouseEventKind::ScrollUp => MouseEventKind::ScrollUp,
            crossterm::event::MouseEventKind::ScrollDown => MouseEventKind::ScrollDown,
            crossterm::event::MouseEventKind::ScrollLeft => MouseEventKind::ScrollLeft,
            crossterm::event::MouseEventKind::ScrollRight => MouseEventKind::ScrollRight,
        },
        modifiers: translate_key_modifiers(mouse.modifiers),
        column: mouse.column,
        row: mouse.row,
    }
}

fn translate_button(btn: crossterm::event::MouseButton) -> MouseButton {
    match btn {
        crossterm::event::MouseButton::Left => MouseButton::Left,
        crossterm::event::MouseButton::Right => MouseButton::Right,
        crossterm::event::MouseButton::Middle => MouseButton::Middle,
    }
}

/// Reads crossterm input events directly on the main thread.
///
/// Used in tests and embedded mode.  Production uses [`UnifiedEventSource`]
/// which runs input on a background thread and integrates PTY wakeups.
///
/// [`UnifiedEventSource`]: super::unified_event_source::UnifiedEventSource
pub struct ConsoleEventSource {
    normalizer: KeyboardNormalizer,
    event_queue: VecDeque<Event>,
    last_event_at: Option<Instant>,
    /// Set by the runner when there's pending work (e.g. countdown timer)
    /// to force frequent polling regardless of recent input activity.
    pending_work: bool,
}

impl Default for ConsoleEventSource {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsoleEventSource {
    pub fn new() -> Self {
        Self {
            normalizer: KeyboardNormalizer::new(),
            event_queue: VecDeque::new(),
            last_event_at: None,
            pending_work: false,
        }
    }

    fn read_internal(&mut self) -> io::Result<Event> {
        loop {
            let evt = crossterm::event::read()?;
            if let Some(translated) = translate_crossterm_event(evt)
                && let Some(normalized) = self.normalizer.normalize(translated)
            {
                return Ok(normalized);
            }
        }
    }
}

impl EventSource for ConsoleEventSource {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        if !self.event_queue.is_empty() {
            self.last_event_at = Some(Instant::now());
            return Ok(true);
        }
        let has_event = crossterm::event::poll(timeout)?;
        if has_event {
            self.last_event_at = Some(Instant::now());
        }
        Ok(has_event)
    }

    fn read(&mut self) -> io::Result<Event> {
        if let Some(evt) = self.event_queue.pop_front() {
            self.last_event_at = Some(Instant::now());
            return Ok(evt);
        }
        let evt = self.read_internal()?;
        self.last_event_at = Some(Instant::now());
        Ok(evt)
    }

    fn next_key(&mut self) -> io::Result<KeyEvent> {
        loop {
            if let Some(index) = self
                .event_queue
                .iter()
                .position(|e| matches!(e, Event::Key(_)))
                && let Some(Event::Key(key)) = self.event_queue.remove(index)
            {
                return Ok(key);
            }

            let evt = self.read_internal()?;
            if let Event::Key(key) = evt {
                return Ok(key);
            } else {
                self.event_queue.push_back(evt);
            }
        }
    }

    fn next_mouse(&mut self) -> io::Result<MouseEvent> {
        loop {
            if let Some(index) = self
                .event_queue
                .iter()
                .position(|e| matches!(e, Event::Mouse(_)))
                && let Some(Event::Mouse(mouse)) = self.event_queue.remove(index)
            {
                return Ok(mouse);
            }

            let evt = self.read_internal()?;
            if let Event::Mouse(mouse) = evt {
                return Ok(mouse);
            } else {
                self.event_queue.push_back(evt);
            }
        }
    }

    fn set_mouse_capture(&mut self, enabled: bool) -> io::Result<()> {
        if enabled {
            crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)
        } else {
            crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)
        }
    }

    /// Called by the runner each cycle to signal whether there's pending
    /// work (e.g. a countdown timer).  When true the event source includes
    /// it in `has_dirty_windows` so the power profile stays at Streaming.
    fn set_pending_work(&mut self, pending: bool) {
        self.pending_work = pending;
    }

    fn poll_interval(&self) -> Duration {
        self.current_profile().poll_interval()
    }

    fn current_profile(&self) -> PowerProfile {
        term_wm_core::power_profile::profile_from_activity(self.last_event_at, self.pending_work)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_key_from_queue() {
        let mut d = ConsoleEventSource::new();
        d.event_queue.push_back(Event::Key(KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        }));
        d.event_queue.push_back(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }));
        let key = d.next_key().unwrap();
        assert_eq!(key.code, KeyCode::Char('a'));
        // the mouse event should remain in the queue
        assert!(matches!(d.event_queue.front(), Some(Event::Mouse(_))));
    }

    #[test]
    fn next_mouse_from_queue() {
        let mut d = ConsoleEventSource::new();
        d.event_queue.push_back(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 2,
            row: 3,
            modifiers: KeyModifiers::NONE,
        }));
        let mouse = d.next_mouse().unwrap();
        assert_eq!(mouse.column, 2);
        assert_eq!(mouse.row, 3);
    }

    #[test]
    fn poll_and_read_from_queue() {
        let mut d = ConsoleEventSource::new();
        d.event_queue.push_back(Event::Key(KeyEvent {
            code: KeyCode::Char('z'),
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        }));
        assert!(d.poll(std::time::Duration::from_millis(0)).unwrap());
        let ev = d.read().unwrap();
        if let Event::Key(k) = ev {
            assert_eq!(k.code, KeyCode::Char('z'));
        } else {
            panic!("expected key");
        }
    }

    #[test]
    fn pending_work_elevates_profile_to_streaming() {
        use term_wm_core::power_profile::PowerProfile;
        let mut d = ConsoleEventSource::new();
        assert_eq!(
            d.current_profile(),
            PowerProfile::PowerSaver,
            "no activity, no pending_work → PowerSaver"
        );
        d.set_pending_work(true);
        assert_eq!(
            d.current_profile(),
            PowerProfile::Streaming,
            "pending_work must elevate to Streaming"
        );
        d.set_pending_work(false);
        assert_eq!(
            d.current_profile(),
            PowerProfile::PowerSaver,
            "clearing pending_work restores PowerSaver"
        );
    }
}
