use std::{ops::Range, sync::Arc};

use linkify::{LinkFinder, LinkKind};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};

pub type LinkMap = Vec<Vec<Option<String>>>;

#[derive(Clone, Debug)]
pub struct LinkFragment {
    pub text: String,
    pub style: Style,
    pub link: Option<String>,
}

impl LinkFragment {
    pub fn new(text: impl Into<String>, style: Style, link: Option<String>) -> Self {
        Self {
            text: text.into(),
            style,
            link,
        }
    }
}

#[derive(Debug)]
pub struct LinkifiedText {
    pub text: Text<'static>,
    pub link_map: LinkMap,
}

pub type LinkHandler = Arc<dyn Fn(&str) -> bool + Send + Sync + 'static>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectedLink {
    pub range: Range<usize>,
    pub url: String,
}

#[derive(Debug, Default)]
pub struct LinkOverlay {
    rows: Vec<RowLinks>,
}

#[derive(Debug, Default, Clone)]
struct RowLinks {
    text: String,
    col_offset: usize,
    cols: Vec<Option<Arc<str>>>,
}

#[derive(Debug)]
pub struct Linkifier {
    finder: LinkFinder,
}

impl Default for Linkifier {
    fn default() -> Self {
        Self::new()
    }
}

impl Linkifier {
    pub fn new() -> Self {
        let mut finder = LinkFinder::new();
        finder.kinds(&[LinkKind::Url]);
        finder.url_must_have_scheme(true);
        Self { finder }
    }

    pub fn linkify_fragments(&self, lines: Vec<Vec<LinkFragment>>) -> LinkifiedText {
        let mut rendered_lines = Vec::with_capacity(lines.len().max(1));
        let mut link_map = Vec::with_capacity(lines.len().max(1));

        if lines.is_empty() {
            rendered_lines.push(Line::from(vec![Span::raw("")]));
            link_map.push(vec![None]);
            return LinkifiedText {
                text: Text::from(rendered_lines),
                link_map,
            };
        }

        for fragments in lines {
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut links: Vec<Option<String>> = Vec::new();
            if fragments.is_empty() {
                spans.push(Span::raw(""));
                links.push(None);
            } else {
                for fragment in fragments {
                    self.process_fragment(fragment, &mut spans, &mut links);
                }
            }
            rendered_lines.push(Line::from(spans));
            link_map.push(links);
        }

        LinkifiedText {
            text: Text::from(rendered_lines),
            link_map,
        }
    }

    pub fn linkify_text(&self, text: Text<'static>) -> LinkifiedText {
        let fragments: Vec<Vec<LinkFragment>> = text
            .lines
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| LinkFragment::new(span.content.to_string(), span.style, None))
                    .collect()
            })
            .collect();
        self.linkify_fragments(fragments)
    }

    pub fn detect_links(&self, text: &str) -> Vec<DetectedLink> {
        let mut links = Vec::new();
        for span in self.finder.links(text) {
            let start = span.start();
            let matched = span.as_str();
            let (url_part, _) = strip_trailing_punctuation(matched);
            if url_part.is_empty() {
                continue;
            }
            let end = start + url_part.len();
            links.push(DetectedLink {
                range: start..end,
                url: url_part.to_string(),
            });
        }
        links
    }

    fn process_fragment(
        &self,
        fragment: LinkFragment,
        spans: &mut Vec<Span<'static>>,
        links: &mut Vec<Option<String>>,
    ) {
        if let Some(link) = fragment.link {
            spans.push(Span::styled(
                fragment.text,
                decorate_link_style(fragment.style),
            ));
            links.push(Some(link));
            return;
        }

        for (segment, detected_link) in self.split_text_with_auto_links(&fragment.text) {
            if segment.is_empty() {
                continue;
            }
            let mut style = fragment.style;
            if detected_link.is_some() {
                style = decorate_link_style(style);
            }
            spans.push(Span::styled(segment, style));
            links.push(detected_link);
        }
    }

    fn split_text_with_auto_links(&self, text: &str) -> Vec<(String, Option<String>)> {
        let mut parts = Vec::new();
        let mut last = 0;

        for link in self.detect_links(text) {
            if link.range.start > last {
                parts.push((text[last..link.range.start].to_string(), None));
            }
            parts.push((text[link.range.clone()].to_string(), Some(link.url.clone())));
            last = link.range.end;
        }

        if last < text.len() {
            parts.push((text[last..].to_string(), None));
        }

        if parts.is_empty() {
            parts.push((text.to_string(), None));
        }

        parts
    }
}

pub fn decorate_link_style(mut style: Style) -> Style {
    if crate::theme::link_underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style.fg(crate::theme::link_color())
}

impl LinkOverlay {
    pub fn new() -> Self {
        Self { rows: Vec::new() }
    }

    pub fn clear(&mut self) {
        self.rows.clear();
    }

    pub fn update_view(
        &mut self,
        height: usize,
        width: usize,
        rows: &[(usize, usize, String, Vec<usize>)],
        linkifier: &Linkifier,
    ) {
        self.resize(height, width);
        let mut visited = vec![false; self.rows.len()];
        for (row_idx, col_offset, text, offsets) in rows {
            if *row_idx >= self.rows.len() {
                continue;
            }
            visited[*row_idx] = true;
            let row = &mut self.rows[*row_idx];
            row.ensure_width(width);
            if row.text == *text && row.col_offset == *col_offset {
                continue;
            }
            row.text = text.clone();
            row.col_offset = *col_offset;
            row.clear_links();
            for link in linkifier.detect_links(text) {
                let start_idx = match offsets.binary_search(&link.range.start) {
                    Ok(idx) => idx,
                    Err(_) => continue,
                };
                let end_idx = match offsets.binary_search(&link.range.end) {
                    Ok(idx) => idx,
                    Err(_) => continue,
                };
                if start_idx >= end_idx {
                    continue;
                }
                let arc_url: Arc<str> = Arc::from(link.url.as_str());
                for col in start_idx..end_idx {
                    let area_col = col.saturating_add(*col_offset);
                    if area_col >= row.cols.len() {
                        break;
                    }
                    row.cols[area_col] = Some(arc_url.clone());
                }
            }
        }

        for (idx, row) in self.rows.iter_mut().enumerate() {
            if idx >= visited.len() || !visited[idx] {
                row.text.clear();
                row.col_offset = 0;
                row.clear_links();
            }
        }
    }

    pub fn is_link_cell(&self, row: usize, col: usize) -> bool {
        self.rows
            .get(row)
            .and_then(|r| r.cols.get(col))
            .and_then(|entry| entry.as_ref())
            .is_some()
    }

    pub fn link_at(&self, row: usize, col: usize) -> Option<String> {
        self.rows
            .get(row)
            .and_then(|r| r.cols.get(col))
            .and_then(|entry| entry.as_ref())
            .map(|url| url.as_ref().to_string())
    }

    fn resize(&mut self, height: usize, width: usize) {
        if self.rows.len() != height {
            self.rows
                .resize_with(height, || RowLinks::with_width(width));
        }
        for row in &mut self.rows {
            row.ensure_width(width);
        }
    }
}

impl RowLinks {
    fn with_width(width: usize) -> Self {
        Self {
            text: String::new(),
            col_offset: 0,
            cols: vec![None; width],
        }
    }

    fn ensure_width(&mut self, width: usize) {
        if self.cols.len() != width {
            self.cols.resize(width, None);
        }
    }

    fn clear_links(&mut self) {
        for cell in &mut self.cols {
            *cell = None;
        }
    }
}

fn strip_trailing_punctuation(s: &str) -> (&str, &str) {
    let mut end = s.len();
    while end > 0 {
        let ch = s[..end].chars().last().unwrap();
        if matches!(
            ch,
            '.' | ',' | '?' | '!' | ':' | ';' | ')' | ']' | '\'' | '"'
        ) {
            end -= ch.len_utf8();
        } else {
            break;
        }
    }
    (&s[..end], &s[end..])
}
