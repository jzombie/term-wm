use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, bounded};

/// Capacity of the channel between `reaper.reap()` calls and the background
/// reaper thread.
const REAPER_CHANNEL_CAPACITY: usize = 64;

/// How long the reaper thread blocks when there are no zombies (essentially
/// idle sleep — the thread serves only as a canary).
const REAPER_IDLE_TIMEOUT: Duration = Duration::from_secs(3600);

/// How aggressively the reaper polls when zombies are present (50 ms).
const REAPER_ACTIVE_TICK: Duration = Duration::from_millis(50);

/// Grace period before escalating from SIGHUP to SIGKILL.
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// A zombie child process and its reader thread, moved out of a closed `Window`.
///
/// The `Reaper` owns these and periodically polls `try_wait()` to reap
/// them asynchronously, avoiding blocking `Drop`.
pub struct ZombieChild {
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    reader_handle: Option<JoinHandle<()>>,
    sighup_sent: bool,
    sigkill_sent: bool,
    killed_at: Option<Instant>,
}

impl ZombieChild {
    pub fn new(
        child: Box<dyn portable_pty::Child + Send + Sync>,
        reader_handle: JoinHandle<()>,
    ) -> Self {
        Self {
            child: Some(child),
            reader_handle: Some(reader_handle),
            sighup_sent: false,
            sigkill_sent: false,
            killed_at: None,
        }
    }
}

/// Async process reaper running on a dedicated background thread.
///
/// The main thread sends `ZombieChild` values via a bounded channel.
/// The reaper thread blocks on `recv_timeout()` with a dynamic timeout:
/// - Indefinitely (3600s) when no zombies exist — zero CPU wakeups.
/// - 50ms tick rate when there are active zombies — responsive reaping.
///
/// On Drop, the reaper thread is signalled to shut down. It force-kills
/// all remaining children and joins all reader threads.
pub struct Reaper {
    tx: Sender<ZombieChild>,
    _handle: JoinHandle<()>,
    shutdown: Arc<AtomicBool>,
}

impl Reaper {
    pub fn new(shutdown_timeout: Duration) -> Self {
        let (tx, rx) = bounded::<ZombieChild>(REAPER_CHANNEL_CAPACITY);
        let shutdown = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&shutdown);
        let _handle = thread::Builder::new()
            .name("pty-reaper".into())
            .spawn(move || {
                let mut zombies: Vec<ZombieChild> = Vec::new();
                loop {
                    if s.load(Ordering::Acquire) {
                        break;
                    }

                    // Dynamic timeout: block indefinitely if no zombies,
                    // poll at 50ms if zombies exist (non-blocking)
                    let timeout = if zombies.is_empty() {
                        REAPER_IDLE_TIMEOUT
                    } else {
                        REAPER_ACTIVE_TICK
                    };
                    if let Ok(z) = rx.recv_timeout(timeout) {
                        zombies.push(z);
                        while let Ok(z) = rx.try_recv() {
                            zombies.push(z);
                        }
                    }

                    // Tick — same lifecycle logic as the old Reaper::tick()
                    zombies.retain_mut(|z| {
                        if !z.sighup_sent {
                            if let Some(ref mut child) = z.child {
                                let _ = child.kill();
                            }
                            z.sighup_sent = true;
                            z.killed_at = Some(Instant::now());
                            return true;
                        }
                        if let Some(ref mut child) = z.child
                            && child.try_wait().ok().flatten().is_some()
                        {
                            drop(z.child.take());
                            if let Some(handle) = z.reader_handle.take() {
                                let _ = handle.join();
                            }
                            return false;
                        }
                        if let Some(killed_at) = z.killed_at
                            && !z.sigkill_sent
                            && killed_at.elapsed() >= shutdown_timeout
                        {
                            if let Some(ref mut child) = z.child {
                                let _ = child.kill();
                            }
                            z.sigkill_sent = true;
                        }
                        true
                    });
                }
                // drain_all on shutdown
                while let Ok(z) = rx.try_recv() {
                    zombies.push(z);
                }
                for mut z in zombies.drain(..) {
                    if let Some(ref mut child) = z.child {
                        let _ = child.kill();
                    }
                    drop(z.child.take());
                    if let Some(handle) = z.reader_handle.take() {
                        let _ = handle.join();
                    }
                }
            })
            .expect("reaper thread");
        Self {
            tx,
            _handle,
            shutdown,
        }
    }

    /// Take ownership of a child process and its reader thread for async reaping.
    /// Uses `&self` (not `&mut`) — channel send does not require mutable access.
    pub fn reap(&self, zombie: ZombieChild) {
        let _ = self.tx.send(zombie);
    }
}

impl Default for Reaper {
    fn default() -> Self {
        Self::new(DEFAULT_SHUTDOWN_TIMEOUT)
    }
}

impl Drop for Reaper {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use portable_pty::{Child, ChildKiller, ExitStatus};
    use std::io;
    use std::time::Duration;

    #[derive(Debug)]
    struct MockChild {
        killed: bool,
        exited: bool,
        exit_status: Option<ExitStatus>,
    }

    impl MockChild {
        fn new() -> Self {
            Self {
                killed: false,
                exited: false,
                exit_status: None,
            }
        }
    }

    impl ChildKiller for MockChild {
        fn kill(&mut self) -> io::Result<()> {
            self.killed = true;
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(MockChild::new())
        }
    }

    impl Child for MockChild {
        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            if self.exited {
                Ok(Some(
                    self.exit_status
                        .take()
                        .unwrap_or(ExitStatus::with_exit_code(0)),
                ))
            } else {
                Ok(None)
            }
        }
        fn wait(&mut self) -> io::Result<ExitStatus> {
            Ok(ExitStatus::with_exit_code(0))
        }
        fn process_id(&self) -> Option<u32> {
            Some(42)
        }

        #[cfg(windows)]
        fn as_raw_handle(&self) -> Option<*mut std::ffi::c_void> {
            None
        }
    }

    fn dummy_handle() -> JoinHandle<()> {
        thread::spawn(|| {})
    }

    fn make_zombie() -> ZombieChild {
        ZombieChild::new(Box::new(MockChild::new()), dummy_handle())
    }

    #[test]
    fn new_creates_reaper() {
        let r = Reaper::new(Duration::from_secs(5));
        // Reaper should be usable — verify send doesn't panic
        r.reap(make_zombie());
    }

    #[test]
    fn default_reaper_created() {
        let r = Reaper::default();
        r.reap(make_zombie());
    }

    /// reap can be called with &self (no &mut needed).
    #[test]
    fn reap_immutable_ref() {
        let r = Reaper::new(Duration::from_secs(5));
        r.reap(make_zombie());
        // sending multiple zombies should also work
        r.reap(make_zombie());
        r.reap(make_zombie());
    }

    #[test]
    fn reaper_responds_to_shutdown() {
        let r = Reaper::new(Duration::from_secs(5));
        r.reap(make_zombie());
        // Drop will trigger shutdown — just verify it doesn't hang
        drop(r);
    }
}
