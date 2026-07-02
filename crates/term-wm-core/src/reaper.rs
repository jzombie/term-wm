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
            if let Some(ref mut child) = z.child
                && child.try_wait().ok().flatten().is_some()
            {
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

            // Step 3: If timeout elapsed and we haven't sent SIGKILL, force-kill.
            if let Some(killed_at) = z.killed_at
                && !z.sigkill_sent
                && killed_at.elapsed() >= timeout
            {
                if let Some(ref mut child) = z.child {
                    let _ = child.kill(); // portable-pty kill sends SIGKILL on second call
                }
                z.sigkill_sent = true;
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

#[cfg(test)]
mod tests {
    use super::*;
    use portable_pty::{Child, ChildKiller, ExitStatus};
    use std::io;
    use std::thread;

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
    }

    fn dummy_handle() -> thread::JoinHandle<()> {
        thread::spawn(|| {})
    }

    #[test]
    fn new_creates_empty_reaper() {
        let r = Reaper::new(Duration::from_secs(5));
        assert!(r.zombies.is_empty());
        assert_eq!(r.shutdown_timeout, Duration::from_secs(5));
    }

    #[test]
    fn default_reaper_has_three_second_timeout() {
        let r = Reaper::default();
        assert_eq!(r.shutdown_timeout, Duration::from_secs(3));
    }

    #[test]
    fn reap_adds_zombie() {
        let mut r = Reaper::new(Duration::from_secs(5));
        let child = Box::new(MockChild::new());
        let z = ZombieChild::new(child, dummy_handle());
        r.reap(z);
        assert_eq!(r.zombies.len(), 1);
    }

    #[test]
    fn tick_sends_sighup_on_first_call() {
        let mut r = Reaper::new(Duration::from_secs(5));
        let child = Box::new(MockChild::new());
        let z = ZombieChild::new(child, dummy_handle());
        r.reap(z);

        let remaining = r.tick();
        assert_eq!(remaining, 1);
        let z = &r.zombies[0];
        assert!(z.sighup_sent);
        assert!(z.killed_at.is_some());
    }

    #[test]
    fn tick_reaps_when_child_exits() {
        let mut r = Reaper::new(Duration::from_secs(5));
        let mut child = Box::new(MockChild::new());
        child.exited = true;
        child.exit_status = Some(ExitStatus::with_exit_code(0));
        let z = ZombieChild::new(child, dummy_handle());
        r.reap(z);

        // First tick sends SIGHUP and keeps zombie
        let remaining = r.tick();
        assert_eq!(remaining, 1);

        // Second tick sees child exited and reaps it
        let remaining = r.tick();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn tick_sends_sigkill_after_timeout() {
        let mut r = Reaper::new(Duration::from_millis(1));
        let child = Box::new(MockChild::new());
        let z = ZombieChild::new(child, dummy_handle());
        r.reap(z);

        // First tick: send SIGHUP
        r.tick();
        thread::sleep(Duration::from_millis(5));

        // Second tick: timeout elapsed, should send SIGKILL
        let remaining = r.tick();
        let z = &r.zombies[0];
        assert!(
            z.sigkill_sent,
            "sigkill should have been sent after timeout"
        );
        assert_eq!(remaining, 1);
    }

    #[test]
    fn tick_sends_sigkill_only_once() {
        let mut r = Reaper::new(Duration::from_millis(1));
        let child = Box::new(MockChild::new());
        let z = ZombieChild::new(child, dummy_handle());
        r.reap(z);

        r.tick(); // SIGHUP
        thread::sleep(Duration::from_millis(5));
        r.tick(); // SIGKILL
        let sigkill_count_before = r.zombies.iter().filter(|z| z.sigkill_sent).count();

        // Third tick — should NOT send SIGKILL again
        r.tick();
        let sigkill_count_after = r.zombies.iter().filter(|z| z.sigkill_sent).count();
        assert_eq!(sigkill_count_after, sigkill_count_before);
    }

    #[test]
    fn tick_handles_no_zombies() {
        let mut r = Reaper::new(Duration::from_secs(5));
        assert_eq!(r.tick(), 0);
    }

    #[test]
    fn drain_all_kills_and_removes_all() {
        let mut r = Reaper::new(Duration::from_secs(5));
        for _ in 0..3 {
            let child = Box::new(MockChild::new());
            let z = ZombieChild::new(child, dummy_handle());
            r.reap(z);
        }
        assert_eq!(r.zombies.len(), 3);

        r.drain_all();
        assert_eq!(r.zombies.len(), 0);
    }

    #[test]
    fn drain_all_on_empty_reaper() {
        let mut r = Reaper::new(Duration::from_secs(5));
        r.drain_all(); // should not panic
        assert_eq!(r.zombies.len(), 0);
    }
}
