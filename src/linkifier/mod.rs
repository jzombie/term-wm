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

    fn process_fragment(
        &self,
        fragment: LinkFragment,
        spans: &mut Vec<Span<'static>>,
        links: &mut Vec<Option<String>>,
    ) {
        if let Some(link) = fragment.link {
            spans.push(Span::styled(
                fragment.text,
                apply_link_style(fragment.style),
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
                style = apply_link_style(style);
            }
            spans.push(Span::styled(segment, style));
            links.push(detected_link);
        }
    }

    fn split_text_with_auto_links(&self, text: &str) -> Vec<(String, Option<String>)> {
        let mut parts = Vec::new();
        let mut last = 0;

        for span in self.finder.links(text) {
            let start = span.start();
            let end = span.end();
            if start > last {
                parts.push((text[last..start].to_string(), None));
            }
            let matched = span.as_str();
            let (url_part, trailing) = strip_trailing_punctuation(matched);
            if !url_part.is_empty() {
                parts.push((url_part.to_string(), Some(url_part.to_string())));
            } else {
                parts.push((matched.to_string(), None));
            }
            if !trailing.is_empty() {
                parts.push((trailing.to_string(), None));
            }
            last = end;
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

fn apply_link_style(mut style: Style) -> Style {
    if crate::theme::link_underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style.fg(crate::theme::link_color())
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
