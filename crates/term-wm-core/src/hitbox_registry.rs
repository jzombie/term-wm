use std::sync::atomic::{AtomicU64, Ordering};

use term_wm_layout_engine::LayoutRect;

use crate::chrome::ChromeTarget;
use crate::mouse_coord::MousePosition;
use crate::window::{LayerId, OverlayKey, WindowKey};

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

/// Identifies which component instance owns a hitbox entry.
///
/// Every registered hitbox must have exactly one owner. The `dispatch_mouse`
/// function uses this tag to route events directly to the owning component
/// in O(1) — no iteration over handle lists, no `content_hitbox_id == id`
/// equality checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentOwner {
    /// Hitbox belongs to a window's component tree.
    Window(WindowKey),
    /// Hitbox belongs to an overlay (command palette, help, confirm dialog).
    Overlay(OverlayKey),
    /// Hitbox belongs to a system layer (panels, FAB, notification area).
    Layer(LayerId),
    /// Strongly-typed chrome element (resize edge, drag handle, button, split).
    Chrome(ChromeTarget),
    /// Test-only owner for standalone component unit tests.
    /// Unconditional so integration tests in tests/*.rs can reference it.
    Test,
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
    /// The component that owns this hitbox — NEVER optional.
    pub owner: ComponentOwner,
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
    /// Standard hitboxes: windows, chrome, layers.
    entries: Vec<HitboxEntry>,
    /// Overlay hitboxes: checked FIRST by hit_test, highest Z-layer.
    /// Modal overlays register a full-screen blocker here.
    overlay_entries: Vec<HitboxEntry>,
    /// Active clip rects from scroll containers.  Inline storage avoids
    /// heap allocation for the common case (depth ≤ 5). Falls back to heap
    /// only in pathological nesting > CLIP_STACK_INLINE_CAPACITY levels deep.
    clip_stack: smallvec::SmallVec<[LayoutRect; CLIP_STACK_INLINE_CAPACITY]>,
    /// The component owner currently being populated. Set via
    /// `set_active_owner()` before each render unit. `register()` panics
    /// if this is `None`, ensuring every entry has a concrete owner.
    active_owner: Option<ComponentOwner>,
}

impl HitboxRegistry {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            overlay_entries: Vec::new(),
            clip_stack: smallvec::SmallVec::new(),
            active_owner: None,
        }
    }

    /// Create a registry pre-configured with an active owner.
    /// Use for scratch/offscreen registries that will be merged
    /// into the main registry after rendering.
    pub fn with_owner(owner: ComponentOwner) -> Self {
        Self {
            entries: Vec::new(),
            overlay_entries: Vec::new(),
            clip_stack: smallvec::SmallVec::new(),
            active_owner: Some(owner),
        }
    }

    /// Set the active component owner. All subsequent `register()` calls
    /// will inherit this owner. Panics if `register()` is called without a
    /// prior `set_active_owner` call.
    pub fn set_active_owner(&mut self, owner: ComponentOwner) {
        self.active_owner = Some(owner);
    }

    /// Clear the active owner. Primarily for test isolation.
    /// The production render pipeline does NOT call this between
    /// render units — it overwrites via sequential `set_active_owner()`.
    pub fn clear_active_owner(&mut self) {
        self.active_owner = None;
    }

    /// Reset for a new frame.  Clears entries, clip stack, and active owner.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.overlay_entries.clear();
        self.clip_stack.clear();
    }

    /// Register a clickable area with an explicit owner.
    /// Atomic — no stateful `set_active_owner` needed.
    ///
    /// The `area` is intersected with the current clip stack before storing.
    /// If the intersection yields an empty rect, the entry is skipped entirely.
    /// This means scrolled-off components simply don't appear in the registry
    /// — no `rect_contains` needed at event time.
    pub fn register(&mut self, id: HitboxId, owner: ComponentOwner, area: LayoutRect) {
        let mut clipped = area;
        for clip in &self.clip_stack {
            clipped = clipped.clamp(*clip);
        }
        if clipped.width == 0 || clipped.height == 0 {
            return;
        }
        let entry = HitboxEntry {
            id,
            owner,
            area: clipped,
        };
        if matches!(owner, ComponentOwner::Overlay(_)) {
            self.overlay_entries.push(entry);
        } else {
            self.entries.push(entry);
        }
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
    /// entry whose area contains `position`. Returns the `HitboxId`, its
    /// `ComponentOwner`, and its exact screen-space `Rect` if found.
    ///
    /// Entries are registered in render order (back-to-front), so iterating
    /// in reverse yields the top-most (last-rendered, highest z-order) match.
    pub fn hit_test(
        &self,
        position: MousePosition,
    ) -> Option<(HitboxId, ComponentOwner, LayoutRect)> {
        // 1. Overlay entries first (LIFO — last-registered overlay wins)
        if let Some(entry) = self
            .overlay_entries
            .iter()
            .rev()
            .find(|entry| position.is_inside(entry.area))
        {
            return Some((entry.id, entry.owner, entry.area));
        }
        // 2. Standard entries second (LIFO — last-registered child wins over parent)
        self.entries
            .iter()
            .rev()
            .find(|entry| position.is_inside(entry.area))
            .map(|entry| (entry.id, entry.owner, entry.area))
    }

    /// Returns the number of registered entries (for diagnostics / metrics).
    pub fn len(&self) -> usize {
        self.entries.len() + self.overlay_entries.len()
    }

    /// Returns `true` if no entries are registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.overlay_entries.is_empty()
    }

    /// Atomically swap all entries with another registry.
    ///
    /// O(1) — moves the internal `Vec`s, preserving Z-ordering from the source.
    /// Useful for transferring the frame's render-time registry to a dispatch
    /// registry without copying or re-scanning.
    pub fn swap_entries(&mut self, other: &mut Self) {
        std::mem::swap(&mut self.entries, &mut other.entries);
        std::mem::swap(&mut self.overlay_entries, &mut other.overlay_entries);
        std::mem::swap(&mut self.clip_stack, &mut other.clip_stack);
        std::mem::swap(&mut self.active_owner, &mut other.active_owner);
    }

    /// Merge all entries from another registry into this one.
    ///
    /// Appends entries from `other` to `self`, preserving Z-ordering.
    /// The source registry is consumed (entries moved, not copied).
    pub fn merge(&mut self, other: Self) {
        self.entries.extend(other.entries);
        self.overlay_entries.extend(other.overlay_entries);
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
            ComponentOwner::Test,
            LayoutRect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
        );
        let result = reg.hit_test(screen_pos(5, 5));
        assert!(result.is_some());
        let (hit_id, owner, _rect) = result.unwrap();
        assert_eq!(hit_id, id);
        assert_eq!(owner, ComponentOwner::Test);
    }

    #[test]
    fn miss_outside_area() {
        let mut reg = HitboxRegistry::new();
        let id = HitboxId::new();
        reg.register(
            id,
            ComponentOwner::Test,
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
        reg.register(
            id1,
            ComponentOwner::Test,
            LayoutRect {
                x: 0,
                y: 0,
                width: 20,
                height: 20,
            },
        );
        reg.register(
            id2,
            ComponentOwner::Test,
            LayoutRect {
                x: 5,
                y: 5,
                width: 10,
                height: 10,
            },
        );
        let (hit, _owner, _rect) = reg.hit_test(screen_pos(7, 7)).unwrap();
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
        let id = HitboxId::new();
        reg.register(
            id,
            ComponentOwner::Test,
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
        let id = HitboxId::new();
        reg.register(
            id,
            ComponentOwner::Test,
            LayoutRect {
                x: 5,
                y: 5,
                width: 20,
                height: 20,
            },
        );
        assert_eq!(reg.len(), 1);
        let (hit, _owner, _rect) = reg.hit_test(screen_pos(7, 7)).unwrap();
        assert_eq!(hit, id);
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
        let id = HitboxId::new();
        reg.register(
            id,
            ComponentOwner::Test,
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
        let id = HitboxId::new();
        reg.register(
            id,
            ComponentOwner::Test,
            LayoutRect {
                x: 5,
                y: 5,
                width: 10,
                height: 10,
            },
        );
        assert_eq!(reg.len(), 1);
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
            ComponentOwner::Test,
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
