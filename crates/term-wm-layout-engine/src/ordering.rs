use core::fmt;

/// A flat z-order list. The last element is the top-most (front-most) item.
#[derive(Debug, Clone)]
pub struct ZOrder<T>(Vec<T>);

impl<T> ZOrder<T> {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn push(&mut self, id: T) {
        self.0.push(id);
    }

    pub fn order(&self) -> &[T] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn contains(&self, id: &T) -> bool
    where
        T: PartialEq,
    {
        self.0.contains(id)
    }
}

impl<T: PartialEq> ZOrder<T> {
    pub fn remove(&mut self, id: &T) {
        self.0.retain(|x| x != id);
    }

    pub fn bring_to_front(&mut self, id: T) {
        self.remove(&id);
        self.0.push(id);
    }

    pub fn retain(&mut self, f: impl FnMut(&T) -> bool) {
        self.0.retain(f);
    }
}

impl<T> Default for ZOrder<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// A cyclic focus ring with a single current item.
#[derive(Debug, Clone)]
pub struct FocusRing<T> {
    order: Vec<T>,
    current: T,
}

impl<T: Copy + Eq> FocusRing<T> {
    pub fn new(current: T) -> Self {
        Self {
            order: vec![current],
            current,
        }
    }

    pub fn current(&self) -> &T {
        &self.current
    }

    pub fn set_current(&mut self, id: T) {
        self.current = id;
    }

    pub fn set_order(&mut self, order: Vec<T>) {
        self.order = order;
        if !self.order.contains(&self.current)
            && let Some(first) = self.order.first().copied()
        {
            self.current = first;
        }
    }

    pub fn order(&self) -> &[T] {
        &self.order
    }

    pub fn advance(&mut self, forward: bool) -> T {
        if self.order.len() <= 1 {
            return self.current;
        }
        let pos = self
            .order
            .iter()
            .position(|x| *x == self.current)
            .unwrap_or(0);
        let len = self.order.len();
        let next = if forward {
            (pos + 1).rem_euclid(len)
        } else {
            (pos + len - 1).rem_euclid(len)
        };
        self.current = self.order[next];
        self.current
    }
}

impl<T: fmt::Debug + Copy + Eq> fmt::Display for FocusRing<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "FocusRing {{ current: {:?}, order: {:?} }}",
            self.current, self.order
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zorder_push_and_contains() {
        let mut zo = ZOrder::new();
        zo.push(1);
        zo.push(2);
        assert!(zo.contains(&1));
        assert!(zo.contains(&2));
        assert_eq!(zo.len(), 2);
    }

    #[test]
    fn zorder_bring_to_front() {
        let mut zo = ZOrder::new();
        zo.push(1);
        zo.push(2);
        zo.push(3);
        zo.bring_to_front(1);
        assert_eq!(zo.order(), &[2, 3, 1]);
    }

    #[test]
    fn zorder_remove() {
        let mut zo = ZOrder::new();
        zo.push(1);
        zo.push(2);
        zo.push(3);
        zo.remove(&2);
        assert_eq!(zo.order(), &[1, 3]);
    }

    #[test]
    fn focus_ring_advance_forward() {
        let mut ring = FocusRing::new(1);
        ring.set_order(vec![1, 2, 3]);
        assert_eq!(ring.advance(true), 2);
        assert_eq!(ring.advance(true), 3);
        assert_eq!(ring.advance(true), 1);
    }

    #[test]
    fn focus_ring_advance_backward() {
        let mut ring = FocusRing::new(1);
        ring.set_order(vec![1, 2, 3]);
        assert_eq!(ring.advance(false), 3);
        assert_eq!(ring.advance(false), 2);
    }

    #[test]
    fn focus_ring_single_element() {
        let mut ring = FocusRing::new(42);
        assert_eq!(ring.advance(true), 42);
        assert_eq!(ring.advance(false), 42);
    }

    #[test]
    fn focus_ring_set_order_falls_back_to_first() {
        let mut ring = FocusRing::new(1);
        ring.set_order(vec![2, 3]);
        assert_eq!(*ring.current(), 2);
    }
}
