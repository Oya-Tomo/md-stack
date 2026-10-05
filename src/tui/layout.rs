//! Lays out a post's Markdown as terminal lines that may embed math images.

use std::fmt;
use std::mem;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui_image::FontSize;
use syntect::highlighting::Theme;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::graphics::MathGeometry;
use super::highlight;
use crate::document;
use crate::store::MathEntry;

/// A laid-out post.
#[derive(Debug, Default)]
pub struct Doc {
    pub lines: Vec<Line>,
    /// Copyable blocks in document order.
    pub blocks: Vec<Block>,
}

/// One visual line. Lines holding math images can be several rows tall.
#[derive(Debug, Default)]
pub struct Line {
    pub height: u16,
    pub segments: Vec<Segment>,
    /// Blocks (indices into [`Doc::blocks`]) shown on this line.
    pub blocks: Vec<usize>,
}

#[derive(Debug)]
pub struct Segment {
    pub col: u16,
    /// Row offset within the line.
    pub row: u16,
    pub content: SegmentContent,
}

#[derive(Debug)]
pub enum SegmentContent {
    Text(Span<'static>),
    /// `index` refers to the post's math expressions.
    Math {
        index: usize,
        geometry: MathGeometry,
    },
}

#[derive(Debug)]
pub struct Block {
    pub kind: BlockKind,
    /// What is copied: the code, or the TeX source.
    pub text: String,
}

#[derive(Debug)]
pub enum BlockKind {
    Code { lang: String },
    DisplayMath,
    InlineMath,
}

impl fmt::Display for BlockKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Code { lang } if lang.is_empty() => f.write_str("code"),
            Self::Code { lang } => write!(f, "{lang} code"),
            Self::DisplayMath => f.write_str("display math"),
            Self::InlineMath => f.write_str("inline math"),
        }
    }
}

#[derive(Clone, Copy)]
pub struct LayoutContext {
    pub width: u16,
    pub font: FontSize,
    pub theme: &'static Theme,
}

pub fn layout(markdown: &str, math: &[MathEntry], ctx: LayoutContext) -> Doc {
    let mut builder = Builder::new(ctx, math.to_vec());
    for event in document::parser(markdown) {
        builder.event(event);
    }
    builder.finish()
}

const DIM: Style = Style::new().fg(Color::DarkGray);
const INLINE_CODE: Style = Style::new().fg(Color::Yellow);
const LINK: Style = Style::new()
    .fg(Color::Blue)
    .add_modifier(Modifier::UNDERLINED);

/// Inline content waiting to be wrapped into lines.
enum Atom {
    Word(String, Style),
    Space,
    Break,
    Math { index: usize, block: usize },
}

/// Something that indents the lines inside it.
enum Container {
    Quote,
    /// A list item shows its marker on its first line only.
    Item {
        marker: String,
        marker_shown: bool,
    },
}

/// A fenced or indented code block being collected.
struct CodeBlock {
    lang: String,
    text: String,
}

struct Table {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: String,
    header_rows: usize,
}

struct Builder {
    ctx: LayoutContext,
    math: Vec<MathEntry>,
    doc: Doc,
    inline: Vec<Atom>,
    styles: Vec<Style>,
    containers: Vec<Container>,
    /// Next number of each open list; `None` for bullet lists.
    lists: Vec<Option<u64>>,
    next_math: usize,
    code: Option<CodeBlock>,
    table: Option<Table>,
    blank_pending: bool,
}

impl Builder {
    fn new(ctx: LayoutContext, math: Vec<MathEntry>) -> Self {
        Self {
            ctx,
            math,
            doc: Doc::default(),
            inline: Vec::new(),
            styles: Vec::new(),
            containers: Vec::new(),
            lists: Vec::new(),
            next_math: 0,
            code: None,
            table: None,
            blank_pending: false,
        }
    }

    fn finish(mut self) -> Doc {
        self.flush_inline();
        self.doc
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => self.inline_text(&code, self.style().patch(INLINE_CODE)),
            Event::InlineMath(tex) => self.math(&tex, false),
            Event::DisplayMath(tex) => self.math(&tex, true),
            Event::Html(html) => {
                for line in html.lines() {
                    let mut line_builder = LineBuilder::default();
                    line_builder.push_text(line, DIM);
                    self.push_line(line_builder);
                }
            }
            Event::InlineHtml(html) => self.inline_text(&html, DIM),
            Event::FootnoteReference(label) => {
                self.inline_text(&format!("[^{label}]"), self.style());
            }
            Event::SoftBreak => self.inline.push(Atom::Space),
            Event::HardBreak => self.inline.push(Atom::Break),
            Event::Rule => {
                self.start_block();
                let mut line = LineBuilder::default();
                line.push_text(&"─".repeat(usize::from(self.content_width())), DIM);
                self.push_line(line);
                self.blank_pending = true;
            }
            Event::TaskListMarker(checked) => {
                self.inline_text(if checked { "☑" } else { "☐" }, self.style());
                self.inline.push(Atom::Space);
            }
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph | Tag::HtmlBlock => self.start_block(),
            Tag::Heading { level, .. } => {
                self.start_block();
                self.push_style(heading_style(level));
            }
            Tag::BlockQuote(_) => {
                self.start_block();
                self.containers.push(Container::Quote);
            }
            Tag::CodeBlock(kind) => {
                self.start_block();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some(CodeBlock {
                    lang,
                    text: String::new(),
                });
            }
            Tag::List(start) => {
                if self.in_list() {
                    self.flush_inline();
                } else {
                    self.start_block();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                self.start_block();
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let marker = format!("{n}. ");
                        *n += 1;
                        marker
                    }
                    _ => "• ".to_owned(),
                };
                self.containers.push(Container::Item {
                    marker,
                    marker_shown: false,
                });
            }
            Tag::Table(_) => {
                self.start_block();
                self.table = Some(Table {
                    rows: Vec::new(),
                    row: Vec::new(),
                    cell: String::new(),
                    header_rows: 0,
                });
            }
            Tag::Emphasis => self.push_style(Style::new().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(Style::new().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.push_style(Style::new().add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { .. } => self.push_style(LINK),
            Tag::Image { .. } => {
                self.push_style(Style::new().add_modifier(Modifier::ITALIC));
                self.inline_text("[image: ", self.style());
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock => self.end_block(),
            TagEnd::Heading(_) => {
                self.end_block();
                self.styles.pop();
            }
            TagEnd::BlockQuote(_) => {
                self.end_block();
                self.containers.pop();
            }
            TagEnd::CodeBlock => {
                if let Some(code) = self.code.take() {
                    self.code_block(code);
                }
                self.blank_pending = true;
            }
            TagEnd::List(_) => {
                self.flush_inline();
                self.lists.pop();
                self.blank_pending |= !self.in_list();
            }
            TagEnd::Item => {
                self.flush_inline();
                self.containers.pop();
            }
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    table.rows.push(mem::take(&mut table.row));
                    table.header_rows = table.rows.len();
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    table.rows.push(mem::take(&mut table.row));
                }
            }
            TagEnd::TableCell => {
                if let Some(table) = &mut self.table {
                    table.row.push(mem::take(&mut table.cell).trim().to_owned());
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.render_table(&table);
                }
                self.blank_pending = true;
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                self.styles.pop();
            }
            TagEnd::Image => {
                self.inline_text("]", self.style());
                self.styles.pop();
            }
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if let Some(code) = &mut self.code {
            code.text.push_str(text);
        } else if let Some(table) = &mut self.table {
            table.cell.push_str(text);
        } else {
            self.inline_text(text, self.style());
        }
    }

    fn math(&mut self, tex: &str, display: bool) {
        let index = self.next_math;
        self.next_math += 1;
        if index >= self.math.len() {
            // The stored metadata does not match the Markdown; show the source instead.
            self.inline_text(&format!("${tex}$"), INLINE_CODE);
            return;
        }
        if let Some(table) = &mut self.table {
            // Table cells hold plain text only.
            table.cell.extend(["$", tex, "$"]);
            return;
        }
        let block = self.doc.blocks.len();
        self.doc.blocks.push(Block {
            kind: if display {
                BlockKind::DisplayMath
            } else {
                BlockKind::InlineMath
            },
            text: tex.trim().to_owned(),
        });
        if display {
            self.flush_inline();
            self.display_math(index, block);
        } else {
            self.inline.push(Atom::Math { index, block });
        }
    }

    // --- Block helpers ---

    /// Ends pending inline content and separates the new block from the previous one.
    fn start_block(&mut self) {
        self.flush_inline();
        if mem::take(&mut self.blank_pending) && !self.doc.lines.is_empty() {
            self.push_line_with(LineBuilder::default(), false);
        }
    }

    fn end_block(&mut self) {
        self.flush_inline();
        self.blank_pending = true;
    }

    fn in_list(&self) -> bool {
        !self.lists.is_empty()
    }

    fn content_width(&self) -> u16 {
        let indent: usize = self.containers.iter().map(Container::width).sum();
        self.ctx.width.saturating_sub(to_cols(indent)).max(1)
    }

    fn code_block(&mut self, CodeBlock { lang, text: code }: CodeBlock) {
        let width = self.content_width();
        let block = self.doc.blocks.len();
        let label = format!(" [{}]", block + 1);
        let title = if lang.is_empty() {
            String::new()
        } else {
            format!(" {lang} ")
        };

        let mut header = LineBuilder::with_block(block);
        let fill = usize::from(width).saturating_sub(2 + title.width() + label.width());
        header.push_text(&format!("──{title}{}{label}", "─".repeat(fill)), DIM);
        self.push_line(header);

        for spans in highlight::highlight(&code, &lang, self.ctx.theme) {
            for chunk in wrap_spans(spans, width.saturating_sub(2).max(1)) {
                let mut line = LineBuilder::with_block(block);
                line.push_text("│ ", DIM);
                for span in chunk {
                    line.push_span(span);
                }
                self.push_line(line);
            }
        }

        let mut footer = LineBuilder::with_block(block);
        footer.push_text(&"─".repeat(usize::from(width)), DIM);
        self.push_line(footer);

        self.doc.blocks.push(Block {
            kind: BlockKind::Code { lang },
            text: code,
        });
    }

    fn display_math(&mut self, index: usize, block: usize) {
        let width = self.content_width();
        let label = format!("[{}]", block + 1);
        let label_width = str_cols(&label);
        let geometry = MathGeometry::fit(
            &self.math[index],
            self.ctx.font,
            width.saturating_sub(2 * (label_width + 1)),
        );
        let mut line = LineBuilder::with_block(block);
        line.advance_to(width.saturating_sub(geometry.cols) / 2);
        line.push_math(index, geometry);
        line.advance_to(width.saturating_sub(label_width));
        line.push_text(&label, DIM);
        self.push_line(line);
    }

    fn render_table(&mut self, table: &Table) {
        const SEPARATOR: &str = " │ ";
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }
        let mut widths = vec![1usize; columns];
        for row in &table.rows {
            for (width, cell) in widths.iter_mut().zip(row) {
                *width = (*width).max(cell.width());
            }
        }
        // Shrink the widest columns until the table fits.
        let available = usize::from(self.content_width());
        let separators = SEPARATOR.width() * (columns - 1);
        while widths.iter().sum::<usize>() + separators > available {
            let widest = widths.iter_mut().max().expect("columns > 0");
            if *widest <= 3 {
                break;
            }
            *widest -= 1;
        }

        for (i, row) in table.rows.iter().enumerate() {
            let style = if i < table.header_rows {
                Style::new().add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            let mut line = LineBuilder::default();
            for (c, &width) in widths.iter().enumerate() {
                if c > 0 {
                    line.push_text(SEPARATOR, DIM);
                }
                line.push_text(
                    &fit_to_width(row.get(c).map_or("", String::as_str), width),
                    style,
                );
            }
            self.push_line(line);
            if i + 1 == table.header_rows {
                let rule: Vec<String> = widths.iter().map(|&w| "─".repeat(w)).collect();
                let mut line = LineBuilder::default();
                line.push_text(&rule.join("─┼─"), DIM);
                self.push_line(line);
            }
        }
    }

    // --- Inline content ---

    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn push_style(&mut self, style: Style) {
        self.styles.push(self.style().patch(style));
    }

    /// Splits text into wrappable atoms: words, spaces, and single wide (CJK) characters.
    fn inline_text(&mut self, text: &str, style: Style) {
        let mut word = String::new();
        for ch in text.chars() {
            let wide = ch.width().unwrap_or(0) > 1;
            if ch.is_whitespace() || wide {
                if !word.is_empty() {
                    self.inline.push(Atom::Word(mem::take(&mut word), style));
                }
                self.inline.push(if wide {
                    Atom::Word(ch.to_string(), style)
                } else {
                    Atom::Space
                });
            } else {
                word.push(ch);
            }
        }
        if !word.is_empty() {
            self.inline.push(Atom::Word(word, style));
        }
    }

    /// Wraps the pending inline atoms into lines.
    fn flush_inline(&mut self) {
        let atoms = mem::take(&mut self.inline);
        if atoms.is_empty() {
            return;
        }
        let width = self.content_width();
        let mut line = LineBuilder::default();
        let mut space_pending = false;
        for atom in atoms {
            match atom {
                Atom::Space => space_pending = line.width > 0,
                Atom::Break => {
                    self.push_line(mem::take(&mut line));
                    space_pending = false;
                }
                Atom::Word(word, style) => {
                    let word_width = str_cols(&word);
                    self.break_or_space(&mut line, &mut space_pending, word_width, width);
                    if word_width <= width {
                        line.push_text(&word, style);
                        continue;
                    }
                    // Longer than a whole line: break between characters.
                    for ch in word.chars() {
                        let ch_width = to_cols(ch.width().unwrap_or(0));
                        if line.width + ch_width > width {
                            self.push_line(mem::take(&mut line));
                        }
                        line.push_text(ch.encode_utf8(&mut [0; 4]), style);
                    }
                }
                Atom::Math { index, block } => {
                    let geometry = MathGeometry::fit(&self.math[index], self.ctx.font, width);
                    self.break_or_space(&mut line, &mut space_pending, geometry.cols, width);
                    line.push_math(index, geometry);
                    line.blocks.push(block);
                }
            }
        }
        self.push_line(line);
    }

    /// Before placing an item of `item_width`, wraps to a new line or emits the pending space.
    fn break_or_space(
        &mut self,
        line: &mut LineBuilder,
        space_pending: &mut bool,
        item_width: u16,
        width: u16,
    ) {
        let space = u16::from(*space_pending);
        if line.width > 0 && line.width + space + item_width > width {
            self.push_line(mem::take(line));
        } else if *space_pending {
            line.push_text(" ", Style::new());
        }
        *space_pending = false;
    }

    // --- Line output ---

    fn push_line(&mut self, line: LineBuilder) {
        self.push_line_with(line, true);
    }

    /// Prefixes the line with container indentation and appends it to the document.
    /// List markers are only consumed by content lines, not by separating blank lines.
    fn push_line_with(&mut self, line: LineBuilder, show_markers: bool) {
        let mut prefix = String::new();
        for container in &mut self.containers {
            match container {
                Container::Quote => prefix.push_str("│ "),
                Container::Item {
                    marker,
                    marker_shown,
                } => {
                    if show_markers && !*marker_shown {
                        *marker_shown = true;
                        prefix.push_str(marker);
                    } else {
                        prefix.push_str(&" ".repeat(marker.width()));
                    }
                }
            }
        }
        let indent = str_cols(&prefix);
        // Text sits on the baseline row; math images extend above and below it.
        let text_row = line.above;
        let height = line.above + 1 + line.below;

        let mut segments = Vec::with_capacity(line.segments.len() + 1);
        if !prefix.is_empty() {
            segments.push(Segment {
                col: 0,
                row: text_row,
                content: SegmentContent::Text(Span::styled(prefix, DIM)),
            });
        }
        for mut segment in line.segments {
            segment.col += indent;
            segment.row = match &segment.content {
                SegmentContent::Text(_) => text_row,
                SegmentContent::Math { geometry, .. } => text_row - geometry.baseline_row,
            };
            segments.push(segment);
        }
        self.doc.lines.push(Line {
            height,
            segments,
            blocks: line.blocks,
        });
    }
}

impl Container {
    fn width(&self) -> usize {
        match self {
            Self::Quote => 2,
            Self::Item { marker, .. } => marker.width(),
        }
    }
}

fn heading_style(level: HeadingLevel) -> Style {
    let bold = Style::new().add_modifier(Modifier::BOLD);
    match level {
        HeadingLevel::H1 => bold.fg(Color::Magenta).add_modifier(Modifier::UNDERLINED),
        HeadingLevel::H2 => bold.fg(Color::Cyan),
        HeadingLevel::H3 => bold.fg(Color::Blue),
        _ => bold,
    }
}

/// A line under construction; segment rows are assigned once its height is known.
///
/// `above` and `below` count the rows that math images need above and below the baseline row.
#[derive(Default)]
struct LineBuilder {
    width: u16,
    above: u16,
    below: u16,
    segments: Vec<Segment>,
    blocks: Vec<usize>,
}

impl LineBuilder {
    fn with_block(block: usize) -> Self {
        Self {
            blocks: vec![block],
            ..Self::default()
        }
    }

    /// Leaves blank columns up to `col`.
    fn advance_to(&mut self, col: u16) {
        self.width = self.width.max(col);
    }

    fn push_text(&mut self, text: &str, style: Style) {
        self.push_span(Span::styled(text.to_owned(), style));
    }

    /// Appends a span, merging it into the previous one when they are contiguous and alike.
    fn push_span(&mut self, span: Span<'static>) {
        let span_width = to_cols(span.width());
        let end = self.width;
        self.width += span_width;
        if let Some(Segment {
            col,
            content: SegmentContent::Text(last),
            ..
        }) = self.segments.last_mut()
            && last.style == span.style
            && *col + to_cols(last.width()) == end
        {
            last.content.to_mut().push_str(&span.content);
            return;
        }
        self.segments.push(Segment {
            col: end,
            row: 0,
            content: SegmentContent::Text(span),
        });
    }

    fn push_math(&mut self, index: usize, geometry: MathGeometry) {
        self.segments.push(Segment {
            col: self.width,
            row: 0,
            content: SegmentContent::Math { index, geometry },
        });
        self.width += geometry.cols;
        self.above = self.above.max(geometry.baseline_row);
        self.below = self.below.max(geometry.rows - geometry.baseline_row - 1);
    }
}

/// Converts a display width to terminal columns, saturating at the largest representable width.
fn to_cols(width: usize) -> u16 {
    u16::try_from(width).unwrap_or(u16::MAX)
}

fn str_cols(text: &str) -> u16 {
    to_cols(text.width())
}

/// Splits a highlighted line into chunks of at most `width` columns.
fn wrap_spans(spans: Vec<Span<'static>>, width: u16) -> Vec<Vec<Span<'static>>> {
    let width = usize::from(width);
    let mut lines = vec![Vec::new()];
    let mut used = 0;
    for span in spans {
        let mut piece = String::new();
        for ch in span.content.chars() {
            let ch_width = ch.width().unwrap_or(0);
            if used + ch_width > width && used > 0 {
                let line = lines.last_mut().expect("lines is never empty");
                line.push(Span::styled(mem::take(&mut piece), span.style));
                lines.push(Vec::new());
                used = 0;
            }
            piece.push(ch);
            used += ch_width;
        }
        if !piece.is_empty() {
            let line = lines.last_mut().expect("lines is never empty");
            line.push(Span::styled(piece, span.style));
        }
    }
    lines
}

/// Pads or truncates (with an ellipsis) to exactly `width` columns.
fn fit_to_width(text: &str, width: usize) -> String {
    if text.width() <= width {
        return format!("{text}{}", " ".repeat(width - text.width()));
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if used + ch_width + 1 > width {
            break;
        }
        out.push(ch);
        used += ch_width;
    }
    out.push('…');
    format!("{out}{}", " ".repeat(width - used - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(width: u16) -> LayoutContext {
        LayoutContext {
            width,
            font: FontSize::new(10, 20),
            theme: highlight::theme(true),
        }
    }

    fn text_of(line: &Line) -> String {
        let mut out = String::new();
        for segment in &line.segments {
            let col = usize::from(segment.col);
            if out.width() < col {
                out.push_str(&" ".repeat(col - out.width()));
            }
            match &segment.content {
                SegmentContent::Text(span) => out.push_str(&span.content),
                SegmentContent::Math { geometry, .. } => {
                    out.push_str(&"M".repeat(usize::from(geometry.cols)));
                }
            }
        }
        out.trim_end().to_owned()
    }

    fn math(display: bool) -> MathEntry {
        MathEntry {
            display,
            tex: "x".into(),
            width_ex: 5.0,
            height_ex: 2.0,
            depth_ex: 0.5,
        }
    }

    #[test]
    fn wraps_words_and_cjk() {
        let doc = layout("hello world foo\n\nあいうえお", &[], ctx(11));
        let lines: Vec<_> = doc.lines.iter().map(text_of).collect();
        assert_eq!(lines, ["hello world", "foo", "", "あいうえお"]);
        let doc = layout("あいうえおか", &[], ctx(7));
        let lines: Vec<_> = doc.lines.iter().map(text_of).collect();
        assert_eq!(lines, ["あいう", "えおか"]);
    }

    #[test]
    fn lists_quotes_and_code_blocks() {
        let md = "- one\n- two\n  1. nested\n\n> quoted\n\n```rust\nfn main() {}\n```\n";
        let doc = layout(md, &[], ctx(30));
        let lines: Vec<_> = doc.lines.iter().map(text_of).collect();
        assert_eq!(
            lines,
            [
                "• one",
                "• two",
                "  1. nested",
                "",
                "│ quoted",
                "",
                "── rust ────────────────── [1]",
                "│ fn main() {}",
                "──────────────────────────────",
            ]
        );
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].text, "fn main() {}\n");
        assert!(doc.lines[6..].iter().all(|l| l.blocks == [0]));
    }

    #[test]
    fn math_becomes_images_and_blocks() {
        let doc = layout("Let $x$ be.\n\n$$y$$", &[math(false), math(true)], ctx(40));
        let lines: Vec<_> = doc.lines.iter().map(text_of).collect();
        assert_eq!(lines[0], "Let MMMMM be.");
        assert!(lines[2].contains("MMMMM") && lines[2].ends_with("[2]"));
        assert_eq!(doc.blocks.len(), 2);
        assert!(matches!(doc.blocks[0].kind, BlockKind::InlineMath));
        assert_eq!(doc.lines[0].blocks, [0]);
    }

    #[test]
    fn tables_fit_width() {
        let md = "| a | long header |\n|---|---|\n| 1 | value |\n";
        let doc = layout(md, &[], ctx(12));
        let lines: Vec<_> = doc.lines.iter().map(text_of).collect();
        assert_eq!(lines, ["a │ long he…", "──┼─────────", "1 │ value"]);
    }
}
