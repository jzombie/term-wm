use ratatui::layout::Rect;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Paragraph, Wrap};

use crate::components::{Component, scroll_view::ScrollViewComponent};
use crate::ui::UiFrame;

#[derive(Debug)]
pub struct TextRendererComponent {
    text: Text<'static>,
    scroll: ScrollViewComponent,
    wrap: bool,
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
            let longest = self.text.lines.iter().map(|l| l.width()).max().unwrap_or(0) as usize;
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
                if let Some(off) = resp.offset {
                    self.scroll.set_offset(off);
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
        }
    }

    pub fn set_text(&mut self, text: Text<'static>) {
        self.text = text;
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
}

fn compute_display_lines(text: &Text<'_>, width: u16) -> usize {
    let usable = width.max(1) as usize;
    text.lines
        .iter()
        .map(|line| {
            let w = line.width();
            if w == 0 { 1 } else { (w + usable - 1) / usable }
        })
        .sum::<usize>()
        .max(1)
}

impl Default for TextRendererComponent {
    fn default() -> Self {
        Self::new()
    }
}
