//! Markdown on plain GPUI elements. Zed's own `markdown` crate is GPL, so this parses with
//! pulldown-cmark and draws headings, paragraphs, lists, code blocks, tables, rules and links.

use std::ops::Range;

use gpui::{
    AnyElement, Context, FontStyle, FontWeight, HighlightStyle, InteractiveText, IntoElement,
    ParentElement, Render, Styled, StyledText, UnderlineStyle, Window, div, prelude::*, px, rgb,
};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::stats;

#[derive(Default, Clone)]
struct Inline {
    text: String,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    links: Vec<(Range<usize>, String)>,
}

enum Block {
    Heading(u8, Inline),
    Paragraph(Inline),
    Item {
        depth: usize,
        marker: String,
        body: Inline,
    },
    Code {
        lang: String,
        text: String,
    },
    Table {
        head: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
    },
    Quote(Inline),
    Rule,
}

fn parse(source: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut cur = Inline::default();
    // Open inline styles, with where each started in `cur.text`.
    let mut bold: Option<usize> = None;
    let mut italic: Option<usize> = None;
    let mut link: Option<(usize, String)> = None;
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut item_marker: Option<String> = None;
    let mut code: Option<(String, String)> = None;
    let mut table: Option<(Vec<Inline>, Vec<Vec<Inline>>, bool)> = None;
    let mut in_quote = false;

    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    for event in Parser::new_ext(source, opts) {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(l) => l.to_string(),
                    pulldown_cmark::CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((lang, text)) = code.take() {
                    blocks.push(Block::Code {
                        lang,
                        text: text.trim_end().to_string(),
                    });
                }
            }
            Event::Text(t) if code.is_some() => code.as_mut().unwrap().1.push_str(&t),
            Event::Start(Tag::List(start)) => {
                // A nested list starts inside an item: flush the item's text first.
                if let Some(marker) = item_marker.take() {
                    blocks.push(Block::Item {
                        depth: lists.len() - 1,
                        marker,
                        body: std::mem::take(&mut cur),
                    });
                }
                lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                let marker = match lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}.");
                        *n += 1;
                        m
                    }
                    _ => "•".to_string(),
                };
                item_marker = Some(marker);
            }
            Event::End(TagEnd::Item) => {
                if let Some(marker) = item_marker.take() {
                    blocks.push(Block::Item {
                        depth: lists.len() - 1,
                        marker,
                        body: std::mem::take(&mut cur),
                    });
                }
            }
            Event::Start(Tag::Table(_)) => table = Some((Vec::new(), Vec::new(), true)),
            Event::End(TagEnd::Table) => {
                if let Some((head, rows, _)) = table.take() {
                    blocks.push(Block::Table { head, rows });
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(t) = table.as_mut() {
                    t.2 = false;
                }
            }
            Event::Start(Tag::TableRow) => {
                if let Some(t) = table.as_mut() {
                    t.1.push(Vec::new());
                }
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(t) = table.as_mut() {
                    let cell = std::mem::take(&mut cur);
                    if t.2 {
                        t.0.push(cell);
                    } else if let Some(row) = t.1.last_mut() {
                        row.push(cell);
                    }
                }
            }
            Event::Start(Tag::BlockQuote(_)) => in_quote = true,
            Event::End(TagEnd::BlockQuote(_)) => in_quote = false,
            Event::End(TagEnd::Heading(level)) => {
                let n = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    _ => 4,
                };
                blocks.push(Block::Heading(n, std::mem::take(&mut cur)));
            }
            Event::End(TagEnd::Paragraph) => {
                // Paragraphs inside list items and quotes belong to the enclosing block.
                if item_marker.is_none() {
                    let body = std::mem::take(&mut cur);
                    blocks.push(if in_quote {
                        Block::Quote(body)
                    } else {
                        Block::Paragraph(body)
                    });
                }
            }
            Event::Start(Tag::Strong) => bold = Some(cur.text.len()),
            Event::End(TagEnd::Strong) => {
                if let Some(s) = bold.take() {
                    let end = cur.text.len();
                    cur.highlights.push((
                        s..end,
                        HighlightStyle {
                            font_weight: Some(FontWeight::BOLD),
                            ..Default::default()
                        },
                    ));
                }
            }
            Event::Start(Tag::Emphasis) => italic = Some(cur.text.len()),
            Event::End(TagEnd::Emphasis) => {
                if let Some(s) = italic.take() {
                    let end = cur.text.len();
                    cur.highlights.push((
                        s..end,
                        HighlightStyle {
                            font_style: Some(FontStyle::Italic),
                            ..Default::default()
                        },
                    ));
                }
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                link = Some((cur.text.len(), dest_url.to_string()))
            }
            Event::End(TagEnd::Link) => {
                if let Some((s, url)) = link.take() {
                    let end = cur.text.len();
                    cur.highlights.push((
                        s..end,
                        HighlightStyle {
                            color: Some(rgb(0x58a6ff).into()),
                            underline: Some(UnderlineStyle {
                                thickness: px(1.),
                                color: Some(rgb(0x58a6ff).into()),
                                wavy: false,
                            }),
                            ..Default::default()
                        },
                    ));
                    cur.links.push((s..end, url));
                }
            }
            Event::Code(t) => {
                let s = cur.text.len();
                cur.text.push_str(&t);
                cur.highlights.push((
                    s..cur.text.len(),
                    HighlightStyle {
                        color: Some(rgb(0xffa657).into()),
                        background_color: Some(rgb(0x2d333b).into()),
                        ..Default::default()
                    },
                ));
            }
            Event::Text(t) => cur.text.push_str(&t),
            Event::SoftBreak => cur.text.push(' '),
            Event::HardBreak => cur.text.push('\n'),
            Event::Rule => blocks.push(Block::Rule),
            _ => {}
        }
    }
    blocks
}

/// Highlights must not overlap for `StyledText`, so nested styles are split at every boundary
/// and their styles merged.
fn flatten(highlights: &[(Range<usize>, HighlightStyle)]) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut cuts: Vec<usize> = highlights
        .iter()
        .flat_map(|(r, _)| [r.start, r.end])
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let mut style: Option<HighlightStyle> = None;
        for (r, h) in highlights {
            if r.start <= a && b <= r.end {
                style = Some(match style {
                    None => *h,
                    Some(s) => s.highlight(*h),
                });
            }
        }
        if let Some(s) = style {
            out.push((a..b, s));
        }
    }
    out
}

fn inline(id: impl Into<gpui::ElementId>, body: &Inline) -> AnyElement {
    let styled = StyledText::new(body.text.clone()).with_highlights(flatten(&body.highlights));
    if body.links.is_empty() {
        return styled.into_any_element();
    }
    let ranges: Vec<_> = body.links.iter().map(|(r, _)| r.clone()).collect();
    let urls: Vec<_> = body.links.iter().map(|(_, u)| u.clone()).collect();
    InteractiveText::new(id.into(), styled)
        .on_click(ranges, move |ix, _window, cx| {
            log::info!("link clicked: {}", urls[ix]);
            cx.open_url(&urls[ix]);
        })
        .into_any_element()
}

pub struct MarkdownView {
    blocks: Vec<Block>,
}

impl MarkdownView {
    pub fn new(source: &str) -> Self {
        Self {
            blocks: parse(source),
        }
    }
}

impl Render for MarkdownView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        stats::bump(&stats::MARKDOWN_RENDERS);
        let children = self
            .blocks
            .iter()
            .enumerate()
            .map(|(i, block)| -> AnyElement {
                let id = ("md", i);
                match block {
                    Block::Heading(level, body) => {
                        let size = match level {
                            1 => 24.,
                            2 => 19.,
                            3 => 16.,
                            _ => 14.,
                        };
                        div()
                            .mt_2()
                            .text_size(px(size))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0xf0f6fc))
                            .child(inline(id, body))
                            .into_any_element()
                    }
                    Block::Paragraph(body) => div().child(inline(id, body)).into_any_element(),
                    Block::Quote(body) => div()
                        .pl_3()
                        .border_l_2()
                        .border_color(rgb(0x3d444d))
                        .text_color(rgb(0x9198a1))
                        .child(inline(id, body))
                        .into_any_element(),
                    Block::Item {
                        depth,
                        marker,
                        body,
                    } => div()
                        .flex()
                        .flex_row()
                        .pl(px(16. * *depth as f32))
                        .gap_2()
                        .child(
                            div()
                                .w(px(18.))
                                .flex_none()
                                .text_color(rgb(0x8b949e))
                                .child(marker.clone()),
                        )
                        .child(div().flex_1().child(inline(id, body)))
                        .into_any_element(),
                    Block::Code { lang, text } => div()
                        .flex()
                        .flex_col()
                        .p_2()
                        .rounded_md()
                        .bg(rgb(0x161b22))
                        .border_1()
                        .border_color(rgb(0x30363d))
                        .font_family("DejaVu Sans Mono")
                        .text_size(px(12.))
                        .when(!lang.is_empty(), |d| {
                            d.child(div().text_color(rgb(0x6e7681)).child(lang.clone()))
                        })
                        .child(div().text_color(rgb(0xe6edf3)).child(text.clone()))
                        .into_any_element(),
                    Block::Table { head, rows } => {
                        let cell = |content: AnyElement, header: bool| {
                            div()
                                .flex_1()
                                .px_2()
                                .py_1()
                                .border_r_1()
                                .border_color(rgb(0x30363d))
                                .when(header, |d| {
                                    d.font_weight(FontWeight::BOLD).bg(rgb(0x161b22))
                                })
                                .child(content)
                        };
                        let row = |cells: Vec<AnyElement>, header: bool| {
                            div()
                                .flex()
                                .flex_row()
                                .border_b_1()
                                .border_color(rgb(0x30363d))
                                .children(cells.into_iter().map(move |c| cell(c, header)))
                        };
                        let head_cells = head
                            .iter()
                            .enumerate()
                            .map(|(j, c)| inline(("md-th", i * 100 + j), c))
                            .collect();
                        div()
                            .flex()
                            .flex_col()
                            .border_1()
                            .border_color(rgb(0x30363d))
                            .child(row(head_cells, true))
                            .children(rows.iter().enumerate().map(|(r, cells)| {
                                row(
                                    cells
                                        .iter()
                                        .enumerate()
                                        .map(|(j, c)| {
                                            inline(("md-td", i * 10_000 + r * 100 + j), c)
                                        })
                                        .collect(),
                                    false,
                                )
                            }))
                            .into_any_element()
                    }
                    Block::Rule => div().h(px(1.)).bg(rgb(0x30363d)).into_any_element(),
                }
            });
        div()
            .id("markdown")
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .text_size(px(14.))
            .text_color(rgb(0xd1d7e0))
            .children(children)
    }
}
