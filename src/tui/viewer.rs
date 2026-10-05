//! The followed Claude Code process, its conversation, and how that is being viewed.

use std::path::PathBuf;

use anyhow::Result;
use ratatui::widgets::ListState;

use super::doc_view::DocState;
use super::layout::{self, Doc, LayoutContext};
use crate::store::{PostMeta, ProcessRecord, Store};

pub struct Viewer {
    pub process: ProcessRecord,
    /// Whether the Claude Code process is still running.
    pub alive: bool,
    pub posts: Vec<PostMeta>,
    /// Selection and scroll offset of the post list; the selection is the post shown.
    pub post_list: ListState,
    pub doc_state: DocState,
    loaded: Option<LoadedPost>,
}

/// The shown post, laid out for a given width.
pub struct LoadedPost {
    pub post: PostMeta,
    pub width: u16,
    pub doc: Doc,
    pub svg_paths: Vec<PathBuf>,
}

impl Viewer {
    pub fn new(process: ProcessRecord) -> Self {
        Self {
            process,
            alive: true,
            posts: Vec::new(),
            post_list: ListState::default(),
            doc_state: DocState::default(),
            loaded: None,
        }
    }

    pub fn current_post(&self) -> Option<&PostMeta> {
        self.posts.get(self.post_list.selected()?)
    }

    /// Re-reads the process record and its posts. Returns whether the shown post changed.
    ///
    /// With `follow`, a newly arrived post is shown.
    pub fn refresh(&mut self, store: &Store, follow: bool) -> Result<bool> {
        let record = store.live_process(self.process.claude_pid)?;
        self.alive = record.is_some();
        // `/clear` or a resume switches the conversation shown in that Claude Code.
        let switched = record
            .as_ref()
            .is_some_and(|process| process.session_id != self.process.session_id);
        if let Some(process) = record {
            self.process = process;
        }
        let posts = store.posts(&self.process.session_id)?;
        let grew = posts.len() > self.posts.len();
        self.posts = posts;

        if switched {
            self.post_list = ListState::default();
            self.loaded = None;
        }
        Ok(if switched || (grew && follow) {
            self.show_last()
        } else {
            let selected = self.post_list.selected().map(|i| i.min(self.last_index()));
            self.show(selected)
        })
    }

    pub fn show_next(&mut self) -> bool {
        let next = self.post_list.selected().map_or(0, |i| i + 1);
        self.show(Some(next.min(self.last_index())))
    }

    pub fn show_previous(&mut self) -> bool {
        let previous = self.post_list.selected().map_or(0, |i| i.saturating_sub(1));
        self.show(Some(previous))
    }

    pub fn show_last(&mut self) -> bool {
        self.show(self.posts.len().checked_sub(1))
    }

    fn last_index(&self) -> usize {
        self.posts.len().saturating_sub(1)
    }

    /// Selects a post, starting it at the top with nothing focused. Returns whether it changed.
    fn show(&mut self, index: Option<usize>) -> bool {
        let index = index.filter(|_| !self.posts.is_empty());
        if index == self.post_list.selected() {
            return false;
        }
        self.post_list.select(index);
        self.doc_state = DocState::default();
        true
    }

    /// Lays out the shown post for `ctx`, reusing the previous layout when nothing changed.
    pub fn load(&mut self, store: &Store, ctx: LayoutContext) -> Result<()> {
        // Borrow fields separately so that `loaded` can be replaced below.
        let Some(post) = self.post_list.selected().and_then(|i| self.posts.get(i)) else {
            self.loaded = None;
            return Ok(());
        };
        let session_id = &self.process.session_id;
        let reusable = self
            .loaded
            .as_ref()
            .is_some_and(|l| l.post.id == post.id && l.width == ctx.width);
        if !reusable {
            let markdown = store.post_markdown(session_id, post.id)?;
            self.loaded = Some(LoadedPost {
                doc: layout::layout(&markdown, &post.math, ctx),
                svg_paths: (0..post.math.len())
                    .map(|i| store.math_svg_path(session_id, post.id, i))
                    .collect(),
                post: post.clone(),
                width: ctx.width,
            });
        }
        Ok(())
    }

    /// The loaded post and the view state, borrowed together for rendering.
    pub fn parts_mut(&mut self) -> (Option<&LoadedPost>, &mut DocState) {
        (self.loaded.as_ref(), &mut self.doc_state)
    }

    pub fn focused_snippet(&self) -> Option<(usize, &layout::Snippet)> {
        let index = self.doc_state.focus()?;
        Some((index, self.loaded.as_ref()?.doc.snippets.get(index)?))
    }
}
