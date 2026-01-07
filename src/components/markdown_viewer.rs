use std::str;

use crossterm::event::Event;
use pulldown_cmark::{Event as MdEvent, Options, Parser, Tag};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};

use crate::components::{Component, TextRendererComponent};
use crate::ui::UiFrame;

#[derive(Debug)]
pub struct MarkdownViewerComponent {
    renderer: TextRendererComponent,
}

impl Component for MarkdownViewerComponent {
    fn resize(&mut self, area: Rect) {
        // Respect the allocated height so scrollbar calculations are stable.
        self.renderer.resize(area);
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
            renderer: TextRendererComponent::new(),
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
            Heading,
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
                    Tag::Heading { .. } => {
                        tag_stack.push(TagKind::Heading);
                        bold = true;
                    }
                    _ => tag_stack.push(TagKind::Other),
                },
                MdEvent::End(_) => {
                    if let Some(kind) = tag_stack.pop() {
                        match kind {
                            TagKind::Strong => bold = false,
                            TagKind::Emphasis => italic = false,
                            TagKind::Item => {
                                if !current.is_empty() {
                                    lines.push(std::mem::take(&mut current));
                                }
                            }
                            TagKind::List => {
                                list_start.pop();
                                list_count.pop();
                                let in_parent_list_item =
                                    tag_stack.iter().any(|k| matches!(k, TagKind::Item));
                                if !in_parent_list_item {
                                    lines.push(vec![Span::raw("")]);
                                }
                            }
                            TagKind::CodeBlock => in_code_block = false,
                            TagKind::Paragraph => {
                                lines.push(std::mem::take(&mut current));
                                let in_list_item =
                                    tag_stack.iter().any(|k| matches!(k, TagKind::Item));
                                if !in_list_item {
                                    lines.push(vec![Span::raw("")]);
                                }
                            }
                            TagKind::Heading => {
                                bold = false;
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
                    if in_code_block {
                        lines.push(std::mem::take(&mut current));
                    } else {
                        current.push(Span::raw(" "));
                    }
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
        let text = Text::from(owned_lines);
        self.renderer.set_text(text);
        self.renderer.set_wrap(true);
    }

    pub fn set_markdown_bytes(&mut self, bytes: &[u8]) {
        if let Ok(s) = str::from_utf8(bytes) {
            self.set_markdown(s);
        }
    }

    pub fn handle_pointer_event(&mut self, event: &Event) -> bool {
        self.renderer.handle_event(event)
    }

    pub fn page_up(&mut self) {
        let page = self.renderer.view().max(1);
        let off = self.renderer.offset().saturating_sub(page);
        self.renderer.set_offset(off);
    }

    pub fn page_down(&mut self) {
        let page = self.renderer.view().max(1);
        let off = self.renderer.offset().saturating_add(page);
        self.renderer.set_offset(off);
    }

    pub fn scroll_up(&mut self) {
        let off = self.renderer.offset().saturating_sub(1);
        self.renderer.set_offset(off);
    }

    pub fn scroll_down(&mut self) {
        let off = self.renderer.offset().saturating_add(1);
        self.renderer.set_offset(off);
    }

    pub fn go_home(&mut self) {
        self.renderer.set_offset(0);
    }

    pub fn go_end(&mut self) {
        // set to a large offset; TextRenderer will clamp to max
        self.renderer.set_offset(usize::MAX);
    }

    pub fn set_keyboard_enabled(&mut self, enabled: bool) {
        self.renderer.set_keyboard_enabled(enabled);
    }

    pub fn handle_key_event(&mut self, key: &crossterm::event::KeyEvent) -> bool {
        // Delegate keyboard handling to renderer
        self.renderer.handle_event(&Event::Key(*key))
    }

    pub fn render_content(&mut self, frame: &mut UiFrame<'_>, area: Rect) {
        self.renderer.render(frame, area, false);
    }
}

impl Default for MarkdownViewerComponent {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod markdown_tests {
    use super::*;
    use indoc::indoc;

    const SAMPLE_HELP_MD: &str = indoc! {
        "
        Lorem ipsum dolor sit amet, consectetur adipiscing elit. Integer nec odio.
        Praesent libero. Sed cursus ante dapibus diam. Sed nisi. Nulla quis sem at nibh
        elementum imperdiet. Duis sagittis ipsum. Praesent mauris.

        - Alpha: first command example
        - Beta: second command example with a bit more text to force wrapping
        - Gamma: another brief example

        Curabitur sodales ligula in libero. Sed dignissim lacinia nunc. Curabitur tortor.
        Pellentesque nibh. Aenean quam. In scelerisque sem at dolor.

        _Notes:_
        - This is a lorem ipsum note used for tests.
        - Another note to validate list rendering and wrapping behavior.
        "
    };

    fn sample_viewer() -> MarkdownViewerComponent {
        let mut mv = MarkdownViewerComponent::new();
        mv.set_markdown(SAMPLE_HELP_MD);
        mv
    }

    #[test]
    fn help_md_contains_notes_and_list_items() {
        let mv = sample_viewer();
        let rendered: Vec<String> = mv.renderer.rendered_lines();
        assert!(
            rendered.iter().any(|line| line.contains("Notes:")),
            "help.md should contain 'Notes:'"
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.to_lowercase().contains("lorem ipsum"))
                || rendered
                    .iter()
                    .any(|line| line.to_lowercase().contains("lorem")),
            "help.md should contain lorem ipsum notes"
        );
    }

    #[test]
    fn notes_lines_visible_at_bottom_scroll() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let mut mv = sample_viewer();

        let area = Rect {
            x: 0,
            y: 0,
            width: 50,
            height: 12,
        };
        let mut scratch = Buffer::empty(area);
        {
            let mut frame = crate::ui::UiFrame::from_parts(area, &mut scratch);
            mv.render_content(&mut frame, area);
        }

        let mut buffer = Buffer::empty(area);
        {
            mv.go_end();
            let mut frame = crate::ui::UiFrame::from_parts(area, &mut buffer);
            mv.render_content(&mut frame, area);
        }

        let mut found_mouse_note = false;
        let mut found_panel_note = false;
        for y in 0..area.height {
            let mut row = String::new();
            for x in 0..area.width {
                if let Some(cell) = buffer.cell((x, y)) {
                    row.push_str(cell.symbol());
                }
            }
            if row.contains("- This is a lorem ipsum note") {
                found_mouse_note = true;
            }
            if row.contains("- Another note to validate list rendering") {
                found_panel_note = true;
            }
        }
        assert!(
            found_mouse_note,
            "Mouse interactions note should render at bottom"
        );
        assert!(found_panel_note, "Panel menu note should render at bottom");
    }

    #[test]
    fn scrollbar_does_not_overlay_text() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::widgets::{Paragraph, Wrap};

        let mut mv = sample_viewer();

        // choose a narrow viewport so a scrollbar will be required
        let area = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 8,
        };

        let mut with_scroll = Buffer::empty(area);
        {
            let mut frame = crate::ui::UiFrame::from_parts(area, &mut with_scroll);
            mv.render_content(&mut frame, area);
        }

        // verify that our viewer actually needed a scrollbar
        // render once into buffer with renderer (which includes scrollbar)

        // render the paragraph into a buffer sized to the content area (width - 1)
        let content_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width.saturating_sub(1),
            height: area.height,
        };
        let mut content_buf = Buffer::empty(content_area);
        {
            let mut frame = crate::ui::UiFrame::from_parts(content_area, &mut content_buf);
            let mut paragraph =
                Paragraph::new(mv.renderer.text_ref().clone()).wrap(Wrap { trim: false });
            paragraph = paragraph.scroll((mv.renderer.offset() as u16, 0));
            frame.render_widget(paragraph, content_area);
        }

        // ensure the content columns match between the two buffers (i.e. paragraph wasn't drawn into the last column)
        for y in 0..area.height {
            for x in 0..content_area.width {
                let a = with_scroll
                    .cell((x, y))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_default();
                let b = content_buf
                    .cell((x, y))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_default();
                assert_eq!(a, b, "Mismatch at ({},{})", x, y);
            }
        }

        // verify there's something in the last column (scrollbar glyphs) and it's distinct from content
        let scrollbar_x = area.x + area.width.saturating_sub(1);
        let mut found_scrollbar = false;
        for y in 0..area.height {
            if let Some(cell) = with_scroll.cell((scrollbar_x, y)) {
                let sym = cell.symbol();
                if sym != " " {
                    found_scrollbar = true;
                }
            }
        }
        assert!(
            found_scrollbar,
            "Expected scrollbar glyphs present in last column"
        );
    }

    #[test]
    fn list_separates_from_following_paragraph() {
        let md = indoc! {
            "
            - item one
            - item two

            Next paragraph begins here.
            "
        };
        let mut mv = MarkdownViewerComponent::new();
        mv.set_markdown(md);
        let rendered = mv.renderer.rendered_lines();
        let idx = rendered
            .iter()
            .position(|line| line.contains("item two"))
            .expect("list text present");
        assert_eq!(
            rendered.get(idx + 1).map(|s| s.as_str()),
            Some(""),
            "expected blank line after list"
        );
        assert!(
            rendered
                .get(idx + 2)
                .map(|s| s.contains("Next paragraph"))
                .unwrap_or(false),
            "paragraph should follow after blank line"
        );
    }
}
