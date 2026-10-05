//! `md-stack tui`: viewer for posts, running in its own terminal (docs/SPEC.md §6).

mod app;
mod clipboard;
mod graphics;
mod highlight;
mod layout;
mod view;

use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyEventKind};
use notify::{RecursiveMode, Watcher};
use ratatui::DefaultTerminal;

use self::app::App;
use self::graphics::Graphics;
use crate::store::Store;

/// How long to wait for input before checking the store for changes.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub fn run() -> Result<()> {
    let store = Store::open()?;
    store.collect_garbage()?;
    let mut terminal = ratatui::init();
    let result = run_app(&mut terminal, store);
    ratatui::restore();
    result
}

fn run_app(terminal: &mut DefaultTerminal, store: Store) -> Result<()> {
    let graphics = Graphics::query()?;

    let (change_sender, store_changes) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        if result.is_ok() {
            let _ = change_sender.send(());
        }
    })
    .context("starting the file watcher")?;
    watcher
        .watch(store.root(), RecursiveMode::Recursive)
        .with_context(|| format!("watching {}", store.root().display()))?;

    // Input is polled on this thread: a separate reader thread would hold crossterm's event
    // reader and starve the cursor-position query that `Terminal::clear` performs.
    let mut app = App::new(store, graphics);
    while !app.quit {
        if app.take_redraw_all() {
            terminal.clear()?;
        }
        terminal.draw(|frame| view::draw(frame, &mut app))?;

        while event::poll(POLL_INTERVAL)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                Event::Resize(..) => app.on_resize(),
                _ => {}
            }
            if app.quit {
                return Ok(());
            }
        }
        // Coalesce a burst of file events into a single re-read.
        if store_changes.try_iter().count() > 0 {
            app.refresh();
        }
    }
    Ok(())
}
