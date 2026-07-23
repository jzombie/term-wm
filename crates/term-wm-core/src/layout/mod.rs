// TODO: Many aspects of this should likely be refactored out of the core, especially which involve rendering

pub mod floating;
pub mod tiling;

pub use tiling::TilingLayout;
pub use tiling::{InsertPosition, LayoutNode, LayoutPlan, SplitHandle};

use crate::Rect;
use std::collections::BTreeMap;

/// Layout direction — pure data, no rendering dependency.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RectSpec {
    Absolute(Rect),
    Percent {
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    },
}

impl RectSpec {
    pub fn resolve(self, area: Rect) -> Rect {
        match self {
            RectSpec::Absolute(rect) => rect,
            RectSpec::Percent {
                x,
                y,
                width,
                height,
            } => {
                let to_abs = |base: u16, pct: u16| (base as u32 * pct as u32 / 100) as u16;
                Rect {
                    x: area.x.saturating_add(i32::from(to_abs(area.width, x))),
                    y: area.y.saturating_add(i32::from(to_abs(area.height, y))),
                    width: to_abs(area.width, width),
                    height: to_abs(area.height, height),
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct FloatingPane<K: Copy + Eq + Ord> {
    pub key: K,
    pub rect: RectSpec,
}

#[derive(Debug, Clone)]
pub struct RegionMap<T: Copy + Eq + Ord> {
    regions: BTreeMap<T, Rect>,
}

impl<T: Copy + Eq + Ord> Default for RegionMap<T> {
    fn default() -> Self {
        Self {
            regions: BTreeMap::new(),
        }
    }
}

impl<T: Copy + Eq + Ord> RegionMap<T> {
    pub fn ids(&self) -> Vec<T> {
        self.regions.keys().copied().collect()
    }

    pub fn set(&mut self, id: T, rect: Rect) {
        self.regions.insert(id, rect);
    }

    pub fn get(&self, id: T) -> Option<Rect> {
        self.regions.get(&id).copied()
    }

    pub fn remove(&mut self, id: T) {
        self.regions.remove(&id);
    }

    pub fn hit_test(&self, column: u16, row: u16, ids: &[T]) -> Option<T> {
        for id in ids {
            if let Some(rect) = self.regions.get(id)
                && rect_contains(*rect, column, row)
            {
                return Some(*id);
            }
        }
        None
    }
}

pub fn rect_contains(rect: Rect, column: u16, row: u16) -> bool {
    if rect.width == 0 || rect.height == 0 {
        return false;
    }
    let max_x = rect.x.saturating_add(i32::from(rect.width));
    let max_y = rect.y.saturating_add(i32::from(rect.height));
    i32::from(column) >= rect.x
        && i32::from(column) < max_x
        && i32::from(row) >= rect.y
        && i32::from(row) < max_y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_spec_resolve_percent_and_absolute() {
        let area = Rect {
            x: 10,
            y: 20,
            width: 200,
            height: 100,
        };
        let abs = RectSpec::Absolute(Rect {
            x: 1,
            y: 2,
            width: 3,
            height: 4,
        });
        assert_eq!(
            abs.resolve(area),
            Rect {
                x: 1,
                y: 2,
                width: 3,
                height: 4
            }
        );

        let pct = RectSpec::Percent {
            x: 50,
            y: 50,
            width: 50,
            height: 50,
        };
        let r = pct.resolve(area);
        // 50% of width=200 is 100; x offset 50% -> 100 + area.x
        assert_eq!(r.width, 100);
        assert_eq!(r.height, 50);
    }

    #[test]
    fn region_map_set_get_hit_test() {
        let mut map = RegionMap::default();
        let a = Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 5,
        };
        let b = Rect {
            x: 6,
            y: 0,
            width: 5,
            height: 5,
        };
        map.set(1u8, a);
        map.set(2u8, b);
        assert_eq!(map.get(1u8), Some(a));
        assert_eq!(map.ids(), vec![1u8, 2u8]);
        // hit inside first
        assert_eq!(map.hit_test(2, 2, &[1u8, 2u8]), Some(1u8));
        // miss both
        assert_eq!(map.hit_test(100, 100, &[1u8, 2u8]), None);
    }

    #[test]
    fn rect_contains_edge_cases() {
        let r = Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 5,
        };
        assert!(!rect_contains(r, 0, 0));
        let r2 = Rect {
            x: 1,
            y: 1,
            width: 3,
            height: 3,
        };
        assert!(rect_contains(r2, 1, 1));
        assert!(!rect_contains(r2, 4, 1));
    }
}
