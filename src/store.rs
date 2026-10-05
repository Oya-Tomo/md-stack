//! File-based store shared by all subcommands (see docs/SPEC.md §3).
//!
//! Writers place every file atomically (temp file + rename) so readers never see partial
//! contents, and a post becomes visible only once its metadata file appears.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::math::MathMetrics;

/// Which conversation a running Claude Code process currently shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessRecord {
    pub claude_pid: u32,
    pub process_start_time: u64,
    pub session_id: String,
    pub cwd: PathBuf,
    pub transcript_path: PathBuf,
    pub updated_at: DateTime<Local>,
}

impl ProcessRecord {
    pub fn is_alive(&self) -> bool {
        process_start_time(self.claude_pid).is_some_and(|t| t == self.process_start_time)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub cwd: PathBuf,
    pub transcript_path: PathBuf,
    pub created_at: DateTime<Local>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostMeta {
    pub id: u32,
    pub title: String,
    pub created_at: DateTime<Local>,
    /// Math expressions in document order; `math[i]` is stored as `NNNN/<i>.svg`.
    pub math: Vec<MathEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MathEntry {
    pub display: bool,
    pub tex: String,
    #[serde(flatten)]
    pub metrics: MathMetrics,
}

/// A rendered math expression to be stored with a new post.
pub struct NewMath {
    pub entry: MathEntry,
    pub svg: String,
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn open() -> Result<Self> {
        let root = dirs::state_dir()
            .context("cannot determine the XDG state directory")?
            .join("md-stack");
        for dir in [root.join("processes"), root.join("sessions")] {
            fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn process_path(&self, pid: u32) -> PathBuf {
        self.root.join("processes").join(format!("{pid}.json"))
    }

    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.root.join("sessions").join(session_id)
    }

    fn post_stem(&self, session_id: &str, post_id: u32) -> PathBuf {
        self.session_dir(session_id).join(format!("{post_id:04}"))
    }

    pub fn record_process(&self, record: &ProcessRecord) -> Result<()> {
        validate_session_id(&record.session_id)?;
        write_json(&self.process_path(record.claude_pid), record)
    }

    /// The record of a Claude Code process, if that process is still running.
    pub fn live_process(&self, pid: u32) -> Result<Option<ProcessRecord>> {
        Ok(read_json::<ProcessRecord>(&self.process_path(pid))?.filter(ProcessRecord::is_alive))
    }

    pub fn live_processes(&self) -> Result<Vec<ProcessRecord>> {
        let mut records = Vec::new();
        for path in json_files(&self.root.join("processes"))? {
            if let Some(record) = read_json::<ProcessRecord>(&path)?.filter(ProcessRecord::is_alive)
            {
                records.push(record);
            }
        }
        records.sort_by_key(|r| r.claude_pid);
        Ok(records)
    }

    pub fn posts(&self, session_id: &str) -> Result<Vec<PostMeta>> {
        let mut posts = Vec::new();
        for path in json_files(&self.session_dir(session_id))? {
            if path.file_stem().is_some_and(|s| s != "session")
                && let Some(post) = read_json::<PostMeta>(&path)?
            {
                posts.push(post);
            }
        }
        posts.sort_by_key(|p| p.id);
        Ok(posts)
    }

    pub fn post_markdown(&self, session_id: &str, post_id: u32) -> Result<String> {
        let path = self.post_stem(session_id, post_id).with_extension("md");
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
    }

    pub fn math_svg_path(&self, session_id: &str, post_id: u32, index: usize) -> PathBuf {
        self.post_stem(session_id, post_id)
            .join(format!("{index}.svg"))
    }

    /// Stores a post in the session of `process` and returns its id.
    ///
    /// `math` holds the post's expressions in document order.
    pub fn add_post(
        &self,
        process: &ProcessRecord,
        title: String,
        markdown: &str,
        math: Vec<NewMath>,
    ) -> Result<u32> {
        let session_id = &process.session_id;
        validate_session_id(session_id)?;
        let dir = self.session_dir(session_id);
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let session_path = dir.join("session.json");
        if !session_path.exists() {
            write_json(
                &session_path,
                &SessionMeta {
                    session_id: session_id.clone(),
                    cwd: process.cwd.clone(),
                    transcript_path: process.transcript_path.clone(),
                    created_at: Local::now(),
                },
            )?;
        }

        // Creating the asset directory reserves the id atomically.
        let mut id = self.posts(session_id)?.last().map_or(1, |p| p.id + 1);
        let stem = loop {
            let stem = self.post_stem(session_id, id);
            match fs::create_dir(&stem) {
                Ok(()) => break stem,
                Err(e) if e.kind() == ErrorKind::AlreadyExists => id += 1,
                Err(e) => return Err(e).with_context(|| format!("creating {}", stem.display())),
            }
        };

        let mut entries = Vec::with_capacity(math.len());
        for (index, NewMath { entry, svg }) in math.into_iter().enumerate() {
            write_atomic(&self.math_svg_path(session_id, id, index), svg.as_bytes())?;
            entries.push(entry);
        }
        write_atomic(&stem.with_extension("md"), markdown.as_bytes())?;
        write_json(
            &stem.with_extension("json"),
            &PostMeta {
                id,
                title,
                created_at: Local::now(),
                math: entries,
            },
        )?;
        Ok(id)
    }

    /// Removes records of exited processes and sessions whose transcript no longer exists.
    pub fn collect_garbage(&self) -> Result<()> {
        let mut active_sessions = Vec::new();
        for path in json_files(&self.root.join("processes"))? {
            match read_json::<ProcessRecord>(&path)? {
                Some(record) if record.is_alive() => active_sessions.push(record.session_id),
                _ => ignore_not_found(fs::remove_file(&path), &path)?,
            }
        }
        for dir in list_dir(&self.root.join("sessions"))? {
            let Some(meta) = read_json::<SessionMeta>(&dir.join("session.json"))? else {
                continue;
            };
            if !meta.transcript_path.exists() && !active_sessions.contains(&meta.session_id) {
                ignore_not_found(fs::remove_dir_all(&dir), &dir)?;
            }
        }
        Ok(())
    }
}

/// Start time of a process in clock ticks since boot (field 22 of `/proc/<pid>/stat`).
pub fn process_start_time(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name may contain spaces; fields after it are space separated.
    let rest = &stat[stat.rfind(')')? + 2..];
    rest.split(' ').nth(19)?.parse().ok()
}

/// Session ids become directory names, so only accept the characters Claude Code uses.
fn validate_session_id(session_id: &str) -> Result<()> {
    let valid = !session_id.is_empty()
        && session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        bail!("invalid session id: {session_id:?}");
    }
    Ok(())
}

fn list_dir(dir: &Path) -> Result<Vec<PathBuf>> {
    match fs::read_dir(dir) {
        Ok(entries) => entries
            .map(|e| Ok(e?.path()))
            .collect::<std::io::Result<_>>()
            .with_context(|| format!("listing {}", dir.display())),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("listing {}", dir.display())),
    }
}

/// The `.json` files in `dir`, leaving out the temporary files of writes in progress.
fn json_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = list_dir(dir)?;
    paths.retain(|path| path.extension().is_some_and(|e| e == "json"));
    Ok(paths)
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_atomic(path, &serde_json::to_vec_pretty(value)?)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("path has no parent directory")?;
    let mut file = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("creating a temporary file in {}", dir.display()))?;
    file.write_all(bytes)?;
    file.persist(path)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Treats a missing file as already removed (another process may have cleaned it up).
fn ignore_not_found(result: std::io::Result<()>, path: &Path) -> Result<()> {
    match result {
        Err(e) if e.kind() != ErrorKind::NotFound => {
            Err(e).with_context(|| format!("removing {}", path.display()))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_metrics_are_stored_flat() {
        let json = r#"{"display":true,"tex":"x","width_ex":1.5,"height_ex":2.0,"depth_ex":0.5}"#;
        let entry: MathEntry = serde_json::from_str(json).unwrap();
        let metrics = MathMetrics {
            width: 1.5,
            height: 2.0,
            depth: 0.5,
        };
        assert_eq!(entry.metrics, metrics);
        assert_eq!(serde_json::to_string(&entry).unwrap(), json);
    }

    #[test]
    fn json_files_leave_out_writes_in_progress() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("1.json");
        fs::write(&record, "{}").unwrap();
        // What `write_atomic` leaves while it writes.
        tempfile::NamedTempFile::new_in(dir.path())
            .unwrap()
            .keep()
            .unwrap();
        assert_eq!(json_files(dir.path()).unwrap(), [record]);
    }
}
