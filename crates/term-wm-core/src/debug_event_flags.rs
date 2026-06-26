use std::sync::atomic::{AtomicBool, Ordering};

static PANIC_PENDING: AtomicBool = AtomicBool::new(false);
static ERROR_PENDING: AtomicBool = AtomicBool::new(false);

pub fn take_panic_pending() -> bool {
    PANIC_PENDING.swap(false, Ordering::SeqCst)
}

pub fn take_error_pending() -> bool {
    ERROR_PENDING.swap(false, Ordering::SeqCst)
}

pub fn trigger_panic_pending() {
    PANIC_PENDING.store(true, Ordering::SeqCst);
}

pub fn trigger_error_pending() {
    ERROR_PENDING.store(true, Ordering::SeqCst);
}
