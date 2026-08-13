use std::collections::VecDeque;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext, SelectionStatus};
use term_wm_core::events::{Event, MouseEventKind};
use term_wm_core::hitbox_registry::{HitboxId, HitboxRegistry};
use term_wm_core::theme::Color;
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::helpers::{color_to_ratatui, downcast_ratatui, layout_rect_to_clipped_rect};

/// A div-like bordered card: draws a border (with an optional title in the
/// top-left) and optional padding around a single content component.
///
/// `border: false` renders as a transparent group wrapper (padding only). The
/// border color is a [`Color`], converted to the
/// renderer's color at draw time so consumers never import the renderer.
///
/// `desired_height` returns `0` (stretch) when the content stretches — a
/// stretching child fills the box interior — else `2·border_inset + 2·padding +
/// content`.
pub struct BoxComponent<C: Component<TermWmAction>> {
    content: C,
    title: Option<String>,
    padding: u16,
    border: bool,
    border_color: Color,
}

impl<C: Component<TermWmAction>> BoxComponent<C> {
    pub fn new(content: C) -> Self {
        Self {
            content,
            title: None,
            padding: 0,
            border: true,
            border_color: Color::DarkGray,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_padding(mut self, padding: u16) -> Self {
        self.padding = padding;
        self
    }

    pub fn with_border(mut self, border: bool) -> Self {
        self.border = border;
        self
    }

    pub fn with_border_color(mut self, color: Color) -> Self {
        self.border_color = color;
        self
    }

    /// Border cells per side: `1` when the border is drawn, else `0`.
    fn border_inset(&self) -> u16 {
        u16::from(self.border)
    }

    /// The content rect inset by the border (when drawn) plus padding on each
    /// side. Recompute per call — never cache.
    fn inner_rect(&self, area: LayoutRect) -> LayoutRect {
        let inset = self.border_inset().saturating_add(self.padding);
        LayoutRect {
            x: area.x.saturating_add(i32::from(inset)),
            y: area.y.saturating_add(i32::from(inset)),
            width: area.width.saturating_sub(inset.saturating_mul(2)),
            height: area.height.saturating_sub(inset.saturating_mul(2)),
        }
    }
}

/// Render one border row at `rect.y + y`, clipped to the row's width.
fn render_row(
    rect: ratatui::layout::Rect,
    y: u16,
    text: String,
    color: Color,
    backend: &mut term_wm_console::RatatuiBackend,
) {
    Paragraph::new(Line::from(Span::styled(
        text,
        Style::default().fg(color_to_ratatui(color)),
    )))
    .render(
        ratatui::layout::Rect::new(rect.x, rect.y.saturating_add(y), rect.width, 1),
        &mut backend.buffer,
    );
}

/// The top border row with the title embedded on the left. Truncates
/// character-by-character (never byte-slices) so multi-byte / wide characters
/// can't panic or overflow the row.
fn top_row(title: &Option<String>, inner_w: u16) -> String {
    let Some(title) = title else {
        return format!("╭{}╮", "─".repeat(inner_w as usize));
    };
    let title_width = UnicodeWidthStr::width(title.as_str()) as u16;
    let budget = inner_w.saturating_sub(3);
    let (title, width) = if title_width > budget {
        let mut w: u16 = 0;
        let mut truncated = String::new();
        for ch in title.chars() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
            if w.saturating_add(cw) > budget {
                break;
            }
            w = w.saturating_add(cw);
            truncated.push(ch);
        }
        (truncated, w)
    } else {
        (title.clone(), title_width)
    };
    let fill = inner_w.saturating_sub(width).saturating_sub(3);
    format!("╭─ {title} {}╮", "─".repeat(fill as usize))
}

impl<C: Component<TermWmAction>> Component<TermWmAction> for BoxComponent<C> {
    fn desired_height(&self, width: u16) -> u16 {
        let inset = self.border_inset();
        let inner_w = width
            .saturating_sub(inset.saturating_mul(2))
            .saturating_sub(self.padding.saturating_mul(2));
        let content_h = self.content.desired_height(inner_w);
        if content_h == 0 {
            // A stretching child propagates stretch up the tree.
            return 0;
        }
        inset
            .saturating_mul(2)
            .saturating_add(self.padding.saturating_mul(2))
            .saturating_add(content_h)
    }

    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut HitboxRegistry,
    ) {
        if area.width < 2 || area.height < 2 {
            return;
        }
        let screen = ctx.screen_area().unwrap_or(area);

        if self.border {
            let rect = layout_rect_to_clipped_rect(area);
            let backend = downcast_ratatui(backend);
            let inner_w = area.width.saturating_sub(2);
            render_row(
                rect,
                0,
                top_row(&self.title, inner_w),
                self.border_color,
                backend,
            );
            for y in 1..area.height.saturating_sub(1) {
                render_row(
                    rect,
                    y,
                    format!("│{}│", " ".repeat(inner_w as usize)),
                    self.border_color,
                    backend,
                );
            }
            render_row(
                rect,
                area.height.saturating_sub(1),
                format!("╰{}╯", "─".repeat(inner_w as usize)),
                self.border_color,
                backend,
            );
        }

        let inner_local = self.inner_rect(area);
        if inner_local.width == 0 || inner_local.height == 0 {
            return;
        }
        let inner_screen = self.inner_rect(screen);
        let child_ctx = ctx.clone().with_screen_area(inner_screen);
        self.content
            .render(backend, inner_local, &child_ctx, registry);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match event {
            Event::Mouse(m) => {
                let screen = ctx.screen_area().unwrap_or_default();
                let inner = self.inner_rect(screen);
                let m_x = i32::from(m.column);
                let m_y = i32::from(m.row);
                // Only a press starts an interaction, so the border/padding eats
                // presses on it; drags, releases, moves and scrolls must always
                // reach the content so an in-flight drag survives crossing the
                // border. All mouse events use the inner-rect context so the
                // content sees the SAME geometry in press and non-press events.
                if matches!(m.kind, MouseEventKind::Press(_))
                    && !(m_x >= inner.x
                        && m_x < inner.x.saturating_add(i32::from(inner.width))
                        && m_y >= inner.y
                        && m_y < inner.y.saturating_add(i32::from(inner.height)))
                {
                    return EventResult::Ignored;
                }
                self.content
                    .handle_events(event, &ctx.clone().with_screen_area(inner))
            }
            // Keys are focus-based; no geometry involved.
            _ => self.content.handle_events(event, ctx),
        }
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        self.content.update(action, ctx, actions);
    }

    fn destroy(&mut self) {
        self.content.destroy();
    }

    fn hitbox_id(&self) -> Option<HitboxId> {
        self.content.hitbox_id()
    }

    fn clear_selection(&mut self) {
        self.content.clear_selection();
    }

    fn selection_status(&self) -> SelectionStatus {
        self.content.selection_status()
    }

    fn selection_text(&self) -> Option<String> {
        self.content.selection_text()
    }

    fn take_pending_title(&mut self) -> Option<String> {
        self.content.take_pending_title()
    }

    fn take_alternate_screen_transition(&mut self) -> Option<bool> {
        self.content.take_alternate_screen_transition()
    }

    fn take_teardown_parts(
        &mut self,
    ) -> Option<(
        Box<dyn std::any::Any + Send + Sync>,
        std::thread::JoinHandle<()>,
    )> {
        self.content.take_teardown_parts()
    }

    fn set_selection_enabled(&mut self, enabled: bool) {
        self.content.set_selection_enabled(enabled);
    }

    fn paste(&mut self, text: &str) -> bool {
        self.content.paste(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_wm_core::events::{
        KeyCode, KeyEvent, KeyKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    fn rect(x: i32, y: i32, w: u16, h: u16) -> LayoutRect {
        LayoutRect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    fn make_backend(w: u16, h: u16) -> term_wm_console::RatatuiBackend {
        let buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, w, h));
        term_wm_console::RatatuiBackend::new_simple(buffer, ratatui::layout::Rect::new(0, 0, w, h))
    }

    fn key_event() -> Event {
        Event::Key(KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyKind::Press,
        ))
    }

    fn mouse(kind: MouseEventKind, col: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            modifiers: KeyModifiers::NONE,
            column: col,
            row,
        })
    }

    #[derive(Default)]
    struct SpyChild {
        height: u16,
        hitbox: Option<HitboxId>,
        seen_render: Option<LayoutRect>,
        seen_events: Vec<LayoutRect>,
    }

    impl Component<TermWmAction> for SpyChild {
        fn desired_height(&self, _width: u16) -> u16 {
            self.height
        }

        fn hitbox_id(&self) -> Option<HitboxId> {
            self.hitbox
        }

        fn render(
            &mut self,
            _b: &mut dyn term_wm_render::RenderBackend,
            _a: LayoutRect,
            ctx: &ComponentContext,
            _r: &mut HitboxRegistry,
        ) {
            self.seen_render = ctx.screen_area();
        }

        fn handle_events(
            &mut self,
            _event: &Event,
            ctx: &ComponentContext,
        ) -> EventResult<TermWmAction> {
            self.seen_events.push(ctx.screen_area().unwrap_or_default());
            EventResult::Ignored
        }
    }

    #[test]
    fn box_render_draws_border_and_title_and_insets_content() {
        let mut b = BoxComponent::new(SpyChild {
            height: 1,
            ..Default::default()
        })
        .with_title("Section");
        let mut backend = make_backend(20, 5);
        let ctx = ComponentContext::new(true).with_screen_area(rect(0, 0, 20, 5));
        let mut registry = HitboxRegistry::new();
        b.render(&mut backend, rect(0, 0, 20, 5), &ctx, &mut registry);

        let content: String = backend
            .buffer
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            content.contains("╭─ Section"),
            "top border with title: {content:?}"
        );
        assert!(content.contains('│'), "side border: {content:?}");
        assert!(content.contains('╰'), "bottom border: {content:?}");
        // Content inset by 1 (border) on every side.
        assert_eq!(b.content.seen_render, Some(rect(1, 1, 18, 3)));
    }

    #[test]
    fn box_without_border_renders_no_border_and_pads_only() {
        let mut b = BoxComponent::new(SpyChild {
            height: 1,
            ..Default::default()
        })
        .with_border(false)
        .with_padding(2);
        let mut backend = make_backend(20, 5);
        let ctx = ComponentContext::new(true).with_screen_area(rect(0, 0, 20, 5));
        let mut registry = HitboxRegistry::new();
        b.render(&mut backend, rect(0, 0, 20, 5), &ctx, &mut registry);

        let content: String = backend
            .buffer
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            !content.contains('╭') && !content.contains('╰') && !content.contains('│'),
            "no border glyphs: {content:?}"
        );
        assert_eq!(b.content.seen_render, Some(rect(2, 2, 16, 1)));
    }

    #[test]
    fn box_desired_height_respects_border_and_padding() {
        let on = BoxComponent::new(SpyChild {
            height: 3,
            ..Default::default()
        })
        .with_padding(1);
        assert_eq!(on.desired_height(40), 2 + 2 + 3);
        let off = BoxComponent::new(SpyChild {
            height: 3,
            ..Default::default()
        })
        .with_border(false)
        .with_padding(1);
        assert_eq!(off.desired_height(40), 2 + 3);
    }

    #[test]
    fn box_desired_height_stretch_propagates() {
        let b = BoxComponent::new(SpyChild {
            height: 0,
            ..Default::default()
        });
        assert_eq!(b.desired_height(40), 0);
    }

    #[test]
    fn box_down_inside_forwards_down_on_border_ignored() {
        let mut b = BoxComponent::new(SpyChild {
            height: 1,
            ..Default::default()
        });
        let ctx = ComponentContext::new(true).with_screen_area(rect(0, 0, 20, 5));
        // Inner rect is (1,1,18,3); Press at (5,2) is inside.
        assert!(
            b.handle_events(&mouse(MouseEventKind::Press(MouseButton::Left), 5, 2), &ctx)
                .is_ignored()
        );
        assert_eq!(b.content.seen_events, vec![rect(1, 1, 18, 3)]);
        // Press on the top border (row 0) -> not forwarded.
        assert!(
            b.handle_events(&mouse(MouseEventKind::Press(MouseButton::Left), 5, 0), &ctx)
                .is_ignored()
        );
        assert_eq!(b.content.seen_events.len(), 1);
    }

    #[test]
    fn box_drag_and_up_forward_unconditionally() {
        let mut b = BoxComponent::new(SpyChild {
            height: 1,
            ..Default::default()
        });
        let ctx = ComponentContext::new(true).with_screen_area(rect(0, 0, 20, 5));
        // A Drag or Release anywhere (even on the border) must reach the
        // content so an in-flight drag isn't dropped.
        assert!(
            b.handle_events(&mouse(MouseEventKind::Drag(MouseButton::Left), 0, 0), &ctx)
                .is_ignored()
        );
        assert!(
            b.handle_events(
                &mouse(MouseEventKind::Release(MouseButton::Left), 19, 4),
                &ctx
            )
            .is_ignored()
        );
        assert_eq!(b.content.seen_events.len(), 2);
    }

    #[test]
    fn box_key_forwards() {
        let mut b = BoxComponent::new(SpyChild {
            height: 1,
            ..Default::default()
        });
        let ctx = ComponentContext::new(true).with_screen_area(rect(0, 0, 20, 5));
        assert!(b.handle_events(&key_event(), &ctx).is_ignored());
        assert_eq!(b.content.seen_events.len(), 1);
    }

    #[test]
    fn box_wide_and_utf8_title_truncates_without_panicking() {
        // A wide ASCII title and a multi-byte title must both truncate safely
        // and never exceed the row width.
        for title in ["A very long title that cannot fit in ten columns", "键⌘界"] {
            let mut b = BoxComponent::new(SpyChild {
                height: 1,
                ..Default::default()
            })
            .with_title(title);
            let mut backend = make_backend(12, 3);
            let ctx = ComponentContext::new(true).with_screen_area(rect(0, 0, 12, 3));
            let mut registry = HitboxRegistry::new();
            b.render(&mut backend, rect(0, 0, 12, 3), &ctx, &mut registry);
            let top: String = backend
                .buffer
                .content()
                .iter()
                .take(12)
                .map(|c| c.symbol())
                .collect();
            assert!(top.starts_with('╭'), "top row starts with corner: {top:?}");
            assert!(top.ends_with('╮'), "top row ends with corner: {top:?}");
        }
    }

    #[test]
    fn box_hitbox_forwards() {
        let id = HitboxId::new();
        let b = BoxComponent::new(SpyChild {
            height: 1,
            hitbox: Some(id),
            ..Default::default()
        });
        assert_eq!(b.hitbox_id(), Some(id));
    }

    #[test]
    fn box_tiny_area_renders_nothing_without_panicking() {
        let mut b = BoxComponent::new(SpyChild {
            height: 1,
            ..Default::default()
        });
        let mut backend = make_backend(1, 1);
        let ctx = ComponentContext::new(true);
        let mut registry = HitboxRegistry::new();
        b.render(&mut backend, rect(0, 0, 1, 1), &ctx, &mut registry);
        // Degenerate inner bounds must skip content rendering without panicking.
        let mut b = BoxComponent::new(SpyChild {
            height: 1,
            ..Default::default()
        });
        let mut backend = make_backend(2, 2);
        let ctx = ComponentContext::new(true).with_screen_area(rect(0, 0, 2, 2));
        let mut registry = HitboxRegistry::new();
        b.render(&mut backend, rect(0, 0, 2, 2), &ctx, &mut registry);
    }
}
