//! TUI state and input handling. Drawing lives in `view`.

use std::rc::Rc;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::clipboard;
use super::graphics::Graphics;
use super::highlight;
use super::layout::{self, Doc, LayoutContext};
use crate::store::{PostMeta, ProcessRecord, Store};

/// Direction of a move through posts or blocks.
#[derive(Clone, Copy)]
enum Step {
    Forward,
    Backward,
}

pub enum Screen {
    Sessions,
    Posts,
}

/// A running Claude Code process offered in the session picker.
pub struct SessionChoice {
    pub process: ProcessRecord,
    pub post_count: usize,
}

/// The Claude Code process being followed and its current conversation.
pub struct Target {
    pub process: ProcessRecord,
    pub alive: bool,
    pub posts: Vec<PostMeta>,
}

pub struct App {
    store: Store,
    pub graphics: Graphics,
    pub screen: Screen,
    pub sessions: Vec<SessionChoice>,
    pub session_index: usize,
    pub target: Option<Target>,
    pub selected: usize,
    pub follow: bool,
    pub scroll: usize,
    pub focus: Option<usize>,
    /// Number of lines that fit on the last drawn page, for paging and focus scrolling.
    pub page_lines: usize,
    pub message: Option<String>,
    pub quit: bool,
    doc: Option<CachedDoc>,
    redraw_all: bool,
}

struct CachedDoc {
    session_id: String,
    post_id: u32,
    width: u16,
    doc: Rc<Doc>,
}

impl App {
    pub fn new(store: Store, graphics: Graphics) -> Self {
        let mut app = Self {
            store,
            graphics,
            screen: Screen::Sessions,
            sessions: Vec::new(),
            session_index: 0,
            target: None,
            selected: 0,
            follow: true,
            scroll: 0,
            focus: None,
            page_lines: 0,
            message: None,
            quit: false,
            doc: None,
            redraw_all: true,
        };
        app.refresh();
        if let [only] = app.sessions.as_slice() {
            let process = only.process.clone();
            app.open(process);
        }
        app
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Whether the next frame must repaint everything, which erases stale Sixel images.
    pub fn take_redraw_all(&mut self) -> bool {
        std::mem::take(&mut self.redraw_all)
    }

    pub fn current_post(&self) -> Option<&PostMeta> {
        self.target.as_ref()?.posts.get(self.selected)
    }

    /// The laid-out current post for a content area `width` columns wide.
    pub fn doc(&mut self, width: u16) -> Option<Rc<Doc>> {
        let target = self.target.as_ref()?;
        let post = target.posts.get(self.selected)?;
        let session_id = &target.process.session_id;
        let cached = self.doc.as_ref().is_some_and(|c| {
            c.session_id == *session_id && c.post_id == post.id && c.width == width
        });
        if !cached {
            let doc = match self.store.post_markdown(session_id, post.id) {
                Ok(markdown) => layout::layout(
                    &markdown,
                    &post.math,
                    LayoutContext {
                        width,
                        font: self.graphics.font(),
                        theme: highlight::theme(self.graphics.is_dark()),
                    },
                ),
                Err(e) => {
                    self.message = Some(format!("{e:#}"));
                    Doc::default()
                }
            };
            self.doc = Some(CachedDoc {
                session_id: session_id.clone(),
                post_id: post.id,
                width,
                doc: Rc::new(doc),
            });
            self.redraw_all = true;
        }
        self.doc.as_ref().map(|c| Rc::clone(&c.doc))
    }

    /// Re-reads the store after it changed on disk.
    pub fn refresh(&mut self) {
        if let Err(e) = self.try_refresh() {
            self.message = Some(format!("{e:#}"));
        }
    }

    fn try_refresh(&mut self) -> Result<()> {
        self.sessions = self
            .store
            .live_processes()?
            .into_iter()
            .map(|process| {
                let post_count = self.store.posts(&process.session_id)?.len();
                Ok(SessionChoice {
                    process,
                    post_count,
                })
            })
            .collect::<Result<_>>()?;
        self.session_index = self
            .session_index
            .min(self.sessions.len().saturating_sub(1));

        let Some(target) = &mut self.target else {
            return Ok(());
        };
        let pid = target.process.claude_pid;
        let mut switched = false;
        match self.store.live_process(pid)? {
            Some(process) => {
                // `/clear` or a resume switched the conversation in that Claude Code.
                switched = process.session_id != target.process.session_id;
                target.process = process;
                target.alive = true;
            }
            None => target.alive = false,
        }
        let posts = self.store.posts(&target.process.session_id)?;
        let grew = posts.len() > target.posts.len();
        target.posts = posts;
        let last = self.last_post_index();
        if switched || (grew && self.follow) {
            self.show_post(last);
        } else {
            self.selected = self.selected.min(last);
        }
        Ok(())
    }

    fn open(&mut self, process: ProcessRecord) {
        self.target = Some(Target {
            process,
            alive: true,
            posts: Vec::new(),
        });
        self.screen = Screen::Posts;
        self.follow = true;
        self.graphics.clear_cache();
        self.refresh();
        self.show_post(self.last_post_index());
    }

    fn last_post_index(&self) -> usize {
        self.target
            .as_ref()
            .map_or(0, |t| t.posts.len().saturating_sub(1))
    }

    fn show_post(&mut self, index: usize) {
        if index != self.selected {
            self.redraw_all = true;
        }
        self.selected = index;
        self.scroll = 0;
        self.focus = None;
    }

    pub fn on_resize(&mut self) {
        self.graphics.clear_cache();
        self.redraw_all = true;
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        self.message = None;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match self.screen {
            Screen::Sessions => self.on_sessions_key(key),
            Screen::Posts => self.on_posts_key(key),
        }
    }

    fn on_sessions_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('j') | KeyCode::Down => {
                self.session_index =
                    (self.session_index + 1).min(self.sessions.len().saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.session_index = self.session_index.saturating_sub(1);
            }
            KeyCode::Enter => {
                if let Some(choice) = self.sessions.get(self.session_index) {
                    let process = choice.process.clone();
                    self.open(process);
                }
            }
            KeyCode::Esc if self.target.is_some() => {
                self.screen = Screen::Posts;
                self.redraw_all = true;
            }
            _ => {}
        }
    }

    fn on_posts_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let half_page = (self.page_lines / 2).max(1);
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('s') => {
                self.screen = Screen::Sessions;
                self.refresh();
            }
            KeyCode::Char('d') if ctrl => self.scroll_to(self.scroll + half_page),
            KeyCode::Char('u') if ctrl => self.scroll_to(self.scroll.saturating_sub(half_page)),
            KeyCode::Char('j') | KeyCode::Down => self.scroll_to(self.scroll + 1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_to(self.scroll.saturating_sub(1)),
            KeyCode::Char('g') | KeyCode::Home => self.scroll_to(0),
            KeyCode::Char('G') | KeyCode::End => self.scroll_to(usize::MAX),
            KeyCode::Char('J' | ']') => self.select_post(Step::Forward),
            KeyCode::Char('K' | '[') => self.select_post(Step::Backward),
            KeyCode::Tab => self.move_focus(Step::Forward),
            KeyCode::BackTab => self.move_focus(Step::Backward),
            KeyCode::Char('y') => self.copy_focused(),
            KeyCode::Char('Y') => self.copy_post(),
            KeyCode::Char('f') => {
                self.follow = !self.follow;
                if self.follow {
                    self.show_post(self.last_post_index());
                }
            }
            _ => {}
        }
    }

    fn line_count(&self) -> usize {
        self.doc.as_ref().map_or(0, |c| c.doc.lines.len())
    }

    /// Scrolls so that line `line` is at the top, clamped to the document.
    fn scroll_to(&mut self, line: usize) {
        let scroll = line.min(self.line_count().saturating_sub(1));
        if scroll != self.scroll {
            self.scroll = scroll;
            self.redraw_all = true;
        }
    }

    fn select_post(&mut self, step: Step) {
        let index = match step {
            Step::Forward => (self.selected + 1).min(self.last_post_index()),
            Step::Backward => self.selected.saturating_sub(1),
        };
        // Choosing a post by hand stops following new ones (docs/SPEC.md §6.3).
        self.follow = false;
        self.show_post(index);
    }

    /// Moves the focus to the next or previous block, wrapping around.
    fn move_focus(&mut self, step: Step) {
        let Some(cached) = &self.doc else {
            return;
        };
        let count = cached.doc.blocks.len();
        if count == 0 {
            return;
        }
        let focus = match (self.focus, step) {
            (Some(f), Step::Forward) => (f + 1) % count,
            (Some(f), Step::Backward) => (f + count - 1) % count,
            (None, Step::Forward) => 0,
            (None, Step::Backward) => count - 1,
        };
        self.focus = Some(focus);
        // Bring the block's first line into view.
        if let Some(line) = cached
            .doc
            .lines
            .iter()
            .position(|l| l.blocks.contains(&focus))
            && (line < self.scroll || line >= self.scroll + self.page_lines)
        {
            self.scroll = line.saturating_sub(2);
            self.redraw_all = true;
        }
    }

    fn copy_focused(&mut self) {
        let focused = self
            .focus
            .and_then(|f| Some((f, self.doc.as_ref()?.doc.blocks.get(f)?)));
        let Some((index, block)) = focused else {
            self.message = Some("No block selected; press Tab to select one.".to_owned());
            return;
        };
        let label = format!("[{}] {}", index + 1, block.kind);
        let result = clipboard::copy(&block.text);
        self.report_copy(result, &label);
    }

    fn copy_post(&mut self) {
        let Some(target) = &self.target else {
            return;
        };
        let Some(post) = target.posts.get(self.selected) else {
            return;
        };
        let label = format!("post #{}", post.id);
        let result = self
            .store
            .post_markdown(&target.process.session_id, post.id)
            .and_then(|markdown| clipboard::copy(&markdown));
        self.report_copy(result, &label);
    }

    fn report_copy(&mut self, result: Result<()>, label: &str) {
        self.message = Some(match result {
            Ok(()) => format!("Copied {label}."),
            Err(e) => format!("Copy failed: {e:#}"),
        });
    }
}
