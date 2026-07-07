use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A handle for monitoring SIGINT (Ctrl-C) signals delivered to the process.
///
/// The signal handler only sets a flag — no I/O is performed in signal
/// context.  Check `received()` periodically and call `ack()` after handling.
pub struct SigintHandle {
    flag: Arc<AtomicBool>,
}

impl SigintHandle {
    /// Returns `true` if SIGINT was received since the last `ack()`.
    pub fn received(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }

    /// Acknowledge (clear) the signal flag.
    pub fn ack(&self) {
        self.flag.store(false, Ordering::Release);
    }
}

/// Install a SIGINT handler that sets a flag instead of terminating.
///
/// The returned [`SigintHandle`] lets event loops check for and acknowledge
/// the signal without performing I/O in signal context.
pub fn install_sigint_handler() -> std::io::Result<SigintHandle> {
    let flag = Arc::new(AtomicBool::new(false));
    let f = Arc::clone(&flag);
    ctrlc::set_handler(move || {
        f.store(true, Ordering::Release);
    })
    .map_err(std::io::Error::other)?;
    Ok(SigintHandle { flag })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigint_handle_initial_state() {
        let handle = SigintHandle {
            flag: Arc::new(AtomicBool::new(false)),
        };
        assert!(!handle.received());
    }

    #[test]
    fn sigint_handle_set_flag() {
        let handle = SigintHandle {
            flag: Arc::new(AtomicBool::new(true)),
        };
        assert!(handle.received());
    }

    #[test]
    fn sigint_handle_ack_clears_flag() {
        let handle = SigintHandle {
            flag: Arc::new(AtomicBool::new(true)),
        };
        handle.ack();
        assert!(!handle.received());
    }

    #[test]
    fn sigint_handle_ack_noop_when_not_set() {
        let handle = SigintHandle {
            flag: Arc::new(AtomicBool::new(false)),
        };
        handle.ack();
        assert!(!handle.received());
    }

    #[test]
    fn sigint_handle_shared_flag() {
        let flag = Arc::new(AtomicBool::new(false));
        let handle = SigintHandle { flag: flag.clone() };
        assert!(!handle.received());
        flag.store(true, Ordering::Release);
        assert!(handle.received());
        handle.ack();
        assert!(!flag.load(Ordering::Acquire));
    }

    #[test]
    fn install_sigint_handler_succeeds() {
        let result = install_sigint_handler();
        assert!(result.is_ok());
    }
}
