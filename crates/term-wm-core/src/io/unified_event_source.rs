use std::collections::HashSet;
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
    /// Signal received flag.
    signal_received: bool,
    /// Keyboard normalizer for consistent event handling.
    normalizer: KeyboardNormalizer,
    /// Timestamp of the last input event (for power profiling).
    last_event_at: Option<Instant>,
}

impl UnifiedEventSource {
    /// Create a new unified event source, spawning a background thread
    /// that reads crossterm events.
    pub fn new() -> io::Result<Self> {
        let (tx, rx) = bounded::<UnifiedEvent>(256);
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
                    if crossterm::event::poll(Duration::from_millis(100)).unwrap_or(false) {
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
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        Ok(Self {
            rx,
            tx,
            _input_handle,
            shutdown,
            dirty_windows: HashSet::new(),
            pending_event: None,
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
        })
    }

    /// Return a sender that PTY reader threads can use to send wakeup pings.
    pub fn pty_wakeup_tx(&self) -> Sender<UnifiedEvent> {
        self.tx.clone()
    }

    /// Drain all pending events from the channel (non-blocking) into internal
    /// state.  Called at the start of each event-loop iteration so PtyWakeup
    /// floods don't cause render-backlog.
    fn drain_pending(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(UnifiedEvent::Input(event)) => {
                    self.last_event_at = Some(Instant::now());
                    if self.pending_event.is_none() {
                        self.pending_event = Some(event);
                    }
                    // Only cache the first input; rest will be picked up
                    // by subsequent poll() calls.
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

        if self.pending_event.is_some() {
            return Ok(true);
        }

        // Block on the channel for up to `timeout`.
        let mut remaining = timeout;

        while remaining > Duration::ZERO {
            match self.rx.recv_timeout(remaining) {
                Ok(UnifiedEvent::Input(event)) => {
                    self.last_event_at = Some(Instant::now());
                    self.pending_event = Some(event);
                    return Ok(true);
                }
                Ok(UnifiedEvent::PtyWakeup(key)) => {
                    self.dirty_windows.insert(key);
                    // Don't return true — wakeups just trigger a render
                    // on the next idle tick.  Drain any more immediate events.
                    remaining = remaining.saturating_sub(Duration::from_millis(1));
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

        Ok(false)
    }

    fn read(&mut self) -> io::Result<Event> {
        if let Some(event) = self.pending_event.take() {
            if let Some(normalized) = self.normalizer.normalize(event) {
                return Ok(normalized);
            }
        }
        // Fallback: block on the channel for an input event.
        loop {
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
        crate::power_profile::profile_from_activity(self.last_event_at)
    }
}

impl Drop for UnifiedEventSource {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
    }
}
