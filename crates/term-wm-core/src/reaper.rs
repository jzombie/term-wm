use std::time::{Duration, Instant};

/// A zombie child process and its reader thread, moved out of a closed `Window`.
///
/// The `Reaper` owns these and periodically polls `try_wait()` to reap
/// them asynchronously, avoiding blocking `Drop`.
pub struct ZombieChild {
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    reader_handle: Option<std::thread::JoinHandle<()>>,
    sighup_sent: bool,
    sigkill_sent: bool,
    killed_at: Option<Instant>,
}

impl ZombieChild {
    pub fn new(
        child: Box<dyn portable_pty::Child + Send + Sync>,
        reader_handle: std::thread::JoinHandle<()>,
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

/// Non-blocking async process reaper.
///
/// Called every idle tick (~16ms).  On each tick:
/// 1. Sends SIGHUP (via `Child::kill()`) on first tick.
/// 2. Calls `try_wait()` — if child exited, close FD, join reader thread.
/// 3. If timeout elapsed since SIGHUP, sends SIGKILL.
///
/// Full reaping lifecycle:
///   sighup_sent=false → send SIGHUP, start timer
///   try_wait()=Some(status) → close FD → join reader → remove from list
///   timeout && !sigkill_sent → send SIGKILL
pub struct Reaper {
    zombies: Vec<ZombieChild>,
    shutdown_timeout: Duration,
}

impl Reaper {
    pub fn new(shutdown_timeout: Duration) -> Self {
        Self {
            zombies: Vec::new(),
            shutdown_timeout,
        }
    }

    /// Take ownership of a child process and its reader thread for async reaping.
    pub fn reap(&mut self, zombie: ZombieChild) {
        self.zombies.push(zombie);
    }

    /// Non-blocking poll — call every idle tick.
    /// Returns the number of zombies still being reaped (0 = all done).
    pub fn tick(&mut self) -> usize {
        let timeout = self.shutdown_timeout;
        self.zombies.retain_mut(|z| {
            // Step 1: If we haven't sent SIGHUP yet, send it.
            if !z.sighup_sent {
                if let Some(ref mut child) = z.child {
                    let _ = child.kill();
                }
                z.sighup_sent = true;
                z.killed_at = Some(Instant::now());
                return true; // keep — give it time
            }

            // Step 2: Try to reap — non-blocking try_wait().
            if let Some(ref mut child) = z.child {
                if child.try_wait().ok().flatten().is_some() {
                    // Child exited — drop child handle to close resources.
                    drop(z.child.take());
                    // The reader thread will unblock once the FD is closed.
                    // Since we dropped the child (and portable-pty closes
                    // the master FD when the last handle drops), the reader
                    // should get EIO/EBADF and exit.
                    if let Some(handle) = z.reader_handle.take() {
                        let _ = handle.join();
                    }
                    return false; // fully reaped
                }
            }

            // Step 3: If timeout elapsed and we haven't sent SIGKILL, force-kill.
            if let Some(killed_at) = z.killed_at {
                if !z.sigkill_sent && killed_at.elapsed() >= timeout {
                    if let Some(ref mut child) = z.child {
                        let _ = child.kill(); // portable-pty kill sends SIGKILL on second call
                    }
                    z.sigkill_sent = true;
                }
            }

            true // still waiting
        });
        self.zombies.len()
    }

    /// Blocking drain — called during app exit.
    /// Force-kills all remaining children and joins all reader threads.
    pub fn drain_all(&mut self) {
        for mut z in self.zombies.drain(..) {
            if let Some(ref mut child) = z.child {
                let _ = child.kill();
            }
            // Drop child handle to close FDs; reader thread unblocks.
            drop(z.child.take());
            if let Some(handle) = z.reader_handle.take() {
                let _ = handle.join();
            }
        }
    }
}

impl Default for Reaper {
    fn default() -> Self {
        Self::new(Duration::from_secs(3))
    }
}
