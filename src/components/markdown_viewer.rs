use std::str;

use crossterm::event::Event;
use pulldown_cmark::{Event as MdEvent, Options, Parser, Tag};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};

use crate::components::{Component, scroll_view::ScrollViewComponent};
use crate::ui::UiFrame;

#[derive(Debug)]
pub struct MarkdownViewerComponent {
    text: Text<'static>,
    scroll: ScrollViewComponent,
    total_lines: usize,
}

impl Component for MarkdownViewerComponent {
    fn resize(&mut self, area: Rect) {
        // Respect the allocated height so scrollbar calculations are stable.
        self.scroll.set_fixed_height(Some(area.height));
    }

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, _focused: bool) {
        self.render_content(frame, area);
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Mouse(_) => self.handle_pointer_event(event),
            Event::Key(key) => self.handle_key_event(key),
            _ => false,
        }
    }
}

impl MarkdownViewerComponent {
    pub fn new() -> Self {
        Self {
            text: Text::from(vec![Line::from(String::new())]),
            scroll: ScrollViewComponent::new(),
            total_lines: 0,
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut mv = Self::new();
        if let Ok(s) = str::from_utf8(bytes) {
            mv.set_markdown(s);
        }
        mv
    }

    pub fn set_markdown(&mut self, raw: &str) {
        let parser = Parser::new_ext(raw, Options::all());

        let mut lines: Vec<Vec<Span>> = Vec::new();
        let mut current: Vec<Span> = Vec::new();

        // list numbering handled via `list_start`/`list_count` vectors

        let mut list_start: Vec<Option<usize>> = Vec::new();
        let mut list_count: Vec<usize> = Vec::new();
        #[derive(Debug, Clone, Copy)]
        enum TagKind {
            Strong,
            Emphasis,
            List,
            Item,
            CodeBlock,
            Paragraph,
            Other,
        }
        let mut tag_stack: Vec<TagKind> = Vec::new();
        let mut bold = false;
        let mut italic = false;
        let mut in_code_block = false;

        for ev in parser {
            match ev {
                MdEvent::Start(tag) => match tag {
                    Tag::Strong => {
                        tag_stack.push(TagKind::Strong);
                        bold = true;
                    }
                    Tag::Emphasis => {
                        tag_stack.push(TagKind::Emphasis);
                        italic = true;
                    }
                    Tag::List(start) => {
                        tag_stack.push(TagKind::List);
                        list_start.push(start.map(|n| n as usize));
                        list_count.push(0);
                    }
                    Tag::Item => {
                        tag_stack.push(TagKind::Item);
                        if let Some(last) = list_count.last_mut() {
                            *last = last.saturating_add(1);
                        }
                        let indent = "  ".repeat(list_count.len().saturating_sub(1));
                        let bullet = if let Some(start) = list_start.last().and_then(|s| *s) {
                            // ordered
                            let idx = list_count.last().copied().unwrap_or(1);
                            format!("{}{}.", indent, start + idx - 1)
                        } else {
                            format!("{}- ", indent)
                        };
                        current.push(Span::raw(bullet));
                    }
                    Tag::CodeBlock(_) => {
                        tag_stack.push(TagKind::CodeBlock);
                        in_code_block = true;
                    }
                    Tag::Paragraph => {
                        tag_stack.push(TagKind::Paragraph);
                    }
                    _ => tag_stack.push(TagKind::Other),
                },
                MdEvent::End(_) => {
                    if let Some(kind) = tag_stack.pop() {
                        match kind {
                            TagKind::Strong => bold = false,
                            TagKind::Emphasis => italic = false,
                            TagKind::Item => {
                                lines.push(std::mem::take(&mut current));
                            }
                            TagKind::List => {
                                list_start.pop();
                                list_count.pop();
                            }
                            TagKind::CodeBlock => in_code_block = false,
                            TagKind::Paragraph => {
                                lines.push(std::mem::take(&mut current));
                                lines.push(vec![Span::raw("")]);
                            }
                            TagKind::Other => {}
                        }
                    }
                }
                MdEvent::Text(text) => {
                    let mut style = Style::default();
                    if bold {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    if italic {
                        style = style.add_modifier(Modifier::ITALIC);
                    }
                    if in_code_block {
                        style = Style::default().fg(Color::Yellow);
                    }
                    current.push(Span::styled(text.to_string(), style));
                }
                MdEvent::Code(text) => {
                    current.push(Span::styled(
                        text.to_string(),
                        Style::default().fg(Color::Yellow),
                    ));
                }
                MdEvent::SoftBreak => {
                    current.push(Span::raw(" "));
                }
                MdEvent::HardBreak => {
                    lines.push(std::mem::take(&mut current));
                }
                MdEvent::Rule => {
                    lines.push(vec![Span::raw("─")]);
                }
                _ => {}
            }
        }

        if !current.is_empty() {
            lines.push(current);
        }
        if lines.is_empty() {
            lines.push(vec![Span::raw("")]);
        }

        let owned_lines: Vec<Line> = lines.into_iter().map(Line::from).collect();
        self.total_lines = owned_lines.len();
        self.text = Text::from(owned_lines);
    }

    pub fn set_markdown_bytes(&mut self, bytes: &[u8]) {
        if let Ok(s) = str::from_utf8(bytes) {
            self.set_markdown(s);
        }
    }

    pub fn handle_pointer_event(&mut self, event: &Event) -> bool {
        // Delegate pointer/scrollbar interactions to the shared ScrollViewComponent implementation.
        let response = self.scroll.handle_event(event);
        if let Some(offset) = response.offset {
            self.scroll.set_offset(offset);
        }
        if response.handled {
            self.scroll
                .set_total_view(self.total_lines, self.scroll.view());
        }
        response.handled
    }

    // Programmatic scrolling helpers; callers (e.g. overlay) decide which keys map here.
    pub fn page_up(&mut self) {
        let page = self.scroll.view().max(1);
        let off = self.scroll.offset().saturating_sub(page);
        self.scroll.set_offset(off);
    }

    pub fn page_down(&mut self) {
        let page = self.scroll.view().max(1);
        let off = self.scroll.offset().saturating_add(page);
        self.scroll
            .set_offset(off.min(self.total_lines.saturating_sub(self.scroll.view())));
    }

    pub fn scroll_up(&mut self) {
        let off = self.scroll.offset().saturating_sub(1);
        self.scroll.set_offset(off);
    }

    pub fn scroll_down(&mut self) {
        let off = self.scroll.offset().saturating_add(1);
        self.scroll
            .set_offset(off.min(self.total_lines.saturating_sub(self.scroll.view())));
    }

    pub fn go_home(&mut self) {
        self.scroll.set_offset(0);
    }

    pub fn go_end(&mut self) {
        let max_off = self.total_lines.saturating_sub(self.scroll.view());
        self.scroll.set_offset(max_off);
    }

    /// Enable or disable the ScrollViewComponent's keyboard handling for this viewer.
    pub fn set_keyboard_enabled(&mut self, enabled: bool) {
        self.scroll.set_keyboard_enabled(enabled);
    }

    /// Pass through keyboard events to the internal ScrollViewComponent handler.
    pub fn handle_key_event(&mut self, key: &crossterm::event::KeyEvent) -> bool {
        self.scroll.handle_key_event(key)
    }

    pub fn render_content(&mut self, frame: &mut UiFrame<'_>, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let total = self.total_lines;
        let view = area.height as usize;
        self.scroll.update(area, total, view);
        // If a vertical scrollbar will be rendered, reserve one column on the
        // right so the scrollbar does not overlay content. This makes text
        // wrapping behave as expected and avoids characters being obscured.
        let scrollbar_visible = total > self.scroll.view() && view > 0 && area.height > 0;
        let content_area = if scrollbar_visible && area.width > 0 {
            Rect {
                x: area.x,
                y: area.y,
                width: area.width.saturating_sub(1),
                height: area.height,
            }
        } else {
            area
        };

        let mut paragraph = Paragraph::new(self.text.clone()).wrap(Wrap { trim: false });
        paragraph = paragraph.scroll((self.scroll.offset() as u16, 0));
        frame.render_widget(paragraph, content_area);
        self.scroll.render(frame);
    }
}

impl Default for MarkdownViewerComponent {
    fn default() -> Self {
        Self::new()
    }
}
