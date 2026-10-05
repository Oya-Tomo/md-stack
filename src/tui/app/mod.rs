//! TUI state, the event loop, and how actions change the state. Drawing lives in `render`.

mod render;

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyEventKind};
use notify::{RecursiveMode, Watcher};
use ratatui::DefaultTerminal;
use ratatui::widgets::ListState;

use super::action::Action;
use super::clipboard;
use super::graphics::Graphics;
use super::viewer::Viewer;
use crate::store::{ProcessRecord, Store};

/// How long to wait for input before checking the store for changes.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

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
    /// Repaint the whole screen before the next frame, which erases stale Sixel images.
    redraw_all: bool,
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
            redraw_all: true,
        };
        app.refresh();
        if let [only] = app.sessions.as_slice() {
            let process = only.process.clone();
            app.open(process);
        }
        app
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let (change_sender, store_changes) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<_>| {
            if event.is_ok() {
                let _ = change_sender.send(());
            }
        })
        .context("starting the file watcher")?;
        watcher
            .watch(self.store.root(), RecursiveMode::Recursive)
            .with_context(|| format!("watching {}", self.store.root().display()))?;

        while !self.quit {
            self.draw(terminal)?;
            self.handle_events(&store_changes)?;
        }
        Ok(())
    }

    fn draw(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        if std::mem::take(&mut self.redraw_all) {
            terminal.clear()?;
        }
        terminal.draw(|frame| self.render(frame))?;
        Ok(())
    }

    // Input is polled on this thread: a separate reader thread would hold crossterm's event
    // reader and starve the cursor-position query that `Terminal::clear` performs.
    fn handle_events(&mut self, store_changes: &Receiver<()>) -> Result<()> {
        if event::poll(POLL_INTERVAL)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    self.message = None;
                    if let Some(action) = Action::from_key(self.screen, key) {
                        self.update(action);
                    }
                }
                Event::Resize(..) => {
                    self.graphics.clear_cache();
                    self.redraw_all = true;
                }
                _ => {}
            }
        }
        // Coalesce a burst of file events into a single re-read.
        if store_changes.try_iter().count() > 0 {
            self.refresh();
        }
        Ok(())
    }

    fn update(&mut self, action: Action) {
        match action {
            Action::Quit => self.quit = true,
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
                    self.redraw_all = true;
                }
            }
            Action::OpenSessions => {
                self.screen = Screen::Sessions;
                self.redraw_all = true;
                self.refresh();
            }
            Action::ToggleFollow => {
                self.follow = !self.follow;
                if self.follow
                    && let Some(viewer) = &mut self.viewer
                {
                    self.redraw_all |= viewer.show_last();
                }
            }
            Action::MovePostList => {
                self.list_placement = self.list_placement.next();
                self.redraw_all = true;
            }
            Action::CopySnippet => self.copy_snippet(),
            Action::CopyPost => self.copy_post(),
            Action::NextPost => self.choose_post(Viewer::show_next),
            Action::PreviousPost => self.choose_post(Viewer::show_previous),
            Action::Move(motion) => {
                if let Some(viewer) = &mut self.viewer
                    && let (Some(loaded), state) = viewer.parts_mut()
                {
                    self.redraw_all |= state.apply(motion, &loaded.doc);
                }
            }
        }
    }

    /// Shows another post chosen by hand, which locks the view to it (docs/SPEC.md §6.3).
    fn choose_post(&mut self, show: fn(&mut Viewer) -> bool) {
        if let Some(viewer) = &mut self.viewer {
            self.follow = false;
            self.redraw_all |= show(viewer);
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
            self.redraw_all |= viewer.refresh(&self.store, self.follow)?;
        }
        Ok(())
    }

    fn open(&mut self, process: ProcessRecord) {
        self.viewer = Some(Viewer::new(process));
        self.screen = Screen::Posts;
        self.follow = true;
        self.graphics.clear_cache();
        self.redraw_all = true;
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
