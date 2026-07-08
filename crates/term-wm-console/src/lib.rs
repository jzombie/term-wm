pub mod console_event_source;
pub mod console_render_target;
pub mod draw_plan_renderer;
pub mod widget_adapter;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect as RatatuiRect;
pub use term_wm_render::RenderBackend;

/// Concrete Ratatui backend implementation.
/// Owns the Buffer by value (satisfying 'static for Any downcasting).
/// Swap-based rendering preserves buffer capacity without allocation.
pub struct RatatuiBackend {
    pub buffer: Buffer,
    pub area: RatatuiRect,
}

impl RenderBackend for RatatuiBackend {
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl RatatuiBackend {
    /// Create a backend owning the given buffer.
    pub fn new(buffer: Buffer, area: RatatuiRect) -> Self {
        Self { buffer, area }
    }

    /// Get mutable reference to the underlying Ratatui buffer.
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }

    /// Convert layout engine's `LayoutRect` to Ratatui's `Rect`.
    /// This is the SINGLE conversion point between spatial types.
    pub fn layout_rect_to_ratatui_rect(rect: &term_wm_layout_engine::LayoutRect) -> RatatuiRect {
        RatatuiRect {
            x: rect.x as u16,
            y: rect.y as u16,
            width: rect.width,
            height: rect.height,
        }
    }
}
