//! `md-stack tui`: viewer for posts, running in its own terminal (docs/SPEC.md §6).

mod action;
mod app;
mod clipboard;
mod doc_view;
mod event;
mod graphics;
mod highlight;
mod layout;
mod viewer;

use anyhow::Result;

use self::app::App;
use self::graphics::Graphics;
use crate::store::Store;

pub fn run() -> Result<()> {
    let store = Store::open()?;
    store.collect_garbage()?;
    ratatui::run(|terminal| {
        // The terminal query needs the alternate screen and must finish before input is read.
        let graphics = Graphics::query()?;
        App::new(store, graphics).run(terminal)
    })
}
