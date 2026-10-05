//! Drawing of the TUI screens.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui_image::Image;

use super::app::{App, Screen};
use super::layout::{Doc, SegmentContent};

const ACCENT: Style = Style::new().fg(Color::Cyan);
const DIM: Style = Style::new().fg(Color::DarkGray);
const SELECTED: Style = Style::new().add_modifier(Modifier::REVERSED);
/// Columns left of the post content, holding the focus marker.
const GUTTER: u16 = 2;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    draw_header(frame, header, app);
    match app.screen {
        Screen::Sessions => draw_sessions(frame, body, app),
        Screen::Posts => draw_posts(frame, body, app),
    }
    draw_status(frame, status, app);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![Span::styled(
        " md-stack ",
        ACCENT.add_modifier(Modifier::BOLD),
    )];
    if let Some(target) = &app.target {
        spans.push(Span::raw(format!("{} ", target.process.cwd.display())));
        spans.push(Span::styled(
            format!("pid {} ", target.process.claude_pid),
            DIM,
        ));
        if !target.alive {
            spans.push(Span::styled(
                "(Claude Code exited) ",
                Style::new().fg(Color::Red),
            ));
        }
        if app.follow {
            spans.push(Span::styled("[follow]", ACCENT));
        }
    }
    frame.render_widget(Line::from(spans), area);
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let text = if let Some(message) = &app.message {
        Span::raw(message.clone())
    } else {
        let help = match app.screen {
            Screen::Sessions => "j/k:move  Enter:open  Esc:back  q:quit",
            Screen::Posts => {
                "s:sessions  J/K:post  j/k:scroll  Tab:block  y:copy  Y:copy post  f:follow  q:quit"
            }
        };
        Span::styled(help, DIM)
    };
    frame.render_widget(Line::from(text), area);
}

fn draw_sessions(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::new()
        .borders(Borders::TOP)
        .title(" Running Claude Code sessions ");
    if app.sessions.is_empty() {
        let text = "Waiting for Claude Code… (the md-stack plugin must be installed)";
        frame.render_widget(Paragraph::new(text).style(DIM).block(block), area);
        return;
    }
    let items: Vec<ListItem> = app
        .sessions
        .iter()
        .map(|choice| {
            let process = &choice.process;
            ListItem::new(Line::from(vec![
                Span::raw(format!("{}  ", process.cwd.display())),
                Span::styled(
                    format!(
                        "{} posts · pid {} · since {}",
                        choice.post_count,
                        process.claude_pid,
                        process.updated_at.format("%H:%M")
                    ),
                    DIM,
                ),
            ]))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(app.session_index));
    frame.render_stateful_widget(
        List::new(items).block(block).highlight_style(SELECTED),
        area,
        &mut state,
    );
}

fn draw_posts(frame: &mut Frame, area: Rect, app: &mut App) {
    let list_width = (area.width / 4).clamp(16, 32);
    let [list_area, content_area] =
        Layout::horizontal([Constraint::Length(list_width), Constraint::Fill(1)]).areas(area);

    let items: Vec<ListItem> = app.target.as_ref().map_or_else(Vec::new, |target| {
        target
            .posts
            .iter()
            .map(|post| ListItem::new(format!("#{} {}", post.id, post.title)))
            .collect()
    });
    let mut state = ListState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(
        List::new(items)
            .block(Block::new().borders(Borders::RIGHT).border_style(DIM))
            .highlight_style(SELECTED),
        list_area,
        &mut state,
    );

    let content_area = content_area.inner(ratatui::layout::Margin::new(1, 0));
    let doc_width = content_area.width.saturating_sub(GUTTER);
    match app.doc(doc_width) {
        Some(doc) => draw_doc(frame, content_area, app, &doc),
        None => frame.render_widget(Paragraph::new("No posts yet.").style(DIM), content_area),
    }
}

/// Draws the visible lines of `doc`, starting at the scroll position.
fn draw_doc(frame: &mut Frame, area: Rect, app: &mut App, doc: &Doc) {
    let (Some(target), Some(post)) = (&app.target, app.current_post()) else {
        return;
    };
    let session_id = target.process.session_id.clone();
    let post = post.clone();

    let mut y = area.y;
    let mut shown = 0;
    for line in doc.lines.iter().skip(app.scroll) {
        if y + line.height > area.bottom() {
            break;
        }
        if app.focus.is_some_and(|f| line.blocks.contains(&f)) {
            for row in y..y + line.height {
                frame.buffer_mut()[(area.x, row)]
                    .set_symbol("▌")
                    .set_style(ACCENT);
            }
        }
        for segment in &line.segments {
            let x = area.x + GUTTER + segment.col;
            let top = y + segment.row;
            match &segment.content {
                SegmentContent::Text(span) => {
                    frame
                        .buffer_mut()
                        .set_span(x, top, span, area.right().saturating_sub(x));
                }
                SegmentContent::Math { index, geometry } => {
                    let Some(entry) = post.math.get(*index) else {
                        continue;
                    };
                    let path = app.store().math_svg_path(&session_id, post.id, *index);
                    let rect = Rect::new(x, top, geometry.cols, geometry.rows).intersection(area);
                    match app.graphics.protocol(&path, entry, *geometry) {
                        Ok(protocol) => frame.render_widget(Image::new(protocol), rect),
                        Err(e) => {
                            frame.render_widget(Span::styled("[math]", DIM), rect);
                            app.message = Some(format!("{e:#}"));
                        }
                    }
                }
            }
        }
        y += line.height;
        shown += 1;
    }
    app.page_lines = shown;
}
