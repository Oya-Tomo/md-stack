//! `md-stack hook`: Claude Code `SessionStart` hook (docs/SPEC.md §4).
//!
//! Records which conversation the calling Claude Code process now shows. It runs on
//! startup, `/clear` and resume alike, since each of them switches the conversation.

use std::io;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::Local;
use serde::Deserialize;

use crate::store::{ProcessRecord, Store, process_start_time};

#[derive(Deserialize)]
struct HookInput {
    session_id: String,
    cwd: PathBuf,
    transcript_path: PathBuf,
}

pub fn run() -> Result<()> {
    let input: HookInput =
        serde_json::from_reader(io::stdin().lock()).context("reading hook input from stdin")?;
    let claude_pid: u32 = std::env::var("CLAUDE_PID")
        .context("CLAUDE_PID is not set; run this as a Claude Code hook")?
        .parse()
        .context("CLAUDE_PID is not a process id")?;
    let process_start_time = process_start_time(claude_pid)
        .with_context(|| format!("Claude Code process {claude_pid} is not running"))?;

    Store::open()?.record_process(&ProcessRecord {
        claude_pid,
        process_start_time,
        session_id: input.session_id,
        cwd: input.cwd,
        transcript_path: input.transcript_path,
        updated_at: Local::now(),
    })
}
