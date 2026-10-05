//! Lays out a post's Markdown as terminal lines that may embed math images.

use std::fmt;
use std::mem;

use pulldown_cmark::{Event, HeadingLevel, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui_image::FontSize;
use syntect::highlighting::Theme;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::graphics::MathGeometry;
use super::highlight;
use crate::document::{self, CodeBlock};
use crate::store::MathEntry;

/// A laid-out post.
#[derive(Debug, Default)]
pub struct Doc {
    pub lines: Vec<DocLine>,
    /// Copyable code blocks and math, in document order.
    pub snippets: Vec<Snippet>,
}

/// One visual line. Lines holding math images can be several rows tall.
#[derive(Debug, Default)]
pub struct DocLine {
    pub height: u16,
    /// Ordered by row, then column.
    pub segments: Vec<Segment>,
    /// Snippets (indices into [`Doc::snippets`]) shown on this line.
    pub snippets: Vec<usize>,
}

#[derive(Debug)]
pub struct Segment {
    pub col: u16,
    /// Row offset within the line.
    pub row: u16,
    pub content: SegmentContent,
}

impl Segment {
    fn text(col: u16, row: u16, span: Span<'static>) -> Self {
        Self {
            col,
            row,
            content: SegmentContent::Text(span),
        }
    }
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
pub struct Snippet {
    pub kind: SnippetKind,
    /// What is copied: the code, or the TeX source.
    pub text: String,
}

#[derive(Debug)]
pub enum SnippetKind {
    Code { lang: String },
    DisplayMath,
    InlineMath,
}

impl fmt::Display for SnippetKind {
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

/// Columns a tab in a code block is expanded to.
const TAB_WIDTH: usize = 4;

const DIM: Style = Style::new().fg(Color::DarkGray);
const INLINE_CODE: Style = Style::new().fg(Color::Yellow);
const LINK: Style = Style::new()
    .fg(Color::Blue)
    .add_modifier(Modifier::UNDERLINED);

/// Inline content waiting to be wrapped into lines.
#[derive(Clone)]
enum Atom {
    Word(String, Style),
    Space,
    Break,
    Math { index: usize, snippet: usize },
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

/// A table being collected; each cell holds the inline atoms of its content.
#[derive(Default)]
struct Table {
    rows: Vec<Vec<Vec<Atom>>>,
    row: Vec<Vec<Atom>>,
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
                self.code = Some(CodeBlock::new(&kind));
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
                self.table = Some(Table::default());
            }
            Tag::Emphasis => self.push_style(Style::new().add_modifier(Modifier::ITALIC)),
            // Header rows of tables are bold, like strong text.
            Tag::Strong | Tag::TableHead => {
                self.push_style(Style::new().add_modifier(Modifier::BOLD));
            }
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
                self.styles.pop();
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
                    table.row.push(mem::take(&mut self.inline));
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.render_table(table);
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
            code.code.push_str(text);
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
        let snippet = self.doc.snippets.len();
        self.doc.snippets.push(Snippet {
            kind: if display {
                SnippetKind::DisplayMath
            } else {
                SnippetKind::InlineMath
            },
            text: tex.trim().to_owned(),
        });
        if display {
            self.flush_inline();
            self.display_math(index, snippet);
        } else {
            self.inline.push(Atom::Math { index, snippet });
        }
    }

    // --- Block helpers ---

    /// Ends pending inline content and separates the new block from the previous one.
    fn start_block(&mut self) {
        self.flush_inline();
        if mem::take(&mut self.blank_pending) && !self.doc.lines.is_empty() {
            self.push_blank_line();
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
        self.ctx
            .width
            .saturating_sub(str_cols(&self.continuation()))
            .max(1)
    }

    /// Draws a code block in a rounded box with its language and block number on the top edge.
    fn code_block(&mut self, CodeBlock { lang, code }: CodeBlock) {
        // Border plus one column of padding on each side.
        const FRAME: u16 = 4;
        let width = self.content_width().max(FRAME + 1);
        let inner = width - FRAME;
        let snippet = self.doc.snippets.len();
        let title = if lang.is_empty() {
            String::new()
        } else {
            format!(" {lang} ")
        };
        let label = format!(" [{}] ", snippet + 1);

        let mut top = LineBuilder::with_snippet(snippet);
        let fill = usize::from(width - FRAME).saturating_sub(title.width() + label.width());
        top.push_text(&format!("╭─{title}{}{label}─╮", "─".repeat(fill)), DIM);
        self.push_line(top);

        // Tabs have no display width of their own, so show them as spaces; copying keeps them.
        let shown = code.replace('\t', &" ".repeat(TAB_WIDTH));
        for spans in highlight::highlight(&shown, &lang, self.ctx.theme) {
            for chunk in wrap_spans(spans, inner) {
                let mut line = LineBuilder::with_snippet(snippet);
                line.push_text("│ ", DIM);
                for span in chunk {
                    line.push_span(span);
                }
                line.advance_to(width - 2);
                line.push_text(" │", DIM);
                self.push_line(line);
            }
        }

        let mut bottom = LineBuilder::with_snippet(snippet);
        bottom.push_text(&format!("╰{}╯", "─".repeat(usize::from(width - 2))), DIM);
        self.push_line(bottom);

        self.doc.snippets.push(Snippet {
            kind: SnippetKind::Code { lang },
            text: code,
        });
    }

    fn display_math(&mut self, index: usize, snippet: usize) {
        let width = self.content_width();
        let label = format!("[{}]", snippet + 1);
        let label_width = str_cols(&label);
        let geometry = MathGeometry::fit(
            &self.math[index],
            self.ctx.font,
            width.saturating_sub(2 * (label_width + 1)),
        );
        let mut line = LineBuilder::with_snippet(snippet);
        line.advance_to(width.saturating_sub(geometry.cols) / 2);
        line.push_math(index, geometry);
        line.advance_to(width.saturating_sub(label_width));
        line.push_text(&label, DIM);
        self.push_line(line);
    }

    /// Lays out a table, wrapping cells inside their columns when it is too wide.
    fn render_table(&mut self, table: Table) {
        const SEPARATOR: &str = " │ ";
        const MIN_COLUMN_WIDTH: u16 = 3;
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }

        // Each column starts as wide as its widest cell laid out on one line.
        let mut widths = vec![1; columns];
        for row in &table.rows {
            for (width, cell) in widths.iter_mut().zip(row) {
                let natural = self.wrap(cell.clone(), u16::MAX);
                *width = natural.iter().map(|line| line.width).fold(*width, u16::max);
            }
        }
        let available = usize::from(self.content_width());
        let separators = SEPARATOR.width() * (columns - 1);
        while widths.iter().map(|&w| usize::from(w)).sum::<usize>() + separators > available {
            let widest = widths.iter_mut().max().expect("columns > 0");
            if *widest <= MIN_COLUMN_WIDTH {
                break;
            }
            *widest -= 1;
        }

        for (index, row) in table.rows.into_iter().enumerate() {
            let cells: Vec<_> = row
                .into_iter()
                .zip(&widths)
                .map(|(cell, &width)| self.wrap(cell, width))
                .collect();
            // The first lines of all cells share a baseline; each cell's further lines follow
            // right below its own previous line.
            let baseline = cells
                .iter()
                .filter_map(|lines| lines.first())
                .map(|line| line.above)
                .max()
                .unwrap_or(0);
            let mut block = Block::new(baseline);
            let mut col = 0;
            let mut cells = cells.into_iter();
            for (column, &width) in widths.iter().enumerate() {
                if column > 0 {
                    block.add_rule(col, Span::styled(SEPARATOR, DIM));
                    col += str_cols(SEPARATOR);
                }
                let mut top = None;
                for line in cells.next().into_iter().flatten() {
                    let line_top = top.unwrap_or(baseline - line.above);
                    top = Some(line_top + line.height());
                    block.add(line, line_top, col);
                }
                col += width;
            }
            self.push_block(block);
            if index + 1 == table.header_rows {
                let rule: Vec<String> =
                    widths.iter().map(|&w| "─".repeat(usize::from(w))).collect();
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

    /// Wraps the pending inline atoms into lines of the document.
    fn flush_inline(&mut self) {
        let atoms = mem::take(&mut self.inline);
        if atoms.is_empty() {
            return;
        }
        for line in self.wrap(atoms, self.content_width()) {
            self.push_line(line);
        }
    }

    /// Wraps inline atoms into lines at most `width` columns wide.
    fn wrap(&self, atoms: Vec<Atom>, width: u16) -> Vec<LineBuilder> {
        let mut wrapper = Wrapper::new(width);
        for atom in atoms {
            match atom {
                Atom::Space => wrapper.space(),
                Atom::Break => wrapper.break_line(),
                Atom::Word(word, style) => wrapper.word(&word, style),
                Atom::Math { index, snippet } => {
                    let geometry = MathGeometry::fit(&self.math[index], self.ctx.font, width);
                    wrapper.math(index, snippet, geometry);
                }
            }
        }
        wrapper.finish()
    }

    // --- Line output ---

    /// Appends a content line. The first line inside a list item shows the item's marker.
    fn push_line(&mut self, line: LineBuilder) {
        self.push_block(Block::from(line));
    }

    /// Appends laid-out content. Its first output inside a list item shows the item's marker.
    fn push_block(&mut self, block: Block) {
        let continuation = self.continuation();
        let mut indent = String::new();
        for container in &mut self.containers {
            match container {
                Container::Item {
                    marker,
                    marker_shown,
                } if !*marker_shown => {
                    *marker_shown = true;
                    indent.push_str(marker);
                }
                container => indent.push_str(&container.continuation()),
            }
        }
        self.emit(&indent, &continuation, block);
    }

    /// Appends an empty line that separates blocks; it never shows a list marker.
    fn push_blank_line(&mut self) {
        let continuation = self.continuation();
        self.emit(&continuation, &continuation, Block::new(0));
    }

    /// The indentation of lines (and rows) that show no list marker.
    fn continuation(&self) -> String {
        self.containers
            .iter()
            .map(Container::continuation)
            .collect()
    }

    /// Adds `block` to the document behind its indentation: `indent` on the text row and
    /// `continuation` on the other rows.
    fn emit(&mut self, indent: &str, continuation: &str, block: Block) {
        let indent_width = str_cols(indent);
        let mut segments = Vec::with_capacity(block.segments.len() + 1);
        for row in 0..block.height {
            let row_indent = if row == block.text_row {
                indent
            } else {
                continuation
            };
            // Rows of plain spaces need no segment.
            if !row_indent.trim().is_empty() {
                segments.push(Segment::text(
                    0,
                    row,
                    Span::styled(row_indent.to_owned(), DIM),
                ));
            }
            for (col, rule) in &block.rules {
                segments.push(Segment::text(col + indent_width, row, rule.clone()));
            }
        }
        segments.extend(block.segments.into_iter().map(|mut segment| {
            segment.col += indent_width;
            segment
        }));
        segments.sort_by_key(|segment| (segment.row, segment.col));
        self.doc.lines.push(DocLine {
            height: block.height,
            segments,
            snippets: block.snippets,
        });
    }
}

impl Container {
    /// The indentation this container adds to lines that show no list marker.
    fn continuation(&self) -> String {
        match self {
            Self::Quote => "│ ".to_owned(),
            Self::Item { marker, .. } => " ".repeat(marker.width()),
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

/// Greedy line breaking of inline content at a fixed width.
struct Wrapper {
    width: u16,
    lines: Vec<LineBuilder>,
    line: LineBuilder,
    /// A space between words, emitted only if the next item stays on the same line.
    space_pending: bool,
}

impl Wrapper {
    fn new(width: u16) -> Self {
        Self {
            width,
            lines: Vec::new(),
            line: LineBuilder::default(),
            space_pending: false,
        }
    }

    fn space(&mut self) {
        self.space_pending = self.line.width > 0;
    }

    fn break_line(&mut self) {
        self.lines.push(mem::take(&mut self.line));
        self.space_pending = false;
    }

    fn word(&mut self, word: &str, style: Style) {
        let word_width = str_cols(word);
        self.make_room(word_width);
        if word_width <= self.width {
            self.line.push_text(word, style);
            return;
        }
        // Longer than a whole line: break between characters.
        for ch in word.chars() {
            if self.line.width + to_cols(ch.width().unwrap_or(0)) > self.width {
                self.break_line();
            }
            self.line.push_text(ch.encode_utf8(&mut [0; 4]), style);
        }
    }

    fn math(&mut self, index: usize, snippet: usize, geometry: MathGeometry) {
        self.make_room(geometry.cols);
        self.line.push_math(index, geometry);
        self.line.snippets.push(snippet);
    }

    /// Before placing an item `item_width` wide, wraps to a new line or emits the pending space.
    fn make_room(&mut self, item_width: u16) {
        let space = u16::from(self.space_pending);
        if self.line.width > 0 && self.line.width + space + item_width > self.width {
            self.break_line();
        } else if self.space_pending {
            self.line.push_text(" ", Style::new());
        }
        self.space_pending = false;
    }

    fn finish(mut self) -> Vec<LineBuilder> {
        self.lines.push(self.line);
        self.lines
    }
}

/// Content placed on rows: one line, or a table row whose cells stack several lines.
struct Block {
    height: u16,
    /// The row that holds the first line's text, where a list marker goes.
    text_row: u16,
    segments: Vec<Segment>,
    /// Text repeated on every row, such as a table's column separators.
    rules: Vec<(u16, Span<'static>)>,
    snippets: Vec<usize>,
}

impl Block {
    fn new(text_row: u16) -> Self {
        Self {
            height: text_row + 1,
            text_row,
            segments: Vec::new(),
            rules: Vec::new(),
            snippets: Vec::new(),
        }
    }

    /// Places `line` with its top at row `top` and its left edge at column `col`; its text goes
    /// on its baseline row and its math images around it.
    fn add(&mut self, line: LineBuilder, top: u16, col: u16) {
        let baseline = top + line.above;
        self.height = self.height.max(top + line.height());
        for mut segment in line.segments {
            segment.col += col;
            segment.row = match &segment.content {
                SegmentContent::Text(_) => baseline,
                SegmentContent::Math { geometry, .. } => baseline - geometry.baseline_row,
            };
            self.segments.push(segment);
        }
        self.snippets.extend(line.snippets);
    }

    fn add_rule(&mut self, col: u16, rule: Span<'static>) {
        self.rules.push((col, rule));
    }
}

impl From<LineBuilder> for Block {
    fn from(line: LineBuilder) -> Self {
        let mut block = Self::new(line.above);
        block.add(line, 0, 0);
        block
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
    snippets: Vec<usize>,
}

impl LineBuilder {
    fn with_snippet(snippet: usize) -> Self {
        Self {
            snippets: vec![snippet],
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

    fn height(&self) -> u16 {
        self.above + 1 + self.below
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

    /// The document as the terminal shows it, one string per row. Math images are drawn as
    /// `M` over the cells they cover.
    fn screen(doc: &Doc) -> Vec<String> {
        let mut rows = Vec::new();
        for line in &doc.lines {
            let mut cells: Vec<Vec<(u16, String)>> = vec![Vec::new(); usize::from(line.height)];
            for segment in &line.segments {
                match &segment.content {
                    SegmentContent::Text(span) => {
                        cells[usize::from(segment.row)]
                            .push((segment.col, span.content.to_string()));
                    }
                    SegmentContent::Math { geometry, .. } => {
                        for row in segment.row..segment.row + geometry.rows {
                            let image = "M".repeat(usize::from(geometry.cols));
                            cells[usize::from(row)].push((segment.col, image));
                        }
                    }
                }
            }
            for mut row in cells {
                row.sort_by_key(|(col, _)| *col);
                let mut out = String::new();
                for (col, text) in row {
                    let col = usize::from(col);
                    if out.width() < col {
                        out.push_str(&" ".repeat(col - out.width()));
                    }
                    out.push_str(&text);
                }
                rows.push(out.trim_end().to_owned());
            }
        }
        rows
    }

    /// Inline math three rows tall: one row above and one below the baseline row.
    fn tall_math() -> MathEntry {
        MathEntry {
            display: false,
            tex: "x".into(),
            metrics: crate::math::MathMetrics {
                width: 3.0,
                height: 6.0,
                depth: 2.0,
            },
        }
    }

    fn math(display: bool) -> MathEntry {
        MathEntry {
            display,
            tex: "x".into(),
            metrics: crate::math::MathMetrics {
                width: 5.0,
                height: 2.0,
                depth: 0.5,
            },
        }
    }

    #[test]
    fn wraps_words_and_cjk() {
        let doc = layout("hello world foo\n\nあいうえお", &[], ctx(11));
        assert_eq!(screen(&doc), ["hello world", "foo", "", "あいうえお"]);
        let doc = layout("あいうえおか", &[], ctx(7));
        assert_eq!(screen(&doc), ["あいう", "えおか"]);
    }

    #[test]
    fn lists_quotes_and_code_blocks() {
        let md = "- one\n- two\n  1. nested\n\n> quoted\n\n```rust\nfn main() {}\n```\n";
        let doc = layout(md, &[], ctx(30));
        assert_eq!(
            screen(&doc),
            [
                "• one",
                "• two",
                "  1. nested",
                "",
                "│ quoted",
                "",
                "╭─ rust ─────────────── [1] ─╮",
                "│ fn main() {}               │",
                "╰────────────────────────────╯",
            ]
        );
        assert_eq!(doc.snippets.len(), 1);
        assert_eq!(doc.snippets[0].text, "fn main() {}\n");
        assert!(doc.lines[6..].iter().all(|l| l.snippets == [0]));
    }

    #[test]
    fn code_tabs_are_shown_as_spaces_but_copied_verbatim() {
        let doc = layout("```\n\tx\n```\n", &[], ctx(20));
        assert_eq!(screen(&doc)[1], "│     x            │");
        assert_eq!(doc.snippets[0].text, "\tx\n");
    }

    #[test]
    fn math_becomes_images_and_snippets() {
        let doc = layout("Let $x$ be.\n\n$$y$$", &[math(false), math(true)], ctx(40));
        let rows = screen(&doc);
        assert_eq!(rows[0], "Let MMMMM be.");
        assert!(rows[2].contains("MMMMM") && rows[2].ends_with("[2]"));
        assert_eq!(doc.snippets.len(), 2);
        assert!(matches!(doc.snippets[0].kind, SnippetKind::InlineMath));
        assert_eq!(doc.lines[0].snippets, [0]);
    }

    #[test]
    fn table_cells_wrap_to_fit() {
        let md = "| a | long header |\n|---|---|\n| 1 | value |\n";
        let doc = layout(md, &[], ctx(12));
        assert_eq!(
            screen(&doc),
            ["a │ long", "  │ header", "──┼─────────", "1 │ value"]
        );
    }

    #[test]
    fn wrapped_cell_lines_follow_each_other_beside_tall_math() {
        // The second line of the left cell sits right below its first line, not below the
        // bottom of the taller math in the right cell.
        let md = "| long text here | $x$ |\n|---|---|\n";
        let doc = layout(md, &[tall_math()], ctx(15));
        assert_eq!(
            screen(&doc)[..3],
            ["          │ MMM", "long text │ MMM", "here      │ MMM"]
        );
    }

    #[test]
    fn rows_added_by_tall_math_keep_quote_bars_and_separators() {
        let doc = layout("> | a | $x$ |\n> |---|---|\n", &[tall_math()], ctx(40));
        assert_eq!(doc.lines[0].height, 3);
        assert_eq!(screen(&doc)[..3], ["│   │ MMM", "│ a │ MMM", "│   │ MMM"]);
    }

    #[test]
    fn table_cells_hold_math() {
        let md = "| case | value |\n|---|---|\n| $x$ | one |\n";
        let doc = layout(md, &[math(false)], ctx(40));
        assert_eq!(screen(&doc)[2], "MMMMM │ one");
        assert!(matches!(doc.snippets[0].kind, SnippetKind::InlineMath));
        assert_eq!(doc.lines[2].snippets, [0]);
    }
}
