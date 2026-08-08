use std::io;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded};

use term_wm_core::events::Event;
use term_wm_crossterm_adapter;

/// Capacity of the channel between the background crossterm reader thread and
/// consumers (standalone use, or the root `UnifiedEventSource` multiplexer).
const CONSOLE_EVENT_CHANNEL_CAPACITY: usize = 256;

/// How often the background thread polls crossterm (100 ms) so it stays
/// responsive to the shutdown flag while idle-friendly.
const CROSSTERM_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// A background-thread crossterm reader used as a dumb conduit.
///
/// The reader thread polls `crossterm::event::poll`, translates each event via
/// `term_wm_crossterm_adapter::try_translate_event`, and forwards RAW
/// translated core `Event`s on a bounded crossbeam channel. It holds NO state:
/// no normalization, no queues, no power-profile bookkeeping — the consumer
/// (e.g. the root `UnifiedEventSource`) owns all of that. This is the only
/// place a background thread reads crossterm in production.
///
/// This reader and the synchronous [`crate::console_event_source::ConsoleEventSource`]
/// are mutually exclusive at runtime (binary path uses this reader via the
/// unified source; the standalone `TermWmApp` path uses `ConsoleEventSource`).
pub struct BackgroundConsoleReader {
    rx: Receiver<Event>,
    shutdown: Arc<AtomicBool>,
    /// Kept alive so the thread is joined-able; the actual shutdown is driven
    /// by the `Drop` implementation setting the flag.
    #[expect(dead_code)]
    handle: JoinHandle<()>,
}

impl BackgroundConsoleReader {
    /// Spawn the background crossterm reader thread and return the conduit.
    pub fn new() -> io::Result<Self> {
        let (tx, rx) = bounded::<Event>(CONSOLE_EVENT_CHANNEL_CAPACITY);
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = Arc::clone(&shutdown);
        let handle = thread::Builder::new()
            .name("crossterm-input".into())
            .spawn(move || {
                input_loop(tx, thread_shutdown);
            })
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(Self {
            rx,
            shutdown,
            handle,
        })
    }

    /// Build from an existing receiver + handle. Primarily for tests and
    /// embedders supplying their own reader thread.
    #[doc(hidden)]
    pub fn from_receiver(
        rx: Receiver<Event>,
        handle: JoinHandle<()>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            rx,
            shutdown,
            handle,
        }
    }

    /// Clone of the receive side so a multiplexer (e.g. the root
    /// `UnifiedEventSource`) can `select!` on console input.
    pub fn receiver(&self) -> Receiver<Event> {
        self.rx.clone()
    }

    /// Enable/disable crossterm mouse capture. Delegates to the adapter, the
    /// single canonical implementation.
    pub fn set_mouse_capture(&self, enabled: bool) -> io::Result<()> {
        term_wm_crossterm_adapter::set_mouse_capture(enabled)
    }
}

impl Drop for BackgroundConsoleReader {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
    }
}

/// The background thread body: poll crossterm, translate, forward raw events.
fn input_loop(tx: Sender<Event>, shutdown: Arc<AtomicBool>) {
    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        match crossterm::event::poll(CROSSTERM_POLL_INTERVAL) {
            Ok(true) => {
                match crossterm::event::read() {
                    Ok(evt) => {
                        // NO normalization here — the consumer normalizes.
                        let Some(core_evt) = term_wm_crossterm_adapter::try_translate_event(evt)
                        else {
                            continue; // unrecognized key — drop the event
                        };
                        if tx.send(core_evt).is_err() {
                            break; // receiver dropped
                        }
                    }
                    Err(_) => break,
                }
            }
            Ok(false) => {} // timeout elapsed — loop and check shutdown
            Err(_) => break, // TTY broken — kill the thread, avoid a CPU spinlock
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_wm_core::events::{KeyCode, KeyEvent, KeyKind, KeyModifiers};

    fn make_event(code: KeyCode) -> Event {
        Event::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        })
    }

    /// Build a reader over an injected channel (no-op thread handle) so the
    /// wiring can be tested without a real terminal.
    fn injected_reader() -> (Sender<Event>, BackgroundConsoleReader, Receiver<Event>) {
        let (tx, rx) = bounded(16);
        let handle = thread::spawn(|| {});
        let src =
            BackgroundConsoleReader::from_receiver(rx, handle, Arc::new(AtomicBool::new(false)));
        let rcv = src.receiver();
        (tx, src, rcv)
    }

    #[test]
    fn receiver_and_injected_events_flow() {
        let (tx, _src, rcv) = injected_reader();
        tx.send(make_event(KeyCode::Char('a'))).unwrap();
        let evt = rcv.recv().unwrap();
        assert!(matches!(evt, Event::Key(_)));
    }

    #[test]
    fn receiver_disconnects_when_all_senders_dropped() {
        let (tx, _src, rcv) = injected_reader();
        drop(tx);
        assert!(rcv.recv().is_err());
    }
}
