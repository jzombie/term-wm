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
