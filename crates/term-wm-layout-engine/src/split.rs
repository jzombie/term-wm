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
}
