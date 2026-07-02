use std::collections::{HashSet, VecDeque};
use std::io;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};
use crossterm::event::{Event, KeyEvent, MouseEvent};

use super::EventSource;
use super::utils::KeyboardNormalizer;
use crate::power_profile::PowerProfile;
use crate::window::WindowKey;

/// Delay window for coalescing PTY wakeups before triggering a render.
/// Multiple wakeups within this window are collapsed into a single render.
/// Capacity of the crossbeam channel between event producers and the event
/// loop. 256 slots provides natural backpressure: when the channel is full,
/// the sender blocks → the producer backs off.
const EVENT_CHANNEL_CAPACITY: usize = 256;

/// How often the crossterm input thread polls for new events (100 ms).
/// Keeps the thread responsive to shutdown signals while being idle-friendly.
const INPUT_THREAD_POLL_INTERVAL: Duration = Duration::from_millis(100);

const COALESCE_DELAY: Duration = Duration::from_millis(3);

/// Events that can flow through the unified event channel.
#[derive(Debug, Clone)]
pub enum UnifiedEvent {
    /// A user-input event from crossterm (key, mouse, resize).
    Input(Event),
    /// A PTY reader thread has new data available for `WindowKey`.
    PtyWakeup(WindowKey),
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
    /// Deadline for coalescing PTY wakeups. When `Some`, wakeups are batched
    /// until this instant before triggering a render.
    coalesce_deadline: Option<Instant>,
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
                            Ok(event) => {
                                if input_tx.send(UnifiedEvent::Input(event)).is_err() {
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
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            coalesce_deadline: None,
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
                Ok(UnifiedEvent::Signal) => {
                    self.signal_received = true;
                }
                Ok(UnifiedEvent::Tick) => {
                    // No-op — tick is implicit in the event-loop cycle.
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

    /// Take accumulated dirty windows and reset.
    pub fn take_dirty_windows(&mut self) -> HashSet<WindowKey> {
        std::mem::take(&mut self.dirty_windows)
    }
}

impl EventSource for UnifiedEventSource {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        // First drain any pending events non-blocking.
        self.drain_pending();

        if self.pending_event.is_some() || !self.input_buffer.is_empty() {
            return Ok(true);
        }

        // Block on the channel for up to `timeout`.
        let mut remaining = timeout;

        while remaining > Duration::ZERO {
            match self.rx.recv_timeout(remaining) {
                Ok(UnifiedEvent::Input(event)) => {
                    self.last_event_at = Some(Instant::now());
                    self.coalesce_deadline = None;
                    self.pending_event = Some(event);
                    return Ok(true);
                }
                Ok(UnifiedEvent::PtyWakeup(key)) => {
                    self.dirty_windows.insert(key);
                    if self.coalesce_deadline.is_none() {
                        self.coalesce_deadline = Some(Instant::now() + COALESCE_DELAY);
                    }
                    if let Some(deadline) = self.coalesce_deadline {
                        let now = Instant::now();
                        if now >= deadline {
                            self.coalesce_deadline = None;
                            self.dirty_windows.clear();
                            return Ok(false);
                        }
                        let coalesce_remaining = deadline.saturating_duration_since(now);
                        remaining = remaining.min(coalesce_remaining);
                    }
                    continue;
                }
                Ok(UnifiedEvent::Signal) => {
                    self.signal_received = true;
                    return Ok(false);
                }
                Ok(UnifiedEvent::Tick) => {
                    return Ok(false);
                }
                Err(_) => break, // timeout or disconnected
            }
        }

        self.coalesce_deadline = None;
        self.dirty_windows.clear();
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

    fn poll_interval(&self) -> Duration {
        self.current_profile().poll_interval()
    }

    fn current_profile(&self) -> PowerProfile {
        crate::power_profile::profile_from_activity(
            self.last_event_at,
            !self.dirty_windows.is_empty(),
        )
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
    use crossterm::event::{KeyCode, KeyModifiers};

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
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            coalesce_deadline: None,
        };
        // Prevent the no-op handle from panicking on join in drop
        let dummy_handle = std::thread::spawn(|| {});
        source._input_handle = dummy_handle;

        // Send 10 input events into the channel
        for i in 0..10u8 {
            let evt = Event::Key(KeyEvent::new(
                KeyCode::Char(char::from(b'a' + i)),
                KeyModifiers::NONE,
            ));
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
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            coalesce_deadline: None,
        };
        // Prevent the no-op handle from panicking on drop
        let dummy_handle = std::thread::spawn(|| {});
        source._input_handle = dummy_handle;

        // Baseline: no input, no dirty → PowerSaver
        assert_eq!(source.current_profile(), PowerProfile::PowerSaver);

        // Send a PtyWakeup — drain_pending will pick it up inside poll()
        tx.send(UnifiedEvent::PtyWakeup(WindowKey::default()))
            .unwrap();

        // poll() should drain the PtyWakeup, arm coalesce, then either
        // coalesce-expire or timeout, clear dirty_windows, and return Ok(false).
        assert!(
            !source.poll(Duration::from_secs(1)).unwrap(),
            "poll must return Ok(false) after PtyWakeup drain"
        );

        // After poll returns, dirty_windows must be empty.
        assert!(
            source.take_dirty_windows().is_empty(),
            "dirty_windows must be cleared after poll returns Ok(false)"
        );

        // With dirty_windows empty and no input, profile returns to PowerSaver.
        assert_eq!(
            source.current_profile(),
            PowerProfile::PowerSaver,
            "profile must return to PowerSaver after dirty_windows cleared"
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
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            coalesce_deadline: None,
        };
        assert_eq!(
            source.current_profile(),
            PowerProfile::Streaming,
            "dirty_windows must elevate profile to Streaming"
        );
    }
}
