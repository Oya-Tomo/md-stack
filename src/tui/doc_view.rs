//! A widget that draws a laid-out post, with scrolling and a focused snippet.

use std::path::PathBuf;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::{StatefulWidget, Widget};
use ratatui_image::Image;

use super::graphics::Graphics;
use super::layout::{Doc, SegmentContent};
use crate::store::PostMeta;

const FOCUS_MARKER: Style = Style::new().fg(Color::Cyan);
const DIM: Style = Style::new().fg(Color::DarkGray);
/// Columns left of the content, holding the focus marker.
pub const GUTTER: u16 = 2;
/// Lines kept above a snippet when scrolling to reveal it.
const REVEAL_CONTEXT: usize = 2;

/// A movement within a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    LineDown,
    LineUp,
    HalfPageDown,
    HalfPageUp,
    Top,
    Bottom,
    NextSnippet,
    PreviousSnippet,
}

/// Scroll position and focus of a [`DocView`].
#[derive(Debug, Default)]
pub struct DocState {
    /// Index of the first visible line.
    scroll: usize,
    focus: Option<usize>,
    /// Number of lines that fit on the last rendered page.
    page_lines: usize,
    /// The last error from drawing math, for the status line.
    error: Option<String>,
}

impl DocState {
    pub fn focus(&self) -> Option<usize> {
        self.focus
    }

    /// Applies `motion` within `doc`.
    pub fn apply(&mut self, motion: Motion, doc: &Doc) {
        let half_page = (self.page_lines / 2).max(1);
        match motion {
            Motion::LineDown => self.scroll_to(self.scroll + 1, doc),
            Motion::LineUp => self.scroll_to(self.scroll.saturating_sub(1), doc),
            Motion::HalfPageDown => self.scroll_to(self.scroll + half_page, doc),
            Motion::HalfPageUp => self.scroll_to(self.scroll.saturating_sub(half_page), doc),
            Motion::Top => self.scroll_to(0, doc),
            Motion::Bottom => self.scroll_to(usize::MAX, doc),
            Motion::NextSnippet | Motion::PreviousSnippet => {
                let count = doc.snippets.len();
                if count == 0 {
                    return;
                }
                self.focus = Some(match (self.focus, motion) {
                    (Some(f), Motion::NextSnippet) => (f + 1) % count,
                    (Some(f), _) => (f + count - 1) % count,
                    (None, Motion::NextSnippet) => 0,
                    (None, _) => count - 1,
                });
                self.reveal_focus(doc);
            }
        }
    }

    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    /// Makes `line` the first visible line, clamped to the document.
    fn scroll_to(&mut self, line: usize, doc: &Doc) {
        self.scroll = line.min(doc.lines.len().saturating_sub(1));
    }

    /// Scrolls the focused snippet's first line into the last rendered page.
    fn reveal_focus(&mut self, doc: &Doc) {
        let Some(line) = self
            .focus
            .and_then(|focus| doc.lines.iter().position(|l| l.snippets.contains(&focus)))
        else {
            return;
        };
        let visible = self.scroll..self.scroll + self.page_lines;
        if !visible.contains(&line) {
            self.scroll_to(line.saturating_sub(REVEAL_CONTEXT), doc);
        }
    }
}

/// How many lines starting at `scroll` fit, without cutting a line, in `height` rows.
fn visible_lines(doc: &Doc, scroll: usize, height: u16) -> usize {
    let mut used = 0;
    doc.lines
        .iter()
        .skip(scroll)
        .take_while(|line| {
            used += line.height;
            used <= height
        })
        .count()
}

/// Draws a post laid out by [`super::layout::layout`], with its math images.
pub struct DocView<'a> {
    pub doc: &'a Doc,
    pub post: &'a PostMeta,
    /// SVG file of each of the post's math expressions.
    pub svg_paths: &'a [PathBuf],
    pub graphics: &'a mut Graphics,
}

impl StatefulWidget for DocView<'_> {
    type State = DocState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut DocState) {
        state.scroll = state.scroll.min(self.doc.lines.len().saturating_sub(1));
        state.page_lines = visible_lines(self.doc, state.scroll, area.height);

        let mut y = area.y;
        for line in self
            .doc
            .lines
            .iter()
            .skip(state.scroll)
            .take(state.page_lines)
        {
            if state.focus.is_some_and(|f| line.snippets.contains(&f)) {
                for row in y..y + line.height {
                    buf[(area.x, row)].set_symbol("▌").set_style(FOCUS_MARKER);
                }
            }
            for segment in &line.segments {
                let x = area.x + GUTTER + segment.col;
                let top = y + segment.row;
                match &segment.content {
                    SegmentContent::Text(span) => {
                        buf.set_span(x, top, span, area.right().saturating_sub(x));
                    }
                    SegmentContent::Math { index, geometry } => {
                        let rect =
                            Rect::new(x, top, geometry.cols, geometry.rows).intersection(area);
                        let (Some(entry), Some(path)) =
                            (self.post.math.get(*index), self.svg_paths.get(*index))
                        else {
                            continue;
                        };
                        match self.graphics.math_image(path, entry, *geometry) {
                            Ok(image) => Image::new(image).render(rect, buf),
                            Err(e) => {
                                Span::styled("[math]", DIM).render(rect, buf);
                                state.error = Some(format!("{e:#}"));
                            }
                        }
                    }
                }
            }
            y += line.height;
        }
    }
}
