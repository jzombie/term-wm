use crate::Rect;

pub const LEFT_BORDER_WIDTH: u16 = 1;
pub const RIGHT_BORDER_WIDTH: u16 = 1;
pub const TOP_BORDER_HEIGHT: u16 = 1;
pub const BOTTOM_BORDER_HEIGHT: u16 = 1;
pub const HEADER_HEIGHT: u16 = 1;
pub const MIN_CONTENT_DIM: u16 = 1;

/// Compute the inner content rectangle from the full frame rect,
/// given per-window chrome flags. This is the single source of truth
/// for content geometry — both the core and console call this.
pub fn content_rect(full: Rect, borders_enabled: bool, header_enabled: bool) -> Rect {
    if !borders_enabled && !header_enabled {
        return full;
    }
    let min_width = if borders_enabled {
        LEFT_BORDER_WIDTH + RIGHT_BORDER_WIDTH + MIN_CONTENT_DIM
    } else {
        MIN_CONTENT_DIM
    };
    let min_height = if borders_enabled && header_enabled {
        TOP_BORDER_HEIGHT + HEADER_HEIGHT + BOTTOM_BORDER_HEIGHT + MIN_CONTENT_DIM
    } else if borders_enabled || header_enabled {
        TOP_BORDER_HEIGHT.max(HEADER_HEIGHT) + MIN_CONTENT_DIM
    } else {
        MIN_CONTENT_DIM
    };
    if full.width < min_width || full.height < min_height {
        return Rect::default();
    }
    let x = if borders_enabled {
        full.x + i32::from(LEFT_BORDER_WIDTH)
    } else {
        full.x
    };
    let y = if borders_enabled && header_enabled {
        full.y + i32::from(TOP_BORDER_HEIGHT) + i32::from(HEADER_HEIGHT)
    } else if header_enabled {
        full.y + i32::from(HEADER_HEIGHT)
    } else if borders_enabled {
        full.y + i32::from(TOP_BORDER_HEIGHT)
    } else {
        full.y
    };
    let width = if borders_enabled {
        full.width
            .saturating_sub(LEFT_BORDER_WIDTH + RIGHT_BORDER_WIDTH)
    } else {
        full.width
    };
    let height = if borders_enabled && header_enabled {
        full.height
            .saturating_sub(TOP_BORDER_HEIGHT + HEADER_HEIGHT + BOTTOM_BORDER_HEIGHT)
    } else if header_enabled {
        full.height.saturating_sub(HEADER_HEIGHT)
    } else if borders_enabled {
        full.height
            .saturating_sub(TOP_BORDER_HEIGHT + BOTTOM_BORDER_HEIGHT)
    } else {
        full.height
    };
    Rect {
        x,
        y,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_borders_no_header_returns_full() {
        let full = Rect { x: 0, y: 0, width: 80, height: 24 };
        assert_eq!(content_rect(full, false, false), full);
    }

    #[test]
    fn borders_only() {
        let full = Rect { x: 0, y: 0, width: 80, height: 24 };
        let inner = content_rect(full, true, false);
        assert_eq!(inner.x, 1);
        assert_eq!(inner.y, 1);
        assert_eq!(inner.width, 78);
        assert_eq!(inner.height, 22);
    }

    #[test]
    fn header_only() {
        let full = Rect { x: 0, y: 0, width: 80, height: 24 };
        let inner = content_rect(full, false, true);
        assert_eq!(inner.x, 0);
        assert_eq!(inner.y, 1);
        assert_eq!(inner.width, 80);
        assert_eq!(inner.height, 23);
    }

    #[test]
    fn borders_and_header() {
        let full = Rect { x: 0, y: 0, width: 80, height: 24 };
        let inner = content_rect(full, true, true);
        assert_eq!(inner.x, 1);
        assert_eq!(inner.y, 2);
        assert_eq!(inner.width, 78);
        assert_eq!(inner.height, 21);
    }

    #[test]
    fn too_small_returns_default() {
        let tiny = Rect { x: 0, y: 0, width: 1, height: 1 };
        assert_eq!(content_rect(tiny, true, true), Rect::default());
    }

    #[test]
    fn borders_only_too_narrow() {
        let narrow = Rect { x: 0, y: 0, width: 2, height: 24 };
        assert_eq!(content_rect(narrow, true, false), Rect::default());
    }

    #[test]
    fn header_only_too_short() {
        let short = Rect { x: 0, y: 0, width: 80, height: 1 };
        assert_eq!(content_rect(short, false, true), Rect::default());
    }

    #[test]
    fn nonzero_origin() {
        let full = Rect { x: 10, y: 20, width: 80, height: 24 };
        let inner = content_rect(full, true, true);
        assert_eq!(inner.x, 11);
        assert_eq!(inner.y, 22);
        assert_eq!(inner.width, 78);
        assert_eq!(inner.height, 21);
    }

    #[test]
    fn min_content_dim_borders_only_exactly_minimal() {
        let full = Rect { x: 0, y: 0, width: 3, height: 2 };
        let inner = content_rect(full, true, false);
        assert_eq!(inner.width, 1);
        assert_eq!(inner.height, 0);
    }
}
