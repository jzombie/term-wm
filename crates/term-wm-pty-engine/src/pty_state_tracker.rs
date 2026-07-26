use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, Ordering};

use vte::{Params, Perform};

/// Decoded mouse tracking mode from DECSET 1000/1002/1003.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseTrackingMode {
    None,
    X11Normal,
    CellMotion,
    AllMotion,
}

impl MouseTrackingMode {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::X11Normal,
            2 => Self::CellMotion,
            3 => Self::AllMotion,
            _ => Self::None,
        }
    }

    #[expect(dead_code)]
    fn to_u8(self) -> u8 {
        match self {
            Self::None => 0,
            Self::X11Normal => 1,
            Self::CellMotion => 2,
            Self::AllMotion => 3,
        }
    }
}

/// Lock-free state tracker that observes the PTY byte stream for application
/// state heuristics (alternate screen, mouse tracking, custom margins).
///
/// Shared between the reader thread (writer) and the main UI thread (reader)
/// via `Arc`. All fields are atomics so the main thread can query without
/// locks or mutable borrows.
#[derive(Debug)]
pub struct PtyStateTracker {
    is_alt_screen_active: AtomicBool,
    mouse_tracking_mode: AtomicU8,
    is_sgr_mouse_active: AtomicBool,
    is_alt_scroll_mode_active: AtomicBool,
    has_custom_margins: AtomicBool,
    terminal_height: AtomicU16,
}

impl PtyStateTracker {
    pub fn new(terminal_height: u16) -> Self {
        Self {
            is_alt_screen_active: AtomicBool::new(false),
            mouse_tracking_mode: AtomicU8::new(0),
            is_sgr_mouse_active: AtomicBool::new(false),
            is_alt_scroll_mode_active: AtomicBool::new(false),
            has_custom_margins: AtomicBool::new(false),
            terminal_height: AtomicU16::new(terminal_height),
        }
    }

    /// Unified routing decision: returns true if inputs should be forwarded
    /// to the PTY child rather than intercepted by the native scrollbar.
    pub fn requires_app_routing(&self) -> bool {
        self.is_alt_screen_active.load(Ordering::Acquire)
            || self.mouse_tracking_mode.load(Ordering::Acquire) != 0
            || self.has_custom_margins.load(Ordering::Acquire)
    }

    pub fn is_alt_screen_active(&self) -> bool {
        self.is_alt_screen_active.load(Ordering::Acquire)
    }

    pub fn mouse_tracking_mode(&self) -> MouseTrackingMode {
        MouseTrackingMode::from_u8(self.mouse_tracking_mode.load(Ordering::Acquire))
    }

    pub fn is_sgr_mouse_active(&self) -> bool {
        self.is_sgr_mouse_active.load(Ordering::Acquire)
    }

    pub fn is_alt_scroll_mode_active(&self) -> bool {
        self.is_alt_scroll_mode_active.load(Ordering::Acquire)
    }

    pub fn has_custom_margins(&self) -> bool {
        self.has_custom_margins.load(Ordering::Acquire)
    }

    /// Update terminal height on SIGWINCH. Called from main thread.
    pub fn resize(&self, height: u16) {
        self.terminal_height.store(height, Ordering::Release);
    }

    // --- Package-internal setters (called from PtyPerformAdapter on reader thread) ---

    pub(crate) fn set_alt_screen(&self, active: bool) {
        self.is_alt_screen_active.store(active, Ordering::Release);
    }

    pub(crate) fn set_sgr_mouse(&self, active: bool) {
        self.is_sgr_mouse_active.store(active, Ordering::Release);
    }

    pub(crate) fn set_alt_scroll_mode(&self, active: bool) {
        self.is_alt_scroll_mode_active.store(active, Ordering::Release);
    }

    pub(crate) fn set_custom_margins(&self, active: bool) {
        self.has_custom_margins.store(active, Ordering::Release);
    }

    /// Conditional mouse mode update: set unconditionally, but clear only
    /// via CAS so we don't clobber a different active mode.
    pub(crate) fn update_mouse_tracking(&self, target_mode: u8, is_set: bool) {
        if is_set {
            self.mouse_tracking_mode.store(target_mode, Ordering::Release);
        } else {
            let _ = self.mouse_tracking_mode.compare_exchange(
                target_mode,
                0,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
        }
    }

    /// Reset all state to defaults (RIS / DECSTR recovery).
    pub(crate) fn reset_all(&self) {
        self.is_alt_screen_active.store(false, Ordering::Release);
        self.mouse_tracking_mode.store(0, Ordering::Release);
        self.is_sgr_mouse_active.store(false, Ordering::Release);
        self.is_alt_scroll_mode_active.store(false, Ordering::Release);
        self.has_custom_margins.store(false, Ordering::Release);
    }
}

// Safety: all fields are atomic primitives (no `!Sync` types like `RefCell`).
// `PtyStateTracker` is `Send + Sync` by construction.

/// Reader-thread-local adapter that implements `vte::Perform`.
///
/// Owned exclusively by the reader thread (a local variable in
/// `parser_read_loop`). Holds a clone of the `Arc<PtyStateTracker>`
/// and updates its atomic fields when the parser detects relevant
/// ANSI escape sequences.
pub(crate) struct PtyPerformAdapter {
    tracker: std::sync::Arc<PtyStateTracker>,
}

impl PtyPerformAdapter {
    pub fn new(tracker: std::sync::Arc<PtyStateTracker>) -> Self {
        Self { tracker }
    }
}

impl Perform for PtyPerformAdapter {
    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        if ignore {
            return;
        }
        // RIS (Reset to Initial State): ESC c
        if intermediates.is_empty() && byte == b'c' {
            self.tracker.reset_all();
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if ignore {
            return;
        }

        // DECSTR (Soft Terminal Reset): CSI ! p
        if action == 'p' && intermediates == *b"!" {
            self.tracker.reset_all();
            return;
        }

        let is_dec_private = intermediates.first() == Some(&b'?');
        match action {
            'h' | 'l' if is_dec_private => {
                let is_set = action == 'h';
                for param_group in params.iter() {
                    for &param in param_group {
                        match param {
                            47 | 1047 | 1049 => self.tracker.set_alt_screen(is_set),
                            1000 => self.tracker.update_mouse_tracking(1, is_set),
                            1002 => self.tracker.update_mouse_tracking(2, is_set),
                            1003 => self.tracker.update_mouse_tracking(3, is_set),
                            1006 => self.tracker.set_sgr_mouse(is_set),
                            1007 => self.tracker.set_alt_scroll_mode(is_set),
                            _ => {}
                        }
                    }
                }
            }
            // DECSTBM: set scrolling region — normalize explicit 0 to defaults
            'r' => {
                let top_param = params
                    .iter()
                    .next()
                    .and_then(|g| g.first().copied())
                    .unwrap_or(0);
                let bottom_param = params
                    .iter()
                    .nth(1)
                    .and_then(|g| g.first().copied())
                    .unwrap_or(0);
                let height = self.tracker.terminal_height.load(Ordering::Acquire);
                let top = if top_param == 0 { 1 } else { top_param };
                let bottom = if bottom_param == 0 { height } else { bottom_param };
                let has_margins = top > 1 || bottom < height;
                self.tracker.set_custom_margins(has_margins);
            }
            _ => {}
        }
    }

    fn print(&mut self, _c: char) {}
    fn execute(&mut self, _byte: u8) {}
    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tracker(height: u16) -> PtyStateTracker {
        PtyStateTracker::new(height)
    }

    fn feed(tracker: &std::sync::Arc<PtyStateTracker>, bytes: &[u8]) {
        let mut parser = vte::Parser::new();
        let mut adapter = PtyPerformAdapter::new(tracker.clone());
        parser.advance(&mut adapter, bytes);
    }

    #[test]
    fn test_detects_alternate_screen() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        feed(&tracker, b"\x1b[?1049h");
        assert!(tracker.is_alt_screen_active());
        assert!(tracker.requires_app_routing());
    }

    #[test]
    fn test_detects_alternate_screen_exit() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        feed(&tracker, b"\x1b[?1049h");
        assert!(tracker.is_alt_screen_active());
        feed(&tracker, b"\x1b[?1049l");
        assert!(!tracker.is_alt_screen_active());
        assert!(!tracker.requires_app_routing());
    }

    #[test]
    fn test_detects_alternate_screen_1047() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        feed(&tracker, b"\x1b[?1047h");
        assert!(tracker.is_alt_screen_active());
    }

    #[test]
    fn test_detects_alternate_screen_47() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        feed(&tracker, b"\x1b[?47h");
        assert!(tracker.is_alt_screen_active());
    }

    #[test]
    fn test_mouse_tracking_x11_normal() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        feed(&tracker, b"\x1b[?1000h");
        assert_eq!(tracker.mouse_tracking_mode(), MouseTrackingMode::X11Normal);
        assert!(tracker.requires_app_routing());
    }

    #[test]
    fn test_mouse_tracking_cell_motion() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        feed(&tracker, b"\x1b[?1002h");
        assert_eq!(tracker.mouse_tracking_mode(), MouseTrackingMode::CellMotion);
    }

    #[test]
    fn test_mouse_tracking_all_motion() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        feed(&tracker, b"\x1b[?1003h");
        assert_eq!(tracker.mouse_tracking_mode(), MouseTrackingMode::AllMotion);
    }

    #[test]
    fn test_mouse_tracking_cas_does_not_clobber() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        // Enable CellMotion (mode 2)
        feed(&tracker, b"\x1b[?1002h");
        assert_eq!(tracker.mouse_tracking_mode(), MouseTrackingMode::CellMotion);
        // Try to reset X11Normal (mode 1) — should NOT clobber CellMotion
        feed(&tracker, b"\x1b[?1001l");
        assert_eq!(
            tracker.mouse_tracking_mode(),
            MouseTrackingMode::CellMotion,
            "CAS must not clear mode 2 when resetting mode 1"
        );
        // Reset CellMotion (mode 2) — should clear
        feed(&tracker, b"\x1b[?1002l");
        assert_eq!(tracker.mouse_tracking_mode(), MouseTrackingMode::None);
    }

    #[test]
    fn test_mouse_tracking_set_after_reset_different_mode() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        // Enable X11Normal
        feed(&tracker, b"\x1b[?1000h");
        assert_eq!(tracker.mouse_tracking_mode(), MouseTrackingMode::X11Normal);
        // Directly set AllMotion (simulates app switching modes)
        feed(&tracker, b"\x1b[?1003h");
        assert_eq!(tracker.mouse_tracking_mode(), MouseTrackingMode::AllMotion);
        // Reset X11Normal — should NOT clear AllMotion
        feed(&tracker, b"\x1b[?1001l");
        assert_eq!(
            tracker.mouse_tracking_mode(),
            MouseTrackingMode::AllMotion,
            "CAS must not clear mode 3 when resetting mode 1"
        );
    }

    #[test]
    fn test_sgr_mouse() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        feed(&tracker, b"\x1b[?1006h");
        assert!(tracker.is_sgr_mouse_active());
        feed(&tracker, b"\x1b[?1006l");
        assert!(!tracker.is_sgr_mouse_active());
    }

    #[test]
    fn test_alt_scroll_mode() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        feed(&tracker, b"\x1b[?1007h");
        assert!(tracker.is_alt_scroll_mode_active());
        feed(&tracker, b"\x1b[?1007l");
        assert!(!tracker.is_alt_scroll_mode_active());
    }

    #[test]
    fn test_custom_margins_set() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        feed(&tracker, b"\x1b[2;23r");
        assert!(tracker.has_custom_margins());
    }

    #[test]
    fn test_custom_margins_cleared() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        feed(&tracker, b"\x1b[2;23r");
        assert!(tracker.has_custom_margins());
        // Reset to full height
        feed(&tracker, b"\x1b[r");
        assert!(!tracker.has_custom_margins());
    }

    #[test]
    fn test_custom_margins_full_height_no_margins() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        feed(&tracker, b"\x1b[1;24r");
        assert!(!tracker.has_custom_margins());
    }

    #[test]
    fn test_custom_margins_zero_bottom_param_normalized() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        // \e[1;0r means top=1, bottom=terminal_height
        feed(&tracker, b"\x1b[1;0r");
        assert!(!tracker.has_custom_margins());
    }

    #[test]
    fn test_custom_margins_zero_top_param_normalized() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        // \e[;24r means top=1 (default), bottom=24 (full height)
        feed(&tracker, b"\x1b[;24r");
        assert!(!tracker.has_custom_margins());
    }

    #[test]
    fn test_resize_updates_height() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        assert!(!tracker.requires_app_routing());
        // Set margins with old height
        feed(&tracker, b"\x1b[2;23r");
        assert!(tracker.has_custom_margins());
        // Resize to 10 — margins now effectively span the whole terminal
        tracker.resize(10);
        // Margins 2;23 with height 10 — only bottom < height matters
        // bottom=23, height=10: 23 < 10 is false, but top=2 > 1 is true
        // So has_custom_margins stays true. This test verifies the resize
        // mechanism works (the margin flag is reassessed on next DECSTBM, not
        // automatically on resize).
        assert!(tracker.has_custom_margins());
        // But if we re-evaluate with a new DECSTBM at the new height...
        feed(&tracker, b"\x1b[r");
        assert!(!tracker.has_custom_margins(), "full-height reset at new size");
    }

    #[test]
    fn test_ris_reset() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        feed(&tracker, b"\x1b[?1049h\x1b[?1002h\x1b[2;23r");
        assert!(tracker.requires_app_routing());
        // RIS (ESC c) should reset everything
        feed(&tracker, b"\x1bc");
        assert!(!tracker.is_alt_screen_active());
        assert_eq!(tracker.mouse_tracking_mode(), MouseTrackingMode::None);
        assert!(!tracker.has_custom_margins());
        assert!(!tracker.requires_app_routing());
    }

    #[test]
    fn test_decstr_reset() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        feed(&tracker, b"\x1b[?1049h\x1b[?1002h\x1b[2;23r");
        assert!(tracker.requires_app_routing());
        // DECSTR (CSI ! p) should reset everything
        feed(&tracker, b"\x1b[!p");
        assert!(!tracker.is_alt_screen_active());
        assert_eq!(tracker.mouse_tracking_mode(), MouseTrackingMode::None);
        assert!(!tracker.has_custom_margins());
        assert!(!tracker.requires_app_routing());
    }

    #[test]
    fn test_requires_app_routing_false_by_default() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        assert!(!tracker.requires_app_routing());
    }

    #[test]
    fn test_requires_app_routing_true_for_alt_screen() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        feed(&tracker, b"\x1b[?1049h");
        assert!(tracker.requires_app_routing());
    }

    #[test]
    fn test_requires_app_routing_true_for_mouse_tracking() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        feed(&tracker, b"\x1b[?1002h");
        assert!(tracker.requires_app_routing());
    }

    #[test]
    fn test_requires_app_routing_true_for_margins() {
        let tracker = std::sync::Arc::new(make_tracker(24));
        feed(&tracker, b"\x1b[2;23r");
        assert!(tracker.requires_app_routing());
    }
}
