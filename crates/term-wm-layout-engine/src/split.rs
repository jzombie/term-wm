use crate::rect::{LayoutRect, Orientation, Ratio};

pub fn split_rect_bsp(
    area: LayoutRect,
    orientation: Orientation,
    ratio: Ratio,
) -> (LayoutRect, LayoutRect) {
    match orientation {
        Orientation::Horizontal => split_horizontal(area, ratio),
        Orientation::Vertical => split_vertical(area, ratio),
    }
}

fn split_horizontal(area: LayoutRect, ratio: Ratio) -> (LayoutRect, LayoutRect) {
    let total = u32::from(area.width);
    let left_w = if ratio.total() == 0 {
        total / 2
    } else {
        total * u32::from(ratio.left_part()) / u32::from(ratio.total())
    };
    let left_w = left_w as u16;
    let right_w = area.width.saturating_sub(left_w);

    let left = LayoutRect {
        x: area.x,
        y: area.y,
        width: left_w,
        height: area.height,
    };
    let right = LayoutRect {
        x: area.x.saturating_add(i32::from(left_w)),
        y: area.y,
        width: right_w,
        height: area.height,
    };
    (left, right)
}

fn split_vertical(area: LayoutRect, ratio: Ratio) -> (LayoutRect, LayoutRect) {
    let total = u32::from(area.height);
    let top_h = if ratio.total() == 0 {
        total / 2
    } else {
        total * u32::from(ratio.left_part()) / u32::from(ratio.total())
    };
    let top_h = top_h as u16;
    let bottom_h = area.height.saturating_sub(top_h);

    let top = LayoutRect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: top_h,
    };
    let bottom = LayoutRect {
        x: area.x,
        y: area.y.saturating_add(i32::from(top_h)),
        width: area.width,
        height: bottom_h,
    };
    (top, bottom)
}

pub fn split_rects_nary(
    area: LayoutRect,
    orientation: Orientation,
    weights: &[u16],
    child_count: usize,
) -> Vec<LayoutRect> {
    if child_count == 0 {
        return Vec::new();
    }
    if child_count == 1 {
        return vec![area];
    }
    let total_weight: u32 = weights.iter().map(|w| u32::from(*w)).sum();
    if total_weight == 0 {
        return split_evenly(area, orientation, child_count);
    }

    let (total_dim, fixed_start) = match orientation {
        Orientation::Horizontal => (u32::from(area.width), area.x),
        Orientation::Vertical => (u32::from(area.height), area.y),
    };

    let mut rects = Vec::with_capacity(child_count);
    let mut offset = fixed_start;
    let mut allocated: u32 = 0;

    for (i, &w) in weights.iter().enumerate() {
        let is_last = i == child_count - 1;
        let size = if is_last {
            (total_dim.saturating_sub(allocated)) as u16
        } else {
            let s = (total_dim * u32::from(w) / total_weight) as u16;
            allocated = allocated.saturating_add(u32::from(s));
            s
        };

        let rect = match orientation {
            Orientation::Horizontal => LayoutRect {
                x: offset,
                y: area.y,
                width: size,
                height: area.height,
            },
            Orientation::Vertical => LayoutRect {
                x: area.x,
                y: offset,
                width: area.width,
                height: size,
            },
        };
        rects.push(rect);

        match orientation {
            Orientation::Horizontal => offset = offset.saturating_add(i32::from(size)),
            Orientation::Vertical => offset = offset.saturating_add(i32::from(size)),
        }
    }

    rects
}

fn split_evenly(area: LayoutRect, orientation: Orientation, count: usize) -> Vec<LayoutRect> {
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![area];
    }

    let (total_dim, fixed_start) = match orientation {
        Orientation::Horizontal => (u32::from(area.width), area.x),
        Orientation::Vertical => (u32::from(area.height), area.y),
    };

    let per_child = total_dim / count as u32;
    let mut remainder = (total_dim % count as u32) as u16;

    let mut rects = Vec::with_capacity(count);
    let mut offset = fixed_start;

    for _ in 0..count {
        let extra = if remainder > 0 {
            remainder -= 1;
            1
        } else {
            0
        };
        let size = (per_child as u16).saturating_add(extra);

        let rect = match orientation {
            Orientation::Horizontal => LayoutRect {
                x: offset,
                y: area.y,
                width: size,
                height: area.height,
            },
            Orientation::Vertical => LayoutRect {
                x: area.x,
                y: offset,
                width: area.width,
                height: size,
            },
        };
        rects.push(rect);

        match orientation {
            Orientation::Horizontal => offset = offset.saturating_add(i32::from(size)),
            Orientation::Vertical => offset = offset.saturating_add(i32::from(size)),
        }
    }

    rects
}

/// Compute the visual thickness of a split handle gap.
pub fn handle_thickness(orientation: Orientation, _total_dim: u16) -> u16 {
    match orientation {
        Orientation::Horizontal => 1,
        Orientation::Vertical => 1,
    }
}

/// Compute the per-gap size between children in a split.
pub fn gap_size(orientation: Orientation, total_dim: u16, child_count: usize, resizable: bool) -> u16 {
    if !resizable || child_count < 2 {
        return 0;
    }
    if total_dim == 0 {
        return 0;
    }
    let min_content = child_count as u16;
    if total_dim <= min_content {
        return 0;
    }
    let max_gap = total_dim.saturating_sub(min_content);
    let per_gap = max_gap / (child_count as u16).saturating_sub(1);
    handle_thickness(orientation, total_dim).min(per_gap)
}

/// Weighted split of a rect into `child_count` rects using integer weights.
pub fn split_rects_weighted(
    area: LayoutRect,
    orientation: Orientation,
    weights: &[u16],
    child_count: usize,
) -> Vec<LayoutRect> {
    let count = child_count.max(1);
    let weights = if weights.len() == count {
        weights.to_vec()
    } else {
        vec![1u16; count]
    };
    let total_weight: u32 = weights.iter().map(|w| u32::from(*w)).sum::<u32>().max(1);
    let total = match orientation {
        Orientation::Horizontal => u32::from(area.width),
        Orientation::Vertical => u32::from(area.height),
    };

    let mut sizes = Vec::with_capacity(count);
    let mut allocated: u32 = 0;
    for (idx, &w) in weights.iter().enumerate() {
        let size = if idx + 1 == count {
            total.saturating_sub(allocated) as u16
        } else {
            let s = (total * u32::from(w) / total_weight) as u16;
            allocated = allocated.saturating_add(u32::from(s));
            s
        };
        sizes.push(size);
    }
    build_rects_from_sizes(area, orientation, &sizes)
}

/// Split a rect into `child_count` rects separated by `gap`-width gaps.
pub fn split_rects_with_gaps(
    area: LayoutRect,
    orientation: Orientation,
    weights: &[u16],
    child_count: usize,
    gap: u16,
) -> (Vec<LayoutRect>, Vec<LayoutRect>) {
    if gap == 0 || child_count < 2 {
        return (split_rects_weighted(area, orientation, weights, child_count), Vec::new());
    }
    let gap_total = gap.saturating_mul((child_count.saturating_sub(1)) as u16);
    let mut shrunk = area;
    match orientation {
        Orientation::Horizontal => {
            shrunk.width = area.width.saturating_sub(gap_total);
        }
        Orientation::Vertical => {
            shrunk.height = area.height.saturating_sub(gap_total);
        }
    }
    let raw = split_rects_weighted(shrunk, orientation, weights, child_count);
    let mut rects = Vec::with_capacity(raw.len());
    for (idx, rect) in raw.into_iter().enumerate() {
        let offset = gap.saturating_mul(idx as u16);
        let shifted = match orientation {
            Orientation::Horizontal => LayoutRect {
                x: rect.x.saturating_add(i32::from(offset)),
                ..rect
            },
            Orientation::Vertical => LayoutRect {
                y: rect.y.saturating_add(i32::from(offset)),
                ..rect
            },
        };
        rects.push(shifted);
    }
    let mut gaps = Vec::with_capacity(child_count.saturating_sub(1));
    for rect in rects.iter().take(child_count.saturating_sub(1)) {
        let gap_rect = match orientation {
            Orientation::Horizontal => LayoutRect {
                x: rect.x.saturating_add(i32::from(rect.width)),
                y: area.y,
                width: gap,
                height: area.height,
            },
            Orientation::Vertical => LayoutRect {
                x: area.x,
                y: rect.y.saturating_add(i32::from(rect.height)),
                width: area.width,
                height: gap,
            },
        };
        gaps.push(gap_rect);
    }
    (rects, gaps)
}

/// Build rects from a list of per-child sizes along an orientation.
pub fn build_rects_from_sizes(
    area: LayoutRect,
    orientation: Orientation,
    sizes: &[u16],
) -> Vec<LayoutRect> {
    let mut rects = Vec::with_capacity(sizes.len());
    let mut cursor_x = area.x;
    let mut cursor_y = area.y;
    for &size in sizes {
        let rect = match orientation {
            Orientation::Horizontal => LayoutRect {
                x: cursor_x,
                y: area.y,
                width: size,
                height: area.height,
            },
            Orientation::Vertical => LayoutRect {
                x: area.x,
                y: cursor_y,
                width: area.width,
                height: size,
            },
        };
        rects.push(rect);
        match orientation {
            Orientation::Horizontal => cursor_x = cursor_x.saturating_add(i32::from(size)),
            Orientation::Vertical => cursor_y = cursor_y.saturating_add(i32::from(size)),
        }
    }
    rects
}

/// Extract the per-child dimension sizes from a split result.
pub fn split_sizes(
    area: LayoutRect,
    orientation: Orientation,
    weights: &[u16],
    child_count: usize,
    gap: u16,
) -> Vec<u16> {
    let (rects, _) = split_rects_with_gaps(area, orientation, weights, child_count, gap);
    rects
        .iter()
        .map(|r| match orientation {
            Orientation::Horizontal => r.width,
            Orientation::Vertical => r.height,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(w: u16, h: u16) -> LayoutRect {
        LayoutRect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    #[test]
    fn split_horizontal_evenly() {
        let (l, r) = split_rect_bsp(area(80, 24), Orientation::Horizontal, Ratio(1, 1));
        assert_eq!(l.width, 40);
        assert_eq!(r.width, 40);
        assert_eq!(l.x, 0);
        assert_eq!(r.x, 40);
    }

    #[test]
    fn split_horizontal_uneven() {
        let (l, r) = split_rect_bsp(area(81, 24), Orientation::Horizontal, Ratio(1, 1));
        assert_eq!(l.width, 40);
        assert_eq!(r.width, 41);
        assert_eq!(l.x, 0);
        assert_eq!(r.x, 40);
    }

    #[test]
    fn split_horizontal_third() {
        let (l, r) = split_rect_bsp(area(90, 24), Orientation::Horizontal, Ratio(1, 2));
        assert_eq!(l.width, 30);
        assert_eq!(r.width, 60);
    }

    #[test]
    fn split_vertical_evenly() {
        let (t, b) = split_rect_bsp(area(80, 24), Orientation::Vertical, Ratio(1, 1));
        assert_eq!(t.height, 12);
        assert_eq!(b.height, 12);
    }

    #[test]
    fn split_vertical_uneven() {
        let (t, b) = split_rect_bsp(area(80, 25), Orientation::Vertical, Ratio(1, 1));
        assert_eq!(t.height, 12);
        assert_eq!(b.height, 13);
    }

    #[test]
    fn split_rects_nary_two_equal() {
        let rects = split_rects_nary(area(80, 24), Orientation::Horizontal, &[1, 1], 2);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].width, 40);
        assert_eq!(rects[1].width, 40);
        assert_eq!(rects[0].x, 0);
        assert_eq!(rects[1].x, 40);
    }

    #[test]
    fn split_rects_nary_three_equal() {
        let rects = split_rects_nary(area(80, 24), Orientation::Vertical, &[1, 1, 1], 3);
        assert_eq!(rects.len(), 3);
        // 24/3 = 8 each
        assert_eq!(rects[0].height, 8);
        assert_eq!(rects[1].height, 8);
        assert_eq!(rects[2].height, 8);
    }

    #[test]
    fn split_rects_nary_weighted() {
        let rects = split_rects_nary(area(80, 24), Orientation::Horizontal, &[1, 3], 2);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].width, 20); // 1/4 of 80
        assert_eq!(rects[1].width, 60); // 3/4 of 80
    }

    #[test]
    fn split_evenly_remainder_distribution() {
        let rects = split_evenly(area(10, 24), Orientation::Horizontal, 3);
        assert_eq!(rects[0].width, 4); // 10/3 = 3, remainder 1 → first gets +1
        assert_eq!(rects[1].width, 3);
        assert_eq!(rects[2].width, 3);
    }

    #[test]
    fn split_rects_no_remainder_dead_zones() {
        let rects = split_rects_nary(area(81, 24), Orientation::Horizontal, &[1, 1, 1], 3);
        let total_w: u16 = rects.iter().map(|r| r.width).sum();
        assert_eq!(total_w, 81);
    }

    #[test]
    fn bsp_split_no_dead_zones() {
        let (l, r) = split_rect_bsp(area(81, 24), Orientation::Horizontal, Ratio(1, 1));
        assert_eq!(l.width + r.width, 81);
        assert_eq!(l.x, 0);
        assert_eq!(r.x, i32::from(l.width));
    }

    #[test]
    fn handle_thickness_is_one() {
        assert_eq!(handle_thickness(Orientation::Horizontal, 100), 1);
        assert_eq!(handle_thickness(Orientation::Vertical, 100), 1);
    }

    #[test]
    fn gap_size_no_gap_when_not_resizable() {
        assert_eq!(gap_size(Orientation::Horizontal, 80, 2, false), 0);
    }

    #[test]
    fn gap_size_zero_when_too_small() {
        assert_eq!(gap_size(Orientation::Horizontal, 2, 3, true), 0);
        assert_eq!(gap_size(Orientation::Horizontal, 0, 2, true), 0);
    }

    #[test]
    fn gap_size_returns_gap() {
        let g = gap_size(Orientation::Horizontal, 80, 4, true);
        assert!(g >= 1);
    }

    #[test]
    fn split_rects_weighted_two_equal() {
        let rects = split_rects_weighted(area(80, 24), Orientation::Horizontal, &[1, 1], 2);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].width, 40);
        assert_eq!(rects[1].width, 40);
    }

    #[test]
    fn split_rects_weighted_uneven() {
        let rects = split_rects_weighted(area(80, 24), Orientation::Horizontal, &[1, 3], 2);
        assert_eq!(rects[0].width, 20);
        assert_eq!(rects[1].width, 60);
    }

    #[test]
    fn split_rects_weighted_vertical() {
        let rects = split_rects_weighted(area(80, 24), Orientation::Vertical, &[1, 1], 2);
        assert_eq!(rects[0].height, 12);
        assert_eq!(rects[1].height, 12);
    }

    #[test]
    fn split_rects_weighted_no_remainder() {
        let rects = split_rects_weighted(area(81, 24), Orientation::Horizontal, &[1, 1, 1], 3);
        let total: u16 = rects.iter().map(|r| r.width).sum();
        assert_eq!(total, 81);
    }

    #[test]
    fn split_rects_with_gaps_horizontal() {
        let (rects, gaps) = split_rects_with_gaps(area(80, 24), Orientation::Horizontal, &[1, 1], 2, 2);
        assert_eq!(rects.len(), 2);
        assert_eq!(gaps.len(), 1);
        assert_eq!(rects[0].width + rects[1].width + gaps[0].width, 80);
        assert_eq!(gaps[0].width, 2);
    }

    #[test]
    fn split_rects_with_gaps_vertical() {
        let (rects, gaps) = split_rects_with_gaps(area(80, 24), Orientation::Vertical, &[1, 1], 2, 2);
        assert_eq!(rects.len(), 2);
        assert_eq!(gaps.len(), 1);
        assert_eq!(rects[0].height + rects[1].height + gaps[0].height, 24);
        assert_eq!(gaps[0].height, 2);
    }

    #[test]
    fn split_rects_with_gaps_zero_gap() {
        let (rects, gaps) = split_rects_with_gaps(area(80, 24), Orientation::Horizontal, &[1, 1], 2, 0);
        assert_eq!(rects.len(), 2);
        assert!(gaps.is_empty());
    }

    #[test]
    fn split_rects_with_gaps_single_child() {
        let (rects, gaps) = split_rects_with_gaps(area(80, 24), Orientation::Horizontal, &[1], 1, 2);
        assert_eq!(rects.len(), 1);
        assert!(gaps.is_empty());
    }

    #[test]
    fn build_rects_from_sizes_horizontal() {
        let rects = build_rects_from_sizes(area(80, 24), Orientation::Horizontal, &[10, 20, 30]);
        assert_eq!(rects.len(), 3);
        assert_eq!(rects[0].x, 0);
        assert_eq!(rects[0].width, 10);
        assert_eq!(rects[1].x, 10);
        assert_eq!(rects[1].width, 20);
        assert_eq!(rects[2].x, 30);
        assert_eq!(rects[2].width, 30);
    }

    #[test]
    fn build_rects_from_sizes_vertical() {
        let rects = build_rects_from_sizes(area(80, 24), Orientation::Vertical, &[5, 10, 9]);
        assert_eq!(rects.len(), 3);
        assert_eq!(rects[0].y, 0);
        assert_eq!(rects[0].height, 5);
        assert_eq!(rects[1].y, 5);
        assert_eq!(rects[1].height, 10);
        assert_eq!(rects[2].y, 15);
        assert_eq!(rects[2].height, 9);
    }

    #[test]
    fn split_sizes_horizontal() {
        let sizes = split_sizes(area(80, 24), Orientation::Horizontal, &[1, 1], 2, 0);
        assert_eq!(sizes, vec![40, 40]);
    }

    #[test]
    fn split_sizes_with_gap() {
        let sizes = split_sizes(area(80, 24), Orientation::Horizontal, &[1, 1], 2, 2);
        assert_eq!(sizes, vec![39, 39]);
    }
}
