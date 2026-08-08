use std::collections::{HashSet, VecDeque};
use std::io;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};

use term_wm_console::background_console_reader::BackgroundConsoleReader;
use term_wm_core::events::{Event, KeyEvent, MouseEvent};
use term_wm_core::io::EventSource;
use term_wm_core::io::frame_pacer::FramePacer;
use term_wm_core::power_profile::PowerProfile;
use term_wm_core::utils::KeyboardNormalizer;
use term_wm_core::window::WindowKey;

/// Capacity of the crossbeam channel between event producers and the event
/// loop. Generous capacity since wakeup gating (dirty.swap) prevents flooding.
pub(crate) const EVENT_CHANNEL_CAPACITY: usize = 256;

/// Events that can flow through the unified event channel.
#[derive(Debug, Clone)]
pub enum UnifiedEvent {
    /// A user-input event from crossterm (key, mouse, resize).
    Input(Event),
    /// A PTY reader thread has new data available for `WindowKey`.
    PtyWakeup(WindowKey),
    /// A PTY child process has exited. Sent from the reader thread on EOF.
    AppExited(WindowKey),
    /// Application direct-input routing state changed (alt screen, mouse tracking, margins).
    DirectInputChanged(WindowKey, bool),
    /// An OS signal was received (SIGINT, SIGTERM).
    Signal,
    /// Periodic tick for timing.
    Tick,
}

/// A unified event source that multiplexes console input, PTY wakeups,
/// and OS signals into a single channel. The main thread blocks on one
/// receiver instead of polling multiple sources.
///
/// Crossterm input reading is delegated to a
/// [`BackgroundConsoleReader`] in `term-wm-console`; this struct owns no
/// crossterm knowledge. It is the sole consumer of that reader's channel and
/// owns normalization + power-profile bookkeeping.
pub struct UnifiedEventSource {
    rx: Receiver<UnifiedEvent>,
    /// One Sender clone kept here so it can be handed to PTY reader threads.
    tx: Sender<UnifiedEvent>,
    /// Background crossterm input thread (owns all crossterm reading).
    console: BackgroundConsoleReader,
    /// Clone of the console reader's receive side, selected on during
    /// poll/read/next_key/next_mouse.
    console_rx: Receiver<Event>,
    /// False once the console channel disconnects; poll/read then fall back to
    /// the unified channel only (avoids a select! busy-loop on an immediately
    /// Err-ing disconnected receiver).
    console_alive: bool,
    /// Accumulated PTY wakeups since the last idle tick — batch-drained
    /// so thousands of wakeups/sec collapse into a single render.
    dirty_windows: HashSet<WindowKey>,
    /// Accumulated window exit notifications since the last drain.
    exited_windows: Vec<WindowKey>,
    /// Accumulated direct-input routing transitions since the last drain.
    direct_input_changed: Vec<(WindowKey, bool)>,
    /// Cached input event (poll returned true, waiting for read).
    pending_event: Option<Event>,
    /// Buffer for input events drained during `drain_pending`/`drain_console`
    /// so none are lost.
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
    /// Global dirty bit — set by `request_redraw()`, consumed by
    /// `take_redraw_request()` in the runner's `None` branch to arm
    /// the FramePacer for non-input-driven state changes.
    pending_redraw: bool,
}

/// Outcome of processing one unified-channel event during `poll`.
enum UnifiedPoll {
    /// A normalized input event is ready.
    Ready,
    /// State changed and a render is due (dirty window, signal, tick, …).
    RenderDue,
    /// Keep waiting for a user-input event.
    Continue,
}

impl UnifiedEventSource {
    /// Create a new unified event source, delegating crossterm input reading
    /// to a background thread owned by `term-wm-console`.
    ///
    /// The bounded channel (256 slots) provides mechanical backpressure:
    /// when the channel is full, PTY reader threads block on `send()` →
    /// OS pipe buffer fills → child process `write()` blocks → prevents
    /// memory exhaustion under extreme output load.
    pub fn new() -> io::Result<Self> {
        let (tx, rx) = bounded::<UnifiedEvent>(EVENT_CHANNEL_CAPACITY);
        let console = BackgroundConsoleReader::new()?;
        let console_rx = console.receiver();
        Ok(Self {
            rx,
            tx,
            console,
            console_rx,
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
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
                    if let Some(normalized) = self.normalizer.normalize(event) {
                        self.input_buffer.push_back(normalized);
                    }
                }
                Ok(UnifiedEvent::PtyWakeup(key)) => {
                    self.dirty_windows.insert(key);
                }
                Ok(UnifiedEvent::AppExited(key)) => {
                    self.exited_windows.push(key);
                }
                Ok(UnifiedEvent::DirectInputChanged(key, enabled)) => {
                    tracing::info!(
                        "[STAGE 3] drain_pending collected DirectInputChanged({:?}, {})",
                        key,
                        enabled
                    );
                    self.dirty_windows.insert(key);
                    self.direct_input_changed.push((key, enabled));
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

    /// Drain all pending console input events (non-blocking) into
    /// `input_buffer`, normalizing with the consumer's own normalizer.
    fn drain_console(&mut self) {
        loop {
            match self.console_rx.try_recv() {
                Ok(event) => {
                    if let Some(normalized) = self.normalizer.normalize(event) {
                        self.input_buffer.push_back(normalized);
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.console_alive = false;
                    break;
                }
            }
        }
    }

    /// Handle one unified-channel event during `poll`, mirroring the previous
    /// `recv_timeout` match arms.
    fn handle_unified_poll(&mut self, evt: UnifiedEvent) -> UnifiedPoll {
        match evt {
            UnifiedEvent::Input(event) => {
                self.last_event_at = Some(Instant::now());
                if let Some(normalized) = self.normalizer.normalize(event) {
                    self.frame_pacer.reset();
                    self.pending_event = Some(normalized);
                    UnifiedPoll::Ready
                } else {
                    // Event filtered out — keep waiting.
                    UnifiedPoll::Continue
                }
            }
            UnifiedEvent::PtyWakeup(key) => {
                self.dirty_windows.insert(key);
                self.frame_pacer.notify_pending(Instant::now());
                if self.frame_pacer.try_expire(Instant::now()) {
                    UnifiedPoll::RenderDue
                } else {
                    UnifiedPoll::Continue
                }
            }
            UnifiedEvent::AppExited(key) => {
                self.exited_windows.push(key);
                self.frame_pacer.notify_pending(Instant::now());
                UnifiedPoll::Continue
            }
            UnifiedEvent::DirectInputChanged(key, enabled) => {
                tracing::info!(
                    "[STAGE 3] recv collected DirectInputChanged({:?}, {})",
                    key,
                    enabled
                );
                self.dirty_windows.insert(key);
                self.direct_input_changed.push((key, enabled));
                self.frame_pacer.notify_pending(Instant::now());
                UnifiedPoll::RenderDue
            }
            UnifiedEvent::Signal => {
                self.signal_received = true;
                UnifiedPoll::RenderDue
            }
            UnifiedEvent::Tick => UnifiedPoll::RenderDue,
        }
    }

    /// Handle one unified-channel event during `read`. Returns `Some(event)`
    /// when a normalized user-input event should be returned.
    fn handle_unified_read(&mut self, evt: UnifiedEvent) -> Option<Event> {
        match evt {
            UnifiedEvent::Input(event) => {
                self.last_event_at = Some(Instant::now());
                self.normalizer.normalize(event)
            }
            UnifiedEvent::PtyWakeup(key) => {
                self.dirty_windows.insert(key);
                None
            }
            UnifiedEvent::AppExited(key) => {
                self.exited_windows.push(key);
                None
            }
            UnifiedEvent::DirectInputChanged(key, enabled) => {
                tracing::info!(
                    "[STAGE 3] read collected DirectInputChanged({:?}, {})",
                    key,
                    enabled
                );
                self.dirty_windows.insert(key);
                self.direct_input_changed.push((key, enabled));
                None
            }
            UnifiedEvent::Signal => {
                self.signal_received = true;
                None
            }
            UnifiedEvent::Tick => None,
        }
    }

    /// Remove the first buffered event matching `predicate` (used by the
    /// `next_key`/`next_mouse` maintenance paths).
    fn pop_matching(&mut self, predicate: impl Fn(&Event) -> bool) -> Option<Event> {
        if let Some(idx) = self.input_buffer.iter().position(predicate) {
            return self.input_buffer.remove(idx);
        }
        None
    }

    /// Check if a signal was received and ack it.
    pub fn take_signal(&mut self) -> bool {
        let sig = self.signal_received;
        self.signal_received = false;
        sig
    }
}

impl EventSource for UnifiedEventSource {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        // First drain any pending events non-blocking.
        self.drain_pending();
        self.drain_console();

        if self.pending_event.is_some() || !self.input_buffer.is_empty() {
            return Ok(true);
        }

        // If drain found dirty windows, arm the frame deadline.
        // This prevents a 3600s freeze when a PtyWakeup arrives between
        // handler(None) and drain_pending (common under heavy streaming):
        // the PtyWakeup is consumed by drain but no frame deadline is set, so
        // the blocking wait would otherwise sleep for the full PowerSaver
        // interval.
        if !self.dirty_windows.is_empty() {
            self.frame_pacer.notify_pending(Instant::now());
        }

        // Clamp remaining to the frame deadline so we never block
        // longer than 16ms when there are unprocessed dirty windows.
        if self.frame_pacer.try_expire(Instant::now()) {
            return Ok(false);
        }
        let mut remaining = timeout;
        if let Some(t) = self.frame_pacer.time_until_deadline(Instant::now()) {
            remaining = remaining.min(t);
        }

        // Clone receivers to locals so `select!` does not borrow `self`,
        // letting the arm bodies mutate `self` freely.
        let local_rx = self.rx.clone();
        let local_console_rx = self.console_rx.clone();

        while remaining > Duration::ZERO {
            // Check frame deadline before each blocking call.
            if self.frame_pacer.try_expire(Instant::now()) {
                return Ok(false);
            }
            if let Some(t) = self.frame_pacer.time_until_deadline(Instant::now()) {
                remaining = remaining.min(t);
                if remaining <= Duration::ZERO {
                    self.frame_pacer.reset();
                    return Ok(false);
                }
            }

            if self.console_alive {
                // `after(remaining)` is a deadline flavor — no timer thread.
                crossbeam_channel::select! {
                    recv(local_console_rx) -> msg => match msg {
                        Ok(event) => {
                            self.last_event_at = Some(Instant::now());
                            if let Some(normalized) = self.normalizer.normalize(event) {
                                self.frame_pacer.reset();
                                self.pending_event = Some(normalized);
                                return Ok(true);
                            }
                            // Normalizer-filtered (e.g. Release) — keep waiting.
                        }
                        Err(_) => {
                            self.console_alive = false;
                        }
                    },
                    recv(local_rx) -> msg => match msg {
                        Ok(evt) => match self.handle_unified_poll(evt) {
                            UnifiedPoll::Ready => return Ok(true),
                            UnifiedPoll::RenderDue => return Ok(false),
                            UnifiedPoll::Continue => {}
                        },
                        Err(_) => break,
                    },
                    recv(crossbeam_channel::after(remaining)) -> _ => {
                        self.frame_pacer.reset();
                        return Ok(false);
                    }
                }
            } else {
                // Console unavailable — original single-channel behavior.
                match local_rx.recv_timeout(remaining) {
                    Ok(evt) => match self.handle_unified_poll(evt) {
                        UnifiedPoll::Ready => return Ok(true),
                        UnifiedPoll::RenderDue => return Ok(false),
                        UnifiedPoll::Continue => {}
                    },
                    Err(_) => {
                        // Check if the frame deadline expired during the wait.
                        if self.frame_pacer.try_expire(Instant::now()) {
                            return Ok(false);
                        }
                        break;
                    }
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
        // Fallback: check buffer, then block on the channels.
        loop {
            if let Some(event) = self.input_buffer.pop_front() {
                self.last_event_at = Some(Instant::now());
                if let Some(normalized) = self.normalizer.normalize(event) {
                    return Ok(normalized);
                }
                continue;
            }

            let local_rx = self.rx.clone();
            let local_console_rx = self.console_rx.clone();
            if self.console_alive {
                crossbeam_channel::select! {
                    recv(local_console_rx) -> msg => match msg {
                        Ok(event) => {
                            self.last_event_at = Some(Instant::now());
                            if let Some(normalized) = self.normalizer.normalize(event) {
                                return Ok(normalized);
                            }
                        }
                        Err(_) => {
                            self.console_alive = false;
                        }
                    },
                    recv(local_rx) -> msg => match msg {
                        Ok(evt) => {
                            if let Some(event) = self.handle_unified_read(evt) {
                                return Ok(event);
                            }
                        }
                        Err(_) => {
                            return Err(io::Error::new(
                                io::ErrorKind::BrokenPipe,
                                "event channel disconnected",
                            ));
                        }
                    }
                }
            } else {
                match local_rx.recv() {
                    Ok(evt) => {
                        if let Some(event) = self.handle_unified_read(evt) {
                            return Ok(event);
                        }
                    }
                    Err(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "event channel disconnected",
                        ));
                    }
                }
            }
        }
    }

    fn next_key(&mut self) -> io::Result<KeyEvent> {
        loop {
            self.drain_pending();
            self.drain_console();
            if let Some(Event::Key(key)) = self.pop_matching(|e| matches!(e, Event::Key(_))) {
                return Ok(key);
            }
            if let Some(event) = self.pending_event.take() {
                if let Event::Key(key) = event {
                    return Ok(key);
                }
                self.input_buffer.push_back(event);
            }

            let local_rx = self.rx.clone();
            let local_console_rx = self.console_rx.clone();
            if self.console_alive {
                crossbeam_channel::select! {
                    recv(local_console_rx) -> msg => match msg {
                        Ok(event) => {
                            if let Some(normalized) = self.normalizer.normalize(event) {
                                if let Event::Key(key) = normalized {
                                    return Ok(key);
                                }
                                self.input_buffer.push_back(normalized);
                            }
                        }
                        Err(_) => {
                            self.console_alive = false;
                        }
                    },
                    recv(local_rx) -> msg => match msg {
                        Ok(UnifiedEvent::Input(event)) => {
                            if let Event::Key(key) = event {
                                return Ok(key);
                            }
                            self.input_buffer.push_back(event);
                        }
                        Ok(UnifiedEvent::PtyWakeup(_)) => {}
                        Ok(UnifiedEvent::AppExited(key)) => {
                            self.exited_windows.push(key);
                        }
                        Ok(UnifiedEvent::DirectInputChanged(key, enabled)) => {
                            self.dirty_windows.insert(key);
                            self.direct_input_changed.push((key, enabled));
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
            } else {
                match local_rx.recv() {
                    Ok(UnifiedEvent::Input(event)) => {
                        if let Event::Key(key) = event {
                            return Ok(key);
                        }
                        self.input_buffer.push_back(event);
                    }
                    Ok(UnifiedEvent::PtyWakeup(_)) => {}
                    Ok(UnifiedEvent::AppExited(key)) => {
                        self.exited_windows.push(key);
                    }
                    Ok(UnifiedEvent::DirectInputChanged(key, enabled)) => {
                        self.dirty_windows.insert(key);
                        self.direct_input_changed.push((key, enabled));
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
    }

    fn next_mouse(&mut self) -> io::Result<MouseEvent> {
        loop {
            self.drain_pending();
            self.drain_console();
            if let Some(Event::Mouse(mouse)) = self.pop_matching(|e| matches!(e, Event::Mouse(_))) {
                return Ok(mouse);
            }
            if let Some(event) = self.pending_event.take() {
                if let Event::Mouse(mouse) = event {
                    return Ok(mouse);
                }
                self.input_buffer.push_back(event);
            }

            let local_rx = self.rx.clone();
            let local_console_rx = self.console_rx.clone();
            if self.console_alive {
                crossbeam_channel::select! {
                    recv(local_console_rx) -> msg => match msg {
                        Ok(event) => {
                            if let Some(normalized) = self.normalizer.normalize(event) {
                                if let Event::Mouse(mouse) = normalized {
                                    return Ok(mouse);
                                }
                                self.input_buffer.push_back(normalized);
                            }
                        }
                        Err(_) => {
                            self.console_alive = false;
                        }
                    },
                    recv(local_rx) -> msg => match msg {
                        Ok(UnifiedEvent::Input(event)) => {
                            if let Event::Mouse(mouse) = event {
                                return Ok(mouse);
                            }
                            self.input_buffer.push_back(event);
                        }
                        Ok(UnifiedEvent::PtyWakeup(_)) => {}
                        Ok(UnifiedEvent::AppExited(key)) => {
                            self.exited_windows.push(key);
                        }
                        Ok(UnifiedEvent::DirectInputChanged(key, enabled)) => {
                            self.dirty_windows.insert(key);
                            self.direct_input_changed.push((key, enabled));
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
            } else {
                match local_rx.recv() {
                    Ok(UnifiedEvent::Input(event)) => {
                        if let Event::Mouse(mouse) = event {
                            return Ok(mouse);
                        }
                        self.input_buffer.push_back(event);
                    }
                    Ok(UnifiedEvent::PtyWakeup(_)) => {}
                    Ok(UnifiedEvent::AppExited(key)) => {
                        self.exited_windows.push(key);
                    }
                    Ok(UnifiedEvent::DirectInputChanged(key, enabled)) => {
                        self.dirty_windows.insert(key);
                        self.direct_input_changed.push((key, enabled));
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
    }

    // Delegates to the console crate's background reader, which owns the
    // single canonical adapter call (term_wm_crossterm_adapter::set_mouse_capture).
    fn set_mouse_capture(&mut self, enabled: bool) -> io::Result<()> {
        self.console.set_mouse_capture(enabled)
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
            None => {
                #[cfg(target_os = "windows")]
                {
                    // Windows ConPTY requires regular event loop ticks to prevent
                    // background PTY reader threads from stalling. When no explicit
                    // cap is set, clamp to WINDOWS_MAX_POLL_INTERVAL maximum.
                    base.min(term_wm_core::power_profile::WINDOWS_MAX_POLL_INTERVAL)
                }
                #[cfg(not(target_os = "windows"))]
                {
                    base
                }
            }
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

    fn take_direct_input_changed(&mut self) -> Vec<(WindowKey, bool)> {
        std::mem::take(&mut self.direct_input_changed)
    }

    fn take_dirty_windows(&mut self) -> HashSet<WindowKey> {
        std::mem::take(&mut self.dirty_windows)
    }

    fn request_redraw(&mut self) {
        self.pending_redraw = true;
    }

    fn take_redraw_request(&mut self) -> bool {
        std::mem::replace(&mut self.pending_redraw, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{KeyCode, KeyKind, KeyModifiers};

    /// Build a background console reader over an injected channel (no-op
    /// thread handle) so the unified source's console wiring can be tested
    /// without a real terminal. Keep `console_tx` alive so the channel stays
    /// connected and the console-alive select path is exercised.
    fn test_console() -> (BackgroundConsoleReader, Receiver<Event>, Sender<Event>) {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let handle = std::thread::spawn(|| {});
        let src = BackgroundConsoleReader::from_receiver(
            rx,
            handle,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );
        let rcv = src.receiver();
        (src, rcv, tx)
    }

    fn key_evt(code: KeyCode) -> Event {
        Event::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        })
    }

    /// Input events drained by `drain_pending` must be preserved in
    /// `input_buffer` so `poll()/read()` can process every event.
    #[test]
    fn drain_pending_preserves_all_input_events() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, _console_tx) = test_console();
        let mut source = UnifiedEventSource {
            rx,
            tx: tx.clone(),
            console,
            console_rx,
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
        };

        // Send 10 input events into the channel
        for i in 0..10u8 {
            tx.send(UnifiedEvent::Input(key_evt(KeyCode::Char(char::from(
                b'a' + i,
            )))))
            .unwrap();
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

    /// `drain_pending` must filter out Release events through the
    /// normalizer, keeping Press and Repeat events in the buffer.
    #[test]
    fn drain_pending_filters_release_events() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, _console_tx) = test_console();
        let mut source = UnifiedEventSource {
            rx,
            tx: tx.clone(),
            console,
            console_rx,
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
        };

        // Send: Press, Release, Repeat, Press
        // On non-Windows: Release filtered, 3 survive (Press, Repeat, Press)
        // On Windows: Release + Repeat filtered, 2 survive (Press, Press)
        for (i, kind) in [
            KeyKind::Press,
            KeyKind::Release,
            KeyKind::Repeat,
            KeyKind::Press,
        ]
        .into_iter()
        .enumerate()
        {
            let evt = Event::Key(KeyEvent {
                code: KeyCode::Char(char::from(b'a' + i as u8)),
                modifiers: KeyModifiers::NONE,
                kind,
            });
            tx.send(UnifiedEvent::Input(evt)).unwrap();
        }

        source.drain_pending();

        assert_eq!(
            source.input_buffer.len(),
            3,
            "Only Release events should be filtered by normalization"
        );

        // Verify Release event (index 1) is absent
        for evt in &source.input_buffer {
            if let Event::Key(k) = evt {
                assert_ne!(
                    k.kind,
                    KeyKind::Release,
                    "Release events must never survive normalization"
                );
            }
        }

        // Verify first and last events are the Press events
        let first = source.input_buffer.front().unwrap();
        let last = source.input_buffer.back().unwrap();
        if let (Event::Key(k1), Event::Key(k2)) = (first, last) {
            assert_eq!(k1.kind, KeyKind::Press);
            assert_eq!(k1.code, KeyCode::Char('a'));
            assert_eq!(k2.kind, KeyKind::Press);
            assert_eq!(k2.code, KeyCode::Char('d'));
        } else {
            panic!("expected Key events");
        }
    }

    /// `poll` must skip Release events that arrive via recv_timeout,
    /// continuing to wait until a valid event arrives or the deadline expires.
    #[test]
    fn poll_filters_release_events() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, _console_tx) = test_console();
        let mut source = UnifiedEventSource {
            rx,
            tx: tx.clone(),
            console,
            console_rx,
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
        };

        // Send only Release events (filtered on all platforms)
        for _ in 0..3 {
            tx.send(UnifiedEvent::Input(Event::Key(KeyEvent {
                code: KeyCode::Char('x'),
                modifiers: KeyModifiers::NONE,
                kind: KeyKind::Release,
            })))
            .unwrap();
        }

        // poll with a short timeout — should consume the filtered events
        // and return Ok(false) since no valid event is available
        let result = source.poll(Duration::from_millis(50));
        assert!(
            !result.unwrap() || source.pending_event.is_none(),
            "poll must not set pending_event for filtered Release events"
        );
    }

    /// Dirty windows must be cleared after `poll()` returns `Ok(false)`,
    /// otherwise the power profile stays at `Streaming` (16ms) forever.
    #[test]
    fn dirty_windows_cleared_after_poll_ok_false() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, _console_tx) = test_console();
        let mut source = UnifiedEventSource {
            rx,
            tx: tx.clone(),
            console,
            console_rx,
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
        };

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
        let (console, console_rx, _console_tx) = test_console();
        let mut set = HashSet::new();
        set.insert(WindowKey::default());
        let source = UnifiedEventSource {
            rx,
            tx: _tx,
            console,
            console_rx,
            console_alive: true,
            dirty_windows: set,
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
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
        let (console1, console_rx1, _console_tx1) = test_console();
        let source = UnifiedEventSource {
            rx: rx1,
            tx: tx1,
            console: console1,
            console_rx: console_rx1,
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
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
        let (console2, console_rx2, _console_tx2) = test_console();
        let stale = Instant::now().checked_sub(Duration::from_secs(3600));
        let source2 = UnifiedEventSource {
            rx: rx2,
            tx: tx2,
            console: console2,
            console_rx: console_rx2,
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
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
        let (console, console_rx, _console_tx) = test_console();
        let key = WindowKey::default();
        let mut source = UnifiedEventSource {
            rx,
            tx: _tx,
            console,
            console_rx,
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: vec![key],
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
        };

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
        let (console, console_rx, _console_tx) = test_console();
        let key = WindowKey::default();
        let mut set = HashSet::new();
        set.insert(key);
        let mut source = UnifiedEventSource {
            rx,
            tx: _tx,
            console,
            console_rx,
            console_alive: true,
            dirty_windows: set,
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
        };

        // Call through the trait, not an inherent method. Would return
        // an empty set if the trait override were missing.
        let taken = EventSource::take_dirty_windows(&mut source);
        assert_eq!(taken.len(), 1, "must return the pre-populated key");
        assert!(taken.contains(&key), "must contain the dirty key");

        let again = EventSource::take_dirty_windows(&mut source);
        assert!(again.is_empty(), "second call must drain");
    }

    #[test]
    fn poll_interval_clamped_by_max_sleep_duration() {
        let (_tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, _console_tx) = test_console();
        let mut source = UnifiedEventSource {
            rx,
            tx: _tx,
            console,
            console_rx,
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
        };

        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            source.poll_interval(),
            PowerProfile::PowerSaver.poll_interval()
        );
        #[cfg(target_os = "windows")]
        assert_eq!(
            source.poll_interval(),
            term_wm_core::power_profile::WINDOWS_MAX_POLL_INTERVAL
        );

        source.set_max_sleep_duration(Some(Duration::from_millis(100)));
        assert_eq!(source.poll_interval(), Duration::from_millis(100));

        source.set_max_sleep_duration(None);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            source.poll_interval(),
            PowerProfile::PowerSaver.poll_interval()
        );
        #[cfg(target_os = "windows")]
        assert_eq!(
            source.poll_interval(),
            term_wm_core::power_profile::WINDOWS_MAX_POLL_INTERVAL
        );
    }

    /// Console input events must be delivered through `poll`/`read`.
    #[test]
    fn poll_and_read_console_input_event() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, console_tx) = test_console();
        let mut source = UnifiedEventSource {
            rx,
            tx,
            console,
            console_rx,
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
        };
        console_tx.send(key_evt(KeyCode::Char('a'))).unwrap();

        assert!(
            source.poll(Duration::ZERO).unwrap(),
            "poll must report a console input event"
        );
        let evt = source.read().unwrap();
        assert!(
            matches!(evt, Event::Key(k) if k.code == KeyCode::Char('a')),
            "read must return the console event"
        );
    }

    /// Console events drained by `drain_console` must be preserved in order.
    #[test]
    fn drain_console_preserves_input_events() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, console_tx) = test_console();
        let mut source = UnifiedEventSource {
            rx,
            tx,
            console,
            console_rx,
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
        };
        for i in 0..5u8 {
            console_tx
                .send(key_evt(KeyCode::Char(char::from(b'a' + i))))
                .unwrap();
        }

        source.drain_console();
        assert_eq!(
            source.input_buffer.len(),
            5,
            "drain_console must buffer all console events"
        );

        for i in 0..5u8 {
            let evt = source.read().unwrap();
            assert!(
                matches!(evt, Event::Key(k) if k.code == KeyCode::Char(char::from(b'a' + i))),
                "read must return console events in order"
            );
        }
    }

    /// `poll` must skip Release events arriving on the console channel,
    /// continuing to wait until a valid event arrives or the deadline expires.
    #[test]
    fn poll_filters_console_release_events() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, console_tx) = test_console();
        let mut source = UnifiedEventSource {
            rx,
            tx,
            console,
            console_rx,
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
        };
        for _ in 0..3 {
            console_tx
                .send(Event::Key(KeyEvent {
                    code: KeyCode::Char('x'),
                    modifiers: KeyModifiers::NONE,
                    kind: KeyKind::Release,
                }))
                .unwrap();
        }

        let result = source.poll(Duration::from_millis(50));
        assert!(
            !result.unwrap() || source.pending_event.is_none(),
            "poll must not set pending_event for filtered console Release events"
        );
    }

    /// A disconnected console channel must not busy-loop: `poll` falls back to
    /// the unified channel and the frame deadline still fires.
    #[test]
    fn console_disconnect_falls_back_to_unified_channel() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, console_tx) = test_console();
        let mut source = UnifiedEventSource {
            rx,
            tx: tx.clone(),
            console,
            console_rx,
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
        };
        // Disconnect the console channel.
        drop(console_tx);
        // A PtyWakeup on the unified channel should still be processed.
        tx.send(UnifiedEvent::PtyWakeup(WindowKey::default()))
            .unwrap();

        let start = Instant::now();
        let result = source.poll(Duration::from_secs(5));
        assert!(!result.unwrap(), "poll must return Ok(false) via fallback");
        assert!(
            start.elapsed() < Duration::from_millis(1000),
            "poll must not busy-loop on a disconnected console channel"
        );
        assert!(
            !source.take_dirty_windows().is_empty(),
            "dirty_windows must be populated from the unified channel"
        );
    }
}
