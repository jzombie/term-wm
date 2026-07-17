use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::widgets::{Clear, Widget};

use term_wm_layout_engine::LayoutRect;

use crate::DialogOverlayComponent;
use crate::helpers::{downcast_ratatui, layout_rect_to_rect};

// TODO: Make this a proper component
/// Renders a centered modal overlay with backdrop dimming and content clearing.
///
/// Uses Ratatui's `Layout` + `Flex::Center` with `Constraint::Length` to
/// position the content rect exactly in the center of `screen_bounds`.
/// Target dimensions are clamped to screen bounds automatically.
///
/// Returns the computed content `LayoutRect` for hit-test caching.
pub fn render_modal(
    backend: &mut dyn term_wm_render::RenderBackend,
    screen_bounds: LayoutRect,
    target_width: u16,
    target_height: u16,
    render_content: &mut dyn FnMut(&mut ratatui::buffer::Buffer, Rect),
) -> LayoutRect {
    let screen_rect = layout_rect_to_rect(screen_bounds);

    let width = target_width.min(screen_rect.width).max(1);
    let height = target_height.min(screen_rect.height).max(1);

    let vert = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .split(screen_rect);
    let horiz = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .split(vert[0]);
    let content_rect = horiz[0];

    let content_layout = LayoutRect {
        x: i32::from(content_rect.x),
        y: i32::from(content_rect.y),
        width: content_rect.width,
        height: content_rect.height,
    };

    let mut dialog = DialogOverlayComponent::new();
    dialog.set_dim_backdrop(true);
    dialog.render_backdrop(backend, screen_bounds, Some(content_layout));

    let ratatui = downcast_ratatui(backend);
    Clear.render(content_rect, &mut ratatui.buffer);

    render_content(&mut ratatui.buffer, content_rect);

    content_layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;

    #[test]
    fn centers_content_in_middle_of_screen() {
        let screen = LayoutRect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        };
        let buf = Buffer::empty(Rect::new(0, 0, 100, 50));
        let area = buf.area;
        let mut backend = term_wm_console::RatatuiBackend::new(buf, area);

        let result = render_modal(&mut backend, screen, 40, 20, &mut |_, _| {});

        assert_eq!(result.x, 30);
        assert_eq!(result.y, 15);
        assert_eq!(result.width, 40);
        assert_eq!(result.height, 20);
    }

    #[test]
    fn clamps_to_screen_when_target_exceeds_bounds() {
        let screen = LayoutRect {
            x: 0,
            y: 0,
            width: 50,
            height: 30,
        };
        let buf = Buffer::empty(Rect::new(0, 0, 50, 30));
        let area = buf.area;
        let mut backend = term_wm_console::RatatuiBackend::new(buf, area);

        let result = render_modal(&mut backend, screen, 200, 100, &mut |_, _| {});

        assert_eq!(result.x, 0);
        assert_eq!(result.y, 0);
        assert_eq!(result.width, 50);
        assert_eq!(result.height, 30);
    }

    #[test]
    fn backdrop_dims_cells_outside_content() {
        let screen = LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let buf = Buffer::empty(Rect::new(0, 0, 10, 10));
        let area = buf.area;
        let mut backend = term_wm_console::RatatuiBackend::new(buf, area);

        let result = render_modal(&mut backend, screen, 4, 4, &mut |_, _| {});

        let buf = backend.buffer;
        let outside = buf.cell((0, 0)).unwrap();
        assert!(
            outside.modifier.contains(Modifier::DIM),
            "cell outside content should be dimmed"
        );
        let inside = buf.cell((result.x as u16, result.y as u16)).unwrap();
        assert!(
            !inside.modifier.contains(Modifier::DIM),
            "cell inside content should NOT be dimmed"
        );
    }

    #[test]
    fn clear_applied_before_content() {
        let screen = LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let buf = Buffer::empty(Rect::new(0, 0, 10, 10));
        let mut buf = buf;
        for y in 3..7 {
            for x in 3..7 {
                buf.cell_mut((x, y)).unwrap().set_symbol("X");
            }
        }
        let area = buf.area;
        let mut backend = term_wm_console::RatatuiBackend::new(buf, area);

        render_modal(&mut backend, screen, 4, 4, &mut |_, _| {});

        let buf = backend.buffer;
        let cell = buf.cell((3, 3)).unwrap();
        assert_eq!(cell.symbol(), " ", "Clear should wipe pre-filled symbols");
    }
}
