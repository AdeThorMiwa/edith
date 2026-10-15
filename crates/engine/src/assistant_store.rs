//! Saving the assistant's conversations, one JSON file per thread in `<data dir>/edith-threads/`.
//!
//! A thread is written after each finished turn (never in the middle of one), atomically: a
//! temporary file, then a rename, so a crash cannot leave half a conversation. Files are read as
//! untrusted: a damaged, oversize or mislabelled file is skipped, a conversation the provider would
//! reject (a tool call nobody answered) is cut back to its valid part, and only the newest few are
//! restored.

use std::path::{Path, PathBuf};

use edith_ai::{AgentEvent, Finish, Response, Thread, TraceMeta, Usage};
use serde::{Deserialize, Serialize};

pub(crate) const THREAD_DIR: &str = "edith-threads";
/// Largest thread file written or read.
const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
/// Most events kept per restored thread.
const MAX_EVENTS: usize = 20_000;
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
pub(crate) struct SavedThread {
    pub version: u32,
    pub id: u64,
    pub title: String,
    pub thread: Thread,
    pub events: Vec<AgentEvent>,
    pub dropped: u64,
    pub finish: Option<Finish>,
    pub usage: Usage,
    pub meta: TraceMeta,
    pub replies: Vec<Response>,
}

pub(crate) fn path(dir: &Path, id: u64) -> PathBuf {
    dir.join(THREAD_DIR).join(format!("{id}.json"))
}

/// Writes a thread atomically. The reason for a failure is safe to show.
pub(crate) fn save(dir: &Path, rec: &SavedThread) -> Result<(), String> {
    let json = serde_json::to_vec(rec).map_err(|_| "it could not be encoded".to_string())?;
    if json.len() as u64 > MAX_FILE_BYTES {
        return Err("it is too large to save".to_string());
    }
    let target = path(dir, rec.id);
    let folder = target.parent().ok_or("the folder is not valid")?;
    std::fs::create_dir_all(folder).map_err(|_| "the folder could not be created".to_string())?;
    let tmp = folder.join(format!("{}.json.tmp", rec.id));
    std::fs::write(&tmp, json).map_err(|_| "the file could not be written".to_string())?;
    std::fs::rename(&tmp, &target).map_err(|_| "the file could not be saved".to_string())
}

pub(crate) fn remove(dir: &Path, id: u64) {
    let _ = std::fs::remove_file(path(dir, id));
}

/// The conversations saved in `dir`, oldest first, at most `limit` (the newest). Also a note when
/// some files had to be skipped.
pub(crate) fn load_all(dir: &Path, limit: usize) -> (Vec<SavedThread>, Option<String>) {
    let Ok(entries) = std::fs::read_dir(dir.join(THREAD_DIR)) else { return (Vec::new(), None) };
    let mut found: Vec<SavedThread> = Vec::new();
    let mut skipped = 0usize;
    for e in entries.flatten() {
        let p = e.path();
        let Some(id) = p.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".json")).and_then(|n| n.parse::<u64>().ok()) else { continue };
        let rec = (|| {
            if e.metadata().ok()?.len() > MAX_FILE_BYTES {
                return None;
            }
            let rec: SavedThread = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            (rec.version == VERSION && rec.id == id).then_some(rec)
        })();
        match rec {
            Some(mut rec) => {
                rec.title = rec.title.chars().filter(|c| !c.is_control()).take(80).collect();
                if rec.title.trim().is_empty() {
                    rec.title = "Conversation".to_string();
                }
                rec.thread.repair();
                if rec.events.len() > MAX_EVENTS {
                    let extra = rec.events.len() - MAX_EVENTS;
                    rec.events.drain(..extra);
                    rec.dropped += extra as u64;
                }
                found.push(rec);
            }
            None => skipped += 1,
        }
    }
    found.sort_by_key(|r| r.id);
    if found.len() > limit {
        found.drain(..found.len() - limit);
    }
    let note = (skipped > 0).then(|| format!("{skipped} saved conversation{} could not be restored.", if skipped == 1 { "" } else { "s" }));
    (found, note)
}
