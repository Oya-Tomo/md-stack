//! Drawing of the screens.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, Paragraph};

use super::{App, ListPlacement, Screen};
use crate::tui::action;
use crate::tui::doc_view::{DocView, GUTTER};
use crate::tui::highlight;
use crate::tui::layout::LayoutContext;

impl App {
    pub(super) fn render(&mut self, frame: &mut Frame) {
        let [header, body, status] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        self.render_header(frame, header);
        match self.screen {
            Screen::Sessions => self.render_sessions(frame, body),
            Screen::Posts => self.render_posts(frame, body),
        }
        let status_line = match &self.message {
            Some(message) => Line::raw(message.as_str()),
            None => Line::raw(action::key_hints(self.screen)).dark_gray(),
        };
        frame.render_widget(status_line, status);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let mut spans = vec![" md-stack ".cyan().bold()];
        if let Some(viewer) = &self.viewer {
            spans.push(Span::raw(format!("{} ", viewer.process.cwd.display())));
            if !viewer.alive {
                spans.push("(Claude Code exited) ".red());
            }
            spans.push(if self.follow {
                "[follow]".cyan()
            } else {
                "[locked]".yellow()
            });
        }
        frame.render_widget(Line::from(spans), area);
    }

    fn render_sessions(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::new()
            .borders(Borders::TOP)
            .title(" Running Claude Code sessions ");
        if self.sessions.is_empty() {
            let waiting = "Waiting for Claude Code… (the md-stack plugin must be installed)";
            frame.render_widget(Paragraph::new(waiting).dark_gray().block(block), area);
            return;
        }
        let items = self.sessions.iter().map(|choice| {
            let process = &choice.process;
            Line::from(vec![
                Span::raw(format!("{}  ", process.cwd.display())),
                format!(
                    "{} posts · since {}",
                    choice.post_count,
                    process.updated_at.format("%H:%M")
                )
                .dark_gray(),
            ])
        });
        let list = List::new(items)
            .block(block)
            .highlight_style(Modifier::REVERSED);
        frame.render_stateful_widget(list, area, &mut self.session_list);
    }

    fn render_posts(&mut self, frame: &mut Frame, area: Rect) {
        let Some(viewer) = &mut self.viewer else {
            return;
        };

        let (content_area, list_area) = match self.list_placement {
            ListPlacement::Side => {
                let width = (area.width / 4).clamp(16, 32);
                let [list, content] =
                    Layout::horizontal([Constraint::Length(width), Constraint::Fill(1)])
                        .areas(area);
                (content, Some((list, Borders::RIGHT)))
            }
            ListPlacement::Bottom => {
                let height = (area.height / 4).clamp(3, 8);
                let [content, list] =
                    Layout::vertical([Constraint::Fill(1), Constraint::Length(height)]).areas(area);
                (content, Some((list, Borders::TOP)))
            }
            ListPlacement::Hidden => (area, None),
        };
        if let Some((list_area, border)) = list_area {
            let titles = viewer
                .posts
                .iter()
                .map(|post| format!("#{} {}", post.id, post.title));
            let list = List::new(titles)
                .block(Block::new().borders(border).dark_gray())
                .highlight_style(Modifier::REVERSED);
            frame.render_stateful_widget(list, list_area, &mut viewer.post_list);
        }

        let content_area = content_area.inner(Margin::new(1, 0));
        let ctx = LayoutContext {
            width: content_area.width.saturating_sub(GUTTER),
            font: self.graphics.font(),
            theme: highlight::theme(self.graphics.is_dark()),
        };
        if let Err(e) = viewer.load(&self.store, ctx) {
            self.message = Some(format!("{e:#}"));
            return;
        }
        let (Some(loaded), state) = viewer.parts_mut() else {
            frame.render_widget(Paragraph::new("No posts yet.").dark_gray(), content_area);
            return;
        };
        let doc_view = DocView {
            doc: &loaded.doc,
            post: &loaded.post,
            svg_paths: &loaded.svg_paths,
            graphics: &mut self.graphics,
        };
        frame.render_stateful_widget(doc_view, content_area, state);
        if let Some(error) = state.take_error() {
            self.message = Some(error);
        }
    }
}
