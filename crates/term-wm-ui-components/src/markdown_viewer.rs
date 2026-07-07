use std::collections::HashMap;
use std::collections::VecDeque;
use std::fmt;
use std::str;

use crossterm::event::Event;
use pulldown_cmark::{Event as MdEvent, Options, Parser, Tag};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use std::sync::Arc;

use crate::text_renderer::TextRendererComponent;
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::ui::UiFrame;
use term_wm_core::utils::linkifier::{LinkFragment, LinkHandler, Linkifier};
use term_wm_core::window::WindowKey;

pub struct MarkdownViewerComponent {
    text: TextRendererComponent,
    link_handler: Option<LinkHandler>,
    linkifier: Linkifier,
    anchors: HashMap<String, usize>,
}

impl fmt::Debug for MarkdownViewerComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MarkdownViewerComponent")
            .field("text", &"TextRendererComponent")
            .finish()
    }
}

impl Component<TermWmAction> for MarkdownViewerComponent {
    fn render(
        &self,
        frame: &mut UiFrame<'_>,
        area: Rect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        self.render_content(frame, area, ctx, registry);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match event {
            Event::Mouse(_) => self.handle_pointer_event(event, ctx),
            Event::Key(key) => self.handle_key_event(key, ctx),
            _ => EventResult::Ignored,
        }
    }

    fn update(
        &mut self,
        _action: TermWmAction,
        _ctx: &ComponentContext,
        _actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
    }

    fn destroy(&mut self) {}

    fn selection_status(&self) -> term_wm_core::components::SelectionStatus {
        self.text.selection_status()
    }

    fn selection_text(&self) -> Option<String> {
        self.text.selection_text()
    }
}

impl MarkdownViewerComponent {
    pub fn new() -> Self {
        Self {
            text: TextRendererComponent::new(),
            link_handler: None,
            linkifier: Linkifier::new(),
            anchors: HashMap::new(),
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut mv = Self::new();
        if let Ok(s) = str::from_utf8(bytes) {
            mv.set_markdown(s, &term_wm_core::theme::NOIR);
        }
        mv
    }

    pub fn set_link_handler(&mut self, handler: Option<LinkHandler>) {
        self.link_handler = handler;
    }

    pub fn set_link_handler_fn<F>(&mut self, handler: F)
    where
        F: Fn(&str) -> bool + Send + Sync + 'static,
    {
        self.link_handler = Some(Arc::new(handler));
    }

    pub fn reset(&mut self) {
        self.text.reset();
    }

    pub fn set_markdown(&mut self, raw: &str, theme: &term_wm_core::theme::Theme) {
        let parser = Parser::new_ext(raw, Options::all());

        let mut lines: Vec<Vec<LinkFragment>> = Vec::new();
        let mut current: Vec<LinkFragment> = Vec::new();

        self.anchors.clear();

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
            Link,
            Other,
        }

        let mut tag_stack: Vec<TagKind> = Vec::new();
        let mut bold = false;
        let mut italic = false;
        let mut in_code_block = false;
        let mut current_link: Option<String> = None;

        let mut gathering_anchor = false;
        let mut current_anchor_text = String::new();

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
                        current.push(LinkFragment::new(bullet, Style::default(), None));
                    }
                    Tag::CodeBlock(_) => {
                        tag_stack.push(TagKind::CodeBlock);
                        in_code_block = true;
                    }
                    Tag::Paragraph => {
                        tag_stack.push(TagKind::Paragraph);
                    }
                    Tag::Link { dest_url, .. } => {
                        tag_stack.push(TagKind::Link);
                        current_link = Some(dest_url.to_string());
                    }
                    Tag::Heading { .. } => {
                        tag_stack.push(TagKind::Heading);
                        bold = true;
                        gathering_anchor = true;
                        current_anchor_text.clear();
                    }
                    _ => tag_stack.push(TagKind::Other),
                },
                MdEvent::End(_) => {
                    if let Some(kind) = tag_stack.pop() {
                        match kind {
                            TagKind::Strong => bold = false,
                            TagKind::Emphasis => italic = false,
                            TagKind::Item => flush_current_line(&mut lines, &mut current),
                            TagKind::List => {
                                list_start.pop();
                                list_count.pop();
                                let in_parent_list_item =
                                    tag_stack.iter().any(|k| matches!(k, TagKind::Item));
                                if !in_parent_list_item {
                                    push_blank_line(&mut lines);
                                }
                            }
                            TagKind::CodeBlock => in_code_block = false,
                            TagKind::Paragraph => {
                                flush_current_line(&mut lines, &mut current);
                                let in_list_item =
                                    tag_stack.iter().any(|k| matches!(k, TagKind::Item));
                                if !in_list_item {
                                    push_blank_line(&mut lines);
                                }
                            }
                            TagKind::Heading => {
                                bold = false;
                                gathering_anchor = false;
                                let slug = slugify(&current_anchor_text);
                                if !slug.is_empty() {
                                    self.anchors.insert(slug, lines.len());
                                }
                                flush_current_line(&mut lines, &mut current);
                                push_blank_line(&mut lines);
                            }
                            TagKind::Link => {
                                current_link = None;
                            }
                            TagKind::Other => {}
                        }
                    }
                }
                MdEvent::Text(text) => {
                    let mut base_style = Style::default();
                    if bold {
                        base_style = base_style.add_modifier(Modifier::BOLD);
                    }
                    if italic {
                        base_style = base_style.add_modifier(Modifier::ITALIC);
                    }
                    if in_code_block {
                        base_style = Style::default().fg(Color::Yellow);
                    }

                    if gathering_anchor {
                        current_anchor_text.push_str(&text);
                    }

                    current.push(LinkFragment::new(
                        text.to_string(),
                        base_style,
                        current_link.clone(),
                    ));
                }
                MdEvent::Html(text) => {
                    current.push(LinkFragment::new(
                        text.to_string(),
                        Style::default(),
                        current_link.clone(),
                    ));
                }
                MdEvent::Code(text) => {
                    let style = Style::default().fg(Color::Yellow);
                    if gathering_anchor {
                        current_anchor_text.push_str(&text);
                    }
                    current.push(LinkFragment::new(
                        text.to_string(),
                        style,
                        current_link.clone(),
                    ));
                }
                MdEvent::SoftBreak => {
                    if in_code_block {
                        flush_current_line(&mut lines, &mut current);
                    } else {
                        current.push(LinkFragment::new(" ", Style::default(), None));
                    }
                }
                MdEvent::HardBreak => {
                    flush_current_line(&mut lines, &mut current);
                }
                MdEvent::Rule => {
                    const RULE_PLACEHOLDER: &str = "\0RULE\0";
                    lines.push(vec![LinkFragment::new(
                        RULE_PLACEHOLDER.to_string(),
                        Style::default(),
                        None,
                    )]);
                }
                _ => {}
            }
        }

        flush_current_line(&mut lines, &mut current);

        if lines.is_empty() {
            push_blank_line(&mut lines);
        }

        let linkified = self.linkifier.linkify_fragments(lines, theme);
        self.text.set_linkified_text(linkified);
        self.text.set_wrap(true);
    }

    pub fn set_markdown_bytes(&mut self, bytes: &[u8], theme: &term_wm_core::theme::Theme) {
        if let Ok(s) = str::from_utf8(bytes) {
            self.set_markdown(s, theme);
        }
    }

    pub fn rendered_lines(&self) -> Vec<String> {
        self.text.rendered_lines()
    }

    pub fn handle_pointer_event(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        let area = ctx.screen_area().unwrap_or_default();
        self.handle_pointer_event_in_area(event, area, ctx)
    }

    pub fn handle_pointer_event_in_area(
        &mut self,
        event: &Event,
        area: Rect,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        use crossterm::event::MouseEventKind;
        if let Event::Mouse(mouse) = event
            && matches!(mouse.kind, MouseEventKind::Down(_))
            && let Some(url) = self.text.link_at(area, *mouse)
        {
            if let Some(anchor) = url.strip_prefix('#')
                && let Some(&line_idx) = self.anchors.get(anchor)
            {
                self.text.jump_to_logical_line(line_idx, area);
                return EventResult::Consumed;
            }

            if self
                .link_handler
                .as_ref()
                .map(|handler| handler(url.as_str()))
                .unwrap_or(false)
            {
                return EventResult::Consumed;
            }
        }
        self.text.handle_events(event, ctx)
    }

    pub fn go_home(&mut self) {
        self.text.scroll_vertical_to(0);
    }

    pub fn go_end(&mut self) {
        self.text.scroll_vertical_to(usize::MAX);
    }

    pub fn set_selection_enabled(&mut self, enabled: bool) {
        self.text.set_selection_enabled(enabled);
    }

    pub fn handle_key_event(
        &mut self,
        key: &crossterm::event::KeyEvent,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        self.text.handle_events(&Event::Key(*key), ctx)
    }

    pub fn render_content(
        &self,
        frame: &mut UiFrame<'_>,
        area: Rect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        self.text.render(frame, area, ctx, registry);
    }
}

impl Default for MarkdownViewerComponent {
    fn default() -> Self {
        Self::new()
    }
}

fn flush_current_line(lines: &mut Vec<Vec<LinkFragment>>, current: &mut Vec<LinkFragment>) {
    if !current.is_empty() {
        lines.push(std::mem::take(current));
    }
}

fn push_blank_line(lines: &mut Vec<Vec<LinkFragment>>) {
    lines.push(vec![LinkFragment::new("", Style::default(), None)]);
}

fn slugify(text: &str) -> String {
    let text = text.trim().to_lowercase();
    let mut result = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' || c == '-' {
            result.push(c);
        } else if c.is_whitespace() && !result.ends_with('-') {
            result.push('-');
        }
    }
    result
}

#[cfg(test)]
mod markdown_tests {
    use super::*;
    use crate::ScrollViewComponent;
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
        mv.set_markdown(SAMPLE_HELP_MD, &term_wm_core::theme::NOIR);
        mv
    }

    #[test]
    fn help_md_contains_notes_and_list_items() {
        let mv = sample_viewer();
        let rendered = mv.rendered_lines();
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

        let scroll = ScrollViewComponent::new(sample_viewer());
        let ctx = ComponentContext::new(true);

        let area = Rect {
            x: 0,
            y: 0,
            width: 50,
            height: 12,
        };
        let mut scratch = Buffer::empty(area);
        {
            let mut frame = term_wm_core::ui::UiFrame::from_parts(area, &mut scratch);
            scroll.render(
                &mut frame,
                area,
                &ctx,
                &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
            );
        }

        let mut buffer = Buffer::empty(area);
        {
            scroll.content.borrow_mut().go_end();
            let mut frame = term_wm_core::ui::UiFrame::from_parts(area, &mut buffer);
            scroll.render(
                &mut frame,
                area,
                &ctx,
                &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
            );
        }

        let mut rows: Vec<String> = Vec::with_capacity(area.height as usize);
        for y in 0..area.height {
            let mut row = String::new();
            for x in 0..area.width {
                if let Some(cell) = buffer.cell((x, y)) {
                    row.push_str(cell.symbol());
                }
            }
            rows.push(row.trim_end().to_string());
        }
        let normalized = rows
            .into_iter()
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            normalized.contains("This is a lorem ipsum note used for tests"),
            "Mouse interactions note should render at bottom"
        );
        assert!(
            normalized.contains("Another note to validate list rendering"),
            "Panel menu note should render at bottom"
        );
    }

    #[test]
    fn scrollbar_does_not_overlay_text() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let scroll = ScrollViewComponent::new(sample_viewer());

        // choose a narrow viewport so a scrollbar will be required
        let area = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 4,
        };

        let mut with_scroll = Buffer::empty(area);
        {
            let mut frame = term_wm_core::ui::UiFrame::from_parts(area, &mut with_scroll);
            scroll.render(
                &mut frame,
                area,
                &ComponentContext::new(true),
                &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
            );
        }

        let viewport = scroll.compute_layout(area);
        assert_eq!(
            viewport.width + 1,
            area.width,
            "Viewport should reserve a column for the scrollbar"
        );
        assert_eq!(
            viewport.height, area.height,
            "Wrapping content should not require a horizontal scrollbar"
        );

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
        mv.set_markdown(md, &term_wm_core::theme::NOIR);
        let rendered = mv.rendered_lines();
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

    #[test]
    fn anchor_links_scroll_to_header() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use ratatui::layout::Rect;

        let md = indoc! {
            "
            [Go to section](#section-two)

            # Section One
            Line 1
            Line 2
            Line 3

            # Section Two
            Target line.
            "
        };
        let mut viewer = MarkdownViewerComponent::new();
        viewer.set_markdown(md, &term_wm_core::theme::NOIR);

        let mut scroll = ScrollViewComponent::new(viewer);
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 5,
        };

        let mut buffer = ratatui::buffer::Buffer::empty(area);
        {
            let mut frame = term_wm_core::ui::UiFrame::from_parts(area, &mut buffer);
            scroll.render(
                &mut frame,
                area,
                &ComponentContext::new(true),
                &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
            );
        }

        let mouse_event = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 0,
            modifiers: KeyModifiers::empty(),
        });

        assert_eq!(scroll.scroll_handle().info().offset_y, 0);

        let ctx = ComponentContext::new(true).with_screen_area(area);
        let result = scroll.handle_events(&mouse_event, &ctx);
        if let term_wm_core::actions::EventResult::Action(action) = result {
            scroll.update(action, &ComponentContext::new(true), &mut VecDeque::new());
        }

        let offset = scroll.scroll_handle().info().offset_y;

        assert!(
            offset > 0,
            "Should have scrolled down to Section Two but offset was {}",
            offset
        );
    }

    #[test]
    fn horizontal_rule_renders_single_line() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let md = indoc! {
            "
            Above paragraph

            ---

            Below paragraph
            "
        };

        let mut mv = MarkdownViewerComponent::new();
        mv.set_markdown(md, &term_wm_core::theme::NOIR);
        let scroll = ScrollViewComponent::new(mv);

        let area = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 6,
        };

        let mut buffer = Buffer::empty(area);
        {
            let mut frame = term_wm_core::ui::UiFrame::from_parts(area, &mut buffer);
            scroll.render(
                &mut frame,
                area,
                &ComponentContext::new(true),
                &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
            );
        }

        // Find the row that contains the rule glyph and ensure it only
        // appears on a single visual row (no wrapped continuation rows).
        let mut rule_rows = 0;
        for y in 0..area.height {
            let mut row = String::new();
            for x in 0..area.width {
                if let Some(cell) = buffer.cell((x, y)) {
                    row.push_str(cell.symbol());
                }
            }
            if row.contains('─') {
                rule_rows += 1;
            }
        }

        assert_eq!(
            rule_rows, 1,
            "rule should occupy a single visual row regardless of content width"
        );
    }
}
