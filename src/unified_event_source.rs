use std::collections::{HashSet, VecDeque};
use std::io;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};

use term_wm_core::events::{
    Event, KeyCode, KeyEvent, KeyKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use term_wm_core::io::EventSource;
use term_wm_core::io::frame_pacer::FramePacer;
use term_wm_core::power_profile::PowerProfile;
use term_wm_core::utils::KeyboardNormalizer;
use term_wm_core::window::WindowKey;

/// Capacity of the crossbeam channel between event producers and the event
/// loop. Generous capacity since wakeup gating (dirty.swap) prevents flooding.
const EVENT_CHANNEL_CAPACITY: usize = 256;

/// How often the crossterm input thread polls for new events (100 ms).
/// Keeps the thread responsive to shutdown signals while being idle-friendly.
const INPUT_THREAD_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Events that can flow through the unified event channel.
#[derive(Debug, Clone)]
pub enum UnifiedEvent {
    /// A user-input event from crossterm (key, mouse, resize).
    Input(Event),
    /// A PTY reader thread has new data available for `WindowKey`.
    PtyWakeup(WindowKey),
    /// A PTY child process has exited. Sent from the reader thread on EOF.
    AppExited(WindowKey),
    /// An OS signal was received (SIGINT, SIGTERM).
    Signal,
    /// Periodic tick for timing.
    Tick,
}

/// A unified event source that multiplexes crossterm input, PTY wakeups,
/// and OS signals into a single channel.  The main thread blocks on one
/// receiver instead of polling multiple sources.
pub struct UnifiedEventSource {
    rx: Receiver<UnifiedEvent>,
    /// One Sender clone kept here so we can create it before the input thread.
    tx: Sender<UnifiedEvent>,
    /// Handle to the crossterm input thread.
    _input_handle: JoinHandle<()>,
    /// When true, the input thread should shut down.
    shutdown: Arc<AtomicBool>,
    /// Accumulated PTY wakeups since the last idle tick — batch-drained
    /// so thousands of wakeups/sec collapse into a single render.
    dirty_windows: HashSet<WindowKey>,
    /// Accumulated window exit notifications since the last drain.
    exited_windows: Vec<WindowKey>,
    /// Cached input event (poll returned true, waiting for read).
    pending_event: Option<Event>,
    /// Buffer for input events drained during `drain_pending` so none are lost.
    input_buffer: VecDeque<Event>,
    /// Signal received flag.
    signal_received: bool,
    /// Keyboard normalizer for consistent event handling.
    normalizer: KeyboardNormalizer,
    /// Timestamp of the last input event (for power profiling).
    last_event_at: Option<Instant>,
    /// Frame pacing: ensures renders fire at most once per 16ms interval.
    frame_pacer: FramePacer,
    /// Set by the runner via [`EventSource::set_pending_work`] when there's
    /// pending work (e.g. countdown timer) that requires frequent polling
    /// regardless of PTY dirty-window state.
    pending_work: bool,
    /// Maximum duration the next [`poll`] call is allowed to block.
    /// Set by the runner via [`EventSource::set_max_sleep_duration`] to
    /// clamp the PowerSaver poll interval to the next scheduler deadline.
    ///
    /// [`poll`]: EventSource::poll
    /// [`set_max_sleep_duration`]: EventSource::set_max_sleep_duration
    max_sleep_duration: Option<Duration>,
}

impl UnifiedEventSource {
    /// Create a new unified event source, spawning a background thread
    /// that reads crossterm events.
    ///
    /// The bounded channel (256 slots) provides mechanical backpressure:
    /// when the channel is full, PTY reader threads block on `send()` →
    /// OS pipe buffer fills → child process `write()` blocks → prevents
    /// memory exhaustion under extreme output load.
    pub fn new() -> io::Result<Self> {
        let (tx, rx) = bounded::<UnifiedEvent>(EVENT_CHANNEL_CAPACITY);
        let shutdown = Arc::new(AtomicBool::new(false));
        let input_tx = tx.clone();
        let input_shutdown = Arc::clone(&shutdown);

        let _input_handle = thread::Builder::new()
            .name("crossterm-input".into())
            .spawn(move || {
                // Loop: poll crossterm with 100ms timeout, check shutdown flag.
                loop {
                    if input_shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    // Blocking poll with short timeout so we can check shutdown.
                    if crossterm::event::poll(INPUT_THREAD_POLL_INTERVAL).unwrap_or(false) {
                        match crossterm::event::read() {
                            Ok(crossterm_event) => {
                                // Translate crossterm event to core-owned event
                                if let Some(core_event) = translate_crossterm_event(crossterm_event)
                                    && input_tx.send(UnifiedEvent::Input(core_event)).is_err()
                                {
                                    break; // receiver dropped
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
            })
            .map_err(|e| io::Error::other(e.to_string()))?;

        Ok(Self {
            rx,
            tx,
            _input_handle,
            shutdown,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
        })
    }

    /// Return a sender that PTY reader threads can use to send wakeup pings.
    pub fn pty_wakeup_tx(&self) -> Sender<UnifiedEvent> {
        self.tx.clone()
    }

    /// Drain all pending events from the channel (non-blocking) into internal
    /// state.  Called at the start of each event-loop iteration so PtyWakeup
    /// floods don't cause render-backlog.
    ///
    /// Input events are moved into `input_buffer` so none are lost during
    /// bursts (paste, key repeat).  `poll()` checks the buffer first.
    fn drain_pending(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(UnifiedEvent::Input(event)) => {
                    self.input_buffer.push_back(event);
                }
                Ok(UnifiedEvent::PtyWakeup(key)) => {
                    self.dirty_windows.insert(key);
                }
                Ok(UnifiedEvent::AppExited(key)) => {
                    self.exited_windows.push(key);
                }
                Ok(UnifiedEvent::Signal) => {
                    self.signal_received = true;
                }
                Ok(UnifiedEvent::Tick) => {
                    // No-op — tick is implicit in the event-cycle loop.
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => break,
            }
        }
    }

    /// Check if a signal was received and ack it.
    pub fn take_signal(&mut self) -> bool {
        let sig = self.signal_received;
        self.signal_received = false;
        sig
    }

}

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

impl EventSource for UnifiedEventSource {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        // First drain any pending events non-blocking.
        self.drain_pending();

        if self.pending_event.is_some() || !self.input_buffer.is_empty() {
            return Ok(true);
        }

        // If drain_pending found dirty windows, arm the frame deadline.
        // This prevents a 3600s freeze when a PtyWakeup arrives between
        // handler(None) and drain_pending (common under heavy streaming):
        // the PtyWakeup is consumed by drain_pending but no frame
        // deadline is set, so recv_timeout would block for the full
        // PowerSaver interval.
        if !self.dirty_windows.is_empty() {
            self.frame_pacer.notify_pending();
        }

        // Clamp remaining to the frame deadline so we never block
        // longer than 16ms when there are unprocessed dirty windows.
        if self.frame_pacer.try_expire() {
            return Ok(false);
        }
        let mut remaining = timeout;
        if let Some(t) = self.frame_pacer.time_until_deadline() {
            remaining = remaining.min(t);
        }

        while remaining > Duration::ZERO {
            // Check frame deadline before each blocking call.
            if self.frame_pacer.try_expire() {
                return Ok(false);
            }
            if let Some(t) = self.frame_pacer.time_until_deadline() {
                remaining = remaining.min(t);
                if remaining <= Duration::ZERO {
                    self.frame_pacer.reset();
                    return Ok(false);
                }
            }

            match self.rx.recv_timeout(remaining) {
                Ok(UnifiedEvent::Input(event)) => {
                    self.last_event_at = Some(Instant::now());
                    self.frame_pacer.reset();
                    self.pending_event = Some(event);
                    return Ok(true);
                }
                Ok(UnifiedEvent::PtyWakeup(key)) => {
                    self.dirty_windows.insert(key);
                    self.frame_pacer.notify_pending();
                    if self.frame_pacer.try_expire() {
                        return Ok(false);
                    }
                    if let Some(t) = self.frame_pacer.time_until_deadline() {
                        remaining = remaining.min(t);
                    }
                    continue;
                }
                Ok(UnifiedEvent::AppExited(key)) => {
                    self.exited_windows.push(key);
                    self.frame_pacer.notify_pending();
                    continue;
                }
                Ok(UnifiedEvent::Signal) => {
                    self.signal_received = true;
                    return Ok(false);
                }
                Ok(UnifiedEvent::Tick) => {
                    return Ok(false);
                }
                Err(_) => {
                    // Check if the frame deadline expired during the wait.
                    if self.frame_pacer.try_expire() {
                        return Ok(false);
                    }
                    break;
                }
            }
        }

        self.frame_pacer.reset();
        Ok(false)
    }

    fn read(&mut self) -> io::Result<Event> {
        // Check pending_event first (set by poll()), then drain input_buffer.
        if let Some(event) = self.pending_event.take()
            && let Some(normalized) = self.normalizer.normalize(event)
        {
            return Ok(normalized);
        }
        // Fallback: check buffer, then block on the channel.
        loop {
            if let Some(event) = self.input_buffer.pop_front() {
                self.last_event_at = Some(Instant::now());
                if let Some(normalized) = self.normalizer.normalize(event) {
                    return Ok(normalized);
                }
                continue;
            }
            match self.rx.recv() {
                Ok(UnifiedEvent::Input(event)) => {
                    self.last_event_at = Some(Instant::now());
                    if let Some(normalized) = self.normalizer.normalize(event) {
                        return Ok(normalized);
                    }
                }
                Ok(UnifiedEvent::PtyWakeup(key)) => {
                    self.dirty_windows.insert(key);
                }
                Ok(UnifiedEvent::AppExited(key)) => {
                    self.exited_windows.push(key);
                }
                Ok(UnifiedEvent::Signal) => {
                    self.signal_received = true;
                }
                Ok(UnifiedEvent::Tick) => {}
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "event channel disconnected",
                    ));
                }
            }
        }
    }

    fn next_key(&mut self) -> io::Result<KeyEvent> {
        loop {
            self.drain_pending();
            if let Some(Event::Key(key)) = self.pending_event.take() {
                return Ok(key);
            }
            match self.rx.recv() {
                Ok(UnifiedEvent::Input(Event::Key(key))) => return Ok(key),
                Ok(UnifiedEvent::Input(event)) => {
                    self.pending_event = Some(event);
                }
                Ok(UnifiedEvent::PtyWakeup(_)) => {}
                Ok(UnifiedEvent::AppExited(key)) => {
                    self.exited_windows.push(key);
                }
                Ok(UnifiedEvent::Signal) => self.signal_received = true,
                Ok(UnifiedEvent::Tick) => {}
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "event channel disconnected",
                    ));
                }
            }
        }
    }

    fn next_mouse(&mut self) -> io::Result<MouseEvent> {
        loop {
            self.drain_pending();
            if let Some(Event::Mouse(mouse)) = self.pending_event.take() {
                return Ok(mouse);
            }
            match self.rx.recv() {
                Ok(UnifiedEvent::Input(Event::Mouse(mouse))) => return Ok(mouse),
                Ok(UnifiedEvent::Input(event)) => {
                    self.pending_event = Some(event);
                }
                Ok(UnifiedEvent::PtyWakeup(_)) => {}
                Ok(UnifiedEvent::AppExited(key)) => {
                    self.exited_windows.push(key);
                }
                Ok(UnifiedEvent::Signal) => self.signal_received = true,
                Ok(UnifiedEvent::Tick) => {}
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "event channel disconnected",
                    ));
                }
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
    /// work (e.g. a countdown timer).  When true the profile stays at
    /// Streaming even if both `dirty_windows` and `last_event_at` are stale.
    fn set_pending_work(&mut self, pending: bool) {
        self.pending_work = pending;
    }

    fn set_max_sleep_duration(&mut self, duration: Option<Duration>) {
        self.max_sleep_duration = duration;
    }

    fn poll_interval(&self) -> Duration {
        let base = self.current_profile().poll_interval();
        match self.max_sleep_duration {
            Some(max_sleep) => base.min(max_sleep),
            None => base,
        }
    }

    fn current_profile(&self) -> PowerProfile {
        crate::power_profile::profile_from_activity(
            self.last_event_at,
            !self.dirty_windows.is_empty() || self.pending_work,
        )
    }

    fn take_exited_windows(&mut self) -> Vec<WindowKey> {
        std::mem::take(&mut self.exited_windows)
    }

    fn take_dirty_windows(&mut self) -> HashSet<WindowKey> {
        std::mem::take(&mut self.dirty_windows)
    }
}

impl Drop for UnifiedEventSource {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{KeyCode, KeyKind, KeyModifiers};

    /// Input events drained by `drain_pending` must be preserved in
    /// `input_buffer` so `poll()/read()` can process every event.
    #[test]
    fn drain_pending_preserves_all_input_events() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let mut source = UnifiedEventSource {
            rx,
            tx: tx.clone(),
            _input_handle: std::thread::spawn(|| {}),
            shutdown: Arc::new(AtomicBool::new(false)),
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
        };
        // Prevent the no-op handle from panicking on join in drop
        let dummy_handle = std::thread::spawn(|| {});
        source._input_handle = dummy_handle;

        // Send 10 input events into the channel
        for i in 0..10u8 {
            let evt = Event::Key(KeyEvent {
                code: KeyCode::Char(char::from(b'a' + i)),
                modifiers: KeyModifiers::NONE,
                kind: KeyKind::Press,
            });
            tx.send(UnifiedEvent::Input(evt)).unwrap();
        }
        // Also mix in some PtyWakeups (the reason drain_pending exists)
        for _ in 0..3 {
            tx.send(UnifiedEvent::PtyWakeup(WindowKey::default()))
                .unwrap();
        }

        // drain_pending must move all Input events into input_buffer
        source.drain_pending();

        assert_eq!(
            source.input_buffer.len(),
            10,
            "all 10 input events must be buffered, not dropped"
        );

        // verify ordering is preserved
        for (i, evt) in source.input_buffer.iter().enumerate() {
            let expected = char::from(b'a' + i as u8);
            match evt {
                Event::Key(k) => {
                    assert_eq!(
                        k.code,
                        KeyCode::Char(expected),
                        "event {} should be '{}'",
                        i,
                        expected
                    );
                }
                _ => panic!("expected Key event at position {}", i),
            }
        }

        // poll should report events available from buffer
        assert!(
            source.poll(Duration::ZERO).unwrap(),
            "poll must return true when buffer is non-empty"
        );

        // read should drain buffer in order
        for i in 0..10u8 {
            let evt = source.read().unwrap();
            let expected = char::from(b'a' + i);
            match evt {
                Event::Key(k) => assert_eq!(k.code, KeyCode::Char(expected)),
                _ => panic!("expected Key event"),
            }
        }

        // buffer should now be empty
        assert!(source.input_buffer.is_empty());
        assert!(
            !source.poll(Duration::ZERO).unwrap(),
            "poll must return false after buffer drained"
        );
    }

    /// Dirty windows must be cleared after `poll()` returns `Ok(false)`,
    /// otherwise the power profile stays at `Streaming` (16ms) forever.
    #[test]
    fn dirty_windows_cleared_after_poll_ok_false() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let mut source = UnifiedEventSource {
            rx,
            tx: tx.clone(),
            _input_handle: std::thread::spawn(|| {}),
            shutdown: Arc::new(AtomicBool::new(false)),
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
        };
        // Prevent the no-op handle from panicking on drop
        let dummy_handle = std::thread::spawn(|| {});
        source._input_handle = dummy_handle;

        // Baseline: no input, no dirty → PowerSaver
        assert_eq!(source.current_profile(), PowerProfile::PowerSaver);

        // Send a PtyWakeup — drain_pending will pick it up inside poll()
        tx.send(UnifiedEvent::PtyWakeup(WindowKey::default()))
            .unwrap();

        // poll() should drain the PtyWakeup, arm the 16ms frame pacer, then
        // let it expire and return Ok(false) with dirty_windows still set.
        assert!(
            !source.poll(Duration::from_secs(1)).unwrap(),
            "poll must return Ok(false) after PtyWakeup expiry"
        );

        // After poll returns, dirty_windows must contain the key
        // (coalesce arms the timer but does NOT clear dirty_windows on expiry).
        assert!(
            !source.take_dirty_windows().is_empty(),
            "dirty_windows must still contain the key after poll"
        );

        // After taking the dirty windows, profile returns to PowerSaver
        // (no input activity, no dirty windows).
        assert_eq!(
            source.current_profile(),
            PowerProfile::PowerSaver,
            "profile must return to PowerSaver after dirty_windows consumed"
        );
    }

    /// Verify that a non-empty dirty_windows causes Streaming profile,
    /// confirming the mechanism the bug fix relies on.
    #[test]
    fn dirty_windows_causes_streaming_profile() {
        let (_tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let mut set = HashSet::new();
        set.insert(WindowKey::default());
        let source = UnifiedEventSource {
            rx,
            tx: _tx,
            _input_handle: std::thread::spawn(|| {}),
            shutdown: Arc::new(AtomicBool::new(false)),
            dirty_windows: set,
            exited_windows: Vec::new(),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
        };
        assert_eq!(
            source.current_profile(),
            PowerProfile::Streaming,
            "dirty_windows must elevate profile to Streaming"
        );
    }

    #[test]
    fn pending_work_causes_streaming_profile() {
        let (tx1, rx1) = bounded(EVENT_CHANNEL_CAPACITY);
        let source = UnifiedEventSource {
            rx: rx1,
            tx: tx1,
            _input_handle: std::thread::spawn(|| {}),
            shutdown: Arc::new(AtomicBool::new(false)),
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: true,
            max_sleep_duration: None,
        };
        assert_eq!(
            source.current_profile(),
            PowerProfile::Streaming,
            "pending_work must elevate profile to Streaming even without dirty_windows"
        );
        // Also verify that stale last_event_at + pending_work still gives Streaming
        let (tx2, rx2) = bounded(EVENT_CHANNEL_CAPACITY);
        let stale = Instant::now().checked_sub(Duration::from_secs(3600));
        let source2 = UnifiedEventSource {
            rx: rx2,
            tx: tx2,
            _input_handle: std::thread::spawn(|| {}),
            shutdown: Arc::new(AtomicBool::new(false)),
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: stale,
            frame_pacer: FramePacer::new(),
            pending_work: true,
            max_sleep_duration: None,
        };
        assert_eq!(
            source2.current_profile(),
            PowerProfile::Streaming,
            "pending_work must keep Streaming even with stale last_event_at"
        );
    }

    /// Regression: `take_exited_windows` must be reachable through the
    /// `EventSource` trait so that generic runner code (`D: EventSource`)
    /// actually gets the accumulated exit keys. Before the fix the method
    /// was only inherent — the trait override was missing, and the default
    /// no-op impl silently returned an empty vec, so exited windows never
    /// closed.
    #[test]
    fn take_exited_windows_returns_accumulated_keys_through_trait() {
        use super::EventSource;
        let (_tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let key = WindowKey::default();
        let mut source = UnifiedEventSource {
            rx,
            tx: _tx,
            _input_handle: std::thread::spawn(|| {}),
            shutdown: Arc::new(AtomicBool::new(false)),
            dirty_windows: HashSet::new(),
            exited_windows: vec![key],
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
        };
        let dummy_handle = std::thread::spawn(|| {});
        source._input_handle = dummy_handle;

        // Call through the trait, not an inherent method. Would return
        // Vec::new() if the trait override were missing.
        let exited = EventSource::take_exited_windows(&mut source);
        assert_eq!(exited, vec![key], "must return the pre-populated key");

        let again = EventSource::take_exited_windows(&mut source);
        assert!(again.is_empty(), "second call must drain");
    }

    /// Regression: `take_dirty_windows` must be reachable through the
    /// `EventSource` trait so that generic runner code (`D: EventSource`)
    /// actually consumes accumulated dirty keys.  Without the trait
    /// override the default no-op impl would silently return an empty
    /// set, leaving dirty_windows accumulated forever.
    #[test]
    fn take_dirty_windows_returns_accumulated_keys_through_trait() {
        use super::EventSource;
        let (_tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let key = WindowKey::default();
        let mut set = HashSet::new();
        set.insert(key);
        let mut source = UnifiedEventSource {
            rx,
            tx: _tx,
            _input_handle: std::thread::spawn(|| {}),
            shutdown: Arc::new(AtomicBool::new(false)),
            dirty_windows: set,
            exited_windows: Vec::new(),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
        };
        let dummy_handle = std::thread::spawn(|| {});
        source._input_handle = dummy_handle;

        // Call through the trait, not an inherent method. Would return
        // an empty set if the trait override were missing.
        let taken = EventSource::take_dirty_windows(&mut source);
        assert_eq!(taken.len(), 1, "must return the pre-populated key");
        assert!(taken.contains(&key), "must contain the dirty key");

        let again = EventSource::take_dirty_windows(&mut source);
        assert!(again.is_empty(), "second call must drain");
    }
}
