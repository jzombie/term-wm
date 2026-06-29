/// Scroll state for a scrollable viewport.
///
/// Tracks an accumulated `pending` offset (from bump operations) and an
/// `offset` that is clamped against the scrollable content range on `apply`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollState {
    pub offset: usize,
    pub pending: isize,
}

impl ScrollState {
    pub fn new() -> Self {
        Self {
            offset: 0,
            pending: 0,
        }
    }

    pub fn reset(&mut self) {
        self.offset = 0;
        self.pending = 0;
    }

    pub fn bump(&mut self, delta: isize) {
        self.pending = self.pending.saturating_add(delta);
    }

    pub fn apply(&mut self, total: usize, view: usize) {
        let max_offset = total.saturating_sub(view);
        let new_offset = (self.offset as isize).saturating_add(self.pending).max(0) as usize;
        self.offset = new_offset.min(max_offset);
        self.pending = 0;
    }
}

impl Default for ScrollState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_apply_clamps_to_max() {
        let mut s = ScrollState::new();
        s.bump(100);
        s.apply(50, 10); // total=50, view=10, max_offset=40
        assert_eq!(s.offset, 40);
    }

    #[test]
    fn scroll_apply_clamps_to_zero() {
        let mut s = ScrollState::new();
        s.bump(-10);
        s.apply(50, 10);
        assert_eq!(s.offset, 0);
    }

    #[test]
    fn scroll_apply_normal() {
        let mut s = ScrollState::new();
        s.bump(5);
        s.apply(50, 10); // max_offset = 40
        assert_eq!(s.offset, 5);
    }

    #[test]
    fn scroll_reset() {
        let mut s = ScrollState::new();
        s.bump(10);
        s.apply(100, 20);
        assert_eq!(s.offset, 10);
        s.reset();
        assert_eq!(s.offset, 0);
        assert_eq!(s.pending, 0);
    }
}
