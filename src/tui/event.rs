//! Events that drive the TUI, delivered over one channel as in ratatui's event-driven template.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event as CrosstermEvent};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// How long the input thread waits for input at a time. Each wait holds crossterm's event
/// reader, so this bounds both how long it takes to notice that the app has quit and how long
/// the cursor-position query of `Terminal::clear` (the redraw key) waits for the reader.
const POLL_TIMEOUT: Duration = Duration::from_millis(100);

pub enum Event {
    /// Input from the terminal.
    Crossterm(CrosstermEvent),
    /// Something in the store changed on disk.
    StoreChanged,
    /// Reading terminal input failed; no more input will arrive.
    InputFailed(std::io::Error),
}

pub struct EventHandler {
    receiver: Receiver<Event>,
    /// Kept alive to keep watching; dropping it stops the notifications.
    _watcher: RecommendedWatcher,
}

impl EventHandler {
    /// Starts reading terminal input and watching the store at `store_root`.
    pub fn new(store_root: &Path) -> Result<Self> {
        let (sender, receiver) = mpsc::channel();

        let store_sender = sender.clone();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<_>| {
            if event.is_ok() {
                let _ = store_sender.send(Event::StoreChanged);
            }
        })
        .context("starting the file watcher")?;
        watcher
            .watch(store_root, RecursiveMode::Recursive)
            .with_context(|| format!("watching {}", store_root.display()))?;

        thread::spawn(move || read_input(&sender));
        Ok(Self {
            receiver,
            _watcher: watcher,
        })
    }

    /// Waits for the next event.
    pub fn next(&self) -> Result<Event> {
        self.receiver.recv().context("the input thread stopped")
    }

    /// An event that has already arrived, without waiting.
    pub fn try_next(&self) -> Option<Event> {
        self.receiver.try_recv().ok()
    }
}

/// Forwards terminal input until the receiving side goes away or reading fails.
fn read_input(sender: &Sender<Event>) {
    loop {
        let event = match event::poll(POLL_TIMEOUT) {
            Ok(false) => continue,
            Ok(true) => event::read().map_or_else(Event::InputFailed, Event::Crossterm),
            Err(e) => Event::InputFailed(e),
        };
        let failed = matches!(event, Event::InputFailed(_));
        if sender.send(event).is_err() || failed {
            return;
        }
    }
}
