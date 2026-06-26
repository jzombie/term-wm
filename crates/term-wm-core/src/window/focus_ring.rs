#[derive(Debug, Clone)]
pub struct FocusRing<T: Copy + Eq> {
    pub order: Vec<T>,
    pub current: T,
}

impl<T: Copy + Eq> FocusRing<T> {
    pub fn new(current: T) -> Self {
        Self {
            order: Vec::new(),
            current,
        }
    }

    pub fn set_order(&mut self, order: Vec<T>) {
        self.order = order;
    }

    pub fn current(&self) -> T {
        self.current
    }

    pub fn set_current(&mut self, current: T) {
        self.current = current;
    }

    pub fn advance(&mut self, forward: bool) {
        if self.order.is_empty() {
            return;
        }
        let idx = self
            .order
            .iter()
            .position(|item| *item == self.current)
            .unwrap_or(0);
        let step = if forward { 1isize } else { -1isize };
        let next = ((idx as isize + step).rem_euclid(self.order.len() as isize)) as usize;
        self.current = self.order[next];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_ring_wraps_and_advances() {
        let mut ring = FocusRing::new(2usize);
        ring.set_order(vec![1usize, 2usize, 3usize]);
        assert_eq!(ring.current(), 2);
        ring.advance(true);
        assert_eq!(ring.current(), 3);
        ring.advance(true);
        assert_eq!(ring.current(), 1);
        ring.advance(false);
        assert_eq!(ring.current(), 3);
    }
}
