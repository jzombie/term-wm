use std::sync::atomic::{AtomicU64, Ordering};

use term_wm_layout_engine::LayoutRect;

use crate::mouse_coord::MousePosition;

/// Globally unique opaque identifier for a clickable surface / widget.
///
/// Assigned once at entity construction time and never changes across frames.
/// The `HitboxRegistry` stores only these IDs — no domain knowledge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HitboxId(pub u64);

impl HitboxId {
    pub fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for HitboxId {
    fn default() -> Self {
        Self::new()
    }
}

/// Maximum depth of nested clipping containers (ScrollViews, overlay bounds,
/// etc.) that the registry supports without heap allocation.
///
/// Depth ≤ 5: In practice, no UI nests more than 4–5 *clipping* boundaries.
/// Layout-only containers (padding, margins, rows, columns) do not clip;
/// only scrollable or bounded containers create clip rects. A button inside
/// a List inside a ScrollView inside a floating Window inside an Overlay
/// is already an extreme case at depth 4. We set the inline capacity to 8
/// to leave a generous safety margin while keeping the entire struct
/// stack-allocated in the common case.
const CLIP_STACK_INLINE_CAPACITY: usize = 8;

/// A single entry in the hitbox registry.
#[derive(Debug, Clone, Copy)]
pub struct HitboxEntry {
    pub id: HitboxId,
    /// Absolute screen coordinates (post-clip intersection).
    pub area: LayoutRect,
}

/// Flat, data-oriented hit-test registry rebuilt every frame.
///
/// Populated during the render pass: components call `register()` to
/// declare their clickable areas, and scroll containers use `push_clip` /
/// `pop_clip` to clip child registrations to their visible viewport.
///
/// At event time, `hit_test()` does a single O(n) reverse scan over a
/// dense array of `HitboxEntry` — no tree walk, no vtable dispatch,
/// no coordinate mutation.
#[derive(Debug, Clone)]
pub struct HitboxRegistry {
    entries: Vec<HitboxEntry>,
    /// Active clip rects from scroll containers.  Inline storage avoids
    /// heap allocation for the common case (depth ≤ 5). Falls back to heap
    /// only in pathological nesting > CLIP_STACK_INLINE_CAPACITY levels deep.
    clip_stack: smallvec::SmallVec<[LayoutRect; CLIP_STACK_INLINE_CAPACITY]>,
}

impl HitboxRegistry {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            clip_stack: smallvec::SmallVec::new(),
        }
    }

    /// Reset for a new frame.  Clears both the entry list and the clip stack.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.clip_stack.clear();
    }

    /// Register a clickable area.
    ///
    /// The `area` is intersected with the current clip stack before storing.
    /// If the intersection yields an empty rect, the entry is skipped entirely.
    /// This means scrolled-off components simply don't appear in the registry
    /// — no `rect_contains` needed at event time.
    pub fn register(&mut self, id: HitboxId, area: LayoutRect) {
        let mut clipped = area;
        for clip in &self.clip_stack {
            clipped = clipped.clamp(*clip);
        }
        if clipped.width == 0 || clipped.height == 0 {
            return;
        }
        self.entries.push(HitboxEntry { id, area: clipped });
    }

    /// Push a clip rect (called by `ScrollViewComponent` before rendering
    /// children).  All subsequent `register()` calls will intersect their
    /// area with this rect.  Stacks: `ScrollView → child → grandchild`.
    pub fn push_clip(&mut self, rect: LayoutRect) {
        self.clip_stack.push(rect);
    }

    /// Pop the active clip rect (called by `ScrollViewComponent` after
    /// rendering children).
    ///
    /// # Panics
    /// Panics if the clip stack is empty (mismatched push/pop).
    pub fn pop_clip(&mut self) {
        self.clip_stack
            .pop()
            .expect("clip_stack pop without matching push");
    }

    /// Query: reverse scan (front-to-back = top-most first) for the top-most
    /// entry whose area contains `position`. Returns the `HitboxId` and its
    /// exact screen-space `Rect` if found.
    ///
    /// Entries are registered in render order (back-to-front), so iterating
    /// in reverse yields the top-most (last-rendered, highest z-order) match.
    pub fn hit_test(&self, position: MousePosition) -> Option<(HitboxId, LayoutRect)> {
        self.entries
            .iter()
            .rev()
            .find(|entry| position.is_inside(entry.area))
            .map(|entry| (entry.id, entry.area))
    }

    /// Returns the number of registered entries (for diagnostics / metrics).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if no entries are registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Atomically swap all entries with another registry.
    ///
    /// O(1) — moves the internal `Vec`s, preserving Z-ordering from the source.
    /// Useful for transferring the frame's render-time registry to a dispatch
    /// registry without copying or re-scanning.
    pub fn swap_entries(&mut self, other: &mut Self) {
        std::mem::swap(&mut self.entries, &mut other.entries);
        std::mem::swap(&mut self.clip_stack, &mut other.clip_stack);
    }

    /// Merge all entries from another registry into this one.
    ///
    /// Appends entries from `other` to `self`, preserving Z-ordering.
    /// The source registry is consumed (entries moved, not copied).
    pub fn merge(&mut self, other: Self) {
        self.entries.extend(other.entries);
    }
}

impl Default for HitboxRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mouse_coord::CoordSpace;

    fn screen_pos(col: i16, row: i16) -> MousePosition {
        MousePosition {
            column: col,
            row,
            space: CoordSpace::Screen,
        }
    }

    #[test]
    fn register_and_hit() {
        let mut reg = HitboxRegistry::new();
        let id = HitboxId::new();
        reg.register(
            id,
            LayoutRect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
        );
        assert!(reg.hit_test(screen_pos(5, 5)).is_some());
    }

    #[test]
    fn miss_outside_area() {
        let mut reg = HitboxRegistry::new();
        let id = HitboxId::new();
        reg.register(
            id,
            LayoutRect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
        );
        assert!(reg.hit_test(screen_pos(20, 20)).is_none());
    }

    #[test]
    fn front_to_back_priority() {
        let mut reg = HitboxRegistry::new();
        let id1 = HitboxId::new();
        let id2 = HitboxId::new();
        // Register back rect first
        reg.register(
            id1,
            LayoutRect {
                x: 0,
                y: 0,
                width: 20,
                height: 20,
            },
        );
        // Register smaller front rect second
        reg.register(
            id2,
            LayoutRect {
                x: 5,
                y: 5,
                width: 10,
                height: 10,
            },
        );

        // Point inside both rects should hit the front (last-registered)
        let (hit, _rect) = reg.hit_test(screen_pos(7, 7)).unwrap();
        assert_eq!(hit, id2);
    }

    #[test]
    fn clip_rect_culls_entries() {
        let mut reg = HitboxRegistry::new();
        reg.push_clip(LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        });
        // Register a rect that is entirely outside the clip
        let id = HitboxId::new();
        reg.register(
            id,
            LayoutRect {
                x: 20,
                y: 20,
                width: 5,
                height: 5,
            },
        );
        assert!(reg.is_empty());
        reg.pop_clip();
    }

    #[test]
    fn clip_rect_intersects_partial_overlap() {
        let mut reg = HitboxRegistry::new();
        reg.push_clip(LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        });
        // Register a rect that partially overlaps the clip
        let id = HitboxId::new();
        reg.register(
            id,
            LayoutRect {
                x: 5,
                y: 5,
                width: 20,
                height: 20,
            },
        );
        assert_eq!(reg.len(), 1);
        // The registered area should be clipped to (5,5,5,5) — the overlap
        let (hit, _rect) = reg.hit_test(screen_pos(7, 7)).unwrap();
        assert_eq!(hit, id);
        // Point in the original rect but outside the clip should miss
        assert!(reg.hit_test(screen_pos(15, 15)).is_none());
        reg.pop_clip();
    }

    #[test]
    fn nested_clip_stack_culls_fully_occluded() {
        let mut reg = HitboxRegistry::new();
        reg.push_clip(LayoutRect {
            x: 0,
            y: 0,
            width: 50,
            height: 50,
        });
        reg.push_clip(LayoutRect {
            x: 10,
            y: 10,
            width: 20,
            height: 20,
        });
        // This rect is inside the first clip but completely outside the second
        let id = HitboxId::new();
        reg.register(
            id,
            LayoutRect {
                x: 0,
                y: 0,
                width: 5,
                height: 5,
            },
        );
        assert!(reg.is_empty());
        reg.pop_clip();
        reg.pop_clip();
    }

    #[test]
    fn nested_clip_stack_partial_intersection() {
        let mut reg = HitboxRegistry::new();
        reg.push_clip(LayoutRect {
            x: 0,
            y: 0,
            width: 50,
            height: 50,
        });
        reg.push_clip(LayoutRect {
            x: 10,
            y: 10,
            width: 20,
            height: 20,
        });
        // This rect partially overlaps the second clip — should be clipped
        let id = HitboxId::new();
        reg.register(
            id,
            LayoutRect {
                x: 5,
                y: 5,
                width: 10,
                height: 10,
            },
        );
        assert_eq!(reg.len(), 1);
        // Check that the result is clipped to the inner clip's bounds
        let entry = reg.entries[0];
        assert_eq!(entry.area.x, 10);
        assert_eq!(entry.area.y, 10);
        assert_eq!(entry.area.width, 5);
        assert_eq!(entry.area.height, 5);
        reg.pop_clip();
        reg.pop_clip();
    }

    #[test]
    fn clear_resets_state() {
        let mut reg = HitboxRegistry::new();
        reg.push_clip(LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        });
        let id = HitboxId::new();
        reg.register(
            id,
            LayoutRect {
                x: 0,
                y: 0,
                width: 5,
                height: 5,
            },
        );
        assert_eq!(reg.len(), 1);
        reg.clear();
        assert!(reg.is_empty());
        assert!(reg.clip_stack.is_empty());
    }

    #[test]
    #[should_panic(expected = "clip_stack pop without matching push")]
    fn pop_empty_clip_panics() {
        let mut reg = HitboxRegistry::new();
        reg.pop_clip();
    }
}
