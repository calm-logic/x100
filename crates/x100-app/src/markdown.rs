//! Markdown on plain GPUI elements, for the CEO's replies and, later, the specs. Zed's own
//! `markdown` crate is GPL, so this parses with pulldown-cmark. It draws headings, paragraphs,
//! nested lists, code, tables, quotes, rules, links, emphasis and strikethrough.

use std::ops::Range;

use gpui::{
    AnyElement, ElementId, FontStyle, FontWeight, HighlightStyle, InteractiveText, SharedString,
    StrikethroughStyle, StyledText, UnderlineStyle, div, prelude::*, px,
};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::theme;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Inline {
    pub text: String,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    links: Vec<(Range<usize>, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading(u8, Inline),
    Paragraph(Inline),
    Item {
        depth: usize,
        marker: String,
        body: Inline,
    },
    Code(String),
    Table {
        head: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
    },
    Quote(Inline),
    Rule,
}

/// A parsed document, drawn on every render without parsing again.
#[derive(Debug, Clone, PartialEq)]
pub struct Markdown {
    blocks: Vec<Block>,
}

/// Where an inline style started, by kind, so nested styles close in the right order.
enum Open {
    Strong(usize),
    Emphasis(usize),
    Strike(usize),
    Link(usize, String),
}

struct Builder {
    blocks: Vec<Block>,
    cur: Inline,
    open: Vec<Open>,
    /// The next number of each open list, `None` for bullets.
    lists: Vec<Option<u64>>,
    /// The marker of the item whose text is being gathered.
    item: Option<String>,
    code: Option<String>,
    /// Head, rows, and whether the head is still being read.
    table: Option<(Vec<Inline>, Vec<Vec<Inline>>, bool)>,
    quote: usize,
}

impl Builder {
    fn flush_item(&mut self) {
        if let Some(marker) = self.item.take() {
            self.blocks.push(Block::Item {
                depth: self.lists.len().saturating_sub(1),
                marker,
                body: std::mem::take(&mut self.cur),
            });
        }
    }

    fn close(&mut self, matches: impl Fn(&Open) -> bool) {
        let Some(at) = self.open.iter().rposition(matches) else {
            return;
        };
        let end = self.cur.text.len();
        let style = match self.open.remove(at) {
            Open::Strong(s) => (s, bold()),
            Open::Emphasis(s) => (
                s,
                HighlightStyle {
                    font_style: Some(FontStyle::Italic),
                    ..Default::default()
                },
            ),
            Open::Strike(s) => (
                s,
                HighlightStyle {
                    strikethrough: Some(StrikethroughStyle {
                        thickness: px(1.),
                        color: None,
                    }),
                    ..Default::default()
                },
            ),
            Open::Link(s, url) => {
                self.cur.links.push((s..end, url));
                (
                    s,
                    HighlightStyle {
                        color: Some(theme::link().into()),
                        underline: Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(theme::link().into()),
                            wavy: false,
                        }),
                        ..Default::default()
                    },
                )
            }
        };
        self.cur.highlights.push((style.0..end, style.1));
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => self.code = Some(String::new()),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(text) = self.code.take() {
                    self.blocks.push(Block::Code(text.trim_end().to_owned()));
                }
            }
            Event::Text(t) if self.code.is_some() => {
                self.code.get_or_insert_default().push_str(&t);
            }
            Event::Start(Tag::List(start)) => {
                // A nested list starts inside an item: that item's own text comes first.
                self.flush_item();
                self.lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                self.lists.pop();
            }
            Event::Start(Tag::Item) => {
                self.item = Some(match self.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}.", *n - 1)
                    }
                    _ => "•".to_owned(),
                });
            }
            Event::End(TagEnd::Item) => self.flush_item(),
            Event::Start(Tag::Table(_)) => self.table = Some((Vec::new(), Vec::new(), true)),
            Event::End(TagEnd::Table) => {
                if let Some((head, rows, _)) = self.table.take() {
                    self.blocks.push(Block::Table { head, rows });
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(t) = self.table.as_mut() {
                    t.2 = false;
                }
            }
            Event::Start(Tag::TableRow) => {
                if let Some(t) = self.table.as_mut() {
                    t.1.push(Vec::new());
                }
            }
            Event::End(TagEnd::TableCell) => {
                let cell = std::mem::take(&mut self.cur);
                if let Some(t) = self.table.as_mut() {
                    if t.2 {
                        t.0.push(cell);
                    } else if let Some(row) = t.1.last_mut() {
                        row.push(cell);
                    }
                }
            }
            Event::Start(Tag::BlockQuote(_)) => self.quote += 1,
            Event::End(TagEnd::BlockQuote(_)) => self.quote = self.quote.saturating_sub(1),
            Event::End(TagEnd::Heading(level)) => {
                let n = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    _ => 4,
                };
                self.blocks
                    .push(Block::Heading(n, std::mem::take(&mut self.cur)));
            }
            Event::End(TagEnd::Paragraph) => {
                if self.item.is_some() {
                    // A loose list item: its paragraphs stay together.
                    self.cur.text.push('\n');
                } else {
                    let body = std::mem::take(&mut self.cur);
                    self.blocks.push(if self.quote > 0 {
                        Block::Quote(body)
                    } else {
                        Block::Paragraph(body)
                    });
                }
            }
            Event::Start(Tag::Strong) => self.open.push(Open::Strong(self.cur.text.len())),
            Event::End(TagEnd::Strong) => self.close(|o| matches!(o, Open::Strong(_))),
            Event::Start(Tag::Emphasis) => self.open.push(Open::Emphasis(self.cur.text.len())),
            Event::End(TagEnd::Emphasis) => self.close(|o| matches!(o, Open::Emphasis(_))),
            Event::Start(Tag::Strikethrough) => self.open.push(Open::Strike(self.cur.text.len())),
            Event::End(TagEnd::Strikethrough) => self.close(|o| matches!(o, Open::Strike(_))),
            Event::Start(Tag::Link { dest_url, .. }) => self
                .open
                .push(Open::Link(self.cur.text.len(), dest_url.to_string())),
            Event::End(TagEnd::Link) => self.close(|o| matches!(o, Open::Link(..))),
            Event::Code(t) => {
                let s = self.cur.text.len();
                self.cur.text.push_str(&t);
                self.cur.highlights.push((
                    s..self.cur.text.len(),
                    HighlightStyle {
                        color: Some(theme::code().into()),
                        background_color: Some(theme::raised().into()),
                        ..Default::default()
                    },
                ));
            }
            Event::Text(t) => self.cur.text.push_str(&t),
            Event::SoftBreak => self.cur.text.push(' '),
            Event::HardBreak => self.cur.text.push('\n'),
            Event::Rule => self.blocks.push(Block::Rule),
            _ => {}
        }
    }
}

impl Markdown {
    pub fn parse(source: &str) -> Self {
        let mut b = Builder {
            blocks: Vec::new(),
            cur: Inline::default(),
            open: Vec::new(),
            lists: Vec::new(),
            item: None,
            code: None,
            table: None,
            quote: 0,
        };
        let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
        for event in Parser::new_ext(source, options) {
            b.event(event);
        }
        // Loose items end their text with the paragraph break they were given.
        for block in &mut b.blocks {
            if let Block::Item { body, .. } = block {
                let trimmed = body.text.trim_end_matches('\n').len();
                body.text.truncate(trimmed);
            }
        }
        Self { blocks: b.blocks }
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// Draws the document. `id` must be unique in the window: links need ids of their own.
    pub fn render(&self, id: &SharedString) -> AnyElement {
        let mut n = 0u64;
        let mut next = || {
            n += 1;
            ElementId::NamedInteger(id.clone(), n)
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .children(self.blocks.iter().map(|block| -> AnyElement {
                match block {
                    Block::Heading(level, body) => div()
                        .mt(px(4.))
                        .text_size(px(match level {
                            1 => 18.,
                            2 => 16.,
                            _ => 14.,
                        }))
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::text())
                        .child(inline(next(), body))
                        .into_any_element(),
                    Block::Paragraph(body) => div().child(inline(next(), body)).into_any_element(),
                    Block::Quote(body) => div()
                        .pl(px(10.))
                        .border_l_2()
                        .border_color(theme::line())
                        .text_color(theme::muted())
                        .child(inline(next(), body))
                        .into_any_element(),
                    Block::Item {
                        depth,
                        marker,
                        body,
                    } => div()
                        .flex()
                        .pl(px(14. * *depth as f32))
                        .gap(px(6.))
                        .child(
                            div()
                                .flex_none()
                                .min_w(px(14.))
                                .text_color(theme::muted())
                                .child(marker.clone()),
                        )
                        .child(div().flex_1().min_w_0().child(inline(next(), body)))
                        .into_any_element(),
                    Block::Code(text) => div()
                        .p(px(8.))
                        .rounded(px(8.))
                        .bg(theme::raised())
                        .border_1()
                        .border_color(theme::line())
                        .font_family(theme::MONO)
                        .text_size(px(theme::SMALL))
                        .text_color(theme::text())
                        .child(text.clone())
                        .into_any_element(),
                    Block::Table { head, rows } => {
                        let row = |cells: Vec<AnyElement>, header: bool| {
                            div()
                                .flex()
                                .border_b_1()
                                .border_color(theme::line())
                                .when(header, |d| d.font_weight(FontWeight::BOLD))
                                .children(cells.into_iter().map(|c| {
                                    div().flex_1().min_w_0().px(px(8.)).py(px(4.)).child(c)
                                }))
                        };
                        let head = head.iter().map(|c| inline(next(), c)).collect();
                        let mut table = div()
                            .flex()
                            .flex_col()
                            .border_1()
                            .border_color(theme::line())
                            .rounded(px(6.))
                            .child(row(head, true));
                        for cells in rows {
                            table = table.child(row(
                                cells.iter().map(|c| inline(next(), c)).collect(),
                                false,
                            ));
                        }
                        table.into_any_element()
                    }
                    Block::Rule => div().h(px(1.)).bg(theme::line()).into_any_element(),
                }
            }))
            .into_any_element()
    }
}

fn bold() -> HighlightStyle {
    HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        ..Default::default()
    }
}

/// `StyledText` takes highlights that do not overlap, so nested styles are cut at every boundary
/// and merged.
fn flatten(highlights: &[(Range<usize>, HighlightStyle)]) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut cuts: Vec<usize> = highlights
        .iter()
        .flat_map(|(r, _)| [r.start, r.end])
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .filter_map(|w| {
            let (a, b) = (w[0], w[1]);
            highlights
                .iter()
                .filter(|(r, _)| r.start <= a && b <= r.end)
                .map(|(_, h)| *h)
                .reduce(|s, h| s.highlight(h))
                .map(|style| (a..b, style))
        })
        .collect()
}

fn inline(id: ElementId, body: &Inline) -> AnyElement {
    let styled = StyledText::new(body.text.clone()).with_highlights(flatten(&body.highlights));
    if body.links.is_empty() {
        return styled.into_any_element();
    }
    let ranges: Vec<_> = body.links.iter().map(|(r, _)| r.clone()).collect();
    let urls: Vec<_> = body.links.iter().map(|(_, u)| u.clone()).collect();
    InteractiveText::new(id, styled)
        .on_click(ranges, move |ix, _, cx| cx.open_url(&urls[ix]))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(md: &Markdown) -> Vec<String> {
        md.blocks()
            .iter()
            .map(|b| match b {
                Block::Heading(n, i) => format!("h{n} {}", i.text),
                Block::Paragraph(i) => format!("p {}", i.text),
                Block::Item {
                    depth,
                    marker,
                    body,
                } => format!("{depth}{marker} {}", body.text),
                Block::Code(t) => format!("code {t}"),
                Block::Table { head, rows } => format!("table {}x{}", head.len(), rows.len()),
                Block::Quote(i) => format!("> {}", i.text),
                Block::Rule => "---".into(),
            })
            .collect()
    }

    #[test]
    fn nested_lists_keep_their_depth_and_numbers() {
        let md = Markdown::parse("1. one\n   - inner **bold**\n2. two\n\n> said\n\n---\n");
        assert_eq!(
            texts(&md),
            ["01. one", "1• inner bold", "02. two", "> said", "---"]
        );
    }

    #[test]
    fn nested_styles_are_merged_not_overlapped() {
        let md = Markdown::parse("**bold *both* bold** [link](https://x100.dev) `code`");
        let Block::Paragraph(p) = &md.blocks()[0] else {
            panic!("a paragraph");
        };
        let flat = flatten(&p.highlights);
        assert!(flat.windows(2).all(|w| w[0].0.end <= w[1].0.start));
        let both = p.text.find("both").unwrap();
        let style = flat.iter().find(|(r, _)| r.start == both).unwrap().1;
        assert_eq!(style.font_weight, Some(FontWeight::BOLD));
        assert_eq!(style.font_style, Some(FontStyle::Italic));
        assert_eq!(p.links.len(), 1);
    }

    #[test]
    fn tables_and_code() {
        let md = Markdown::parse("| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn x() {}\n```\n");
        assert_eq!(texts(&md), ["table 2x1", "code fn x() {}"]);
    }
}
