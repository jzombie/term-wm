use std::collections::{HashSet, VecDeque};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};

use term_wm_console::background_console_reader::BackgroundConsoleReader;
use term_wm_core::events::{Event, KeyEvent, MouseEvent};
use term_wm_core::io::EventSource;
use term_wm_core::io::frame_pacer::FramePacer;
use term_wm_core::power_profile::PowerProfile;
use term_wm_core::utils::KeyboardNormalizer;
use term_wm_core::window::WindowKey;
use term_wm_pty_engine::DirectInputMode;

/// Capacity of the crossbeam channel between event producers and the event
/// loop. Generous capacity since wakeup gating (dirty.swap) prevents flooding.
pub(crate) const EVENT_CHANNEL_CAPACITY: usize = 256;

/// Events that can flow through the unified event channel.
#[derive(Debug, Clone)]
pub enum UnifiedEvent {
    /// A user-input event with optional connection attribution.
    /// `conn_id = None` for local console, `Some(id)` for remote Muxio viewers.
    Input {
        conn_id: Option<usize>,
        event: Event,
    },
    /// A PTY reader thread has new data available for `WindowKey`.
    PtyWakeup(WindowKey),
    /// A PTY child process has exited. Sent from the reader thread on EOF.
    AppExited(WindowKey),
    /// Application direct-input routing state changed. Carries the new mode
    /// snapshot so sub-mode shifts notify even when the aggregate is unchanged.
    DirectInputChanged(WindowKey, DirectInputMode),
    /// An OS signal was received (SIGINT, SIGTERM).
    Signal,
    /// Periodic tick for timing.
    Tick,
    /// Workspace entered notification (server pushed `OnWorkspaceEntered`).
    #[cfg(feature = "session-persistence")]
    WorkspaceEntered(String),
    /// A remote user connected to the current workspace.
    #[cfg(feature = "session-persistence")]
    UserConnected(term_session::protocol::UserInfo),
    /// A remote user disconnected.
    #[cfg(feature = "session-persistence")]
    UserDisconnected(usize),
    /// A user resized their terminal (server-coalesced `(conn_id, cols, rows)`).
    #[cfg(feature = "session-persistence")]
    UserResized((usize, u16, u16)),
    /// Fresh snapshot of connected users from `ListUsers`.
    #[cfg(feature = "session-persistence")]
    UserCacheRefreshed(Vec<term_session::protocol::UserInfo>),
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
    /// `None` in headless/internal-session mode.
    console: Option<BackgroundConsoleReader>,
    /// Clone of the console reader's receive side, selected on during
    /// poll/read/next_key/next_mouse.
    /// `None` in headless/internal-session mode.
    console_rx: Option<Receiver<Event>>,
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
    direct_input_changed: Vec<(WindowKey, DirectInputMode)>,
    /// Cached input event (poll returned true, waiting for read).
    pending_event: Option<Event>,
    /// Buffer for input events drained during `drain_pending`/`drain_console`
    /// so none are lost. Each entry carries an optional `conn_id` for
    /// attributed input routing.
    input_buffer: VecDeque<(Option<usize>, Event)>,
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
    /// Shared event owner — updated when an event is popped from the buffer.
    /// Cloned into `App` for action attribution.
    event_owner: Arc<Mutex<Option<usize>>>,
    /// Accumulated workspace-entered notifications.
    #[cfg(feature = "session-persistence")]
    workspace_entered: Vec<String>,
    /// Accumulated user-connected events.
    #[cfg(feature = "session-persistence")]
    user_connected: Vec<term_session::protocol::UserInfo>,
    /// Accumulated user-disconnected events.
    #[cfg(feature = "session-persistence")]
    user_disconnected: Vec<usize>,
    /// Accumulated user-resized events as `(conn_id, cols, rows)` tuples.
    #[cfg(feature = "session-persistence")]
    user_resized: Vec<(usize, u16, u16)>,
    /// Latest user cache snapshot.
    #[cfg(feature = "session-persistence")]
    user_cache_refreshed: Option<Vec<term_session::protocol::UserInfo>>,
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
    /// If `headless` is `true` (internal session mode), crossterm reading is
    /// disabled and attributed input arrives via `pty_wakeup_tx()`.
    ///
    /// Returns the source and a shared `Arc<Mutex<Option<usize>>>` for
    /// event attribution that should be passed to `App`.
    pub fn new(headless: bool) -> io::Result<(Self, Arc<Mutex<Option<usize>>>)> {
        let (tx, rx) = bounded::<UnifiedEvent>(EVENT_CHANNEL_CAPACITY);
        let event_owner = Arc::new(Mutex::new(None));
        let (console, console_rx) = if headless {
            (None, None)
        } else {
            let console = BackgroundConsoleReader::new()?;
            let console_rx = console.receiver();
            (Some(console), Some(console_rx))
        };
        let console_alive = console_rx.is_some();
        Ok((
            Self {
                rx,
                tx,
                console,
                console_rx,
                console_alive,
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
                event_owner: event_owner.clone(),
                #[cfg(feature = "session-persistence")]
                workspace_entered: Vec::new(),
                #[cfg(feature = "session-persistence")]
                user_connected: Vec::new(),
                #[cfg(feature = "session-persistence")]
                user_disconnected: Vec::new(),
                #[cfg(feature = "session-persistence")]
                user_resized: Vec::new(),
                #[cfg(feature = "session-persistence")]
                user_cache_refreshed: None,
            },
            event_owner,
        ))
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
                Ok(UnifiedEvent::Input {
                    conn_id: None,
                    event,
                }) => {
                    if let Some(normalized) = self.normalizer.normalize(event) {
                        self.input_buffer.push_back((None, normalized));
                    }
                }
                Ok(UnifiedEvent::Input {
                    conn_id: Some(conn_id),
                    event,
                }) => {
                    if let Some(normalized) = self.normalizer.normalize(event) {
                        self.input_buffer.push_back((Some(conn_id), normalized));
                    }
                }
                Ok(UnifiedEvent::PtyWakeup(key)) => {
                    self.dirty_windows.insert(key);
                }
                Ok(UnifiedEvent::AppExited(key)) => {
                    self.exited_windows.push(key);
                }
                Ok(UnifiedEvent::DirectInputChanged(key, mode)) => {
                    tracing::info!(
                        "[STAGE 3] drain_pending collected DirectInputChanged({:?}, {:?})",
                        key,
                        mode
                    );
                    self.dirty_windows.insert(key);
                    self.direct_input_changed.push((key, mode));
                }
                Ok(UnifiedEvent::Signal) => {
                    self.signal_received = true;
                }
                Ok(UnifiedEvent::Tick) => {
                    // No-op — tick is implicit in the event-cycle loop.
                }
                #[cfg(feature = "session-persistence")]
                Ok(UnifiedEvent::WorkspaceEntered(ws)) => {
                    self.workspace_entered.push(ws);
                }
                #[cfg(feature = "session-persistence")]
                Ok(UnifiedEvent::UserConnected(info)) => {
                    self.user_connected.push(info);
                }
                #[cfg(feature = "session-persistence")]
                Ok(UnifiedEvent::UserDisconnected(conn_id)) => {
                    self.user_disconnected.push(conn_id);
                }
                #[cfg(feature = "session-persistence")]
                Ok(UnifiedEvent::UserResized(resized)) => {
                    self.user_resized.push(resized);
                }
                #[cfg(feature = "session-persistence")]
                Ok(UnifiedEvent::UserCacheRefreshed(users)) => {
                    self.user_cache_refreshed = Some(users);
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => break,
            }
        }
    }

    /// Drain all pending console input events (non-blocking) into
    /// `input_buffer`, normalizing with the consumer's own normalizer.
    fn drain_console(&mut self) {
        while let Some(ref console_rx) = self.console_rx {
            match console_rx.try_recv() {
                Ok(event) => {
                    if let Some(normalized) = self.normalizer.normalize(event) {
                        self.input_buffer.push_back((None, normalized));
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
            UnifiedEvent::Input {
                conn_id: None,
                event,
            } => {
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
            UnifiedEvent::DirectInputChanged(key, mode) => {
                tracing::info!(
                    "[STAGE 3] recv collected DirectInputChanged({:?}, {:?})",
                    key,
                    mode
                );
                self.dirty_windows.insert(key);
                self.direct_input_changed.push((key, mode));
                self.frame_pacer.notify_pending(Instant::now());
                UnifiedPoll::RenderDue
            }
            UnifiedEvent::Signal => {
                self.signal_received = true;
                UnifiedPoll::RenderDue
            }
            UnifiedEvent::Tick => UnifiedPoll::RenderDue,
            UnifiedEvent::Input {
                conn_id: Some(conn_id),
                event,
            } => {
                self.last_event_at = Some(Instant::now());
                if let Some(normalized) = self.normalizer.normalize(event) {
                    self.frame_pacer.reset();
                    self.pending_event = Some(normalized);
                    *self
                        .event_owner
                        .lock()
                        .unwrap_or_else(|err| err.into_inner()) = Some(conn_id);
                    UnifiedPoll::Ready
                } else {
                    UnifiedPoll::Continue
                }
            }
            #[cfg(feature = "session-persistence")]
            UnifiedEvent::WorkspaceEntered(ws) => {
                self.workspace_entered.push(ws);
                self.frame_pacer.notify_pending(Instant::now());
                UnifiedPoll::RenderDue
            }
            #[cfg(feature = "session-persistence")]
            UnifiedEvent::UserConnected(info) => {
                self.user_connected.push(info);
                self.frame_pacer.notify_pending(Instant::now());
                UnifiedPoll::RenderDue
            }
            #[cfg(feature = "session-persistence")]
            UnifiedEvent::UserDisconnected(conn_id) => {
                self.user_disconnected.push(conn_id);
                self.frame_pacer.notify_pending(Instant::now());
                UnifiedPoll::RenderDue
            }
            #[cfg(feature = "session-persistence")]
            UnifiedEvent::UserResized(resized) => {
                self.user_resized.push(resized);
                self.frame_pacer.notify_pending(Instant::now());
                UnifiedPoll::RenderDue
            }
            #[cfg(feature = "session-persistence")]
            UnifiedEvent::UserCacheRefreshed(users) => {
                self.user_cache_refreshed = Some(users);
                self.frame_pacer.notify_pending(Instant::now());
                UnifiedPoll::RenderDue
            }
        }
    }

    /// Handle one unified-channel event during `read`. Returns `Some(event)`
    /// when a normalized user-input event should be returned.
    fn handle_unified_read(&mut self, evt: UnifiedEvent) -> Option<Event> {
        match evt {
            UnifiedEvent::Input {
                conn_id: None,
                event,
            } => {
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
            UnifiedEvent::DirectInputChanged(key, mode) => {
                tracing::info!(
                    "[STAGE 3] read collected DirectInputChanged({:?}, {:?})",
                    key,
                    mode
                );
                self.dirty_windows.insert(key);
                self.direct_input_changed.push((key, mode));
                None
            }
            UnifiedEvent::Signal => {
                self.signal_received = true;
                None
            }
            UnifiedEvent::Tick => None,
            UnifiedEvent::Input {
                conn_id: Some(conn_id),
                event,
            } => {
                self.last_event_at = Some(Instant::now());
                *self
                    .event_owner
                    .lock()
                    .unwrap_or_else(|err| err.into_inner()) = Some(conn_id);
                self.normalizer.normalize(event)
            }
            #[cfg(feature = "session-persistence")]
            UnifiedEvent::WorkspaceEntered(ws) => {
                self.workspace_entered.push(ws);
                None
            }
            #[cfg(feature = "session-persistence")]
            UnifiedEvent::UserConnected(info) => {
                self.user_connected.push(info);
                None
            }
            #[cfg(feature = "session-persistence")]
            UnifiedEvent::UserDisconnected(conn_id) => {
                self.user_disconnected.push(conn_id);
                None
            }
            #[cfg(feature = "session-persistence")]
            UnifiedEvent::UserResized(resized) => {
                self.user_resized.push(resized);
                None
            }
            #[cfg(feature = "session-persistence")]
            UnifiedEvent::UserCacheRefreshed(users) => {
                self.user_cache_refreshed = Some(users);
                None
            }
        }
    }

    /// Remove the first buffered event matching `predicate` (used by the
    /// `next_key`/`next_mouse` maintenance paths).
    fn pop_matching(&mut self, predicate: impl Fn(&Event) -> bool) -> Option<Event> {
        if let Some(idx) = self.input_buffer.iter().position(|(_, evt)| predicate(evt))
            && let Some((conn_id, event)) = self.input_buffer.remove(idx)
        {
            *self
                .event_owner
                .lock()
                .unwrap_or_else(|err| err.into_inner()) = conn_id;
            return Some(event);
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
        let local_console_rx = self.console_rx.clone().unwrap_or_else(|| {
            // Headless mode: create a disconnected channel
            let (_tx, rx) = bounded(1);
            rx
        });

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
            if let Some((conn_id, event)) = self.input_buffer.pop_front() {
                *self
                    .event_owner
                    .lock()
                    .unwrap_or_else(|err| err.into_inner()) = conn_id;
                self.last_event_at = Some(Instant::now());
                if let Some(normalized) = self.normalizer.normalize(event) {
                    return Ok(normalized);
                }
                continue;
            }

            let local_rx = self.rx.clone();
            let local_console_rx = self.console_rx.clone().unwrap_or_else(|| {
                // Headless mode: create a disconnected channel
                let (_tx, rx) = bounded(1);
                rx
            });
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
                self.input_buffer.push_back((None, event));
            }

            let local_rx = self.rx.clone();
            let local_console_rx = self.console_rx.clone().unwrap_or_else(|| {
                // Headless mode: create a disconnected channel
                let (_tx, rx) = bounded(1);
                rx
            });
            if self.console_alive {
                crossbeam_channel::select! {
                    recv(local_console_rx) -> msg => match msg {
                        Ok(event) => {
                            if let Some(normalized) = self.normalizer.normalize(event) {
                                if let Event::Key(key) = normalized {
                                    return Ok(key);
                                }
                                self.input_buffer.push_back((None, normalized));
                            }
                        }
                        Err(_) => {
                            self.console_alive = false;
                        }
                    },
                    recv(local_rx) -> msg => match msg {
                        Ok(UnifiedEvent::Input { conn_id: None, event }) => {
                            if let Event::Key(key) = event {
                                return Ok(key);
                            }
                            self.input_buffer.push_back((None, event));
                        }
                        Ok(UnifiedEvent::Input { conn_id: Some(conn_id), event }) => {
                            if let Event::Key(key) = event {
                                *self.event_owner.lock().unwrap_or_else(|err| err.into_inner()) = Some(conn_id);
                                return Ok(key);
                            }
                            self.input_buffer.push_back((Some(conn_id), event));
                        }
                        Ok(UnifiedEvent::PtyWakeup(_)) => {}
                        Ok(UnifiedEvent::AppExited(key)) => {
                            self.exited_windows.push(key);
                        }
                        Ok(UnifiedEvent::DirectInputChanged(key, mode)) => {
                            self.dirty_windows.insert(key);
                            self.direct_input_changed.push((key, mode));
                        }
                        Ok(UnifiedEvent::Signal) => self.signal_received = true,
                        Ok(UnifiedEvent::Tick) => {}
                        #[cfg(feature = "session-persistence")]
                        Ok(UnifiedEvent::WorkspaceEntered(ws)) => {
                            self.workspace_entered.push(ws);
                        }
                        #[cfg(feature = "session-persistence")]
                        Ok(UnifiedEvent::UserConnected(info)) => {
                            self.user_connected.push(info);
                        }
                        #[cfg(feature = "session-persistence")]
                        Ok(UnifiedEvent::UserDisconnected(id)) => {
                            self.user_disconnected.push(id);
                        }
                        #[cfg(feature = "session-persistence")]
                        Ok(UnifiedEvent::UserResized(resized)) => {
                            self.user_resized.push(resized);
                        }
                        #[cfg(feature = "session-persistence")]
                        Ok(UnifiedEvent::UserCacheRefreshed(users)) => {
                            self.user_cache_refreshed = Some(users);
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
                    Ok(UnifiedEvent::Input {
                        conn_id: None,
                        event,
                    }) => {
                        if let Event::Key(key) = event {
                            return Ok(key);
                        }
                        self.input_buffer.push_back((None, event));
                    }
                    Ok(UnifiedEvent::Input {
                        conn_id: Some(conn_id),
                        event,
                    }) => {
                        if let Event::Key(key) = event {
                            *self
                                .event_owner
                                .lock()
                                .unwrap_or_else(|err| err.into_inner()) = Some(conn_id);
                            return Ok(key);
                        }
                        self.input_buffer.push_back((Some(conn_id), event));
                    }
                    Ok(UnifiedEvent::PtyWakeup(_)) => {}
                    Ok(UnifiedEvent::AppExited(key)) => {
                        self.exited_windows.push(key);
                    }
                    Ok(UnifiedEvent::DirectInputChanged(key, mode)) => {
                        self.dirty_windows.insert(key);
                        self.direct_input_changed.push((key, mode));
                    }
                    Ok(UnifiedEvent::Signal) => self.signal_received = true,
                    Ok(UnifiedEvent::Tick) => {}
                    #[cfg(feature = "session-persistence")]
                    Ok(UnifiedEvent::WorkspaceEntered(ws)) => {
                        self.workspace_entered.push(ws);
                    }
                    #[cfg(feature = "session-persistence")]
                    Ok(UnifiedEvent::UserConnected(info)) => {
                        self.user_connected.push(info);
                    }
                    #[cfg(feature = "session-persistence")]
                    Ok(UnifiedEvent::UserDisconnected(id)) => {
                        self.user_disconnected.push(id);
                    }
                    #[cfg(feature = "session-persistence")]
                    Ok(UnifiedEvent::UserResized(resized)) => {
                        self.user_resized.push(resized);
                    }
                    #[cfg(feature = "session-persistence")]
                    Ok(UnifiedEvent::UserCacheRefreshed(users)) => {
                        self.user_cache_refreshed = Some(users);
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
                self.input_buffer.push_back((None, event));
            }

            let local_rx = self.rx.clone();
            let local_console_rx = self.console_rx.clone().unwrap_or_else(|| {
                // Headless mode: create a disconnected channel
                let (_tx, rx) = bounded(1);
                rx
            });
            if self.console_alive {
                crossbeam_channel::select! {
                    recv(local_console_rx) -> msg => match msg {
                        Ok(event) => {
                            if let Some(normalized) = self.normalizer.normalize(event) {
                                if let Event::Mouse(mouse) = normalized {
                                    return Ok(mouse);
                                }
                                self.input_buffer.push_back((None, normalized));
                            }
                        }
                        Err(_) => {
                            self.console_alive = false;
                        }
                    },
                    recv(local_rx) -> msg => match msg {
                        Ok(UnifiedEvent::Input { conn_id: None, event }) => {
                            if let Event::Mouse(mouse) = event {
                                return Ok(mouse);
                            }
                            self.input_buffer.push_back((None, event));
                        }
                        Ok(UnifiedEvent::Input { conn_id: Some(conn_id), event }) => {
                            if let Event::Mouse(mouse) = event {
                                *self.event_owner.lock().unwrap_or_else(|err| err.into_inner()) = Some(conn_id);
                                return Ok(mouse);
                            }
                            self.input_buffer.push_back((Some(conn_id), event));
                        }
                        Ok(UnifiedEvent::PtyWakeup(_)) => {}
                        Ok(UnifiedEvent::AppExited(key)) => {
                            self.exited_windows.push(key);
                        }
                        Ok(UnifiedEvent::DirectInputChanged(key, mode)) => {
                            self.dirty_windows.insert(key);
                            self.direct_input_changed.push((key, mode));
                        }
                        Ok(UnifiedEvent::Signal) => self.signal_received = true,
                        Ok(UnifiedEvent::Tick) => {}
                        #[cfg(feature = "session-persistence")]
                        Ok(UnifiedEvent::WorkspaceEntered(ws)) => {
                            self.workspace_entered.push(ws);
                        }
                        #[cfg(feature = "session-persistence")]
                        Ok(UnifiedEvent::UserConnected(info)) => {
                            self.user_connected.push(info);
                        }
                        #[cfg(feature = "session-persistence")]
                        Ok(UnifiedEvent::UserDisconnected(id)) => {
                            self.user_disconnected.push(id);
                        }
                        #[cfg(feature = "session-persistence")]
                        Ok(UnifiedEvent::UserResized(resized)) => {
                            self.user_resized.push(resized);
                        }
                        #[cfg(feature = "session-persistence")]
                        Ok(UnifiedEvent::UserCacheRefreshed(users)) => {
                            self.user_cache_refreshed = Some(users);
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
                    Ok(UnifiedEvent::Input {
                        conn_id: None,
                        event,
                    }) => {
                        if let Event::Mouse(mouse) = event {
                            return Ok(mouse);
                        }
                        self.input_buffer.push_back((None, event));
                    }
                    Ok(UnifiedEvent::Input {
                        conn_id: Some(conn_id),
                        event,
                    }) => {
                        if let Event::Mouse(mouse) = event {
                            *self
                                .event_owner
                                .lock()
                                .unwrap_or_else(|err| err.into_inner()) = Some(conn_id);
                            return Ok(mouse);
                        }
                        self.input_buffer.push_back((Some(conn_id), event));
                    }
                    Ok(UnifiedEvent::PtyWakeup(_)) => {}
                    Ok(UnifiedEvent::AppExited(key)) => {
                        self.exited_windows.push(key);
                    }
                    Ok(UnifiedEvent::DirectInputChanged(key, mode)) => {
                        self.dirty_windows.insert(key);
                        self.direct_input_changed.push((key, mode));
                    }
                    Ok(UnifiedEvent::Signal) => self.signal_received = true,
                    Ok(UnifiedEvent::Tick) => {}
                    #[cfg(feature = "session-persistence")]
                    Ok(UnifiedEvent::WorkspaceEntered(ws)) => {
                        self.workspace_entered.push(ws);
                    }
                    #[cfg(feature = "session-persistence")]
                    Ok(UnifiedEvent::UserConnected(info)) => {
                        self.user_connected.push(info);
                    }
                    #[cfg(feature = "session-persistence")]
                    Ok(UnifiedEvent::UserDisconnected(id)) => {
                        self.user_disconnected.push(id);
                    }
                    #[cfg(feature = "session-persistence")]
                    Ok(UnifiedEvent::UserResized(resized)) => {
                        self.user_resized.push(resized);
                    }
                    #[cfg(feature = "session-persistence")]
                    Ok(UnifiedEvent::UserCacheRefreshed(users)) => {
                        self.user_cache_refreshed = Some(users);
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

    // Delegates to the console crate's background reader, which owns the
    // single canonical adapter call (term_wm_crossterm_adapter::set_mouse_capture).
    fn set_mouse_capture(&mut self, enabled: bool) -> io::Result<()> {
        if let Some(ref console) = self.console {
            console.set_mouse_capture(enabled)
        } else {
            Ok(())
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

    fn take_direct_input_changed(&mut self) -> Vec<(WindowKey, DirectInputMode)> {
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

    #[cfg(feature = "session-persistence")]
    fn take_workspace_entered(&mut self) -> Vec<String> {
        std::mem::take(&mut self.workspace_entered)
    }

    #[cfg(feature = "session-persistence")]
    fn take_user_connected(&mut self) -> Vec<term_wm_core::user_registry::UserEntry> {
        self.user_connected
            .drain(..)
            .map(|info| term_wm_core::user_registry::UserEntry {
                conn_id: info.conn_id,
                user: info.user,
                hostname: info.hostname,
                ssh_ip: info.ssh_ip,
                ssh_port: info.ssh_port,
                cols: info.cols,
                rows: info.rows,
                connected_at_unix: info.connected_at_unix,
                pid: info.pid,
            })
            .collect()
    }

    #[cfg(feature = "session-persistence")]
    fn take_user_disconnected(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.user_disconnected)
    }

    fn take_user_resized(&mut self) -> Vec<(usize, u16, u16)> {
        #[cfg(feature = "session-persistence")]
        {
            std::mem::take(&mut self.user_resized)
        }
        #[cfg(not(feature = "session-persistence"))]
        {
            Vec::new()
        }
    }

    #[cfg(feature = "session-persistence")]
    fn take_user_cache_refreshed(&mut self) -> Option<Vec<term_wm_core::user_registry::UserEntry>> {
        self.user_cache_refreshed.take().map(|users| {
            users
                .into_iter()
                .map(|info| term_wm_core::user_registry::UserEntry {
                    conn_id: info.conn_id,
                    user: info.user,
                    hostname: info.hostname,
                    ssh_ip: info.ssh_ip,
                    ssh_port: info.ssh_port,
                    cols: info.cols,
                    rows: info.rows,
                    connected_at_unix: info.connected_at_unix,
                    pid: info.pid,
                })
                .collect()
        })
    }
}

#[allow(clippy::unwrap_used)]
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
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            #[cfg(feature = "session-persistence")]
            workspace_entered: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_connected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_disconnected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_resized: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_cache_refreshed: None,
        };

        // Send 10 input events into the channel
        for i in 0..10u8 {
            tx.send(UnifiedEvent::Input {
                conn_id: None,
                event: key_evt(KeyCode::Char(char::from(b'a' + i))),
            })
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
        for (i, (_conn_id, evt)) in source.input_buffer.iter().enumerate() {
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
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            #[cfg(feature = "session-persistence")]
            workspace_entered: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_connected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_disconnected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_resized: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_cache_refreshed: None,
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
            tx.send(UnifiedEvent::Input {
                conn_id: None,
                event: evt,
            })
            .unwrap();
        }

        source.drain_pending();

        assert_eq!(
            source.input_buffer.len(),
            3,
            "Only Release events should be filtered by normalization"
        );

        // Verify Release event (index 1) is absent
        for (_conn_id, evt) in &source.input_buffer {
            if let Event::Key(k) = evt {
                assert_ne!(
                    k.kind,
                    KeyKind::Release,
                    "Release events must never survive normalization"
                );
            }
        }

        // Verify first and last events are the Press events
        let (_c1, first) = source.input_buffer.front().unwrap();
        let (_c2, last) = source.input_buffer.back().unwrap();
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
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            #[cfg(feature = "session-persistence")]
            workspace_entered: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_connected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_disconnected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_resized: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_cache_refreshed: None,
        };

        // Send only Release events (filtered on all platforms)
        for _ in 0..3 {
            tx.send(UnifiedEvent::Input {
                conn_id: None,
                event: Event::Key(KeyEvent {
                    code: KeyCode::Char('x'),
                    modifiers: KeyModifiers::NONE,
                    kind: KeyKind::Release,
                }),
            })
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
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            #[cfg(feature = "session-persistence")]
            workspace_entered: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_connected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_disconnected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_resized: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_cache_refreshed: None,
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
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: set,
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            #[cfg(feature = "session-persistence")]
            workspace_entered: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_connected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_disconnected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_resized: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_cache_refreshed: None,
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
            console: Some(console1),
            console_rx: Some(console_rx1),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: true,
            max_sleep_duration: None,
            #[cfg(feature = "session-persistence")]
            workspace_entered: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_connected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_disconnected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_resized: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_cache_refreshed: None,
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
            console: Some(console2),
            console_rx: Some(console_rx2),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: stale,
            frame_pacer: FramePacer::new(),
            pending_work: true,
            max_sleep_duration: None,
            #[cfg(feature = "session-persistence")]
            workspace_entered: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_connected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_disconnected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_resized: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_cache_refreshed: None,
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
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: vec![key],
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            #[cfg(feature = "session-persistence")]
            workspace_entered: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_connected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_disconnected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_resized: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_cache_refreshed: None,
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
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: set,
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            #[cfg(feature = "session-persistence")]
            workspace_entered: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_connected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_disconnected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_resized: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_cache_refreshed: None,
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
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            #[cfg(feature = "session-persistence")]
            workspace_entered: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_connected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_disconnected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_resized: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_cache_refreshed: None,
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
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            #[cfg(feature = "session-persistence")]
            workspace_entered: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_connected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_disconnected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_resized: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_cache_refreshed: None,
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
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            #[cfg(feature = "session-persistence")]
            workspace_entered: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_connected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_disconnected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_resized: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_cache_refreshed: None,
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
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            #[cfg(feature = "session-persistence")]
            workspace_entered: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_connected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_disconnected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_resized: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_cache_refreshed: None,
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
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            #[cfg(feature = "session-persistence")]
            workspace_entered: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_connected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_disconnected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_resized: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_cache_refreshed: None,
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

    /// Remote (attributed) viewer input must carry its `conn_id` through
    /// `drain_pending` → `read()` onto the shared `event_owner`, while local
    /// console input (`conn_id: None`) resets the owner. This is the
    /// attribution contract the "Detach Viewer" action and multi-viewer
    /// routing rely on.
    #[test]
    fn attributed_conn_id_is_preserved_through_read() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, _console_tx) = test_console();
        let event_owner = std::sync::Arc::new(std::sync::Mutex::new(None));
        let mut source = UnifiedEventSource {
            rx,
            tx: tx.clone(),
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: event_owner.clone(),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            #[cfg(feature = "session-persistence")]
            workspace_entered: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_connected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_disconnected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_resized: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_cache_refreshed: None,
        };

        // Remote viewer key (conn_id 7) followed by a local console key.
        tx.send(UnifiedEvent::Input {
            conn_id: Some(7),
            event: key_evt(KeyCode::Char('r')),
        })
        .unwrap();
        tx.send(UnifiedEvent::Input {
            conn_id: None,
            event: key_evt(KeyCode::Char('l')),
        })
        .unwrap();

        source.drain_pending();
        assert_eq!(source.input_buffer.len(), 2);

        // First read surfaces the remote viewer's conn_id on the shared owner.
        match source.read().unwrap() {
            Event::Key(k) => assert_eq!(k.code, KeyCode::Char('r')),
            _ => panic!("expected Key event"),
        }
        assert_eq!(
            *event_owner.lock().unwrap_or_else(|e| e.into_inner()),
            Some(7)
        );

        // Local console input resets the owner to None.
        match source.read().unwrap() {
            Event::Key(k) => assert_eq!(k.code, KeyCode::Char('l')),
            _ => panic!("expected Key event"),
        }
        assert_eq!(*event_owner.lock().unwrap_or_else(|e| e.into_inner()), None);
    }

    /// `UnifiedEventSource::new(true)` (internal-session mode) must skip
    /// crossterm console reading entirely and route attributed input through
    /// `pty_wakeup_tx()` into the main channel — the fix for event-loop
    /// starvation in headless mode.
    #[test]
    fn headless_new_routes_attributed_input_through_pty_wakeup_tx() {
        let (mut source, owner) = UnifiedEventSource::new(true).expect("headless new");
        assert!(source.console.is_none());
        assert!(source.console_rx.is_none());
        assert!(
            !source.console_alive,
            "headless source must not use a console"
        );

        // Attributed input arrives through the shared wakeup channel.
        source
            .pty_wakeup_tx()
            .send(UnifiedEvent::Input {
                conn_id: Some(9),
                event: key_evt(KeyCode::Char('x')),
            })
            .unwrap();

        // poll() must wake immediately (no console select path) and read()
        // must surface the event with its conn_id on the shared owner.
        assert!(
            source.poll(Duration::from_millis(500)).unwrap(),
            "poll must report an available event in headless mode"
        );
        match source.read().unwrap() {
            Event::Key(k) => assert_eq!(k.code, KeyCode::Char('x')),
            _ => panic!("expected Key event"),
        }
        assert_eq!(
            *owner.lock().unwrap_or_else(|e| e.into_inner()),
            Some(9),
            "attributed conn_id must be published to the shared owner"
        );
    }

    /// The drain/state-accessor surface of the unified source: direct-input
    /// transitions, signals, and redraw requests must be collected and
    /// consumed exactly once.
    #[test]
    fn drain_collects_direct_input_signal_and_redraw_requests() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, _console_tx) = test_console();
        let mut source = UnifiedEventSource {
            rx,
            tx: tx.clone(),
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            #[cfg(feature = "session-persistence")]
            workspace_entered: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_connected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_disconnected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_resized: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_cache_refreshed: None,
        };

        let key = WindowKey::default();
        let mode = DirectInputMode::default();
        tx.send(UnifiedEvent::DirectInputChanged(key, mode))
            .unwrap();
        tx.send(UnifiedEvent::Signal).unwrap();
        source.request_redraw();
        source.set_max_sleep_duration(Some(Duration::from_secs(1)));

        source.drain_pending();

        assert!(
            source.take_signal(),
            "Signal must be latched by drain_pending"
        );
        assert!(!source.take_signal(), "take_signal must be consume-once");
        assert_eq!(source.take_direct_input_changed(), vec![(key, mode)]);
        assert!(source.take_direct_input_changed().is_empty());
        assert!(
            source.take_redraw_request(),
            "request_redraw must be latched"
        );
        assert!(
            !source.take_redraw_request(),
            "take_redraw_request must be consume-once"
        );
    }

    /// `next_key` (used by keybinding evaluation) must surface attributed
    /// keys and publish their conn_id on the shared owner.
    #[test]
    fn next_key_surfaces_attributed_key_and_owner() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, _console_tx) = test_console();
        let event_owner = std::sync::Arc::new(std::sync::Mutex::new(None));
        let mut source = UnifiedEventSource {
            rx,
            tx: tx.clone(),
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: event_owner.clone(),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            #[cfg(feature = "session-persistence")]
            workspace_entered: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_connected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_disconnected: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_resized: Vec::new(),
            #[cfg(feature = "session-persistence")]
            user_cache_refreshed: None,
        };

        tx.send(UnifiedEvent::Input {
            conn_id: Some(11),
            event: key_evt(KeyCode::Char('k')),
        })
        .unwrap();

        let key = source.next_key().expect("next_key");
        assert_eq!(key.code, KeyCode::Char('k'));
        assert_eq!(
            *event_owner.lock().unwrap_or_else(|e| e.into_inner()),
            Some(11),
            "attributed key must set the shared owner"
        );
    }

    /// Helper to build a `UnifiedEventSource` with session-persistence fields
    /// for the tests below.
    #[cfg(feature = "session-persistence")]
    fn make_source_with_session_fields(
        workspace_entered: Vec<String>,
        user_connected: Vec<term_session::protocol::UserInfo>,
        user_disconnected: Vec<usize>,
        user_cache_refreshed: Option<Vec<term_session::protocol::UserInfo>>,
    ) -> UnifiedEventSource {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, _console_tx) = test_console();
        UnifiedEventSource {
            rx,
            tx,
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            workspace_entered,
            user_connected,
            user_disconnected,
            user_resized: Vec::new(),
            user_cache_refreshed,
        }
    }

    /// Build a `UserInfo` with all fields set to distinct, recognisable values
    /// so a transposition bug is immediately caught.
    #[cfg(feature = "session-persistence")]
    fn make_user_info(conn_id: usize) -> term_session::protocol::UserInfo {
        term_session::protocol::UserInfo {
            conn_id,
            user: format!("user_{conn_id}"),
            hostname: format!("host_{conn_id}"),
            ssh_ip: Some(format!("10.0.0.{conn_id}")),
            ssh_port: Some(2200 + conn_id as u16),
            cols: 100 + conn_id as u16,
            rows: 50 + conn_id as u16,
            connected_at_unix: 1000 + conn_id as u64,
            pid: 2000 + conn_id as u64,
        }
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    fn take_workspace_entered_drains_accumulator() {
        use super::EventSource;
        let mut source = make_source_with_session_fields(
            vec!["ws1".into(), "ws2".into()],
            Vec::new(),
            Vec::new(),
            None,
        );
        let taken = EventSource::take_workspace_entered(&mut source);
        assert_eq!(taken, vec!["ws1", "ws2"]);
        assert!(EventSource::take_workspace_entered(&mut source).is_empty());
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    fn take_user_disconnected_drains_accumulator() {
        use super::EventSource;
        let mut source = make_source_with_session_fields(Vec::new(), Vec::new(), vec![3, 7], None);
        let taken = EventSource::take_user_disconnected(&mut source);
        assert_eq!(taken, vec![3, 7]);
        assert!(EventSource::take_user_disconnected(&mut source).is_empty());
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    fn take_user_resized_drains_accumulator() {
        use super::EventSource;
        let mut source = make_source_with_session_fields(Vec::new(), Vec::new(), Vec::new(), None);
        source.user_resized = vec![(3, 100, 30), (7, 200, 50)];
        let taken = EventSource::take_user_resized(&mut source);
        assert_eq!(taken, vec![(3, 100, 30), (7, 200, 50)]);
        assert!(EventSource::take_user_resized(&mut source).is_empty());
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    fn take_user_connected_maps_all_fields() {
        use super::EventSource;
        let info = make_user_info(42);
        let mut source = make_source_with_session_fields(Vec::new(), vec![info], Vec::new(), None);
        let entries = EventSource::take_user_connected(&mut source);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.conn_id, 42);
        assert_eq!(e.user, "user_42");
        assert_eq!(e.hostname, "host_42");
        assert_eq!(e.ssh_ip.as_deref(), Some("10.0.0.42"));
        assert_eq!(e.ssh_port, Some(2242));
        assert_eq!(e.cols, 142);
        assert_eq!(e.rows, 92);
        assert_eq!(e.connected_at_unix, 1042);
        assert_eq!(e.pid, 2042);
        assert!(
            EventSource::take_user_connected(&mut source).is_empty(),
            "second call must drain"
        );
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    fn take_user_cache_refreshed_maps_fields_and_handles_none() {
        use super::EventSource;
        let mut source = make_source_with_session_fields(Vec::new(), Vec::new(), Vec::new(), None);
        assert!(
            EventSource::take_user_cache_refreshed(&mut source).is_none(),
            "None must remain None"
        );

        let info = make_user_info(99);
        let mut source2 =
            make_source_with_session_fields(Vec::new(), Vec::new(), Vec::new(), Some(vec![info]));
        let entries =
            EventSource::take_user_cache_refreshed(&mut source2).expect("must return Some");
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.conn_id, 99);
        assert_eq!(e.user, "user_99");
        assert_eq!(e.hostname, "host_99");
        assert_eq!(e.ssh_ip.as_deref(), Some("10.0.0.99"));
        assert_eq!(e.ssh_port, Some(2299));
        assert_eq!(e.cols, 199);
        assert_eq!(e.rows, 149);
        assert_eq!(e.connected_at_unix, 1099);
        assert_eq!(e.pid, 2099);
        assert!(
            EventSource::take_user_cache_refreshed(&mut source2).is_none(),
            "second call must return None"
        );
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    fn drain_pending_accumulates_workspace_entered() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, _console_tx) = test_console();
        let mut source = UnifiedEventSource {
            rx,
            tx: tx.clone(),
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            workspace_entered: Vec::new(),
            user_connected: Vec::new(),
            user_disconnected: Vec::new(),
            user_resized: Vec::new(),
            user_cache_refreshed: None,
        };

        tx.send(UnifiedEvent::WorkspaceEntered("ws1".into()))
            .unwrap();
        tx.send(UnifiedEvent::WorkspaceEntered("ws2".into()))
            .unwrap();
        source.drain_pending();
        assert_eq!(
            source.workspace_entered,
            vec!["ws1", "ws2"],
            "drain_pending must accumulate WorkspaceEntered events"
        );
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    fn drain_pending_accumulates_user_connected() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, _console_tx) = test_console();
        let mut source = UnifiedEventSource {
            rx,
            tx: tx.clone(),
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            workspace_entered: Vec::new(),
            user_connected: Vec::new(),
            user_disconnected: Vec::new(),
            user_resized: Vec::new(),
            user_cache_refreshed: None,
        };

        let info = make_user_info(5);
        tx.send(UnifiedEvent::UserConnected(info)).unwrap();
        source.drain_pending();
        assert_eq!(
            source.user_connected.len(),
            1,
            "drain_pending must accumulate UserConnected"
        );
        assert_eq!(source.user_connected[0].conn_id, 5);
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    fn drain_pending_accumulates_user_disconnected() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, _console_tx) = test_console();
        let mut source = UnifiedEventSource {
            rx,
            tx: tx.clone(),
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            workspace_entered: Vec::new(),
            user_connected: Vec::new(),
            user_disconnected: Vec::new(),
            user_resized: Vec::new(),
            user_cache_refreshed: None,
        };

        tx.send(UnifiedEvent::UserDisconnected(42)).unwrap();
        tx.send(UnifiedEvent::UserDisconnected(7)).unwrap();
        source.drain_pending();
        assert_eq!(
            source.user_disconnected,
            vec![42, 7],
            "drain_pending must accumulate UserDisconnected"
        );
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    fn drain_pending_user_cache_refreshed_overwrites() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, _console_tx) = test_console();
        let mut source = UnifiedEventSource {
            rx,
            tx: tx.clone(),
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            workspace_entered: Vec::new(),
            user_connected: Vec::new(),
            user_disconnected: Vec::new(),
            user_resized: Vec::new(),
            user_cache_refreshed: None,
        };

        let info1 = make_user_info(1);
        let info2 = make_user_info(2);
        tx.send(UnifiedEvent::UserCacheRefreshed(vec![info1]))
            .unwrap();
        tx.send(UnifiedEvent::UserCacheRefreshed(vec![info2]))
            .unwrap();
        source.drain_pending();
        let cached = source
            .user_cache_refreshed
            .as_ref()
            .expect("must be Some after UserCacheRefreshed");
        assert_eq!(
            cached.len(),
            1,
            "UserCacheRefreshed must overwrite, not append"
        );
        assert_eq!(cached[0].conn_id, 2, "must keep the last value");
    }

    #[test]
    #[cfg(feature = "session-persistence")]
    fn drain_pending_mixed_session_events() {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        let (console, console_rx, _console_tx) = test_console();
        let mut source = UnifiedEventSource {
            rx,
            tx: tx.clone(),
            console: Some(console),
            console_rx: Some(console_rx),
            console_alive: true,
            dirty_windows: HashSet::new(),
            exited_windows: Vec::new(),
            direct_input_changed: Vec::new(),
            pending_redraw: false,
            event_owner: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_event: None,
            input_buffer: VecDeque::new(),
            signal_received: false,
            normalizer: KeyboardNormalizer::new(),
            last_event_at: None,
            frame_pacer: FramePacer::new(),
            pending_work: false,
            max_sleep_duration: None,
            workspace_entered: Vec::new(),
            user_connected: Vec::new(),
            user_disconnected: Vec::new(),
            user_resized: Vec::new(),
            user_cache_refreshed: None,
        };

        tx.send(UnifiedEvent::WorkspaceEntered("ws1".into()))
            .unwrap();
        tx.send(UnifiedEvent::UserConnected(make_user_info(1)))
            .unwrap();
        tx.send(UnifiedEvent::UserDisconnected(2)).unwrap();
        tx.send(UnifiedEvent::UserCacheRefreshed(vec![make_user_info(3)]))
            .unwrap();
        source.drain_pending();
        assert_eq!(source.workspace_entered, vec!["ws1"]);
        assert_eq!(source.user_connected.len(), 1);
        assert_eq!(source.user_disconnected, vec![2]);
        assert!(source.user_cache_refreshed.is_some());
    }
}
