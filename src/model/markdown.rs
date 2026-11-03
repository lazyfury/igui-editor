//! Markdown -> flat block model for the preview pane.
//!
//! [`pulldown-cmark`] supplies the CommonMark event stream; this module folds it
//! into a **flat** block list that a top-down painter can render without a tree
//! walk: quote depth and list indentation are stored per block, and inline
//! styling is stored as styled runs.
//!
//! Deliberately flat: nested lists and quotes become `indent` / `quote` values,
//! not nested nodes, so the preview layout stays a single loop and tables /
//! footnotes can be added later without touching the renderer's structure.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// Inline emphasis of a run of text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Plain,
    Emphasis,
    Strong,
    Strikethrough,
    /// Inline code.
    Code,
    Link,
}

/// One contiguous span of same-styled text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub style: Style,
}

/// Column alignment of a table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// A rendered table: one header row plus body rows, cells hold inline runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub aligns: Vec<Align>,
    pub header: Vec<Vec<Run>>,
    pub rows: Vec<Vec<Vec<Run>>>,
}

impl Table {
    /// Number of columns (at least one), padding short rows with empty cells.
    pub fn columns(&self) -> usize {
        self.aligns
            .len()
            .max(self.header.len())
            .max(self.rows.iter().map(Vec::len).max().unwrap_or(0))
            .max(1)
    }

    pub fn align(&self, column: usize) -> Align {
        self.aligns.get(column).copied().unwrap_or(Align::Left)
    }
}

/// What a block renders as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Heading {
        level: u8,
        runs: Vec<Run>,
    },
    Paragraph(Vec<Run>),
    /// Fenced / indented code. Lines are not wrapped.
    Code {
        lang: String,
        lines: Vec<String>,
    },
    /// One list item (lists are flattened: see [`Block::indent`]).
    Item {
        ordered: bool,
        number: u64,
        runs: Vec<Run>,
    },
    Rule,
    Image {
        alt: String,
    },
    /// A table: header row + body rows, inline runs per cell.
    Table(Table),
}

/// One block with its structural position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub quote: usize,
    pub indent: usize,
    pub kind: BlockKind,
}

/// Parses Markdown (CommonMark + strikethrough/GFM lists) into blocks.
pub fn parse(text: &str) -> Vec<Block> {
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_GFM | Options::ENABLE_TABLES;
    let parser = Parser::new_ext(text, options);
    let mut state = ParseState::default();
    for event in parser {
        state.event(event);
    }
    state.finish()
}

#[derive(Clone, Copy, Debug)]
struct ListState {
    ordered: bool,
    next: u64,
}

#[derive(Debug, Default)]
struct ParseState {
    blocks: Vec<Block>,
    quote: usize,
    lists: Vec<ListState>,
    item: Option<ItemState>,
    heading: Option<u8>,
    code: Option<CodeState>,
    runs: Vec<Run>,
    styles: Vec<Style>,
    image: Option<String>,
    table: Option<TableState>,
}

#[derive(Debug)]
struct TableState {
    aligns: Vec<Align>,
    header: Vec<Vec<Run>>,
    rows: Vec<Vec<Vec<Run>>>,
    row: Vec<Vec<Run>>,
    cell: Vec<Run>,
    in_cell: bool,
}

#[derive(Debug)]
struct ItemState {
    ordered: bool,
    number: u64,
    depth: usize,
}

#[derive(Debug)]
struct CodeState {
    lang: String,
    text: String,
}

impl ParseState {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                if let Some(code) = &mut self.code {
                    code.text.push_str(&text);
                } else if let Some(alt) = &mut self.image {
                    alt.push_str(&text);
                } else {
                    self.push_inline(&text, self.current_style());
                }
            }
            Event::Code(code) => {
                if let Some(alt) = &mut self.image {
                    alt.push_str(&code);
                } else {
                    self.push_inline(&code, Style::Code);
                }
            }
            Event::SoftBreak => self.push_inline(" ", self.current_style()),
            Event::HardBreak => self.push_inline("\n", self.current_style()),
            Event::Rule => {
                self.flush_paragraph();
                self.push_block(BlockKind::Rule);
            }
            Event::TaskListMarker(checked) => {
                self.push_inline(if checked { "[x] " } else { "[ ] " }, Style::Plain);
            }
            // Raw HTML has no renderer here; ignore it rather than showing tags.
            Event::Html(_) | Event::InlineHtml(_) => {}
            Event::InlineMath(math) | Event::DisplayMath(math) => self.push_run(&math, Style::Code),
            Event::FootnoteReference(_) => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Heading { level, .. } => self.heading = Some(level as u8),
            Tag::Paragraph => {}
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some(CodeState {
                    lang,
                    text: String::new(),
                });
            }
            Tag::BlockQuote(_) => self.quote += 1,
            Tag::List(start) => {
                // A nested list starts after the parent item's leading text:
                // flush that text now so block order matches reading order.
                self.flush_item();
                self.lists.push(ListState {
                    ordered: start.is_some(),
                    next: start.unwrap_or(1),
                });
            }
            Tag::Item => {
                let depth = self.lists.len().saturating_sub(1);
                let (ordered, number) = match self.lists.last_mut() {
                    Some(list) => {
                        let number = list.next;
                        list.next += 1;
                        (list.ordered, number)
                    }
                    None => (false, 1),
                };
                self.item = Some(ItemState {
                    ordered,
                    number,
                    depth,
                });
            }
            Tag::Emphasis => self.styles.push(Style::Emphasis),
            Tag::Strong => self.styles.push(Style::Strong),
            Tag::Strikethrough => self.styles.push(Style::Strikethrough),
            Tag::Link { .. } => self.styles.push(Style::Link),
            Tag::Image { .. } => {
                self.flush_paragraph();
                self.image = Some(String::new());
            }
            Tag::Table(aligns) => {
                self.flush_paragraph();
                self.flush_item();
                self.table = Some(TableState {
                    aligns: aligns
                        .into_iter()
                        .map(|align| match align {
                            Alignment::Center => Align::Center,
                            Alignment::Right => Align::Right,
                            _ => Align::Left,
                        })
                        .collect(),
                    header: Vec::new(),
                    rows: Vec::new(),
                    row: Vec::new(),
                    cell: Vec::new(),
                    in_cell: false,
                });
            }
            Tag::TableHead => {
                if let Some(table) = &mut self.table {
                    table.row.clear();
                }
            }
            Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.row.clear();
                }
            }
            Tag::TableCell => {
                if let Some(table) = &mut self.table {
                    table.cell.clear();
                    table.in_cell = true;
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                let level = self.heading.take().unwrap_or(1);
                let runs = std::mem::take(&mut self.runs);
                if !runs.is_empty() {
                    self.push_block(BlockKind::Heading { level, runs });
                }
            }
            TagEnd::Paragraph => {
                if self.item.is_none() {
                    self.flush_paragraph();
                }
            }
            TagEnd::CodeBlock => {
                if let Some(code) = self.code.take() {
                    let mut lines: Vec<String> =
                        code.text.split('\n').map(str::to_string).collect();
                    while lines.last().is_some_and(|line| line.is_empty()) {
                        lines.pop();
                    }
                    self.push_block(BlockKind::Code {
                        lang: code.lang,
                        lines,
                    });
                }
            }
            TagEnd::Item => self.flush_item(),
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::BlockQuote(_) => {
                self.flush_paragraph();
                self.quote = self.quote.saturating_sub(1);
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                self.styles.pop();
            }
            TagEnd::Image => {
                let alt = self.image.take().unwrap_or_default();
                self.push_block(BlockKind::Image { alt });
            }
            TagEnd::TableCell => {
                if let Some(table) = &mut self.table {
                    table.in_cell = false;
                    let cell = std::mem::take(&mut table.cell);
                    table.row.push(cell);
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    table.header = std::mem::take(&mut table.row);
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    let row = std::mem::take(&mut table.row);
                    if !row.is_empty() {
                        table.rows.push(row);
                    }
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.push_block(BlockKind::Table(Table {
                        aligns: table.aligns,
                        header: table.header,
                        rows: table.rows,
                    }));
                }
            }
            _ => {}
        }
    }

    fn finish(mut self) -> Vec<Block> {
        self.flush_paragraph();
        self.flush_item();
        self.blocks
    }

    fn current_style(&self) -> Style {
        self.styles.last().copied().unwrap_or(Style::Plain)
    }

    /// Text goes to the open table cell when one is active, otherwise to the
    /// current block's inline buffer.
    fn push_inline(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        if let Some(table) = &mut self.table {
            if table.in_cell {
                if let Some(last) = table.cell.last_mut() {
                    if last.style == style {
                        last.text.push_str(text);
                        return;
                    }
                }
                table.cell.push(Run {
                    text: text.to_string(),
                    style,
                });
                return;
            }
        }
        self.push_run(text, style);
    }

    fn push_run(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.runs.last_mut() {
            if last.style == style {
                last.text.push_str(text);
                return;
            }
        }
        self.runs.push(Run {
            text: text.to_string(),
            style,
        });
    }

    fn flush_paragraph(&mut self) {
        let runs = std::mem::take(&mut self.runs);
        if !runs.is_empty() {
            self.push_block(BlockKind::Paragraph(runs));
        }
    }

    fn flush_item(&mut self) {
        if let Some(item) = self.item.take() {
            let runs = std::mem::take(&mut self.runs);
            if runs.is_empty() {
                return;
            }
            self.blocks.push(Block {
                quote: self.quote,
                indent: item.depth,
                kind: BlockKind::Item {
                    ordered: item.ordered,
                    number: item.number,
                    runs,
                },
            });
        }
    }

    fn push_block(&mut self, kind: BlockKind) {
        let indent = self.item.as_ref().map_or(0, |item| item.depth);
        self.blocks.push(Block {
            quote: self.quote,
            indent,
            kind,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(runs: &[Run]) -> String {
        runs.iter().map(|run| run.text.as_str()).collect()
    }

    #[test]
    fn headings_and_paragraphs() {
        let blocks = parse("# Title\n\nBody text.\n");
        assert_eq!(blocks.len(), 2);
        match &blocks[0].kind {
            BlockKind::Heading { level, runs } => {
                assert_eq!(*level, 1);
                assert_eq!(plain(runs), "Title");
            }
            other => panic!("expected heading, got {other:?}"),
        }
        match &blocks[1].kind {
            BlockKind::Paragraph(runs) => assert_eq!(plain(runs), "Body text."),
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn inline_styles_are_runs() {
        let blocks = parse("a **bold** and *soft* and `code` and ~~gone~~ and [link](x)\n");
        let BlockKind::Paragraph(runs) = &blocks[0].kind else {
            panic!("expected paragraph");
        };
        assert!(runs
            .iter()
            .any(|run| run.style == Style::Strong && run.text == "bold"));
        assert!(runs
            .iter()
            .any(|run| run.style == Style::Emphasis && run.text == "soft"));
        assert!(runs
            .iter()
            .any(|run| run.style == Style::Code && run.text == "code"));
        assert!(runs
            .iter()
            .any(|run| run.style == Style::Strikethrough && run.text == "gone"));
        assert!(runs
            .iter()
            .any(|run| run.style == Style::Link && run.text == "link"));
    }

    #[test]
    fn fenced_code_keeps_language_and_lines() {
        let blocks = parse("```rust\nfn main() {}\nlet x = 1;\n```\n");
        match &blocks[0].kind {
            BlockKind::Code { lang, lines } => {
                assert_eq!(lang, "rust");
                assert_eq!(lines, &["fn main() {}", "let x = 1;"]);
            }
            other => panic!("expected code block, got {other:?}"),
        }
    }

    #[test]
    fn lists_flatten_with_numbers_and_depth() {
        let blocks = parse("- one\n- two\n  1. nested\n- three\n");
        let items: Vec<(bool, u64, usize, String)> = blocks
            .iter()
            .filter_map(|block| match &block.kind {
                BlockKind::Item {
                    ordered,
                    number,
                    runs,
                } => Some((*ordered, *number, block.indent, plain(runs))),
                _ => None,
            })
            .collect();
        assert_eq!(
            items,
            vec![
                (false, 1, 0, "one".to_string()),
                (false, 2, 0, "two".to_string()),
                (true, 1, 1, "nested".to_string()),
                (false, 3, 0, "three".to_string()),
            ]
        );
    }

    #[test]
    fn quotes_carry_depth() {
        let blocks = parse("> quoted\n>\n> > deeper\n");
        assert_eq!(blocks[0].quote, 1);
        assert!(blocks.iter().any(|block| block.quote == 2));
    }

    #[test]
    fn rule_and_image() {
        let blocks = parse("---\n\n![alt text](x.png)\n");
        assert!(blocks.iter().any(|block| block.kind == BlockKind::Rule));
        assert!(blocks.iter().any(|block| block.kind
            == BlockKind::Image {
                alt: "alt text".into()
            }));
    }

    #[test]
    fn hard_break_forces_a_newline_run() {
        let blocks = parse("a  \nb\n");
        let BlockKind::Paragraph(runs) = &blocks[0].kind else {
            panic!("expected paragraph");
        };
        assert!(plain(runs).contains('\n'));
    }

    #[test]
    fn tables_keep_alignment_header_and_cells() {
        let blocks =
            parse("| Name | Value |\n|:-----|------:|\n| a    | **1** |\n| b    | 2     |\n");
        let block = blocks
            .iter()
            .find(|block| matches!(block.kind, BlockKind::Table(_)))
            .expect("table block");
        let BlockKind::Table(table) = &block.kind else {
            unreachable!()
        };
        assert_eq!(table.columns(), 2);
        assert_eq!(table.align(0), Align::Left);
        assert_eq!(table.align(1), Align::Right);
        assert_eq!(plain(&table.header[0]), "Name");
        assert_eq!(plain(&table.header[1]), "Value");
        assert_eq!(table.rows.len(), 2);
        assert_eq!(plain(&table.rows[0][0]), "a");
        assert!(table.rows[0][1]
            .iter()
            .any(|run| run.style == Style::Strong && run.text == "1"));
        assert_eq!(plain(&table.rows[1][1]), "2");
    }

    #[test]
    fn html_is_dropped_not_rendered_as_tags() {
        let blocks = parse("hello <b>world</b>\n");
        assert!(!format!("{blocks:?}").contains("<b>"));
    }
}
