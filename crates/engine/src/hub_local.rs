//! Your own folders as a source for the Source Hub (fork roadmap F05.5): footage, pictures and
//! sound you already have. You name a folder and say whether what is in it is **your own** work or
//! **licensed to you**; searches then look at the file and folder names for your words, and a
//! chosen file is imported where it is (never copied) with that recorded as its provenance.
//!
//! | command | does |
//! |---|---|
//! | `hub.folders` | the folders you have added |
//! | `hub.addFolder {path, tag}` | adds a folder; `tag` is `own` or `licensed` |
//! | `hub.removeFolder {path}` | forgets a folder (nothing on disk is touched) |
//!
//! The folder list is kept in `edith-media-folders.json` beside the preferences. Looking is by name
//! only: nothing is opened or read until you add a file. Symbolic links are not followed, depth and
//! file counts are capped, and every path is checked to still lie inside an added folder before it is
//! imported.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use edith_sources::{Candidate, Download, Kind, Page, Request, Rights};

use crate::commands::{CommandSpec, always, bad, str_p};
use crate::{Result, Session};

const MAX_FOLDERS: usize = 16;
const MAX_DEPTH: usize = 8;
const MAX_FILES: usize = 30_000;
const FILE: &str = "edith-media-folders.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub path: String,
    /// `own` or `licensed`.
    pub tag: String,
}

const VIDEO: &[&str] = &["mp4", "mov", "m4v", "mkv", "webm", "avi", "mxf", "mpg", "mpeg", "mts", "m2ts", "mp4v"];
const IMAGE: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "tif", "tiff", "bmp"];
const AUDIO: &[&str] = &["wav", "mp3", "aac", "m4a", "flac", "ogg", "oga", "opus", "aif", "aiff"];

fn kind_of(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let ext = ext.as_str();
    if VIDEO.contains(&ext) {
        Some(Kind::Video)
    } else if IMAGE.contains(&ext) {
        Some(Kind::Image)
    } else if AUDIO.contains(&ext) {
        Some(Kind::Audio)
    } else {
        None
    }
}

fn rights_for(tag: &str) -> Rights {
    let name = if tag == "own" { "Own work" } else { "Licensed to you" };
    Rights { name: name.into(), url: String::new(), commercial: Some(true), attribution_required: Some(false) }
}

/// A short stable id for a path (FNV-1a), so a result keeps its id across searches.
fn short_id(path: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The words of a path, lower-cased, for matching.
fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// Looks through `folders` for files matching `req`: media of a kind asked for whose folder or file
/// name holds at least one of the words, best match first. Reads names only.
pub fn find(folders: &[Folder], req: &Request) -> Page {
    let wanted = words(&crate::hub_local::keywords(&req.text));
    let mut hits: Vec<(usize, Candidate)> = Vec::new();
    let mut seen = 0usize;
    for f in folders {
        let root = PathBuf::from(&f.path);
        let mut stack = vec![(root.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(read) = std::fs::read_dir(&dir) else { continue };
            for entry in read.flatten() {
                let path = entry.path();
                // a link could lead anywhere, even in a circle: leave it
                let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
                if meta.file_type().is_symlink() {
                    continue;
                }
                if meta.is_dir() {
                    if depth < MAX_DEPTH && !entry.file_name().to_string_lossy().starts_with('.') {
                        stack.push((path, depth + 1));
                    }
                    continue;
                }
                seen += 1;
                if seen > MAX_FILES {
                    break;
                }
                let Some(kind) = kind_of(&path).filter(|k| req.wants(*k)) else { continue };
                let rel = path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().to_lowercase();
                let have = words(&rel);
                let score = wanted.iter().filter(|w| have.iter().any(|h| h == *w || h.starts_with(w.as_str()))).count();
                if score == 0 && !wanted.is_empty() {
                    continue;
                }
                let full = path.to_string_lossy().into_owned();
                let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                hits.push((
                    score,
                    Candidate {
                        id: format!("local:{}", short_id(&full)),
                        source: "local".into(),
                        kind,
                        title: title.chars().take(150).collect(),
                        author: String::new(),
                        author_url: String::new(),
                        source_url: String::new(),
                        thumb: if kind == Kind::Image && meta.len() <= 12 * 1024 * 1024 { format!("file://{full}") } else { String::new() },
                        width: None,
                        height: None,
                        seconds: None,
                        tags: vec![f.tag.clone()],
                        rights: rights_for(&f.tag),
                        download: Some(Download { url: full, ext, width: None, height: None, bytes: Some(meta.len()), note: String::new(), resolve: false }),
                    },
                ));
            }
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.title.cmp(&b.1.title)));
    let total = hits.len() as u64;
    let n = req.count.clamp(1, 40) as usize;
    let skip = (req.page.saturating_sub(1) as usize).saturating_mul(n);
    Page { candidates: hits.into_iter().skip(skip).take(n).map(|(_, c)| c).collect(), total }
}

/// The words to match: the content words of what was typed (the same rules the sites get).
pub fn keywords(text: &str) -> String {
    edith_sources::query::keywords(text, 8, 100)
}

fn file_path(s: &Session) -> Option<PathBuf> {
    s.prefs_path.as_ref().and_then(|p| p.parent()).map(|d| d.join(FILE))
}

impl Session {
    /// The folders, read from disk the first time. Without a data directory (tests) they live in
    /// memory only.
    pub fn hub_folders(&mut self) -> Vec<Folder> {
        if !self.hub.folders_loaded {
            self.hub.folders_loaded = true;
            if let Some(p) = file_path(self)
                && let Ok(meta) = std::fs::metadata(&p)
                && meta.len() <= 1024 * 1024
                && let Ok(text) = std::fs::read_to_string(&p)
                && let Ok(list) = serde_json::from_str::<Vec<Folder>>(&text)
            {
                self.hub.folders = list.into_iter().filter(|f| matches!(f.tag.as_str(), "own" | "licensed") && !f.path.is_empty()).take(MAX_FOLDERS).collect();
            }
        }
        self.hub.folders.clone()
    }

    fn save_folders(&self) {
        if let Some(p) = file_path(self) {
            let Ok(json) = serde_json::to_string_pretty(&self.hub.folders) else { return };
            let tmp = p.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }
}

fn folders_json(s: &mut Session) -> Value {
    let list: Vec<Value> = s.hub_folders().iter().map(|f| json!({"path": f.path, "tag": f.tag, "exists": Path::new(&f.path).is_dir(), "name": Path::new(&f.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| f.path.clone())})).collect();
    json!({"folders": list})
}

fn folders(s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(folders_json(s))
}

fn add_folder(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "hub.addFolder";
    let path = str_p(p, "path").filter(|x| !x.is_empty() && x.len() <= 1000).ok_or_else(|| bad(cmd, "`path` must be the path of a folder"))?;
    let tag = str_p(p, "tag")
        .filter(|t| matches!(*t, "own" | "licensed"))
        .ok_or_else(|| bad(cmd, "`tag` must be `own` (your own footage) or `licensed` (licensed to you)"))?;
    let canonical = std::fs::canonicalize(path).map_err(|_| bad(cmd, "that folder could not be found"))?;
    if !canonical.is_dir() {
        return Err(bad(cmd, "that is not a folder"));
    }
    // a folder that is the whole disk, or a drive, would take forever and mean nothing
    if canonical.parent().is_none() {
        return Err(bad(cmd, "choose a folder, not a whole drive"));
    }
    let path = canonical.to_string_lossy().into_owned();
    let mut list = s.hub_folders();
    if list.len() >= MAX_FOLDERS && !list.iter().any(|f| f.path == path) {
        return Err(bad(cmd, format!("at most {MAX_FOLDERS} folders; remove one first")));
    }
    list.retain(|f| f.path != path);
    list.push(Folder { path, tag: tag.to_string() });
    s.hub.folders = list;
    s.save_folders();
    Ok(folders_json(s))
}

fn remove_folder(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "hub.removeFolder";
    let path = str_p(p, "path").filter(|x| !x.is_empty() && x.len() <= 1000).ok_or_else(|| bad(cmd, "`path` must be a folder you added"))?;
    let canonical = std::fs::canonicalize(path).map(|c| c.to_string_lossy().into_owned()).unwrap_or_else(|_| path.to_string());
    let mut list = s.hub_folders();
    let before = list.len();
    list.retain(|f| f.path != canonical && f.path != path);
    if list.len() == before {
        return Err(bad(cmd, "that is not one of your folders"));
    }
    s.hub.folders = list;
    s.save_folders();
    Ok(folders_json(s))
}

/// True when `path` lies inside one of `folders` (checked on the real path, links resolved).
pub fn inside(folders: &[Folder], path: &str) -> bool {
    let Ok(real) = std::fs::canonicalize(path) else { return false };
    folders.iter().any(|f| std::fs::canonicalize(&f.path).is_ok_and(|root| real.starts_with(root)))
}

/// The record source for a local file: `own` or `licensed`.
pub fn record_source(c: &Candidate) -> &'static str {
    if c.rights.name == "Own work" { "own" } else { "licensed" }
}

pub fn commands() -> Vec<CommandSpec> {
    let spec = |id, label, params, run: fn(&mut Session, &Value) -> Result<Value>| CommandSpec {
        id,
        label,
        menu: &[],
        shortcut: None,
        params,
        enabled: always,
        run,
        journal: false,
    };
    vec![
        spec("hub.folders", "Media Folders", "{}", folders),
        spec("hub.addFolder", "Add a Media Folder", r#"{"path":str,"tag":"own|licensed"}"#, add_folder),
        spec("hub.removeFolder", "Remove a Media Folder", r#"{"path":str}"#, remove_folder),
    ]
}
