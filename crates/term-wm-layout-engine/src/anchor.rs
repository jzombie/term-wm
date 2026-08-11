use crate::LayoutRect;

/// Preferred side/corner of the anchor rect an element is placed against.
///
/// The first component selects the horizontal alignment (`Left` aligns the
/// element's left edge to the anchor's left edge, `Right` aligns right edges),
/// the second the vertical (`Below` sits under the anchor, `Above` over it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorPlacement {
    BelowLeft,
    BelowRight,
    AboveLeft,
    AboveRight,
}

/// Place an element of `content_size` adjacent to `anchor`, keeping it inside
/// `bounds`.
///
/// Ordering matters: the size is truncated to `bounds` **first** so the result
/// is never larger than `bounds` (guards small-terminal / post-resize cases
/// where `content_size` exceeds `bounds` and would otherwise make downstream
/// buffer slices go out of bounds). The preferred placement is then applied,
/// flipping to the opposite side when the preferred side would overflow
/// `bounds`, and finally the origin is saturating-clamped.
///
/// Post-conditions (asserted in tests):
/// - `rect.x + rect.width <= bounds.x + bounds.width`
/// - `rect.y + rect.height <= bounds.y + bounds.height`
/// - `rect.width <= min(content_size.0, bounds.width)`
/// - `rect.height <= min(content_size.1, bounds.height)`
///
/// Pure function — reusable for positioning any element relative to an anchor.
pub fn place_anchored(
    anchor: LayoutRect,
    content_size: (u16, u16),
    placement: AnchorPlacement,
    bounds: LayoutRect,
) -> LayoutRect {
    let w = content_size.0.min(bounds.width);
    let h = content_size.1.min(bounds.height);
    if w == 0 || h == 0 {
        return LayoutRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
    }

    let bounds_right = bounds.x.saturating_add(i32::from(bounds.width));
    let bounds_bottom = bounds.y.saturating_add(i32::from(bounds.height));
    let max_x = bounds_right.saturating_sub(i32::from(w));
    let max_y = bounds_bottom.saturating_sub(i32::from(h));

    let (mut x, mut y) = match placement {
        AnchorPlacement::BelowLeft => (
            anchor.x,
            anchor.y.saturating_add(i32::from(anchor.height)),
        ),
        AnchorPlacement::BelowRight => (
            anchor
                .x
                .saturating_add(i32::from(anchor.width))
                .saturating_sub(i32::from(w)),
            anchor.y.saturating_add(i32::from(anchor.height)),
        ),
        AnchorPlacement::AboveLeft => (
            anchor.x,
            anchor.y.saturating_sub(i32::from(h)),
        ),
        AnchorPlacement::AboveRight => (
            anchor
                .x
                .saturating_add(i32::from(anchor.width))
                .saturating_sub(i32::from(w)),
            anchor.y.saturating_sub(i32::from(h)),
        ),
    };

    if y < bounds.y || y.saturating_add(i32::from(h)) > bounds_bottom {
        y = match placement {
            AnchorPlacement::BelowLeft | AnchorPlacement::BelowRight => {
                anchor.y.saturating_sub(i32::from(h))
            }
            AnchorPlacement::AboveLeft | AnchorPlacement::AboveRight => {
                anchor.y.saturating_add(i32::from(anchor.height))
            }
        };
    }
    if x < bounds.x || x.saturating_add(i32::from(w)) > bounds_right {
        x = match placement {
            AnchorPlacement::BelowLeft | AnchorPlacement::AboveLeft => {
                anchor
                    .x
                    .saturating_add(i32::from(anchor.width))
                    .saturating_sub(i32::from(w))
            }
            AnchorPlacement::BelowRight | AnchorPlacement::AboveRight => anchor.x,
        };
    }

    x = x.clamp(bounds.x, max_x);
    y = y.clamp(bounds.y, max_y);

    LayoutRect {
        x,
        y,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> LayoutRect {
        LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        }
    }

    fn assert_inside(rect: LayoutRect, bounds: LayoutRect) {
        assert!(
            rect.x.saturating_add(i32::from(rect.width))
                <= bounds.x.saturating_add(i32::from(bounds.width)),
            "right edge overflow: {rect:?} in {bounds:?}"
        );
        assert!(
            rect.y.saturating_add(i32::from(rect.height))
                <= bounds.y.saturating_add(i32::from(bounds.height)),
            "bottom edge overflow: {rect:?} in {bounds:?}"
        );
        assert!(rect.width <= bounds.width);
        assert!(rect.height <= bounds.height);
    }

    #[test]
    fn below_left_alignment() {
        let anchor = LayoutRect {
            x: 10,
            y: 10,
            width: 20,
            height: 5,
        };
        let r = place_anchored(anchor, (10, 5), AnchorPlacement::BelowLeft, area());
        assert_eq!(
            r,
            LayoutRect {
                x: 10,
                y: 15,
                width: 10,
                height: 5,
            }
        );
        assert_inside(r, area());
    }

    #[test]
    fn below_right_alignment() {
        let anchor = LayoutRect {
            x: 10,
            y: 10,
            width: 20,
            height: 5,
        };
        let r = place_anchored(anchor, (10, 5), AnchorPlacement::BelowRight, area());
        assert_eq!(
            r,
            LayoutRect {
                x: 20,
                y: 15,
                width: 10,
                height: 5,
            }
        );
        assert_inside(r, area());
    }

    #[test]
    fn above_left_alignment() {
        let anchor = LayoutRect {
            x: 10,
            y: 10,
            width: 20,
            height: 5,
        };
        let r = place_anchored(anchor, (10, 5), AnchorPlacement::AboveLeft, area());
        assert_eq!(
            r,
            LayoutRect {
                x: 10,
                y: 5,
                width: 10,
                height: 5,
            }
        );
        assert_inside(r, area());
    }

    #[test]
    fn above_right_alignment() {
        let anchor = LayoutRect {
            x: 10,
            y: 10,
            width: 20,
            height: 5,
        };
        let r = place_anchored(anchor, (10, 5), AnchorPlacement::AboveRight, area());
        assert_eq!(
            r,
            LayoutRect {
                x: 20,
                y: 5,
                width: 10,
                height: 5,
            }
        );
        assert_inside(r, area());
    }

    #[test]
    fn flips_above_when_below_overflows() {
        // Anchor sits at the very bottom edge — Below would leave the screen.
        let anchor = LayoutRect {
            x: 0,
            y: 20,
            width: 10,
            height: 4,
        };
        let r = place_anchored(anchor, (10, 5), AnchorPlacement::BelowLeft, area());
        assert_eq!(r.y, 15, "must flip to AboveLeft");
        assert_inside(r, area());
    }

    #[test]
    fn flips_left_when_right_overflows() {
        // Anchor hugs the right edge — BelowRight would leave the screen.
        let anchor = LayoutRect {
            x: 72,
            y: 0,
            width: 20,
            height: 4,
        };
        let r = place_anchored(anchor, (10, 5), AnchorPlacement::BelowRight, area());
        assert_eq!(r.x, 70, "must flip to BelowLeft and clamp");
        assert_inside(r, area());
    }

    #[test]
    fn anchor_entirely_outside_bounds_clamps_inside() {
        let anchor = LayoutRect {
            x: 100,
            y: 100,
            width: 5,
            height: 5,
        };
        for placement in [
            AnchorPlacement::BelowLeft,
            AnchorPlacement::BelowRight,
            AnchorPlacement::AboveLeft,
            AnchorPlacement::AboveRight,
        ] {
            let r = place_anchored(anchor, (10, 5), placement, area());
            assert_inside(r, area());
        }
    }

    #[test]
    fn anchor_above_bounds_clamps_to_top() {
        let anchor = LayoutRect {
            x: 0,
            y: -10,
            width: 10,
            height: 5,
        };
        let r = place_anchored(anchor, (10, 5), AnchorPlacement::AboveLeft, area());
        assert_eq!(r.y, 0);
        assert_inside(r, area());
    }

    #[test]
    fn content_larger_than_bounds_truncated() {
        let anchor = LayoutRect {
            x: 40,
            y: 12,
            width: 10,
            height: 5,
        };
        for placement in [
            AnchorPlacement::BelowLeft,
            AnchorPlacement::BelowRight,
            AnchorPlacement::AboveLeft,
            AnchorPlacement::AboveRight,
        ] {
            let r = place_anchored(anchor, (200, 200), placement, area());
            assert_eq!((r.width, r.height), (80, 24));
            assert_inside(r, area());
        }
    }

    #[test]
    fn zero_size_bounds_returns_zero_rect() {
        let bounds = LayoutRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        let anchor = LayoutRect {
            x: 10,
            y: 10,
            width: 5,
            height: 5,
        };
        let r = place_anchored(anchor, (10, 10), AnchorPlacement::BelowLeft, bounds);
        assert_eq!(
            r,
            LayoutRect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            }
        );
    }

    #[test]
    fn zero_width_bounds_returns_zero_rect() {
        let bounds = LayoutRect {
            x: 0,
            y: 0,
            width: 0,
            height: 24,
        };
        let anchor = LayoutRect {
            x: 10,
            y: 10,
            width: 5,
            height: 5,
        };
        let r = place_anchored(anchor, (10, 10), AnchorPlacement::BelowLeft, bounds);
        assert_eq!(
            r,
            LayoutRect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            }
        );
    }

    #[test]
    fn bounds_offset_by_origin() {
        // bounds not starting at (0,0) — the FAB/monocle bottom row case.
        let bounds = LayoutRect {
            x: 5,
            y: 3,
            width: 40,
            height: 20,
        };
        let anchor = LayoutRect {
            x: 5,
            y: 21,
            width: 10,
            height: 2,
        };
        let r = place_anchored(anchor, (10, 5), AnchorPlacement::BelowLeft, bounds);
        assert_eq!(r.x, 5);
        assert_eq!(r.y, 16, "flip above anchor, inside bounds");
        assert_inside(r, bounds);
    }
}
