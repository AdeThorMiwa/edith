//! The Source Hub (fork roadmap F05): find free stock video, pictures and sound, shortlist what you
//! like, and bring a chosen file into the project with its licence recorded.
//!
//! | command | does |
//! |---|---|
//! | `hub.providers` | the sources, what each offers, and whether its key is stored |
//! | `hub.search {text, kinds?, sources?, orientation?, minWidth?, minHeight?, minSeconds?, maxSeconds?, count?, page?, wait?}` | asks the sources at once (a background search; `wait: true` blocks) |
//! | `hub.results` | the state of the latest search: each source's status and the candidates found |
//! | `hub.shortlist {id?, on?}` | with `id`, adds or removes a candidate from your shortlist; without, lists it |
//! | `hub.acquire {id, bin?, wait?}` | downloads a candidate (a cancellable job), imports it into the `Sources/<site>` bin and records its provenance (F04) |
//! | `hub.cancel` | stops the running search |
//!
//! A candidate can only be acquired by the id of a result this session has seen: the address that is
//! fetched always comes from a site's own answer, never from a command's parameters. Answers are
//! kept for 24 hours (Pixabay's terms ask for it, and it saves rate limit). Nothing is downloaded
//! until you acquire it. Everything goes through `edith-net`: Offline mode blocks it, the request log
//! shows it, and no key appears in any result.

// Without the `net` feature the search and download code is not built; what remains is unused.
#![cfg_attr(not(feature = "net"), allow(dead_code, unused_imports))]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value, json};

use edith_sources::{Candidate, Kind, Orientation, Page, Request};

use crate::commands::{CommandSpec, always, bad, str_p};
use crate::panels::Level;
use crate::{EngineError, Job, Result, Session};

/// How long an answer is reused.
const CACHE_SECS: u64 = 24 * 3600;
const MAX_CACHE: usize = 100;
const MAX_SHORTLIST: usize = 200;
const MAX_THUMBS: usize = 400;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone, Debug, PartialEq)]
pub enum SourceStatus {
    Waiting,
    Done,
    Failed(String),
}

#[derive(Clone, Debug)]
struct SourceState {
    id: String,
    status: SourceStatus,
    candidates: Vec<Candidate>,
    total: u64,
}

struct Search {
    id: u64,
    request: Request,
    sources: Vec<SourceState>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

/// Where a download is.
#[derive(Clone, Debug, PartialEq)]
pub enum Acquired {
    Downloading,
    Done { item: u64 },
    Failed(String),
}

/// How a download ended: the file's SHA-256, or what went wrong.
type Finished = Arc<Mutex<Option<std::result::Result<String, String>>>>;

struct Pending {
    candidate: Candidate,
    path: String,
    bin: Option<u64>,
    finished: Finished,
    job: u64,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    progress: Arc<filmcraft_export::Progress>,
}

enum Thumb {
    Loading,
    Ready(Arc<Vec<u8>>),
    Failed,
}

/// Everything the Source Hub keeps on a session. Not part of the project and not undoable.
#[derive(Default)]
pub struct Hub {
    search: Option<Arc<Mutex<Search>>>,
    counter: u64,
    cache: Arc<Mutex<BTreeMap<String, (u64, Page)>>>,
    shortlist: Vec<Candidate>,
    pending: Vec<Pending>,
    acquired: BTreeMap<String, Acquired>,
    thumbs: Arc<Mutex<BTreeMap<String, Thumb>>>,
    #[cfg(feature = "net")]
    sources: Option<Arc<edith_sources::Sources>>,
}

impl Hub {
    /// Uses `sources` instead of the real sites. For tests and tools.
    #[cfg(feature = "net")]
    pub fn use_sources(&mut self, sources: Arc<edith_sources::Sources>) {
        self.sources = Some(sources);
    }

    #[cfg(feature = "net")]
    fn sources(&mut self) -> Arc<edith_sources::Sources> {
        self.sources
            .get_or_insert_with(|| {
                let client = edith_net::http::Client::new(edith_net::http::ClientConfig::default(), edith_net::shared().clone());
                Arc::new(edith_sources::Sources::new(Arc::new(client), edith_sources::keychain_keys()))
            })
            .clone()
    }

    /// A candidate this session has seen, by id.
    pub fn candidate(&self, id: &str) -> Option<Candidate> {
        if let Some(s) = &self.search
            && let Some(c) = lock(s).sources.iter().flat_map(|x| x.candidates.iter()).find(|c| c.id == id)
        {
            return Some(c.clone());
        }
        self.shortlist.iter().chain(self.pending.iter().map(|p| &p.candidate)).find(|c| c.id == id).cloned()
    }

    pub fn is_shortlisted(&self, id: &str) -> bool {
        self.shortlist.iter().any(|c| c.id == id)
    }

    pub fn acquired(&self, id: &str) -> Option<&Acquired> {
        self.acquired.get(id)
    }

    /// The bytes of a candidate's thumbnail once fetched; asks for them (in the background) the
    /// first time. `None` while loading, or when it could not be had.
    #[cfg(feature = "net")]
    pub fn thumbnail(&mut self, c: &Candidate) -> Option<Arc<Vec<u8>>> {
        if c.thumb.is_empty() {
            return None;
        }
        {
            let mut map = lock(&self.thumbs);
            match map.get(&c.id) {
                Some(Thumb::Ready(b)) => return Some(b.clone()),
                Some(_) => return None,
                None => {}
            }
            // a few at a time, so a page of results does not become fifty requests at once
            if map.values().filter(|t| matches!(t, Thumb::Loading)).count() >= 4 {
                return None;
            }
            if map.len() >= MAX_THUMBS {
                map.retain(|_, t| matches!(t, Thumb::Ready(_)));
                if map.len() >= MAX_THUMBS {
                    map.clear();
                }
            }
            map.insert(c.id.clone(), Thumb::Loading);
        }
        let (sources, thumbs, id, url) = (self.sources(), self.thumbs.clone(), c.id.clone(), c.thumb.clone());
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let spawned = std::thread::Builder::new().name("hub-thumb".into()).spawn({
            let thumbs = thumbs.clone();
            let id = id.clone();
            move || {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sources.thumbnail(&url, &cancel)))
                    .unwrap_or(Err(edith_sources::SourceError::Cancelled));
                lock(&thumbs).insert(id, r.map_or(Thumb::Failed, |b| Thumb::Ready(Arc::new(b))));
            }
        });
        if spawned.is_err() {
            lock(&thumbs).insert(id, Thumb::Failed);
        }
        None
    }
}

fn request_from(p: &Value, cmd: &str) -> Result<Request> {
    let text = str_p(p, "text")
        .map(str::trim)
        .filter(|t| !t.is_empty() && t.chars().count() <= 300)
        .ok_or_else(|| bad(cmd, "`text` is required: what to look for, up to 300 characters"))?
        .to_string();
    let mut r = Request { text, ..Request::default() };
    if let Some(v) = p.get("kinds").filter(|v| !v.is_null()) {
        let list = v.as_array().filter(|a| !a.is_empty() && a.len() <= 3).ok_or_else(|| bad(cmd, "`kinds` must list video, image and/or audio"))?;
        r.kinds = list
            .iter()
            .map(|k| k.as_str().and_then(Kind::parse).ok_or_else(|| bad(cmd, "`kinds` may only hold video, image and audio")))
            .collect::<Result<Vec<_>>>()?;
    }
    if let Some(v) = p.get("orientation").filter(|v| !v.is_null()) {
        r.orientation = Some(v.as_str().and_then(Orientation::parse).ok_or_else(|| bad(cmd, "`orientation` must be landscape, portrait or square"))?);
    }
    let whole = |key: &str, max: u64| -> Result<Option<u32>> {
        match p.get(key).filter(|v| !v.is_null()) {
            None => Ok(None),
            Some(v) => v.as_u64().filter(|n| *n <= max).map(|n| Some(n as u32)).ok_or_else(|| bad(cmd, format!("`{key}` must be a whole number up to {max}"))),
        }
    };
    r.min_width = whole("minWidth", 16_000)?;
    r.min_height = whole("minHeight", 16_000)?;
    if let Some(n) = whole("count", 40)? {
        r.count = n.max(1);
    }
    if let Some(n) = whole("page", 50)? {
        r.page = n.max(1);
    }
    let secs = |key: &str| -> Result<Option<f64>> {
        match p.get(key).filter(|v| !v.is_null()) {
            None => Ok(None),
            Some(v) => v
                .as_f64()
                .filter(|x| x.is_finite() && (0.0..=86_400.0).contains(x))
                .map(Some)
                .ok_or_else(|| bad(cmd, format!("`{key}` must be a number of seconds"))),
        }
    };
    r.min_seconds = secs("minSeconds")?;
    r.max_seconds = secs("maxSeconds")?;
    Ok(r.sanitized())
}

/// The sources a request should go to: those named (default all) that offer one of the kinds asked.
fn sources_for(p: &Value, req: &Request, cmd: &str) -> Result<Vec<String>> {
    let named: Vec<String> = match p.get("sources").filter(|v| !v.is_null()) {
        None => edith_sources::SOURCES.iter().map(|s| s.to_string()).collect(),
        Some(v) => v
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= 8)
            .ok_or_else(|| bad(cmd, "`sources` must list source ids (pexels, pixabay, freesound)"))?
            .iter()
            .map(|s| {
                s.as_str()
                    .filter(|s| edith_sources::SOURCES.contains(s))
                    .map(str::to_string)
                    .ok_or_else(|| bad(cmd, "unknown source in `sources`; use pexels, pixabay or freesound"))
            })
            .collect::<Result<_>>()?,
    };
    let out: Vec<String> = named.into_iter().filter(|id| edith_sources::info(id).is_some_and(|i| i.kinds.iter().any(|k| req.wants(*k)))).collect();
    if out.is_empty() {
        return Err(bad(cmd, "none of those sources offers the kinds asked for"));
    }
    Ok(out)
}

fn providers(_s: &mut Session, _p: &Value) -> Result<Value> {
    let list: Vec<Value> = edith_sources::SOURCES
        .iter()
        .filter_map(|id| edith_sources::info(id))
        .map(|i| {
            let service = edith_net::services::find(i.id);
            json!({
                "id": i.id, "label": i.label, "kinds": i.kinds.iter().map(|k| k.name()).collect::<Vec<_>>(),
                "connected": crate::net_commands::connected(i.id), "keyUrl": service.and_then(|s| s.key_url),
                "credit": i.credit, "creditUrl": i.credit_url,
            })
        })
        .collect();
    Ok(json!({"providers": list, "available": cfg!(feature = "net")}))
}

#[cfg(feature = "net")]
fn search(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "hub.search";
    s.pump_hub();
    let req = request_from(p, cmd)?;
    let ids = sources_for(p, &req, cmd)?;
    let wait = match p.get("wait") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(cmd, "`wait` must be true or false")),
    };
    if edith_net::shared().offline() {
        return Err(EngineError::Other("Offline mode is on; turn it off to search for media.".to_string()));
    }
    // a new search replaces the old one
    if let Some(old) = &s.hub.search {
        lock(old).cancel.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    s.hub.counter += 1;
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let state = Arc::new(Mutex::new(Search {
        id: s.hub.counter,
        request: req.clone(),
        sources: ids.iter().map(|id| SourceState { id: id.clone(), status: SourceStatus::Waiting, candidates: Vec::new(), total: 0 }).collect(),
        cancel: cancel.clone(),
    }));
    s.hub.search = Some(state.clone());
    let sources = s.hub.sources();
    for (n, id) in ids.iter().enumerate() {
        let (shared, sources, cache, req, id, cancel) = (state.clone(), sources.clone(), s.hub.cache.clone(), req.clone(), id.clone(), cancel.clone());
        let key = format!("{id}|{}", serde_json::to_string(&req).unwrap_or_default());
        let work = move || {
            let now = crate::autosave::unix_now().max(0) as u64;
            let cached = lock(&cache).get(&key).filter(|(at, _)| now.saturating_sub(*at) < CACHE_SECS).map(|(_, p)| p.clone());
            let result = match cached {
                Some(page) => Ok(page),
                None => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sources.search(&id, &req, &cancel)))
                    .unwrap_or_else(|_| Err(edith_sources::SourceError::Unavailable("the search failed internally".into()))),
            };
            if let Ok(page) = &result {
                let mut c = lock(&cache);
                if c.len() >= MAX_CACHE {
                    c.clear();
                }
                c.insert(key, (now, page.clone()));
            }
            let mut st = lock(&shared);
            if let Some(slot) = st.sources.get_mut(n) {
                match result {
                    Ok(page) => {
                        slot.status = SourceStatus::Done;
                        slot.total = page.total;
                        slot.candidates = page.candidates;
                    }
                    Err(e) => slot.status = SourceStatus::Failed(e.to_string()),
                }
            }
        };
        if let Err(e) = std::thread::Builder::new().name("hub-search".into()).spawn(work)
            && let Some(slot) = lock(&state).sources.get_mut(n)
        {
            slot.status = SourceStatus::Failed(format!("could not start the search: {e}"));
        }
    }
    if wait {
        let started = web_time::Instant::now();
        while lock(&state).sources.iter().any(|x| x.status == SourceStatus::Waiting) && started.elapsed() < std::time::Duration::from_secs(60) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    results(s, &json!({}))
}

#[cfg(not(feature = "net"))]
fn search(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(EngineError::Other("searching for media is not available in this build (built without the `net` feature)".to_string()))
}

fn candidate_json(h: &Hub, c: &Candidate) -> Value {
    let mut v = serde_json::to_value(c).unwrap_or(Value::Null);
    v["shortlisted"] = json!(h.is_shortlisted(&c.id));
    v["acquired"] = match h.acquired(&c.id) {
        None => Value::Null,
        Some(Acquired::Downloading) => json!({"state": "downloading"}),
        Some(Acquired::Done { item }) => json!({"state": "done", "item": item}),
        Some(Acquired::Failed(m)) => json!({"state": "failed", "message": m}),
    };
    v
}

fn results(s: &mut Session, _p: &Value) -> Result<Value> {
    s.pump_hub();
    let Some(search) = &s.hub.search else { return Ok(json!({"search": null, "running": false, "sources": [], "candidates": []})) };
    let st = lock(search);
    let sources: Vec<Value> = st
        .sources
        .iter()
        .map(|x| {
            let (state, message) = match &x.status {
                SourceStatus::Waiting => ("searching", Value::Null),
                SourceStatus::Done => ("done", Value::Null),
                SourceStatus::Failed(m) => ("failed", json!(m)),
            };
            json!({"id": x.id, "state": state, "message": message, "found": x.candidates.len(), "total": x.total})
        })
        .collect();
    // take one from each source in turn, so no site's results bury the others
    let mut candidates = Vec::new();
    let mut i = 0;
    loop {
        let mut any = false;
        for src in &st.sources {
            if let Some(c) = src.candidates.get(i) {
                candidates.push(candidate_json(&s.hub, c));
                any = true;
            }
        }
        if !any {
            break;
        }
        i += 1;
    }
    Ok(json!({
        "search": st.id, "request": st.request, "running": st.sources.iter().any(|x| x.status == SourceStatus::Waiting),
        "sources": sources, "candidates": candidates,
    }))
}

fn cancel_cmd(s: &mut Session, _p: &Value) -> Result<Value> {
    let running = s.hub.search.as_ref().is_some_and(|x| {
        let st = lock(x);
        st.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        st.sources.iter().any(|x| x.status == SourceStatus::Waiting)
    });
    Ok(json!({"stopping": running}))
}

fn shortlist(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "hub.shortlist";
    if let Some(v) = p.get("id").filter(|v| !v.is_null()) {
        let id = v.as_str().filter(|i| !i.is_empty() && i.len() <= 100).ok_or_else(|| bad(cmd, "`id` must be a result id such as pexels:123"))?;
        let c = s.hub.candidate(id).ok_or_else(|| bad(cmd, "no result with that id in this session"))?;
        let want = match p.get("on") {
            None | Some(Value::Null) => !s.hub.is_shortlisted(id),
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(bad(cmd, "`on` must be true or false")),
        };
        s.hub.shortlist.retain(|x| x.id != id);
        if want {
            if s.hub.shortlist.len() >= MAX_SHORTLIST {
                return Err(bad(cmd, format!("the shortlist holds at most {MAX_SHORTLIST}")));
            }
            s.hub.shortlist.push(c);
        }
    }
    let list: Vec<Value> = s.hub.shortlist.iter().map(|c| candidate_json(&s.hub, c)).collect();
    Ok(json!({"shortlist": list}))
}

/// A bin path (`Sources` then the site), created if missing.
fn ensure_bin(s: &mut Session, names: &[&str]) -> Result<filmcraft_project::BinId> {
    use filmcraft_project::BinEntry;
    let mut at = s.project.root.id;
    for name in names {
        let found = s.project.root.find_bin(at).and_then(|b| {
            b.children.iter().find_map(|c| match c {
                BinEntry::Bin(x) if x.name == *name => Some(x.id),
                _ => None,
            })
        });
        at = match found {
            Some(id) => id,
            None => s.edit("New Bin", |pr, _| Ok(pr.add_bin(name, Some(at))))?,
        };
    }
    Ok(at)
}

fn slug(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let t = t.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    t.chars().take(40).collect::<String>().trim_matches('-').to_string()
}

/// The credit line to record for a candidate whose licence asks for one.
pub(crate) fn credit_text(c: &Candidate) -> String {
    if c.rights.attribution_required != Some(true) {
        return String::new();
    }
    let mut parts = vec![if c.author.is_empty() { c.title.clone() } else { format!("{} by {}", c.title, c.author) }];
    if !c.rights.name.is_empty() {
        parts.push(c.rights.name.clone());
    }
    if !c.source_url.is_empty() {
        parts.push(c.source_url.clone());
    }
    parts.retain(|p| !p.trim().is_empty());
    parts.join(" · ")
}

#[cfg(feature = "net")]
fn acquire(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "hub.acquire";
    s.pump_hub();
    let id = str_p(p, "id").filter(|i| !i.is_empty() && i.len() <= 100).ok_or_else(|| bad(cmd, "`id` must be a result id such as pexels:123"))?;
    let wait = match p.get("wait") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(cmd, "`wait` must be true or false")),
    };
    let bin = match p.get("bin").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => Some(
            v.as_u64()
                .filter(|b| s.project.root.find_bin(filmcraft_project::BinId(*b)).is_some())
                .ok_or_else(|| bad(cmd, "`bin` must be a bin of this project"))?,
        ),
    };
    let c = s.hub.candidate(id).ok_or_else(|| bad(cmd, "no result with that id in this session; search first"))?;
    let download = c.download.clone().ok_or_else(|| bad(cmd, "this result has no file to download"))?;
    if matches!(s.hub.acquired.get(id), Some(Acquired::Downloading)) {
        return Err(bad(cmd, "this is already downloading"));
    }
    if edith_net::shared().offline() {
        return Err(EngineError::Other("Offline mode is on; turn it off to download media.".to_string()));
    }
    let label = edith_sources::info(&c.source).map_or(c.source.clone(), |i| i.label.to_string());
    let native = c.id.split_once(':').map_or(c.id.as_str(), |x| x.1);
    let dir = std::path::Path::new(&crate::voiceover::media_dir(s, &json!({}), "Sources")).join("Sources").join(&label);
    let name = format!("{}-{}-{}.{}", c.source, slug(native), slug(&c.title), download.ext);
    let path = dir.join(name).to_string_lossy().into_owned();
    let sources = s.hub.sources();
    let finished: Finished = Arc::new(Mutex::new(None));
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let job = Job {
        id: s.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1,
        label: format!("Download {} from {label}", c.title),
        progress: Default::default(),
        result: Default::default(),
    };
    let progress = job.progress.clone();
    let (cand, fin, can, prog, path2) = (c.clone(), finished.clone(), cancel.clone(), progress.clone(), path.clone());
    let work = crate::export_tools::guard_job(job.progress.clone(), job.result.clone(), move || {
        let outcome = sources
            .download(&cand, std::path::Path::new(&path2), &can, |done, total| {
                prog.done.store(done, std::sync::atomic::Ordering::Relaxed);
                prog.total.store(total.unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
            })
            .map(|d| d.sha256)
            .map_err(|e| e.to_string());
        *lock(&fin) = Some(outcome);
    });
    let job_id = job.id;
    s.jobs.push(job);
    s.hub.acquired.insert(c.id.clone(), Acquired::Downloading);
    s.hub.pending.push(Pending { candidate: c.clone(), path: path.clone(), bin, finished, job: job_id, cancel, progress });
    if let Err(e) = std::thread::Builder::new().name("hub-download".into()).spawn(work) {
        s.hub.pending.retain(|x| x.job != job_id);
        s.hub.acquired.insert(c.id.clone(), Acquired::Failed("could not start the download".into()));
        return Err(EngineError::Other(format!("could not start the download: {e}")));
    }
    if wait {
        while s.hub.pending.iter().any(|x| x.job == job_id) {
            s.pump_hub();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    Ok(json!({"id": c.id, "job": job_id, "path": path, "acquired": s.hub.acquired.get(&c.id).map(|a| match a {
        Acquired::Downloading => json!({"state": "downloading"}),
        Acquired::Done { item } => json!({"state": "done", "item": item}),
        Acquired::Failed(m) => json!({"state": "failed", "message": m}),
    })}))
}

#[cfg(not(feature = "net"))]
fn acquire(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(EngineError::Other("downloading media is not available in this build (built without the `net` feature)".to_string()))
}

impl Session {
    /// Finishes downloads that have ended: imports the file into its bin and records where it came
    /// from, on this (the session) thread; passes a job's Cancel on to its download. Cheap when
    /// nothing is pending. Commands and the UI frame call it.
    pub fn pump_hub(&mut self) {
        if self.hub.pending.is_empty() {
            return;
        }
        for p in &self.hub.pending {
            if p.progress.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                p.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let done: Vec<usize> = self.hub.pending.iter().enumerate().filter(|(_, p)| lock(&p.finished).is_some()).map(|(i, _)| i).collect();
        for i in done.into_iter().rev() {
            let p = self.hub.pending.remove(i);
            let outcome = lock(&p.finished).take().unwrap_or_else(|| Err("the download ended without an answer".to_string()));
            let id = p.candidate.id.clone();
            let label = edith_sources::info(&p.candidate.source).map_or(p.candidate.source.clone(), |x| x.label.to_string());
            let result = outcome.and_then(|sha| self.bring_in(&p, &label, &sha).map_err(|e| e.to_string()));
            match result {
                Ok(item) => {
                    self.log.push(Level::Info, "sources", format!("Added \"{}\" from {label} to the project.", p.candidate.title));
                    p.progress.finished.store(true, std::sync::atomic::Ordering::Relaxed);
                    *lock(&p.progress.status) = "Added to the project".to_string();
                    *lock(&self.jobs.iter().find(|j| j.id == p.job).map(|j| j.result.clone()).unwrap_or_default()) = Some(Ok(filmcraft_export::Report {
                        path: p.path.clone(),
                        frames: 0,
                        seconds: 0.0,
                        bytes: 0,
                        render_fps: 0.0,
                        extra_files: Vec::new(),
                    }));
                    self.hub.acquired.insert(id, Acquired::Done { item });
                }
                Err(why) => {
                    let msg = if p.cancel.load(std::sync::atomic::Ordering::SeqCst) { "Stopped.".to_string() } else { why };
                    self.log.push(Level::Warning, "sources", format!("Could not get \"{}\" from {label}: {msg}", p.candidate.title));
                    *lock(&p.progress.error) = Some(msg.clone());
                    p.progress.finished.store(true, std::sync::atomic::Ordering::Relaxed);
                    self.hub.acquired.insert(id, Acquired::Failed(msg));
                }
            }
        }
    }

    /// Imports the downloaded file into `Sources/<site>` and records its provenance.
    fn bring_in(&mut self, p: &Pending, label: &str, sha256: &str) -> Result<u64> {
        let bin = match p.bin {
            Some(b) => filmcraft_project::BinId(b),
            None => ensure_bin(self, &["Sources", label])?,
        };
        let r = self.execute("file.import", json!({"paths": [p.path], "bin": bin.0}))?;
        let item = r["items"][0].as_u64().ok_or_else(|| {
            EngineError::Other(format!("the downloaded file could not be imported{}", r["errors"][0].as_str().map(|e| format!(": {e}")).unwrap_or_default()))
        })?;
        let c = &p.candidate;
        let mut note = c.download.as_ref().map(|d| d.note.clone()).unwrap_or_default();
        if !note.is_empty() {
            note.push(' ');
        }
        note.push_str("Found with the Source Hub.");
        self.execute(
            "provenance.set",
            json!({
                "item": item, "source": c.source, "author": c.author, "sourceUrl": c.source_url, "hash": sha256, "note": note,
                "license": {"name": c.rights.name, "url": c.rights.url, "commercial": c.rights.commercial, "attributionRequired": c.rights.attribution_required, "creditText": credit_text(c)},
            }),
        )?;
        Ok(item)
    }
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
        spec("hub.providers", "Media Sources", "{}", providers),
        spec(
            "hub.search",
            "Search for Media",
            r#"{"text":str,"kinds":["video|image|audio"]?,"sources":["pexels|pixabay|freesound"]?,"orientation":"landscape|portrait|square"?,"minWidth":u32?,"minHeight":u32?,"minSeconds":f64?,"maxSeconds":f64?,"count":1..40?,"page":1..50?,"wait":bool?}"#,
            search,
        ),
        spec("hub.results", "Media Search Results", "{}", results),
        spec("hub.shortlist", "Media Shortlist", r#"{"id":str?,"on":bool?}"#, shortlist),
        spec("hub.acquire", "Get Media", r#"{"id":str,"bin":binId?,"wait":bool?}"#, acquire),
        spec("hub.cancel", "Stop the Media Search", "{}", cancel_cmd),
    ]
}
