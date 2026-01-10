use std::collections::HashMap;
use std::fmt;

use crossterm::event::MouseEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Widget, Wrap};

use crate::component_context::{ViewportContext, ViewportHandle};
use crate::components::{
    Component, ComponentContext,
    selectable_text::{
        LogicalPosition, SelectionController, SelectionHost, SelectionViewport,
        handle_selection_mouse, maintain_selection_drag,
    },
};
use crate::linkifier::LinkifiedText;
use crate::ui::UiFrame;

pub struct TextRendererComponent {
    text: Text<'static>,
    wrap: bool,
    link_map: Vec<Vec<Option<String>>>,
    selection: SelectionController,
    selection_enabled: bool,
    last_area: Rect,
    viewport_handle: Option<ViewportHandle>,
    viewport_cache: ViewportContext,
    content_area: Rect,
    content_width: usize,
    content_height: usize,
}

impl fmt::Debug for TextRendererComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextRendererComponent").finish()
    }
}

impl Component for TextRendererComponent {
    fn resize(&mut self, area: Rect, _ctx: &ComponentContext) {
        self.last_area = area;
    }

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, ctx: &ComponentContext) {
        self.apply_focus_state(ctx.focused());
        if area.width == 0 || area.height == 0 {
            self.last_area = Rect::default();
            self.content_area = Rect::default();
            return;
        }

        self.last_area = area;
        self.content_area = area;
        self.viewport_cache = ctx.viewport();
        self.viewport_handle = ctx.viewport_handle();

        let viewport_cache = self.viewport_cache;
        let viewport_handle = &self.viewport_handle;

        // Calculate Metrics
        let usable_width = area.width.max(1) as usize;
        let content_height = if self.wrap {
            compute_display_lines(&self.text, usable_width as u16)
        } else {
            self.text.lines.len().max(1)
        };

        let content_width = if self.wrap {
            usable_width
        } else {
            self.text
                .lines
                .iter()
                .map(|line| line.width())
                .max()
                .unwrap_or(0)
        };

        self.content_height = content_height;
        self.content_width = content_width;

        if let Some(handle) = &viewport_handle {
            handle.set_content_size(content_width, content_height);
        }

        maintain_selection_drag(self);

        let v_off = viewport_cache.offset_y as u16;
        let h_off = viewport_cache.offset_x as u16;

        use crate::ui::safe_set_string;

        const RULE_PLACEHOLDER: &str = "\0RULE\0";
        let usable = usable_width;

        // Optimization: Pre-calculate just enough to skip invisible lines?
        // Or reuse existing logic.

        let mut visual_heights: Vec<usize> = Vec::with_capacity(self.text.lines.len());
        for line in &self.text.lines {
            let w = line.width();
            let vh = if w == 0 {
                1
            } else if self.wrap {
                (w + usable - 1).div_euclid(usable)
            } else {
                1
            };
            visual_heights.push(vh);
        }

        let mut cum_visual = 0usize;
        let mut y_cursor = area.y;
        let mut remaining = area.height as usize;

        for (idx, line) in self.text.lines.iter().enumerate() {
            let line_vh = visual_heights.get(idx).copied().unwrap_or(1);
            if cum_visual + line_vh <= v_off as usize {
                cum_visual += line_vh;
                continue;
            }

            let start_in_line = (v_off as usize).saturating_sub(cum_visual);
            let rows_available = line_vh.saturating_sub(start_in_line);
            if rows_available == 0 {
                cum_visual += line_vh;
                continue;
            }

            if remaining == 0 {
                break;
            }

            let rows_to_render = rows_available.min(remaining);
            let is_rule = line.spans.iter().any(|s| s.content == RULE_PLACEHOLDER);

            if is_rule {
                if start_in_line == 0 && rows_to_render > 0 {
                    let sep = "─".repeat(area.width as usize);
                    safe_set_string(
                        frame.buffer_mut(),
                        area,
                        area.x,
                        y_cursor,
                        &sep,
                        Style::default(),
                    );
                    y_cursor = y_cursor.saturating_add(1);
                    remaining = remaining.saturating_sub(1);
                }
                cum_visual += line_vh;
                continue;
            }

            let single_text = Text::from(vec![line.clone()]);
            let mut paragraph = Paragraph::new(single_text);
            if self.wrap {
                paragraph = paragraph.wrap(Wrap { trim: false });
            }
            paragraph = paragraph.scroll((start_in_line as u16, h_off));
            frame.render_widget(
                paragraph,
                Rect {
                    x: area.x,
                    y: y_cursor,
                    width: area.width,
                    height: rows_to_render as u16,
                },
            );

            y_cursor = y_cursor.saturating_add(rows_to_render as u16);
            remaining = remaining.saturating_sub(rows_to_render);
            cum_visual += line_vh;
        }
        self.render_selection_overlay(frame);
    }

    fn handle_event(&mut self, event: &crossterm::event::Event, ctx: &ComponentContext) -> bool {
        self.viewport_cache = ctx.viewport();
        if let Some(handle) = ctx.viewport_handle() {
            self.viewport_handle = Some(handle);
        }

        match event {
            crossterm::event::Event::Mouse(mouse) => {
                handle_selection_mouse(self, self.selection_enabled, mouse)
            }
            crossterm::event::Event::Key(_) => {
                self.selection.clear();
                false
            }
            _ => false,
        }
    }
}

// removed usage of ScrollAreaContent

impl TextRendererComponent {
    pub fn new() -> Self {
        Self {
            text: Text::from(vec![Line::from(String::new())]),
            wrap: true,
            link_map: Vec::new(),
            selection: SelectionController::new(),
            selection_enabled: false,
            last_area: Rect::default(),
            viewport_handle: None,
            viewport_cache: ViewportContext::default(),
            content_area: Rect::default(),
            content_width: 0,
            content_height: 0,
        }
    }

    pub fn set_text(&mut self, text: Text<'static>) {
        self.text = text;
        self.link_map.clear();
    }

    pub fn set_linkified_text(&mut self, linkified: LinkifiedText) {
        self.text = linkified.text;
        self.link_map = linkified.link_map;
    }

    pub fn set_wrap(&mut self, wrap: bool) {
        self.wrap = wrap;
    }

    pub fn set_selection_enabled(&mut self, enabled: bool) {
        if self.selection_enabled == enabled {
            return;
        }
        self.selection_enabled = enabled;
        if !enabled {
            self.selection.clear();
        }
    }

    pub fn jump_to_logical_line(&mut self, line_idx: usize, area: Rect) {
        if self.text.lines.is_empty() || area.width == 0 {
            if let Some(handle) = &self.viewport_handle {
                handle.scroll_vertical_to(0);
            }
            return;
        }

        let usable = area.width.max(1) as usize;
        let mut offset = 0;
        for (i, line) in self.text.lines.iter().enumerate() {
            if i >= line_idx {
                break;
            }
            if self.wrap {
                let w = line.width();
                if w == 0 {
                    offset += 1;
                } else {
                    offset += (w + usable - 1).div_euclid(usable);
                }
            } else {
                offset += 1;
            }
        }
        if let Some(handle) = &self.viewport_handle {
            handle.scroll_vertical_to(offset);
        }
    }

    pub fn scroll_vertical_to(&mut self, offset: usize) {
        if let Some(handle) = &self.viewport_handle {
            handle.scroll_vertical_to(offset);
        }
    }

    pub fn text_ref(&self) -> &Text<'static> {
        &self.text
    }

    pub fn rendered_lines(&self) -> Vec<String> {
        self.text
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect()
    }

    // Internal helper methods

    fn apply_focus_state(&mut self, focused: bool) {
        if !focused {
            self.selection.clear();
        }
    }

    fn logical_position_from_point_impl(&self, column: u16, row: u16) -> Option<LogicalPosition> {
        if self.content_area.width == 0 || self.content_area.height == 0 {
            return None;
        }
        let max_x = self
            .content_area
            .x
            .saturating_add(self.content_area.width)
            .saturating_sub(1);
        let max_y = self
            .content_area
            .y
            .saturating_add(self.content_area.height)
            .saturating_sub(1);
        let clamped_col = column.clamp(self.content_area.x, max_x);
        let clamped_row = row.clamp(self.content_area.y, max_y);
        let local_col = clamped_col.saturating_sub(self.content_area.x) as usize;
        let local_row = clamped_row.saturating_sub(self.content_area.y) as usize;
        let row_base = self.viewport_cache.offset_y;
        let col_base = self.viewport_cache.offset_x;
        Some(LogicalPosition::new(
            row_base.saturating_add(local_row),
            col_base.saturating_add(local_col),
        ))
    }

    fn render_selection_overlay(&mut self, frame: &mut UiFrame<'_>) {
        if !self.selection_enabled {
            return;
        }
        let Some(range) = self
            .selection
            .selection_range()
            .filter(|r| r.is_non_empty())
            .map(|r| r.normalized())
        else {
            return;
        };
        let mut bounds = self.content_area;
        let buffer = frame.buffer_mut();
        bounds = bounds.intersection(buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        let row_base = self.viewport_cache.offset_y;
        let col_base = self.viewport_cache.offset_x;
        for y in bounds.y..bounds.y.saturating_add(bounds.height) {
            let local_row = y.saturating_sub(self.content_area.y) as usize;
            for x in bounds.x..bounds.x.saturating_add(bounds.width) {
                let local_col = x.saturating_sub(self.content_area.x) as usize;
                let pos = LogicalPosition::new(
                    row_base.saturating_add(local_row),
                    col_base.saturating_add(local_col),
                );
                if range.contains(pos)
                    && let Some(cell) = buffer.cell_mut((x, y))
                {
                    let style = cell
                        .style()
                        .bg(crate::theme::selection_bg())
                        .fg(crate::theme::selection_fg());
                    cell.set_style(style);
                }
            }
        }
    }

    fn build_hit_test_palette(&self) -> Option<HitTestPalette> {
        let mut url_ids: HashMap<String, u32> = HashMap::new();
        let mut urls: Vec<String> = Vec::new();
        let mut has_links = false;
        let mut lines: Vec<Line<'static>> = Vec::with_capacity(self.text.lines.len());

        for (line_idx, line) in self.text.lines.iter().enumerate() {
            let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len());
            let line_links = self.link_map.get(line_idx);
            for (span_idx, span) in line.spans.iter().enumerate() {
                let mut new_span = span.clone();
                if let Some(link) = line_links
                    .and_then(|entries| entries.get(span_idx))
                    .and_then(|opt| opt.clone())
                {
                    has_links = true;
                    let id = *url_ids.entry(link.clone()).or_insert_with(|| {
                        urls.push(link.clone());
                        urls.len() as u32
                    });
                    new_span.style = new_span.style.fg(encode_link_color(id));
                }
                spans.push(new_span);
            }
            lines.push(Line::from(spans));
        }

        if !has_links {
            return None;
        }

        Some(HitTestPalette {
            text: Text::from(lines),
            urls,
        })
    }

    pub fn link_at(&self, area: Rect, mouse: MouseEvent) -> Option<String> {
        if self.link_map.is_empty() {
            return None;
        }
        if area.width == 0 || area.height == 0 {
            return None;
        }
        if mouse.column < area.x
            || mouse.column >= area.x.saturating_add(area.width)
            || mouse.row < area.y
            || mouse.row >= area.y.saturating_add(area.height)
        {
            return None;
        }

        let content_width = area.width;
        if content_width == 0 {
            return None;
        }

        let local_x = mouse.column.saturating_sub(area.x);
        let local_y = mouse.row.saturating_sub(area.y);
        if local_x >= content_width || local_y >= area.height {
            return None;
        }

        let hit_palette = self.build_hit_test_palette()?;
        let HitTestPalette { mut text, urls } = hit_palette;
        {
            use std::borrow::Cow;
            const RULE_PLACEHOLDER: &str = "\0RULE\0";
            let repeat_len = content_width as usize;
            if repeat_len > 0 {
                let mut sep = String::with_capacity(repeat_len * 3);
                for i in 0..repeat_len {
                    sep.push('─');
                    if i + 1 < repeat_len {
                        sep.push('\u{2060}');
                    }
                }
                for line in text.lines.iter_mut() {
                    for span in line.spans.iter_mut() {
                        if span.content == RULE_PLACEHOLDER {
                            span.content = Cow::Owned(sep.clone());
                        }
                    }
                }
            }
        }
        if urls.is_empty() {
            return None;
        }

        let mut paragraph = Paragraph::new(text);
        if self.wrap {
            paragraph = paragraph.wrap(Wrap { trim: false });
        }
        let viewport = self.viewport_cache;
        paragraph = paragraph.scroll((viewport.offset_y as u16, viewport.offset_x as u16));

        let mut buffer = Buffer::empty(Rect {
            x: 0,
            y: 0,
            width: content_width,
            height: area.height,
        });
        paragraph.render(
            Rect {
                x: 0,
                y: 0,
                width: content_width,
                height: area.height,
            },
            &mut buffer,
        );

        if let Some(cell) = buffer.cell((local_x, local_y))
            && let Some(id) = decode_link_color(cell.fg)
            && let Some(idx) = id.checked_sub(1)
            && let Some(url) = urls.get(idx as usize)
        {
            return Some(url.clone());
        }

        None
    }

    pub fn reset(&mut self) {
        if let Some(handle) = &self.viewport_handle {
            handle.scroll_vertical_to(0);
            handle.scroll_horizontal_to(0);
        }
    }
}

impl SelectionViewport for TextRendererComponent {
    fn selection_viewport(&self) -> Rect {
        self.content_area
    }

    fn logical_position_from_point(&mut self, column: u16, row: u16) -> Option<LogicalPosition> {
        self.logical_position_from_point_impl(column, row)
    }

    fn scroll_selection_vertical(&mut self, delta: isize) {
        if delta == 0 {
            return;
        }
        if let Some(handle) = &self.viewport_handle {
            handle.scroll_vertical_by(delta);
        }
    }

    fn scroll_selection_horizontal(&mut self, delta: isize) {
        if delta == 0 {
            return;
        }
        if let Some(handle) = &self.viewport_handle {
            handle.scroll_horizontal_by(delta);
        }
    }

    fn selection_viewport_offsets(&self) -> (usize, usize) {
        (self.viewport_cache.offset_x, self.viewport_cache.offset_y)
    }

    fn selection_content_size(&self) -> (usize, usize) {
        (self.content_width, self.content_height)
    }
}

impl SelectionHost for TextRendererComponent {
    fn selection_controller(&mut self) -> &mut SelectionController {
        &mut self.selection
    }
}

fn compute_display_lines(text: &Text<'_>, width: u16) -> usize {
    let usable = width.max(1) as usize;
    text.lines
        .iter()
        .map(|line| {
            let w = line.width();
            if w == 0 {
                1
            } else {
                (w + usable - 1).div_euclid(usable)
            }
        })
        .sum::<usize>()
        .max(1)
}

impl Default for TextRendererComponent {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::ScrollViewComponent;
    use crate::ui::UiFrame;
    use crossterm::event::{
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
        MouseEventKind,
    };
    use ratatui::{buffer::Buffer, layout::Rect, text::Text};

    fn key_event(code: KeyCode) -> KeyEvent {
        let mut ev = KeyEvent::new(code, KeyModifiers::NONE);
        ev.kind = KeyEventKind::Press;
        ev
    }

    #[test]
    fn key_press_clears_selection() {
        let mut comp = TextRendererComponent::new();
        comp.set_selection_enabled(true);
        {
            comp.selection_controller()
                .begin_drag(LogicalPosition::new(0, 0));
            comp.selection_controller()
                .update_drag(LogicalPosition::new(0, 5));
            assert!(comp.selection_controller().has_selection());
        }

        let handled = comp.handle_event(
            &Event::Key(key_event(KeyCode::Char('a'))),
            &ComponentContext::new(true),
        );
        assert!(!handled);
        assert!(!comp.selection_controller().has_selection());
    }

    #[test]
    fn selection_drag_auto_scrolls_left_at_edge() {
        use ratatui::text::Line;
        let mut renderer = TextRendererComponent::new();
        renderer.set_selection_enabled(true);
        renderer.set_wrap(false);
        let long_line = Line::from("0123456789".repeat(20));
        renderer.set_text(Text::from(vec![long_line]));
        let mut scroll_view = ScrollViewComponent::new(renderer);
        let area = Rect::new(0, 0, 20, 3);
        let mut buffer = Buffer::empty(area);
        {
            let mut frame = UiFrame::from_parts(area, &mut buffer);
            scroll_view.render(&mut frame, area, &ComponentContext::new(true));
        }

        scroll_view.viewport_handle().scroll_horizontal_to(25);
        let ctx = ComponentContext::new(true);
        let down = Event::Mouse(MouseEvent {
            column: 10,
            row: 1,
            kind: MouseEventKind::Down(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
        });
        scroll_view.handle_event(&down, &ctx);
        let before = scroll_view.viewport_handle().info().offset_x;
        assert!(before > 0);

        let drag = Event::Mouse(MouseEvent {
            column: 0,
            row: 1,
            kind: MouseEventKind::Drag(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
        });
        scroll_view.handle_event(&drag, &ctx);
        let after = scroll_view.viewport_handle().info().offset_x;
        assert!(
            after < before,
            "expected horizontal auto-scroll towards origin"
        );
    }

    #[test]
    fn blur_clears_selection() {
        let mut comp = TextRendererComponent::new();
        {
            comp.selection_controller()
                .begin_drag(LogicalPosition::new(0, 0));
            comp.selection_controller()
                .update_drag(LogicalPosition::new(0, 2));
            assert!(comp.selection_controller().has_selection());
            comp.apply_focus_state(false);
            assert!(!comp.selection_controller().has_selection());
        }
    }

    #[test]
    fn scrollbar_drag_bypasses_selection() {
        let mut renderer = TextRendererComponent::new();
        renderer.set_selection_enabled(true);
        let lines: Vec<Line<'static>> = (0..20)
            .map(|idx| Line::from(format!("line {idx}")))
            .collect();
        renderer.set_text(Text::from(lines));
        let mut scroll_view = ScrollViewComponent::new(renderer);
        let area = Rect::new(0, 0, 20, 5);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        {
            let mut frame = crate::ui::UiFrame::from_parts(area, &mut buffer);
            scroll_view.render(&mut frame, area, &ComponentContext::new(true));
        }

        let scrollbar_x = area.x + area.width.saturating_sub(1);
        let handled = scroll_view.handle_event(
            &Event::Mouse(MouseEvent {
                column: scrollbar_x,
                row: area.y + 1,
                kind: MouseEventKind::Down(MouseButton::Left),
                modifiers: KeyModifiers::NONE,
            }),
            &ComponentContext::new(true),
        );

        assert!(handled);
        assert!(!scroll_view.content.selection_controller().is_dragging());
    }
}

#[derive(Debug)]
struct HitTestPalette {
    text: Text<'static>,
    urls: Vec<String>,
}

fn encode_link_color(id: u32) -> Color {
    debug_assert!(id > 0 && id <= 0x00FF_FFFF, "hit-test color id overflow");
    let r = ((id >> 16) & 0xFF) as u8;
    let g = ((id >> 8) & 0xFF) as u8;
    let b = (id & 0xFF) as u8;
    Color::Rgb(r, g, b)
}

fn decode_link_color(color: Color) -> Option<u32> {
    match color {
        Color::Rgb(r, g, b) => {
            let id = ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
            if id == 0 { None } else { Some(id) }
        }
        _ => None,
    }
}
