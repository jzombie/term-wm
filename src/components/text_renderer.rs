use std::collections::HashMap;

use crossterm::event::MouseEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Widget, Wrap};

use crate::components::{Component, scroll_view::ScrollViewComponent};
use crate::linkifier::LinkifiedText;
use crate::ui::UiFrame;

#[derive(Debug)]
pub struct TextRendererComponent {
    text: Text<'static>,
    scroll: ScrollViewComponent,
    wrap: bool,
    link_map: Vec<Vec<Option<String>>>,
}

impl Component for TextRendererComponent {
    fn resize(&mut self, area: Rect) {
        self.scroll.set_fixed_height(Some(area.height));
    }

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, _focused: bool) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let view = area.height as usize;
        // Determine content width and whether vertical scrollbar is needed.
        let mut content_width = area.width;

        // Compute totals depending on wrap mode.
        let (mut v_total, mut h_total) = if self.wrap {
            // when wrapping, compute display lines after wrapping and set h_total to content width
            let total = compute_display_lines(&self.text, content_width);
            (total, content_width as usize)
        } else {
            // no wrapping: each Text line maps to one visual line; compute longest width for h_total
            let total = self.text.lines.len().max(1);
            let longest = self.text.lines.iter().map(|l| l.width()).max().unwrap_or(0);
            (total, longest)
        };

        let v_scroll_needed = v_total > view && content_width > 0;
        if v_scroll_needed {
            content_width = content_width.saturating_sub(1);
            if self.wrap {
                v_total = compute_display_lines(&self.text, content_width);
            }
        }

        // If wrapping is enabled, horizontal total should reflect the final content width
        if self.wrap {
            h_total = content_width as usize;
        }

        self.scroll.update(area, v_total, view);
        self.scroll
            .set_horizontal_total_view(h_total, content_width as usize);

        let v_off = self.scroll.offset() as u16;
        let h_off = self.scroll.h_offset() as u16;

        let mut paragraph = Paragraph::new(self.text.clone());
        if self.wrap {
            paragraph = paragraph.wrap(Wrap { trim: false });
        }
        paragraph = paragraph.scroll((v_off, h_off));
        frame.render_widget(
            paragraph,
            Rect {
                x: area.x,
                y: area.y,
                width: content_width,
                height: area.height,
            },
        );
        self.scroll.render(frame);
    }

    fn handle_event(&mut self, event: &crossterm::event::Event) -> bool {
        match event {
            crossterm::event::Event::Mouse(_) => {
                let resp = self.scroll.handle_event(event);
                if let Some(off) = resp.v_offset {
                    self.scroll.set_offset(off);
                }
                if let Some(off) = resp.h_offset {
                    self.scroll.set_h_offset(off);
                }
                resp.handled
            }
            crossterm::event::Event::Key(key) => self.scroll.handle_key_event(key),
            _ => false,
        }
    }
}

impl TextRendererComponent {
    pub fn new() -> Self {
        Self {
            text: Text::from(vec![Line::from(String::new())]),
            scroll: ScrollViewComponent::new(),
            wrap: true,
            link_map: Vec::new(),
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

    pub fn set_keyboard_enabled(&mut self, enabled: bool) {
        self.scroll.set_keyboard_enabled(enabled);
    }

    pub fn offset(&self) -> usize {
        self.scroll.offset()
    }

    pub fn set_offset(&mut self, offset: usize) {
        self.scroll.set_offset(offset);
    }

    pub fn view(&self) -> usize {
        self.scroll.view()
    }

    pub fn update(&mut self, area: Rect, total: usize, view: usize) {
        self.scroll.set_fixed_height(Some(area.height));
        self.scroll.update(area, total, view);
    }

    pub fn set_horizontal_total_view(&mut self, total: usize, view: usize) {
        self.scroll.set_horizontal_total_view(total, view);
    }

    pub fn jump_to_logical_line(&mut self, line_idx: usize, area: Rect) {
        if self.text.lines.is_empty() || area.width == 0 {
            self.scroll.set_offset(0);
            return;
        }

        let mut content_width = area.width;
        let view = area.height as usize;

        if self.wrap {
            let v_total = compute_display_lines(&self.text, content_width);
            let v_scroll_needed = v_total > view && content_width > 0;
            if v_scroll_needed {
                content_width = content_width.saturating_sub(1);
            }
        } else {
            let total = self.text.lines.len().max(1);
            let v_scroll_needed = total > view && content_width > 0;
            if v_scroll_needed {
                content_width = content_width.saturating_sub(1);
            }
        }

        let usable = content_width.max(1) as usize;
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
        self.scroll.set_offset(offset);
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

        let mut content_width = area.width;
        let view = area.height as usize;
        let total_lines = if self.wrap {
            compute_display_lines(&self.text, content_width)
        } else {
            self.text.lines.len().max(1)
        };

        let v_scroll_needed = total_lines > view && content_width > 0;
        if v_scroll_needed {
            content_width = content_width.saturating_sub(1);
        }

        if content_width == 0 {
            return None;
        }

        let local_x = mouse.column.saturating_sub(area.x);
        let local_y = mouse.row.saturating_sub(area.y);
        if local_x >= content_width || local_y >= area.height {
            return None;
        }

        let hit_palette = self.build_hit_test_palette()?;
        let HitTestPalette { text, urls } = hit_palette;
        if urls.is_empty() {
            return None;
        }

        let mut paragraph = Paragraph::new(text);
        if self.wrap {
            paragraph = paragraph.wrap(Wrap { trim: false });
        }
        paragraph = paragraph.scroll((self.scroll.offset() as u16, self.scroll.h_offset() as u16));

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
        self.scroll.reset();
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
