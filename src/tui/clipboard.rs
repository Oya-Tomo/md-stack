//! Clipboard access through the terminal (OSC 52).

use std::io::{self, Write};

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// Asks the terminal to put `text` on the system clipboard.
pub fn copy(text: &str) -> Result<()> {
    let mut stdout = io::stdout().lock();
    write!(stdout, "\x1b]52;c;{}\x07", STANDARD.encode(text))
        .and_then(|()| stdout.flush())
        .context("writing to the terminal")
}
