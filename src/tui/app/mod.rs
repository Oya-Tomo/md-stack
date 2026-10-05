//! TUI state, the event loop, and how actions change the state. Drawing lives in `render`.

mod render;

use std::iter;

use anyhow::{Context, Result};
use crossterm::event::{Event as CrosstermEvent, KeyEventKind};
use ratatui::DefaultTerminal;
use ratatui::widgets::ListState;

use super::action::Action;
use super::clipboard;
use super::event::{Event, EventHandler};
use super::graphics::Graphics;
use super::viewer::Viewer;
use crate::store::{ProcessRecord, Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Sessions,
    Posts,
}

/// Where the post list is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListPlacement {
    Side,
    Bottom,
    Hidden,
}

impl ListPlacement {
    fn next(self) -> Self {
        match self {
            Self::Side => Self::Bottom,
            Self::Bottom => Self::Hidden,
            Self::Hidden => Self::Side,
        }
    }
}

/// A running Claude Code process offered in the session picker.
struct SessionChoice {
    process: ProcessRecord,
    post_count: usize,
}

pub struct App {
    store: Store,
    graphics: Graphics,
    screen: Screen,
    sessions: Vec<SessionChoice>,
    /// Selection and scroll offset of the session picker.
    session_list: ListState,
    viewer: Option<Viewer>,
    /// Whether new posts are shown as they arrive; otherwise the view is locked to its post.
    follow: bool,
    list_placement: ListPlacement,
    message: Option<String>,
    quit: bool,
    /// Clear the terminal before the next frame, so that everything is drawn anew.
    redraw: bool,
}

impl App {
    pub fn new(store: Store, graphics: Graphics) -> Self {
        let mut app = Self {
            store,
            graphics,
            screen: Screen::Sessions,
            sessions: Vec::new(),
            session_list: ListState::default().with_selected(Some(0)),
            viewer: None,
            follow: true,
            list_placement: ListPlacement::Side,
            message: None,
            quit: false,
            redraw: false,
        };
        app.refresh();
        if let [only] = app.sessions.as_slice() {
            let process = only.process.clone();
            app.open(process);
        }
        app
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let events = EventHandler::new(self.store.root())?;
        while !self.quit {
            if std::mem::take(&mut self.redraw) {
                terminal.clear()?;
            }
            terminal.draw(|frame| self.render(frame))?;
            // Handle everything that queued up while drawing, so that a burst of key presses
            // costs one frame instead of one frame each.
            let first = events.next()?;
            let mut store_changed = false;
            for event in iter::once(first).chain(iter::from_fn(|| events.try_next())) {
                match event {
                    Event::Crossterm(event) => self.handle_crossterm_event(&event),
                    Event::StoreChanged => store_changed = true,
                    Event::InputFailed(e) => return Err(e).context("reading terminal input"),
                }
            }
            if store_changed {
                self.refresh();
            }
        }
        Ok(())
    }

    fn handle_crossterm_event(&mut self, event: &CrosstermEvent) {
        match event {
            CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                self.message = None;
                if let Some(action) = Action::from_key(self.screen, *key) {
                    self.update(action);
                }
            }
            // Images are laid out for the old size; drop them instead of keeping them around.
            CrosstermEvent::Resize(..) => self.graphics.clear_cache(),
            _ => {}
        }
    }

    fn update(&mut self, action: Action) {
        match action {
            Action::Quit => self.quit = true,
            Action::Redraw => self.redraw = true,
            Action::NextSession => self.session_list.select_next(),
            Action::PreviousSession => self.session_list.select_previous(),
            Action::OpenSession => {
                let selected = self.session_list.selected();
                if let Some(choice) = selected.and_then(|i| self.sessions.get(i)) {
                    let process = choice.process.clone();
                    self.open(process);
                }
            }
            Action::CloseSessions => {
                if self.viewer.is_some() {
                    self.screen = Screen::Posts;
                }
            }
            Action::OpenSessions => {
                self.screen = Screen::Sessions;
                self.refresh();
            }
            Action::ToggleFollow => {
                self.follow = !self.follow;
                if self.follow
                    && let Some(viewer) = &mut self.viewer
                {
                    viewer.show_last();
                }
            }
            Action::MovePostList => self.list_placement = self.list_placement.next(),
            Action::CopySnippet => self.copy_snippet(),
            Action::CopyPost => self.copy_post(),
            Action::NextPost => self.choose_post(Viewer::show_next),
            Action::PreviousPost => self.choose_post(Viewer::show_previous),
            Action::Move(motion) => {
                if let Some(viewer) = &mut self.viewer
                    && let (Some(loaded), state) = viewer.parts_mut()
                {
                    state.apply(motion, &loaded.doc);
                }
            }
        }
    }

    /// Shows another post chosen by hand, which locks the view to it (docs/SPEC.md §6.3).
    fn choose_post(&mut self, show: fn(&mut Viewer)) {
        if let Some(viewer) = &mut self.viewer {
            self.follow = false;
            show(viewer);
        }
    }

    /// Re-reads the store after it changed on disk.
    fn refresh(&mut self) {
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
        if let Some(viewer) = &mut self.viewer {
            viewer.refresh(&self.store, self.follow)?;
        }
        Ok(())
    }

    fn open(&mut self, process: ProcessRecord) {
        self.viewer = Some(Viewer::new(process));
        self.screen = Screen::Posts;
        self.follow = true;
        self.graphics.clear_cache();
        // Loads the posts and, since the view follows, shows the latest one.
        self.refresh();
    }

    fn copy_snippet(&mut self) {
        let Some((index, snippet)) = self.viewer.as_ref().and_then(Viewer::focused_snippet) else {
            self.message = Some("No snippet selected; press Tab to select one.".to_owned());
            return;
        };
        let label = format!("[{}] {}", index + 1, snippet.kind);
        let result = clipboard::copy(&snippet.text);
        self.report_copy(&result, &label);
    }

    fn copy_post(&mut self) {
        let Some(viewer) = &self.viewer else {
            return;
        };
        let Some(post) = viewer.current_post() else {
            return;
        };
        let label = format!("post #{}", post.id);
        let result = self
            .store
            .post_markdown(&viewer.process.session_id, post.id)
            .and_then(|markdown| clipboard::copy(&markdown));
        self.report_copy(&result, &label);
    }

    fn report_copy(&mut self, result: &Result<()>, label: &str) {
        self.message = Some(match result {
            Ok(()) => format!("Copied {label}."),
            Err(e) => format!("Copy failed: {e:#}"),
        });
    }
}
