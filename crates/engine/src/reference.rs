//! References (fork roadmap F12): films you admire, measured into style fingerprints.
//!
//! | command | does |
//! |---|---|
//! | `reference.measure {path, name?, sampleFps?, pictures?, sensitivity?, wait?}` | measure a film file into a fingerprint (`edith-analysis`), as a background job you can watch and stop; `wait: true` returns the fingerprint |
//! | `reference.result {job}` | where a measuring job is: running, done (with the fingerprint) or failed |
//! | `reference.add {path, rights, name?, …}` | measure a film file (as `reference.measure`) and keep its fingerprint in your library; `rights` is a short note of why you may use the file (public domain, your own export, a licence) and is required |
//! | `reference.list`, `reference.get {id}` | the library, and one fingerprint in full |
//! | `reference.rename {id, name}`, `reference.remove {id}` | tidy the library |
//! | `reference.export {id, path}`, `reference.import {path}` | a fingerprint as a small JSON file, to share without sharing any footage, and back |
//! | `reference.compare {id?, sequence?}` | your edit against a reference, row by row in plain words (the adopted reference when `id` is left out) |
//! | `reference.adopt {id}` / `reference.adopt {clear: true}`, `reference.adopted` | make a reference the coach's target for this project (its average shot length and loudness), kept in the project as numbers and undoable; read what is adopted |
//!
//! The library is global: `edith-references.json` beside the preferences, for every project (in memory
//! only where there is no data folder, as in tests). Everything read from it, or imported, is
//! untrusted: a fingerprint must pass `Fingerprint::check`, names and notes are cut to their limits, and
//! at most [`MAX_REFERENCES`] are kept.
//!
//! The file is read as a stream (a feature-length film is not loaded whole). The job finds the hard
//! cuts by looking at `sampleFps` frames a second (default 8: a cut is placed to within an eighth of a
//! second, and a feature is practical), samples `pictures` pictures spread over the film with the
//! camera motion around each, and measures the sound in 30 second pieces, with the whole programme's
//! loudness taken by streaming. Only the numbers are kept: nothing of the film is stored.
//!
//! A file you point at is untrusted media: a frame that cannot be decoded is skipped (too many and the
//! job stops saying so), a file with neither picture nor sound is refused, and sizes are bounded.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use edith_analysis as an;
use filmcraft_media::{FrameRequest, SharedSource};
use filmcraft_render::scene::{SceneDetector, SceneOptions};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::analysis::{frame_at, lock, motion_between, secs, to_value};
use crate::commands::{CommandSpec, always, bad, bool_p, f64_p, str_p, u64_p};
use crate::{EngineError, Result, Session};
use filmcraft_project::{ItemId, ItemKind, Project};

/// Frames the cut finder looks at, at most (a day of film at 8 a second).
const MAX_CUT_FRAMES: usize = 700_000;
/// The cut finder works on small pictures.
const CUT_WIDTH: f64 = 256.0;
const PICTURE_WIDTH: f64 = 640.0;
/// A fraction of the cut-finding frames that may fail to decode before the job gives up.
const MAX_FAILED: f64 = 0.10;
const SOUND_PIECE_SECONDS: f64 = 30.0;
/// Longest path accepted.
const MAX_PATH: usize = 4096;

pub(crate) const FILE: &str = "edith-references.json";
const MAX_FILE: u64 = 8 * 1024 * 1024;
const MAX_IMPORT: u64 = 2 * 1024 * 1024;
/// Most references kept.
pub const MAX_REFERENCES: usize = 50;
const MAX_NAME: usize = 120;
const MAX_RIGHTS: usize = 300;

/// One film in the library: its name, the note of why it may be used, and the numbers. Nothing of the
/// film itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reference {
    pub id: String,
    pub name: String,
    pub rights: String,
    pub added_unix: i64,
    pub fingerprint: an::Fingerprint,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Library {
    pub version: u32,
    pub next_id: u64,
    pub references: Vec<Reference>,
}

type SharedLibrary = Arc<Mutex<Option<Library>>>;

/// What the session keeps: where finished and failed measuring jobs left their answers, and the library
/// (read from its file the first time it is needed).
#[derive(Clone, Default)]
pub struct Hub {
    done: Arc<Mutex<BTreeMap<u64, std::result::Result<Value, String>>>>,
    library: SharedLibrary,
}

fn clean_text(t: &str, max: usize) -> String {
    t.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}

fn file_path(s: &Session) -> Option<PathBuf> {
    s.prefs_path.as_ref().and_then(|p| p.parent()).map(|d| d.join(FILE))
}

/// The library as the file holds it: a damaged file is an empty library, a damaged entry is left out.
fn read_library(file: Option<&Path>) -> Library {
    let Some(p) = file else { return Library { version: 1, ..Default::default() } };
    let empty = Library { version: 1, ..Default::default() };
    let Ok(meta) = std::fs::metadata(p) else { return empty };
    if meta.len() > MAX_FILE {
        return empty;
    }
    let Some(mut lib) = std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<Library>(&t).ok()) else { return empty };
    let mut seen = Vec::new();
    lib.references.retain(|r| {
        let ok = r.id.len() <= 20 && r.id.chars().all(|c| c.is_ascii_alphanumeric()) && !seen.contains(&r.id) && r.fingerprint.check().is_ok();
        if ok {
            seen.push(r.id.clone());
        }
        ok
    });
    lib.references.truncate(MAX_REFERENCES);
    for r in &mut lib.references {
        r.name = clean_text(&r.name, MAX_NAME);
        r.rights = clean_text(&r.rights, MAX_RIGHTS);
    }
    lib.version = 1;
    lib
}

fn save_library(file: Option<&Path>, lib: &Library) {
    let Some(p) = file else { return };
    if let Ok(json) = serde_json::to_string_pretty(lib) {
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, p).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// Runs `f` on the library, loading it first if need be, and keeps what `f` did (saved to the file).
fn with_library<T>(shared: &SharedLibrary, file: Option<&Path>, f: impl FnOnce(&mut Library) -> T) -> T {
    let mut g = lock(shared);
    let lib = g.get_or_insert_with(|| read_library(file));
    let out = f(lib);
    save_library(file, lib);
    out
}

/// A reference added to the library: the next id, the note and the numbers. `Err` when the library is full.
fn store(lib: &mut Library, name: &str, rights: &str, fp: an::Fingerprint) -> std::result::Result<String, String> {
    if lib.references.len() >= MAX_REFERENCES {
        return Err(format!("the library already holds {MAX_REFERENCES} references; remove one first"));
    }
    lib.next_id = lib.next_id.max(lib.references.len() as u64) + 1;
    let id = format!("r{}", lib.next_id);
    lib.references.push(Reference {
        id: id.clone(),
        name: clean_text(name, MAX_NAME),
        rights: clean_text(rights, MAX_RIGHTS),
        added_unix: crate::autosave::unix_now(),
        fingerprint: fp,
    });
    Ok(id)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Options {
    pub sample_fps: f64,
    pub pictures: usize,
    pub sensitivity: f32,
}

impl Options {
    pub(crate) fn from_params(p: &Value) -> Options {
        Options {
            sample_fps: f64_p(p, "sampleFps").filter(|v| v.is_finite()).unwrap_or(8.0).clamp(1.0, 30.0),
            pictures: u64_p(p, "pictures").map_or(120, |n| usize::try_from(n).unwrap_or(600)).clamp(8, 600),
            sensitivity: f64_p(p, "sensitivity").filter(|v| v.is_finite()).unwrap_or(50.0).clamp(0.0, 100.0) as f32,
        }
    }
}

fn cancelled(prog: &filmcraft_export::Progress) -> bool {
    prog.cancel.load(Ordering::Relaxed)
}

/// The hard cuts of a film, in seconds, looking at `fps` frames a second.
fn find_cuts(src: &SharedSource, duration: f64, width: u32, o: &Options, prog: &filmcraft_export::Progress) -> std::result::Result<Vec<f64>, String> {
    let frames = ((duration * o.sample_fps).floor() as usize).clamp(1, MAX_CUT_FRAMES);
    let step = TICKS_PER_SECOND as f64 / o.sample_fps;
    let scale = (CUT_WIDTH / f64::from(width.max(1))).min(1.0) as f32;
    let mut detector = SceneDetector::new();
    let mut times: Vec<f64> = Vec::with_capacity(frames);
    let mut failed = 0usize;
    for i in 0..frames {
        if cancelled(prog) {
            return Err("stopped".into());
        }
        let at = Tick((i as f64 * step) as i64);
        match src.video_frame(FrameRequest { time: at, scale }) {
            Ok(f) => {
                detector.push_rgba(&f.to_rgba8(), f.width as usize, f.height as usize);
                times.push(i as f64 / o.sample_fps);
            }
            Err(_) => {
                failed += 1;
                if failed as f64 > MAX_FAILED * frames as f64 + 5.0 {
                    return Err(format!("{failed} of the first {} frames could not be read: the file may be damaged", i + 1));
                }
            }
        }
        prog.done.fetch_add(1, Ordering::Relaxed);
        if i % 200 == 0 {
            *lock(&prog.status) = format!("Looking for cuts: {} of {}", clock(i as f64 / o.sample_fps), clock(duration));
        }
    }
    // a shot is at least half a second
    let min_shot = ((0.5 * o.sample_fps).round() as usize).max(2);
    let cuts = detector.cuts(&SceneOptions { sensitivity: o.sensitivity, min_shot_frames: min_shot });
    Ok(cuts.into_iter().filter_map(|i| times.get(i).copied()).collect())
}

fn clock(seconds: f64) -> String {
    let n = seconds.max(0.0) as u64;
    if n >= 3600 { format!("{}:{:02}:{:02}", n / 3600, (n / 60) % 60, n % 60) } else { format!("{}:{:02}", n / 60, n % 60) }
}

/// Pictures spread over the film, each with the camera motion across a quarter second around it.
fn sample_pictures(
    src: &SharedSource,
    duration: f64,
    width: u32,
    o: &Options,
    prog: &filmcraft_export::Progress,
) -> std::result::Result<Vec<an::fingerprint::PictureSample>, String> {
    let scale = (PICTURE_WIDTH / f64::from(width.max(1))).min(1.0) as f32;
    let total = (duration * TICKS_PER_SECOND as f64) as i64;
    let gap = TICKS_PER_SECOND / 8;
    let mut out = Vec::new();
    for i in 0..o.pictures {
        if cancelled(prog) {
            return Err("stopped".into());
        }
        let t = Tick((i128::from(total) * (2 * i as i128 + 1) / (2 * o.pictures as i128)) as i64);
        *lock(&prog.status) = format!("Picture {} of {}", i + 1, o.pictures);
        if let Ok(stats) = frame_at(src, t, scale).and_then(|(w, h, rgba)| an::analyze_rgba(w, h, &rgba).map_err(|e| e.to_string())) {
            let (ta, tb) = (Tick((t.0 - gap / 2).max(0)), Tick((t.0 + gap / 2).min(total)));
            let motion = if tb.0 - ta.0 >= TICKS_PER_SECOND / 25 { motion_between(src, ta, tb, secs(tb - ta), scale) } else { None };
            out.push(an::fingerprint::PictureSample {
                time_seconds: secs(t),
                stats,
                camera: motion.map(|m| an::summarize(&[m]).camera),
                motion_energy: motion.map(|m| m.energy),
            });
        }
        prog.done.fetch_add(1, Ordering::Relaxed);
    }
    Ok(out)
}

/// The sound of the whole film: the programme's loudness by streaming, and each 30 s piece's kind and
/// loudness.
fn measure_sound(src: &SharedSource, duration: f64, prog: &filmcraft_export::Progress) -> std::result::Result<Option<an::fingerprint::SoundInput>, String> {
    let Some(a) = src.info().audio().cloned() else { return Ok(None) };
    let sr = a.sample_rate;
    if !(an::audio::MIN_SAMPLE_RATE..=an::audio::MAX_SAMPLE_RATE).contains(&sr) {
        return Ok(None);
    }
    // the programme is measured as the file is: one channel as mono, two or more as the first two
    let programme_channels = (a.channels as usize).clamp(1, 2);
    let mut meter = an::MixMeter::new(sr, programme_channels as u16).map_err(|e| e.to_string())?;
    let piece = (SOUND_PIECE_SECONDS * f64::from(sr)) as usize;
    let want = (duration * f64::from(sr)) as usize;
    let mut segments = Vec::new();
    let mut at = 0usize;
    while at < want {
        if cancelled(prog) {
            return Err("stopped".into());
        }
        *lock(&prog.status) = format!("Sound: {} of {}", clock(at as f64 / f64::from(sr)), clock(duration));
        let take = piece.min(want - at);
        let buf = match src.audio(at as i64, take, sr) {
            Ok(b) => b,
            Err(e) if at == 0 => return Err(format!("the sound could not be read: {e}")),
            Err(_) => break,
        };
        let frames = buf.frames();
        if frames == 0 {
            break;
        }
        let channels = buf.channel_count().clamp(1, 2);
        let mut pcm: Vec<f32> = Vec::with_capacity(frames * channels);
        for i in 0..frames {
            for c in 0..channels {
                pcm.push(buf.channels.get(c).and_then(|ch| ch.get(i)).copied().unwrap_or(0.0));
            }
        }
        if channels == programme_channels {
            meter.push(&pcm);
        } else if programme_channels == 2 {
            meter.push(&pcm.iter().flat_map(|v| [*v, *v]).collect::<Vec<f32>>());
        } else {
            // the file said mono and the decoder gave more: the first channel is the programme
            meter.push(&pcm.chunks_exact(channels).map(|f| f.first().copied().unwrap_or(0.0)).collect::<Vec<f32>>());
        }
        if let Ok(stats) = an::analyze_audio(sr, channels as u16, &pcm) {
            segments.push(an::fingerprint::SoundSegment {
                start_seconds: at as f64 / f64::from(sr),
                duration_seconds: frames as f64 / f64::from(sr),
                kind: stats.kind,
                loudness_lufs: stats.integrated_lufs,
            });
        }
        prog.done.fetch_add(1, Ordering::Relaxed);
        at += frames;
        if frames < take {
            break;
        }
    }
    Ok(Some(an::fingerprint::SoundInput { mix: meter.finish(), segments }))
}

/// Measures a film into a fingerprint. Runs on a worker thread; `stopped` is the error of a cancelled run.
pub(crate) fn measure(src: &SharedSource, name: &str, o: &Options, prog: &filmcraft_export::Progress) -> std::result::Result<an::Fingerprint, String> {
    let info = src.info().clone();
    let duration = secs(info.duration);
    if !(duration.is_finite() && duration > 0.0) {
        return Err("the file has no length to measure".into());
    }
    let video = info.video.clone();
    if video.is_none() && info.audio().is_none() {
        return Err("the file has neither picture nor sound to measure".into());
    }
    let units = video.as_ref().map_or(0, |_| ((duration * o.sample_fps).floor() as usize).clamp(1, MAX_CUT_FRAMES) + o.pictures)
        + if info.audio().is_some() { (duration / SOUND_PIECE_SECONDS).ceil() as usize } else { 0 };
    prog.total.store(units.max(1) as u64, Ordering::Relaxed);
    let (width, height) = video.as_ref().map_or((0, 0), |v| (v.width, v.height));
    let (cuts, pictures) = match &video {
        Some(v) => (find_cuts(src, duration, v.width, o, prog)?, sample_pictures(src, duration, v.width, o, prog)?),
        None => (Vec::new(), Vec::new()),
    };
    let sound = measure_sound(src, duration, prog)?;
    an::fingerprint(&an::ReferenceInput { name: name.to_string(), duration_seconds: duration, width, height, cuts, pictures, sound }).map_err(|e| e.to_string())
}

fn path_param<'a>(p: &'a Value, cmd: &str) -> Result<&'a str> {
    let path = str_p(p, "path").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad(cmd, "`path` is required: the film file to measure"))?;
    if path.len() > MAX_PATH || path.contains('\0') {
        return Err(bad(cmd, "`path` is not a usable file path"));
    }
    Ok(path)
}

fn name_of(p: &Value, path: &str) -> String {
    let given =
        str_p(p, "name").map(|n| n.chars().filter(|c| !c.is_control()).take(120).collect::<String>()).map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    given.unwrap_or_else(|| std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().chars().take(120).collect()).unwrap_or_default())
}

/// What to keep when a measurement finishes in `reference.add`: the note, and where the library is.
struct Keep {
    rights: String,
    file: Option<PathBuf>,
    library: SharedLibrary,
}

/// Starts a measuring job; with `keep`, the fingerprint is stored in the library when it is done.
fn start(s: &mut Session, p: &Value, cmd: &str, keep: Option<String>) -> Result<Value> {
    let path = path_param(p, cmd)?.to_string();
    let name = name_of(p, &path);
    let options = Options::from_params(p);
    let id = s.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
    let label = if keep.is_some() { format!("Add reference {name}") } else { format!("Measure reference {name}") };
    let job = crate::Job { id, label, progress: Default::default(), result: Default::default() };
    let (prog, res) = (job.progress.clone(), job.result.clone());
    let (media, services) = (s.media.clone(), s.services.clone());
    let done = s.reference.done.clone();
    let keep = keep.map(|rights| Keep { rights, file: file_path(s), library: s.reference.library.clone() });
    let run = move || {
        let t0 = web_time::Instant::now();
        *lock(&prog.status) = "Opening the file".into();
        let outcome = media
            .open_file(&path, &*services)
            .map_err(|e| format!("could not open {}: {e}", Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
            .and_then(|src| measure(&src, &name, &options, &prog))
            .and_then(|fp| match &keep {
                Some(k) => with_library(&k.library, k.file.as_deref(), |lib| store(lib, &name, &k.rights, fp.clone())).map(|rid| (fp, Some(rid))),
                None => Ok((fp, None)),
            });
        let seconds = t0.elapsed().as_secs_f64();
        let r = match outcome {
            Ok((fp, reference)) => {
                lock(&done).insert(id, Ok(json!({"fingerprint": to_value(&fp), "reference": reference})));
                *lock(&prog.status) = format!("Measured in {seconds:.0}s");
                Ok(filmcraft_export::Report {
                    path: String::new(),
                    frames: prog.done.load(Ordering::Relaxed),
                    seconds,
                    bytes: 0,
                    render_fps: 0.0,
                    extra_files: Vec::new(),
                })
            }
            Err(e) => {
                *lock(&prog.status) = if e == "stopped" { "Stopped: nothing was kept".into() } else { e.clone() };
                lock(&done).insert(id, Err(e.clone()));
                Err(e)
            }
        };
        *lock(&res) = Some(r);
        prog.finished.store(true, Ordering::Relaxed);
    };
    let run = crate::export_tools::guard_job(job.progress.clone(), job.result.clone(), run);
    s.jobs.push(job);
    let mut out = json!({"job": id});
    if bool_p(p, "wait").unwrap_or(false) || cfg!(target_arch = "wasm32") {
        run();
        match lock(&s.reference.done).get(&id).cloned() {
            Some(Ok(done)) => {
                out["fingerprint"] = done["fingerprint"].clone();
                out["reference"] = done["reference"].clone();
            }
            Some(Err(e)) => return Err(EngineError::Other(e)),
            None => return Err(EngineError::Other("the measurement ended without an answer".into())),
        }
    } else {
        std::thread::Builder::new().name("edith-reference".into()).spawn(run).map_err(|e| EngineError::Other(e.to_string()))?;
    }
    Ok(out)
}

fn measure_command(s: &mut Session, p: &Value) -> Result<Value> {
    start(s, p, "reference.measure", None)
}

fn add_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "reference.add";
    let rights = str_p(p, "rights")
        .map(|r| clean_text(r, MAX_RIGHTS + 1))
        .filter(|r| !r.is_empty())
        .ok_or_else(|| bad(cmd, "`rights` is required: a short note of why you may use this film (public domain, your own export, a licence)"))?;
    if rights.chars().count() > MAX_RIGHTS {
        return Err(bad(cmd, format!("`rights` is at most {MAX_RIGHTS} characters")));
    }
    if with_library(&s.reference.library, file_path(s).as_deref(), |l| l.references.len()) >= MAX_REFERENCES {
        return Err(bad(cmd, format!("the library already holds {MAX_REFERENCES} references; remove one first")));
    }
    start(s, p, cmd, Some(rights))
}

fn result_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "reference.result";
    let id = u64_p(p, "job").ok_or_else(|| bad(cmd, "`job` is required: the number `reference.measure` returned"))?;
    if !s.jobs.iter().any(|j| j.id == id) && !lock(&s.reference.done).contains_key(&id) {
        return Err(bad(cmd, format!("there is no job {id}")));
    }
    Ok(match lock(&s.reference.done).get(&id) {
        Some(Ok(done)) => json!({"state": "done", "fingerprint": done["fingerprint"], "reference": done["reference"]}),
        Some(Err(e)) => json!({"state": "failed", "error": e}),
        None => json!({"state": "running"}),
    })
}

// ---------------------------------------------------------------------------- the library

fn summary(r: &Reference) -> Value {
    let f = &r.fingerprint;
    json!({
        "id": r.id, "name": r.name, "rights": r.rights, "addedUnix": r.added_unix,
        "durationSeconds": f.duration_seconds,
        "shots": {"count": f.shots.count, "meanSeconds": f.shots.mean_seconds, "medianSeconds": f.shots.median_seconds, "cutsPerMinute": f.shots.cuts_per_minute},
        "arc": f.rhythm.arc,
        "hasPicture": f.picture.is_some(),
        "integratedLufs": f.sound.as_ref().and_then(|s| s.integrated_lufs),
    })
}

fn all(s: &Session) -> Vec<Reference> {
    with_library(&s.reference.library, file_path(s).as_deref(), |l| l.references.clone())
}

pub(crate) fn find(s: &Session, id: &str) -> Option<Reference> {
    all(s).into_iter().find(|r| r.id == id)
}

fn id_param<'a>(p: &'a Value, cmd: &str) -> Result<&'a str> {
    str_p(p, "id").filter(|i| !i.is_empty() && i.len() <= 20).ok_or_else(|| bad(cmd, "`id` is required: a reference's id from `reference.list`"))
}

fn list_command(s: &mut Session, _p: &Value) -> Result<Value> {
    let refs = all(s);
    Ok(
        json!({"references": refs.iter().map(summary).collect::<Vec<_>>(), "folder": file_path(s).map(|p| p.to_string_lossy().into_owned()), "max": MAX_REFERENCES}),
    )
}

fn get_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "reference.get";
    let id = id_param(p, cmd)?;
    let r = find(s, id).ok_or_else(|| bad(cmd, format!("there is no reference {id:?}; `reference.list` shows what there is")))?;
    Ok(json!({"id": r.id, "name": r.name, "rights": r.rights, "addedUnix": r.added_unix, "fingerprint": to_value(&r.fingerprint)}))
}

fn rename_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "reference.rename";
    let id = id_param(p, cmd)?.to_string();
    let name = str_p(p, "name").map(|n| clean_text(n, MAX_NAME + 1)).filter(|n| !n.is_empty() && n.chars().count() <= MAX_NAME);
    let name = name.ok_or_else(|| bad(cmd, format!("`name` is required, at most {MAX_NAME} characters")))?;
    let found = with_library(&s.reference.library, file_path(s).as_deref(), |l| {
        l.references.iter_mut().find(|r| r.id == id).map(|r| {
            r.name = name.clone();
            r.fingerprint.name = name.clone();
        })
    });
    found.map(|()| json!({"id": id, "name": name})).ok_or_else(|| bad(cmd, format!("there is no reference {id:?}")))
}

fn remove_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "reference.remove";
    let id = id_param(p, cmd)?.to_string();
    let removed = with_library(&s.reference.library, file_path(s).as_deref(), |l| {
        let before = l.references.len();
        l.references.retain(|r| r.id != id);
        before != l.references.len()
    });
    if removed { Ok(json!({"removed": id})) } else { Err(bad(cmd, format!("there is no reference {id:?}"))) }
}

const EXPORT_FORMAT: &str = "edith-fingerprint";

fn export_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "reference.export";
    let id = id_param(p, cmd)?;
    let r = find(s, id).ok_or_else(|| bad(cmd, format!("there is no reference {id:?}")))?;
    let path = path_param(p, cmd)?;
    let body = json!({"format": EXPORT_FORMAT, "version": 1, "name": r.name, "rights": r.rights, "fingerprint": to_value(&r.fingerprint)});
    let text = serde_json::to_string_pretty(&body).map_err(|e| EngineError::Other(e.to_string()))?;
    s.services.write_file(path, text.as_bytes()).map_err(|e| EngineError::Other(format!("could not write the file: {e}")))?;
    Ok(json!({"path": path, "bytes": text.len()}))
}

fn import_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "reference.import";
    let path = path_param(p, cmd)?;
    if s.services.file_size(path).map_err(|e| bad(cmd, format!("could not read the file: {e}")))? > MAX_IMPORT {
        return Err(bad(cmd, "that file is too big to be a fingerprint (the limit is 2 MB)"));
    }
    let bytes = s.services.read_file(path).map_err(|e| bad(cmd, format!("could not read the file: {e}")))?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|_| bad(cmd, "that file is not JSON"))?;
    if v["format"] != EXPORT_FORMAT {
        return Err(bad(cmd, "that is not a fingerprint file (it was not made by `reference.export`)"));
    }
    let fp: an::Fingerprint = serde_json::from_value(v["fingerprint"].clone())
        .map_err(|e| bad(cmd, format!("the fingerprint in that file is damaged: {}", e.to_string().chars().take(100).collect::<String>())))?;
    fp.check().map_err(|e| bad(cmd, e))?;
    let name = clean_text(v["name"].as_str().filter(|n| !n.trim().is_empty()).unwrap_or(&fp.name), MAX_NAME);
    let rights = v["rights"]
        .as_str()
        .map(|r| clean_text(r, MAX_RIGHTS))
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| "Imported fingerprint; no note of rights came with it".to_string());
    let id = with_library(&s.reference.library, file_path(s).as_deref(), |lib| store(lib, &name, &rights, fp)).map_err(|e| bad(cmd, e))?;
    Ok(json!({"id": id, "name": name}))
}

// ---------------------------------------------------------------------------- compare and adopt

/// The key in `Project::ext` that holds the adopted reference.
pub(crate) const ADOPTED_KEY: &str = "edith.reference";

/// The reference this project aims at: a snapshot of its numbers, so the project is complete without the
/// library.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Adopted {
    pub id: String,
    pub name: String,
    pub rights: String,
    pub fingerprint: an::Fingerprint,
}

/// The project's adopted reference, if it has a usable one (a damaged value counts as none).
pub(crate) fn adopted(p: &Project) -> Option<Adopted> {
    let a: Adopted = serde_json::from_value(p.ext.get(ADOPTED_KEY)?.clone()).ok()?;
    a.fingerprint.check().ok()?;
    Some(Adopted { name: clean_text(&a.name, MAX_NAME), rights: clean_text(&a.rights, MAX_RIGHTS), ..a })
}

fn sequence_of(s: &Session, p: &Value, cmd: &str) -> Result<ItemId> {
    match p.get("sequence").filter(|v| !v.is_null() && **v != json!(true)) {
        Some(v) => v.as_u64().map(ItemId).ok_or_else(|| bad(cmd, "`sequence` must be a sequence's item id")),
        None => s.state.active_sequence.ok_or(EngineError::NoSequence),
    }
}

/// A fingerprint of the sequence, made the way a film's is: its cuts, and its pictures and mix when they
/// have been measured (`analysis_run`), so the two compare like with like.
pub(crate) fn fingerprint_of_sequence(s: &Session, id: ItemId, cmd: &str) -> Result<an::Fingerprint> {
    let item = s.project.item(id).ok_or_else(|| bad(cmd, format!("no sequence {}", id.0)))?;
    let ItemKind::Sequence(q) = &item.kind else { return Err(bad(cmd, format!("“{}” is not a sequence", item.name))) };
    let input = crate::analysis::sequence_input(s, q);
    let shots = an::shot_spans(&input).map_err(|e| bad(cmd, e.to_string()))?;
    let (Some(first), Some(last)) = (shots.first(), shots.last()) else {
        return Err(bad(cmd, format!("“{}” has no shots on its video tracks to compare", item.name)));
    };
    let tps = TICKS_PER_SECOND as f64;
    let duration = (last.end - first.start) as f64 / tps;
    let cuts: Vec<f64> = shots.iter().skip(1).map(|sh| (sh.start - first.start) as f64 / tps).collect();
    // the pictures the shot analysis looked at, if it has been run for this edit
    let cache = crate::analysis::Cache::of(s);
    let pictures = crate::analysis::current_report_for(s, &cache, "sequence", id.0)
        .map(|r| {
            r["shots"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|sh| {
                            let stats: an::FrameStats = serde_json::from_value(sh["stats"].clone()).ok()?;
                            let mid = sh["start"].as_f64()? + sh["duration"].as_f64().unwrap_or(0.0) / 2.0 - first.start as f64 / tps;
                            Some(an::fingerprint::PictureSample {
                                time_seconds: mid.max(0.0),
                                stats,
                                camera: serde_json::from_value(sh["camera"].clone()).ok(),
                                motion_energy: sh["motion"]["energy"].as_f64().map(|e| e as f32),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let sound = crate::analysis::measured_mix(s, id).map(|mix| an::fingerprint::SoundInput { mix, segments: Vec::new() });
    an::fingerprint(&an::ReferenceInput {
        name: item.name.clone(),
        duration_seconds: duration,
        width: q.settings.width,
        height: q.settings.height,
        cuts,
        pictures,
        sound,
    })
    .map_err(|e| bad(cmd, e.to_string()))
}

fn reference_for(s: &Session, p: &Value, cmd: &str) -> Result<(String, String, an::Fingerprint)> {
    match str_p(p, "id").filter(|i| !i.is_empty()) {
        Some(id) => {
            let r = find(s, id).ok_or_else(|| bad(cmd, format!("there is no reference {id:?}; `reference.list` shows what there is")))?;
            Ok((r.id, r.name, r.fingerprint))
        }
        None => adopted(&s.project)
            .map(|a| (a.id, a.name, a.fingerprint))
            .ok_or_else(|| bad(cmd, "give `id` (a reference from `reference.list`), or adopt one first")),
    }
}

fn compare_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "reference.compare";
    let (id, name, theirs) = reference_for(s, p, cmd)?;
    let seq = sequence_of(s, p, cmd)?;
    let yours = fingerprint_of_sequence(s, seq, cmd)?;
    let comparison = an::compare(&yours, &theirs);
    let seq_name = s.project.item(seq).map(|i| i.name.clone()).unwrap_or_default();
    Ok(json!({
        "reference": {"id": id, "name": name},
        "sequence": {"id": seq.0, "name": seq_name},
        "comparison": to_value(&comparison),
        "measured": {"pictures": yours.picture.is_some(), "mix": yours.sound.is_some()},
    }))
}

fn adopted_json(a: &Adopted) -> Value {
    let f = &a.fingerprint;
    json!({
        "id": a.id, "name": a.name, "rights": a.rights,
        "targets": {
            "averageShotSeconds": f.shots.mean_seconds,
            "loudnessLufs": f.sound.as_ref().and_then(|s| s.integrated_lufs),
        },
    })
}

fn adopted_command(s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(json!({"adopted": adopted(&s.project).map(|a| adopted_json(&a))}))
}

fn adopt_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "reference.adopt";
    if bool_p(p, "clear") == Some(true) {
        if !s.project.ext.contains_key(ADOPTED_KEY) {
            return Ok(json!({"adopted": Value::Null}));
        }
        s.edit("Stop Using Reference", |pr, _| {
            pr.ext.remove(ADOPTED_KEY);
            Ok(())
        })?;
        return Ok(json!({"adopted": Value::Null}));
    }
    let id = id_param(p, cmd)?;
    let r = find(s, id).ok_or_else(|| bad(cmd, format!("there is no reference {id:?}; `reference.list` shows what there is")))?;
    let a = Adopted { id: r.id, name: r.name, rights: r.rights, fingerprint: r.fingerprint };
    let value = serde_json::to_value(&a).map_err(|e| EngineError::Other(e.to_string()))?;
    let out = adopted_json(&a);
    s.edit("Adopt Reference", |pr, _| {
        pr.ext.insert(ADOPTED_KEY.to_string(), value);
        Ok(())
    })?;
    Ok(json!({"adopted": out}))
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
        spec(
            "reference.measure",
            "Measure Reference",
            r#"{"path":str,"name":str?,"sampleFps":n=8,"pictures":n=120,"sensitivity":0..100=50,"wait":bool=false}"#,
            measure_command,
        ),
        spec(
            "reference.add",
            "Add Reference",
            r#"{"path":str,"rights":str,"name":str?,"sampleFps":n=8,"pictures":n=120,"sensitivity":0..100=50,"wait":bool=false}"#,
            add_command,
        ),
        spec("reference.result", "Reference Result", r#"{"job":id}"#, result_command),
        spec("reference.list", "List References", "{}", list_command),
        spec("reference.get", "Get Reference", r#"{"id":str}"#, get_command),
        spec("reference.rename", "Rename Reference", r#"{"id":str,"name":str}"#, rename_command),
        spec("reference.remove", "Remove Reference", r#"{"id":str}"#, remove_command),
        spec("reference.export", "Export Fingerprint", r#"{"id":str,"path":str}"#, export_command),
        spec("reference.import", "Import Fingerprint", r#"{"path":str}"#, import_command),
        spec("reference.compare", "Compare With Reference", r#"{"id":str?,"sequence":id?}"#, compare_command),
        spec("reference.adopted", "Adopted Reference", "{}", adopted_command),
        CommandSpec {
            id: "reference.adopt",
            label: "Adopt Reference",
            menu: &[],
            shortcut: None,
            params: r#"{"id":str | "clear":true}"#,
            enabled: always,
            run: adopt_command,
            journal: true,
        },
    ]
}
