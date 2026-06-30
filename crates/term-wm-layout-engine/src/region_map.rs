use crate::rect::{LayoutRect, rect_contains};

#[cfg(not(feature = "std"))]
use alloc::collections::BTreeMap;
#[cfg(feature = "std")]
use std::collections::BTreeMap;

/// A map from ID to rectangle, with hit-testing support.
///
/// Regions are tested in insertion order (via BTreeMap iteration).
/// Explicitly pass a slice of IDs to control the hit-test order
/// (e.g., reverse for topmost-first).
#[derive(Debug, Clone)]
pub struct RegionMap<T: Ord> {
    regions: BTreeMap<T, LayoutRect>,
}

impl<T: Ord> RegionMap<T> {
    pub fn new() -> Self {
        Self {
            regions: BTreeMap::new(),
        }
    }

    pub fn ids(&self) -> Vec<T>
    where
        T: Copy,
    {
        self.regions.keys().copied().collect()
    }

    pub fn set(&mut self, id: T, rect: LayoutRect) {
        self.regions.insert(id, rect);
    }

    pub fn get(&self, id: &T) -> Option<LayoutRect> {
        self.regions.get(id).copied()
    }

    pub fn remove(&mut self, id: &T) {
        self.regions.remove(id);
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    pub fn contains_key(&self, id: &T) -> bool {
        self.regions.contains_key(id)
    }

    /// Find the first ID (in the given order) whose region contains (col, row).
    pub fn hit_test(&self, col: u16, row: u16, ids: &[T]) -> Option<T>
    where
        T: Copy,
    {
        for id in ids {
            if let Some(rect) = self.regions.get(id)
                && rect_contains(rect, col, row)
            {
                return Some(*id);
            }
        }
        None
    }
}

impl<T: Ord> Default for RegionMap<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: u16, h: u16) -> LayoutRect {
        LayoutRect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn region_map_set_get_remove() {
        let mut map: RegionMap<u8> = RegionMap::new();
        map.set(1, rect(0, 0, 10, 10));
        assert_eq!(map.get(&1), Some(rect(0, 0, 10, 10)));
        map.remove(&1);
        assert_eq!(map.get(&1), None);
    }

    #[test]
    fn region_map_hit_test() {
        let mut map: RegionMap<u8> = RegionMap::new();
        map.set(1, rect(0, 0, 10, 10));
        map.set(2, rect(10, 0, 10, 10));
        assert_eq!(map.hit_test(5, 5, &[1, 2]), Some(1));
        assert_eq!(map.hit_test(15, 5, &[1, 2]), Some(2));
        assert_eq!(map.hit_test(50, 50, &[1, 2]), None);
    }

    #[test]
    fn region_map_ids() {
        let mut map: RegionMap<u8> = RegionMap::new();
        map.set(3, rect(0, 0, 1, 1));
        map.set(1, rect(0, 0, 1, 1));
        let mut ids = map.ids();
        ids.sort();
        assert_eq!(ids, vec![1, 3]);
    }

    #[test]
    fn region_map_is_empty() {
        let map: RegionMap<u8> = RegionMap::new();
        assert!(map.is_empty());
    }
}
