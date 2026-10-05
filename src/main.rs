#![forbid(unsafe_code)]

mod document;
mod hook;
mod math;
mod mcp;
mod store;
mod tui;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    version,
    about = "Render Claude Code's Markdown, math and code in a separate terminal"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the MCP server (spawned by Claude Code over stdio)
    Mcp,
    /// Run the `SessionStart` hook (spawned by Claude Code)
    Hook,
    /// Open the viewer in this terminal
    Tui,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Mcp => mcp::run(),
        Command::Tui => tui::run(),
        Command::Hook => {
            // A failing hook must not disturb Claude Code, so report and exit successfully.
            if let Err(e) = hook::run() {
                eprintln!("md-stack hook: {e:#}");
            }
            Ok(())
        }
    }
}
