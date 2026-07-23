use std::collections::VecDeque;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect as RatatuiRect;
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::component_context::{ComponentContext, ScrollViewport};
use term_wm_core::components::{Component, SelectionStatus};
use term_wm_core::events::Event;
use term_wm_core::hitbox_registry::HitboxRegistry;
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;

/// Determines how `CanvasScrollView` computes its virtual canvas width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasSizingPolicy {
    /// Lock `content_width` to `area.width`.  Prevents horizontal scrollbars
    /// on 1D vertical stacks (lists, forms, panels).
    FitViewportWidth,
    /// Fixed virtual width for 2D scrollable content (wide tables, ASCII art).
    Fixed(u16),
}

/// Offscreen canvas adapter for `ScrollViewComponent`.
///
/// Renders `inner` into a full-size offscreen buffer at `(0, 0)` and blits
/// only the visible slice to the screen.  Eliminates manual scroll-offset
/// math in child components.
pub struct CanvasScrollView<C> {
    pub inner: C,
    pub sizing_policy: CanvasSizingPolicy,
    content_width: u16,
    content_height: u16,
    offscreen_buf: Buffer,
}

impl<C> CanvasScrollView<C> {
    pub fn new(inner: C, policy: CanvasSizingPolicy) -> Self {
        Self {
            inner,
            sizing_policy: policy,
            content_width: 0,
            content_height: 0,
            offscreen_buf: Buffer::empty(RatatuiRect::new(0, 0, 1, 1)),
        }
    }

    fn effective_width(&self, viewport_width: u16) -> u16 {
        match self.sizing_policy {
            CanvasSizingPolicy::FitViewportWidth => viewport_width,
            CanvasSizingPolicy::Fixed(w) => w,
        }
    }
}

impl<C: Component<TermWmAction>> Component<TermWmAction> for CanvasScrollView<C> {
    fn desired_height(&self, width: u16) -> u16 {
        let eff_w = self.effective_width(width);
        self.inner.desired_height(eff_w)
    }

    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut HitboxRegistry,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let virtual_width = self.effective_width(area.width);
        let virtual_height = self.inner.desired_height(virtual_width);

        if virtual_width == 0 || virtual_height == 0 {
            return;
        }

        self.content_width = virtual_width;
        self.content_height = virtual_height;

        if let Some(handle) = ctx.scroll_handle() {
            handle.set_content_size(virtual_width as usize, virtual_height as usize);
        }

        let vp = ctx.viewport();
        let offset_x = vp.offset_x;
        let offset_y = vp.offset_y;

        let canvas_ctx = ctx
            .with_viewport(
                ScrollViewport {
                    offset_x: 0,
                    offset_y: 0,
                    width: virtual_width as usize,
                    height: virtual_height as usize,
                },
                None,
            )
            .with_screen_area(LayoutRect {
                x: 0,
                y: 0,
                width: virtual_width,
                height: virtual_height,
            })
            .with_direct_mode(false);

        let virtual_area = LayoutRect {
            x: 0,
            y: 0,
            width: virtual_width,
            height: virtual_height,
        };

        // Resize buffer BEFORE taking ownership — prevents stale dimension desync
        if self.offscreen_buf.area.width != virtual_width
            || self.offscreen_buf.area.height != virtual_height
        {
            self.offscreen_buf = Buffer::empty(RatatuiRect::new(
                0,
                0,
                virtual_width.max(1),
                virtual_height.max(1),
            ));
        } else {
            self.offscreen_buf.reset();
        }

        let temp_buf = std::mem::take(&mut self.offscreen_buf);
        let ratatui_area = RatatuiRect::new(0, 0, virtual_width, virtual_height);
        let mut mock_backend = term_wm_console::RatatuiBackend::new(temp_buf, ratatui_area);

        let mut scratch_registry = HitboxRegistry::new();
        self.inner.render(
            &mut mock_backend,
            virtual_area,
            &canvas_ctx,
            &mut scratch_registry,
        );
        self.offscreen_buf = mock_backend.buffer;

        let translation_x = area.x - offset_x as i32;
        let translation_y = area.y - offset_y as i32;
        registry.merge_with_transform(scratch_registry, translation_x, translation_y, area);

        let dst_buf = if let Some(rb) = backend
            .as_any_mut()
            .downcast_mut::<term_wm_console::RatatuiBackend>()
        {
            &mut rb.buffer
        } else {
            return;
        };

        let src_start_x = offset_x as u16;
        let src_start_y = offset_y as u16;
        let copy_width = area.width.min(virtual_width.saturating_sub(src_start_x));
        let copy_height = area.height.min(virtual_height.saturating_sub(src_start_y));

        for y in 0..copy_height {
            let dst_y = area.y + y as i32;
            if dst_y < 0 || dst_y >= dst_buf.area.height as i32 {
                continue;
            }
            for x in 0..copy_width {
                let dst_x = area.x + x as i32;
                if dst_x < 0 || dst_x >= dst_buf.area.width as i32 {
                    continue;
                }
                let src_cell = &self.offscreen_buf[(src_start_x + x, src_start_y + y)];
                dst_buf[(dst_x as u16, dst_y as u16)] = src_cell.clone();
            }
        }
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        let screen_area = ctx.screen_area().unwrap_or_default();

        // Pre-render fallback: compute dimensions if first render hasn't run yet
        let cw = if self.content_width == 0 {
            self.effective_width(screen_area.width)
        } else {
            self.content_width
        };
        let ch = if self.content_height == 0 {
            self.inner.desired_height(cw)
        } else {
            self.content_height
        };
        if cw == 0 || ch == 0 {
            return EventResult::Ignored;
        }

        let virtual_ctx = ctx
            .with_viewport(
                ScrollViewport {
                    offset_x: 0,
                    offset_y: 0,
                    width: cw as usize,
                    height: ch as usize,
                },
                None,
            )
            .with_screen_area(LayoutRect {
                x: 0,
                y: 0,
                width: cw,
                height: ch,
            });

        // Translate mouse events using canonical centralized method
        let Event::Mouse(mouse) = event else {
            return self.inner.handle_events(event, &virtual_ctx);
        };
        let vp = ctx.viewport();
        let Some(translated) = mouse.to_local_offset(screen_area, vp.offset_x, vp.offset_y, cw, ch)
        else {
            return EventResult::Ignored;
        };
        self.inner
            .handle_events(&Event::Mouse(translated), &virtual_ctx)
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        self.inner.update(action, ctx, actions);
    }

    fn destroy(&mut self) {
        self.inner.destroy();
    }
    fn clear_selection(&mut self) {
        self.inner.clear_selection();
    }
    fn selection_status(&self) -> SelectionStatus {
        self.inner.selection_status()
    }
    fn selection_text(&self) -> Option<String> {
        self.inner.selection_text()
    }
    fn set_selection_enabled(&mut self, enabled: bool) {
        self.inner.set_selection_enabled(enabled);
    }
    fn paste(&mut self, text: &str) -> bool {
        self.inner.paste(text)
    }
}
