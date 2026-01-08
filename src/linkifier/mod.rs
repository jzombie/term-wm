//! Lightweight link detection and overlay caching utilities.
//!
//! This module provides two related responsibilities:
//!
//! - `Linkifier`: a small wrapper around `linkify::LinkFinder` that exposes
//!   convenience helpers to detect and break text into link-aware fragments
//!   suitable for rendering.
//! - `LinkOverlay`: a compact, per-view cache of detected links for a grid of
//!   rendered rows. `LinkOverlay` remembers a small `OverlaySignature` and will
//!   skip recomputing per-row link maps when the upstream viewport and PTY
//!   content haven't changed. This keeps link detection cheap when the visible
//!   buffer is stable (helps with high-frequency rendering workloads).
//!
//! Typical usage:
//!
//! - Call `Linkifier::detect_links()` to obtain ranges/URLs when producing
//!   renderable text fragments, or use `Linkifier::linkify_text()` to convert
//!   a `ratatui::Text` into a `LinkifiedText` (for viewers like the Markdown
//!   component).
//! - For interactive terminal views, call `LinkOverlay::update_view()` with a
//!   small slice of visible rows; the overlay will internally decide whether to
//!   recompute based on the provided `OverlaySignature`.
//!
//! The helper `strip_trailing_punctuation` exists to trim extraneous
//! punctuation characters that `linkify` may include when matching ranges in
//! natural text (e.g. a URL followed by a period in a sentence). See
//! `Linkifier::detect_links()` where it is applied.

use std::{ops::Range, sync::Arc};

use linkify::{LinkFinder, LinkKind};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};

/// Map of lines -> columns -> optional URL string used by linkified renderers.
pub type LinkMap = Vec<Vec<Option<String>>>;

#[derive(Clone, Debug)]
/// A piece of text with rendering `style` and optional `link` metadata.
///
/// `LinkFragment` represents a contiguous span emitted by a renderer (e.g.
/// `MarkdownViewer` or a `ratatui::Span`) which may either be explicitly a
/// hyperlink (the `link` field) or plain text that should be scanned for
/// automatic links via the `Linkifier`.
pub struct LinkFragment {
    /// The textual content of the fragment.
    pub text: String,
    /// Visual styling to apply when rendering this fragment.
    pub style: Style,
    /// Optional explicit hyperlink associated with this fragment.
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
/// Result of converting renderable text into link-aware spans.
///
/// `text` is the `ratatui::Text` value ready to render, and `link_map` is a
/// parallel structure mapping each cell/spans position to an optional URL.
pub struct LinkifiedText {
    pub text: Text<'static>,
    pub link_map: LinkMap,
}

/// A callback that consumes a URL and returns `true` if it handled opening it.
pub type LinkHandler = Arc<dyn Fn(&str) -> bool + Send + Sync + 'static>;

#[derive(Clone, Debug, PartialEq, Eq)]
/// A link that was detected inside a single string slice.
///
/// `range` is the byte range inside the input string and `url` is the
/// substring (after trimming punctuation) that should be used when opening
/// the link.
pub struct DetectedLink {
    pub range: Range<usize>,
    pub url: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Small fingerprint used by `LinkOverlay` to determine whether the cached
/// per-row link map remains valid.
///
/// It includes a monotonic `bytes_seen` counter (from the PTY), the current
/// `scrollback` offset, the viewport size, and the top-left offset into the
/// PTY screen. When the signature matches, `LinkOverlay` can safely skip the
/// expensive detect pass for rows that haven't changed.
pub struct OverlaySignature {
    bytes_seen: usize,
    scrollback: usize,
    area_width: u16,
    area_height: u16,
    start_row: u16,
    start_col: u16,
}

impl OverlaySignature {
    pub fn new(
        bytes_seen: usize,
        scrollback: usize,
        area_width: u16,
        area_height: u16,
        start_row: u16,
        start_col: u16,
    ) -> Self {
        Self {
            bytes_seen,
            scrollback,
            area_width,
            area_height,
            start_row,
            start_col,
        }
    }
}

#[derive(Debug, Default)]
/// Cache of detected links for a rectangular viewport.
///
/// Callers should provide only the visible rows (and a signature) via
/// `update_view()`; `LinkOverlay` will update only rows that changed and will
/// keep an internal signature to skip redundant work.
pub struct LinkOverlay {
    rows: Vec<RowLinks>,
    signature: Option<OverlaySignature>,
}

#[derive(Debug, Default, Clone)]
struct RowLinks {
    text: String,
    col_offset: usize,
    cols: Vec<Option<Arc<str>>>,
}

#[derive(Debug)]
/// Convenience wrapper around `linkify::LinkFinder` with helpers used by the
/// UI components in this crate.
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
            // Trim any common trailing punctuation characters from the
            // matched substring. `linkify` can include trailing characters
            // when links appear at the end of a sentence; callers typically
            // want the raw URL without punctuation when opening it.
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
        Self {
            rows: Vec::new(),
            signature: None,
        }
    }

    pub fn clear(&mut self) {
        self.rows.clear();
        self.signature = None;
    }

    pub fn is_signature_current(&self, signature: &OverlaySignature) -> bool {
        self.signature.as_ref() == Some(signature)
    }

    pub fn update_view(
        &mut self,
        signature: OverlaySignature,
        height: usize,
        width: usize,
        rows: &[(usize, usize, String, Vec<usize>)],
        linkifier: &Linkifier,
    ) {
        self.signature = Some(signature);
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
    // Return the largest prefix without trailing punctuation and the
    // remaining suffix. This is used to strip sentence punctuation like
    // ".,?!:;)]'\"" from URLs that `linkify` may include at match time.
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
