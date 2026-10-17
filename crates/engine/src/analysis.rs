//! Analysis (fork roadmap F10.5): measuring footage, audio and the edit with `edith-analysis`, as
//! background jobs whose results are cached.
//!
//! | command | does |
//! |---|---|
//! | `analysis.run {item \| sequence \| mix, samples?, shots?, maxShots?, wait?, force?}` | measure a media item (pictures at `samples` evenly spaced moments with the camera motion around each, and the first five minutes of its sound), a sequence (the edit's own numbers, and the picture of each shot) or its mix (loudness, loudness range, true peak and silence over the whole programme, up to three hours); a background job unless `wait` |
//! | `analysis.get {item \| sequence \| mix \| key}` | the cached report |
//! | `coach.findings {sequence?}` | what is worth telling the editor about the edit (the rules in `edith_analysis::coach`), measured against the plan's Format Pack targets and, when it has been measured, the mix |
//! | `analysis.list` | every cached report, and whether it is still current |
//! | `analysis.clear {key \| item \| sequence \| stale \| all}` | remove reports |
//!
//! A report is keyed by what it measured and how: the media file's fingerprint (size plus a hash of
//! its first and last MiB, the one the Link Media dialog uses) or, for a sequence, a hash of its
//! layout and of the media its shots show, plus the version of every algorithm and the options
//! that change the numbers. Change the file, the edit or an algorithm and the key changes, so an
//! old report is never returned for new content; `analysis.list` marks it `current: false` and
//! `analysis.clear {stale: true}` removes it. Reports are a cache, never in the project: files in
//! an `Analysis` folder of the media cache (memory only where there is no folder), safe to delete.
//!
//! Everything measured is untrusted media: a frame or a stretch of sound that cannot be decoded is
//! recorded as an error in the report and the job carries on; the measures themselves never panic.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use edith_analysis as an;
use filmcraft_media::{FrameRequest, MediaKind, SharedSource};
use filmcraft_project::{ItemId, ItemKind, MediaClip, MediaRef, Project, TrackItem, TrackKind};
use filmcraft_time::{TICKS_PER_SECOND, Tick, TimeRange};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, str_p, u64_p};
use crate::{EngineError, Result, Session};

/// The report layout; bump with the algorithms' versions when the JSON changes shape.
pub const REPORT_VERSION: u32 = 1;
const DEFAULT_SAMPLES: usize = 60;
const MAX_SAMPLES: usize = 600;
const DEFAULT_SHOTS: usize = 300;
const MAX_SHOTS: usize = 2000;
/// Pictures are measured at about this width.
const FRAME_WIDTH: f64 = 640.0;
/// How far apart the two pictures of a motion measurement are.
const MOTION_GAP: f64 = 0.25;
/// Sound is measured over at most this long from the start, in segments of [`SEGMENT_SECONDS`].
const AUDIO_SECONDS: f64 = 300.0;
const SEGMENT_SECONDS: f64 = 30.0;
const MAX_BEATS_KEPT: usize = 2000;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------- the cache

/// The session's part of the cache: reports kept in memory (the disk copy is in the media cache).
#[derive(Clone, Default)]
pub struct Store {
    memory: Arc<Mutex<BTreeMap<String, String>>>,
}

#[derive(Clone)]
pub(crate) struct Cache {
    pub(crate) dir: Option<PathBuf>,
    pub(crate) memory: Arc<Mutex<BTreeMap<String, String>>>,
}

/// A key is made of letters, digits, `-` and `.` only, so it can never name a path.
pub(crate) fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 200 && !key.starts_with('.') && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

impl Cache {
    fn of(s: &Session) -> Cache {
        Cache { dir: s.media_cache_dir().map(|d| d.join("Analysis")), memory: s.analysis.memory.clone() }
    }

    fn path(&self, key: &str) -> Option<PathBuf> {
        valid_key(key).then(|| self.dir.as_ref().map(|d| d.join(format!("{key}.json")))).flatten()
    }

    pub(crate) fn read(&self, key: &str) -> Option<Value> {
        if !valid_key(key) {
            return None;
        }
        if let Some(text) = lock(&self.memory).get(key) {
            return serde_json::from_str(text).ok();
        }
        let text = std::fs::read_to_string(self.path(key)?).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub(crate) fn write(&self, key: &str, report: &Value) {
        if !valid_key(key) {
            return;
        }
        let Ok(text) = serde_json::to_string(report) else { return };
        if let Some(path) = self.path(key) {
            // best effort: a folder we cannot write to leaves the report in memory only
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, &text).is_ok() && std::fs::rename(&tmp, &path).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
        }
        lock(&self.memory).insert(key.to_string(), text);
    }

    pub(crate) fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = lock(&self.memory).keys().cloned().collect();
        if let Some(entries) = self.dir.as_ref().and_then(|d| std::fs::read_dir(d).ok()) {
            for e in entries.flatten().take(100_000) {
                let path = e.path();
                if path.extension().is_some_and(|x| x == "json")
                    && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                    && valid_key(stem)
                {
                    keys.push(stem.to_string());
                }
            }
        }
        keys.sort();
        keys.dedup();
        keys
    }

    pub(crate) fn remove(&self, key: &str) -> bool {
        let in_memory = lock(&self.memory).remove(key).is_some();
        let on_disk = self.path(key).is_some_and(|p| std::fs::remove_file(p).is_ok());
        in_memory || on_disk
    }

    fn bytes(&self, key: &str) -> u64 {
        lock(&self.memory).get(key).map_or_else(|| self.path(key).and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len()), |t| t.len() as u64)
    }
}

/// Every algorithm's version, so a report says what made it and a change makes it stale.
fn algorithms() -> Value {
    json!({"image": an::image::IMAGE_VERSION, "audio": an::audio::AUDIO_VERSION, "motion": an::motion::MOTION_VERSION, "sequence": an::sequence::SEQUENCE_VERSION, "report": REPORT_VERSION})
}

fn version_tag() -> String {
    format!("i{}a{}m{}s{}r{}", an::image::IMAGE_VERSION, an::audio::AUDIO_VERSION, an::motion::MOTION_VERSION, an::sequence::SEQUENCE_VERSION, REPORT_VERSION)
}

/// 128 bits of FNV-1a over `parts`, in hex: a stable key for content that is not a file.
fn hash128(parts: &[&[u8]]) -> String {
    let (mut a, mut b) = (0xcbf2_9ce4_8422_2325u64, 0x8422_2325_cbf2_9ce4u64);
    for part in parts {
        for byte in *part {
            a = (a ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
            b = (b ^ u64::from(*byte).rotate_left(3)).wrapping_mul(0x0100_0000_01b3);
        }
        // a boundary, so ["ab","c"] and ["a","bc"] differ
        a = (a ^ 0xff).wrapping_mul(0x0100_0000_01b3);
        b = (b ^ 0xfe).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{a:016x}{b:016x}")
}

/// What identifies a media item's content. `fresh` reads the file as it is now (its size and a hash of
/// its first and last MiB): right for one file. The quick form, for an edit with many files, trusts the
/// fingerprint taken at import while the file's size is unchanged, and reads the file when it is not.
/// A generator is identified by its definition.
fn media_fingerprint(s: &Session, m: &MediaClip, fresh: bool) -> String {
    let of = |id: filmcraft_project::MediaIdentity| format!("{}-{:016x}", id.size, id.fingerprint);
    match &m.media {
        MediaRef::File { path } => {
            if !fresh
                && let Some(id) = m.identity
                && s.services.file_size(path).is_ok_and(|size| size == id.size)
            {
                return of(id);
            }
            match crate::relink::identity_of(&*s.services, path) {
                Ok(id) => of(id),
                Err(_) => m.identity.map_or_else(|| hash128(&[path.as_bytes(), &m.info.file_size.unwrap_or(0).to_le_bytes()]), of),
            }
        }
        MediaRef::Generator(g) => hash128(&[serde_json::to_string(g).unwrap_or_default().as_bytes()]),
    }
}

// ---------------------------------------------------------------------------- planning a run

struct ItemWork {
    src: SharedSource,
    info: filmcraft_media::MediaInfo,
    /// The part of the media that is analysed (a subclip's range), else all of it.
    window: TimeRange,
    samples: usize,
}

struct ShotWork {
    clip: u64,
    start: Tick,
    end: Tick,
    item: TrackItem,
    src: Option<SharedSource>,
}

struct SeqWork {
    input: an::SequenceInput,
    shots: Vec<ShotWork>,
    shots_total: usize,
}

/// Measuring the whole mix of a sequence: its sound as the mixer makes it, a second at a time.
struct MixWork {
    project: Arc<Project>,
    seq: ItemId,
    provider: crate::media_pool::PoolProvider,
    rate: u32,
    frames: i64,
    truncated: bool,
}

enum Work {
    Item(ItemWork),
    Sequence(SeqWork),
    Mix(MixWork),
}

struct Plan {
    key: String,
    kind: &'static str,
    target: u64,
    name: String,
    params: Value,
    work: Work,
}

pub(crate) fn samples_param(p: &Value) -> usize {
    u64_p(p, "samples").map_or(DEFAULT_SAMPLES, |n| usize::try_from(n).unwrap_or(MAX_SAMPLES)).clamp(1, MAX_SAMPLES)
}

fn plan_item(s: &Session, id: ItemId, samples: usize, cmd: &str) -> Result<Plan> {
    let item = s.project.item(id).ok_or_else(|| bad(cmd, format!("no project item {}", id.0)))?;
    let (root, media, range) =
        s.project.resolve_media(id).ok_or_else(|| bad(cmd, format!("“{}” is not footage or audio; pick a clip, a still or a sound", item.name)))?;
    let src = s.source(root).ok_or_else(|| bad(cmd, format!("“{}” is offline: link its media first", item.name)))?;
    let info = media.info.clone();
    let window = range.unwrap_or(TimeRange { start: Tick::ZERO, duration: info.duration });
    let key = format!("item-{}-{}-{}-n{samples}-{}", media_fingerprint(s, media, true), window.start.0, window.duration.0, version_tag());
    Ok(Plan {
        key,
        kind: "item",
        target: id.0,
        name: item.name.clone(),
        params: json!({"samples": samples}),
        work: Work::Item(ItemWork { src, info, window, samples }),
    })
}

/// Whether an item shows pictures that count as a shot (a title, graphic or adjustment layer does not).
fn is_shot_item(p: &Project, id: ItemId) -> bool {
    match p.item(id).map(|i| &i.kind) {
        Some(ItemKind::Media(m)) => m.info.video.is_some(),
        Some(ItemKind::Subclip { parent, .. }) => is_shot_item(p, *parent),
        Some(ItemKind::Sequence(_)) => true,
        _ => false,
    }
}

/// The timeline as the analysis sees it: clips, transitions and narration as plain spans. A switched-off
/// track and a disabled clip are left out: they show and sound like nothing.
pub(crate) fn sequence_input(s: &Session, q: &filmcraft_project::Sequence) -> an::SequenceInput {
    let span = |t: &filmcraft_project::TrackItem, link: Option<u64>, is_shot: bool| an::sequence::ClipSpan {
        id: t.id.0,
        start: t.start.0,
        duration: t.duration.0,
        link,
        is_shot,
    };
    let tracks = |kind: TrackKind, tracks: &[filmcraft_project::Track]| -> Vec<an::sequence::TrackSpans> {
        tracks
            .iter()
            .filter(|t| t.kind == kind)
            .map(|t| {
                // a switched-off track shows nothing and counts for nothing
                if (kind == TrackKind::Video && !t.enabled) || (kind == TrackKind::Audio && t.muted) {
                    return an::sequence::TrackSpans::default();
                }
                an::sequence::TrackSpans {
                    clips: t
                        .items
                        .iter()
                        .filter(|c| c.enabled)
                        .map(|c| span(c, c.link, kind == TrackKind::Video && is_shot_item(&s.project, c.item)))
                        .collect(),
                    transitions: t
                        .transitions
                        .iter()
                        .map(|x| an::sequence::TransitionSpan {
                            start: x.start.0,
                            duration: x.duration.0,
                            kind: x.effect.effect.clone(),
                            between_clips: x.from.is_some() && x.to.is_some(),
                        })
                        .collect(),
                }
            })
            .collect()
    };
    let narration = q
        .audio_tracks
        .iter()
        .filter(|t| !t.muted)
        .flat_map(|t| t.items.iter().filter(|c| c.enabled))
        .filter_map(|c| {
            let n = s.project.narrations.get(&c.item)?;
            let words = edith_planner::script::words_and_pauses(&n.text).0;
            Some(an::sequence::NarrationSpan { start: c.start.0, duration: c.duration.0, words: u32::try_from(words).unwrap_or(u32::MAX) })
        })
        .collect();
    an::SequenceInput { video: tracks(TrackKind::Video, &q.video_tracks), audio: tracks(TrackKind::Audio, &q.audio_tracks), narration }
}

fn plan_sequence(s: &Session, id: ItemId, shots: bool, max_shots: usize, cmd: &str) -> Result<Plan> {
    let item = s.project.item(id).ok_or_else(|| bad(cmd, format!("no sequence {}", id.0)))?;
    let ItemKind::Sequence(q) = &item.kind else { return Err(bad(cmd, format!("“{}” is not a sequence", item.name))) };
    let input = sequence_input(s, q);
    // the shots to measure: evenly spread when there are more than `max_shots`
    let all = an::shot_spans(&input).map_err(|e| bad(cmd, e.to_string()))?;
    let shots_total = all.len();
    let chosen: Vec<an::ShotSpan> = if !shots {
        Vec::new()
    } else if all.len() <= max_shots {
        all
    } else {
        (0..max_shots).filter_map(|i| all.get(i * all.len() / max_shots).copied()).collect()
    };
    let mut works = Vec::new();
    let mut fingerprints = Vec::new();
    for sh in &chosen {
        let Some(it) = q.video_tracks.iter().flat_map(|t| &t.items).find(|c| c.id.0 == sh.clip) else { continue };
        let root = s.project.resolve_media(it.item).map(|r| r.0).or(Some(it.item));
        let src = root.and_then(|r| s.source(r));
        if let Some((_, m, _)) = s.project.resolve_media(it.item) {
            fingerprints.push(media_fingerprint(s, m, false));
        }
        works.push(ShotWork { clip: sh.clip, start: Tick(sh.start), end: Tick(sh.end), item: it.clone(), src });
    }
    let key = format!(
        "sequence-{}-{}",
        hash128(&[
            serde_json::to_string(&input).unwrap_or_default().as_bytes(),
            fingerprints.join(",").as_bytes(),
            format!("{shots}{max_shots}").as_bytes(),
            serde_json::to_string(&q.settings).unwrap_or_default().as_bytes(),
        ]),
        version_tag()
    );
    Ok(Plan {
        key,
        kind: "sequence",
        target: id.0,
        name: item.name.clone(),
        params: json!({"shots": shots, "maxShots": max_shots}),
        work: Work::Sequence(SeqWork { input, shots: works, shots_total }),
    })
}

/// The longest stretch of a mix that is measured.
const MAX_MIX_SECONDS: i64 = 3 * 3600;

fn plan_mix(s: &Session, id: ItemId, cmd: &str) -> Result<Plan> {
    let item = s.project.item(id).ok_or_else(|| bad(cmd, format!("no sequence {}", id.0)))?;
    let ItemKind::Sequence(q) = &item.kind else { return Err(bad(cmd, format!("“{}” is not a sequence", item.name))) };
    let rate = q.settings.sample_rate.clamp(8_000, 384_000);
    let end = q.audio_tracks.iter().filter(|t| !t.muted).flat_map(|t| t.items.iter().filter(|c| c.enabled)).map(|c| c.end()).max().unwrap_or(Tick::ZERO);
    let frames = end.to_units_floor(i64::from(rate));
    if frames <= 0 {
        return Err(bad(cmd, format!("“{}” has nothing on its audio tracks to measure", item.name)));
    }
    let mut items: Vec<u64> = q.audio_tracks.iter().flat_map(|t| &t.items).map(|c| c.item.0).collect();
    items.sort_unstable();
    items.dedup();
    let fingerprints: Vec<String> = items.iter().filter_map(|i| s.project.resolve_media(ItemId(*i)).map(|(_, m, _)| media_fingerprint(s, m, false))).collect();
    let key = format!(
        "mix-{}-{}",
        // only what makes the sound: a change to the picture leaves a measured mix current
        hash128(&[
            serde_json::to_string(&(&q.audio_tracks, &q.submix_tracks, q.master_volume_db, &q.master_effects, &q.master_mixer)).unwrap_or_default().as_bytes(),
            fingerprints.join(",").as_bytes(),
            &rate.to_le_bytes(),
        ]),
        version_tag()
    );
    let limit = i64::from(rate) * MAX_MIX_SECONDS;
    let provider = s.media.full_res_provider(s.project.clone(), s.services.clone());
    Ok(Plan {
        key,
        kind: "mix",
        target: id.0,
        name: item.name.clone(),
        params: json!({}),
        work: Work::Mix(MixWork { project: s.project.clone(), seq: id, provider, rate, frames: frames.min(limit), truncated: frames > limit }),
    })
}

/// The target of a command: `item` (a project item id) or `sequence` (`true` for the open one, or an id).
fn plan(s: &Session, p: &Value, cmd: &str) -> Result<Plan> {
    if let Some(item) = u64_p(p, "item") {
        return plan_item(s, ItemId(item), samples_param(p), cmd);
    }
    if bool_p(p, "mix") == Some(true) {
        let id = match p.get("sequence").and_then(Value::as_u64) {
            Some(id) => ItemId(id),
            None => s.state.active_sequence.ok_or(EngineError::NoSequence)?,
        };
        return plan_mix(s, id, cmd);
    }
    match p.get("sequence") {
        Some(Value::Bool(true)) => {
            let id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
            plan_sequence(s, id, bool_p(p, "shots").unwrap_or(true), max_shots_param(p), cmd)
        }
        Some(v) if v.as_u64().is_some() => plan_sequence(s, ItemId(v.as_u64().unwrap_or(0)), bool_p(p, "shots").unwrap_or(true), max_shots_param(p), cmd),
        _ => Err(bad(cmd, "give `item` (a project item id), `sequence` (true for the open sequence, or its item id) or `mix: true`")),
    }
}

pub(crate) fn max_shots_param(p: &Value) -> usize {
    u64_p(p, "maxShots").map_or(DEFAULT_SHOTS, |n| usize::try_from(n).unwrap_or(MAX_SHOTS)).clamp(1, MAX_SHOTS)
}

// ---------------------------------------------------------------------------- measuring

fn secs(t: Tick) -> f64 {
    t.0 as f64 / TICKS_PER_SECOND as f64
}

fn to_value<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// The picture of a frame at `time`, measured; `Err` says why it could not be.
fn frame_at(src: &SharedSource, time: Tick, scale: f32) -> std::result::Result<(u32, u32, Vec<u8>), String> {
    let f = src.video_frame(FrameRequest { time, scale }).map_err(|e| e.to_string())?;
    Ok((f.width, f.height, f.to_rgba8()))
}

fn motion_between(src: &SharedSource, a: Tick, b: Tick, seconds: f64, scale: f32) -> Option<an::MotionPair> {
    let (fa, fb) = (frame_at(src, a, scale).ok()?, frame_at(src, b, scale).ok()?);
    let (ma, mb) = (an::Frame::new(fa.0, fa.1, &fa.2).ok()?, an::Frame::new(fb.0, fb.1, &fb.2).ok()?);
    an::analyze_motion(&ma, &mb, seconds).ok()
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let (sum, n) = values.fold((0.0, 0u32), |(s, n), v| (s + v, n + 1));
    (n > 0).then(|| sum / f64::from(n))
}

fn median(mut values: Vec<f32>) -> f32 {
    values.sort_by(f32::total_cmp);
    values.get(values.len() / 2).copied().unwrap_or(0.0)
}

/// The colours that recur across the samples' palettes: close colours merged, the biggest five.
fn merged_palette(samples: &[an::FrameStats]) -> Vec<an::image::PaletteColor> {
    let mut merged: Vec<(an::image::PaletteColor, f32)> = Vec::new();
    for c in samples.iter().flat_map(|s| s.palette.iter()) {
        match merged.iter_mut().find(|(m, _)| an::color::lab_distance(m.lab, c.lab) < 12.0) {
            Some((m, weight)) => {
                let total = *weight + c.share;
                for i in 0..3 {
                    m.lab[i] = (m.lab[i] * *weight + c.lab[i] * c.share) / total.max(f32::MIN_POSITIVE);
                    m.rgb[i] = (m.rgb[i] * *weight + c.rgb[i] * c.share) / total.max(f32::MIN_POSITIVE);
                }
                *weight = total;
            }
            None => merged.push((*c, c.share)),
        }
    }
    merged.sort_by(|a, b| b.1.total_cmp(&a.1));
    let total: f32 = merged.iter().take(5).map(|m| m.1).sum();
    merged
        .into_iter()
        .take(5)
        .map(|(mut c, w)| {
            c.share = if total > 0.0 { w / total } else { 0.0 };
            c
        })
        .collect()
}

fn video_report(w: &ItemWork, prog: &filmcraft_export::Progress) -> std::result::Result<Value, String> {
    let Some(v) = &w.info.video else { return Ok(Value::Null) };
    let still = w.info.kind == MediaKind::Still;
    let n = if still { 1 } else { w.samples };
    let dur = w.window.duration.0.max(0);
    let scale = (FRAME_WIDTH / f64::from(v.width.max(1))).min(1.0) as f32;
    let gap = (MOTION_GAP * TICKS_PER_SECOND as f64) as i64;
    let mut samples = Vec::new();
    let mut stats = Vec::new();
    let mut pairs = Vec::new();
    let mut failed = 0u32;
    for i in 0..n {
        if prog.cancel.load(Ordering::Relaxed) {
            return Err("stopped".into());
        }
        let t = Tick(w.window.start.0 + (i128::from(dur) * (2 * i as i128 + 1) / (2 * n as i128)) as i64);
        *lock(&prog.status) = format!("Picture {} of {n}", i + 1);
        match frame_at(&w.src, t, scale).and_then(|(fw, fh, rgba)| an::analyze_rgba(fw, fh, &rgba).map_err(|e| e.to_string())) {
            Ok(st) => {
                // the motion around this moment: two pictures a quarter second apart, inside the window
                let (ta, tb) = (Tick((t.0 - gap / 2).max(w.window.start.0)), Tick((t.0 + gap / 2).min(w.window.end().0)));
                let motion = if !still && tb.0 - ta.0 >= TICKS_PER_SECOND / 20 { motion_between(&w.src, ta, tb, secs(tb - ta), scale) } else { None };
                if let Some(m) = motion {
                    pairs.push(m);
                }
                samples.push(json!({"time": secs(t), "stats": to_value(&st), "motion": motion.map(|m| to_value(&m))}));
                stats.push(st);
            }
            Err(e) => {
                failed += 1;
                samples.push(json!({"time": secs(t), "error": e}));
            }
        }
        prog.done.fetch_add(1, Ordering::Relaxed);
    }
    if stats.is_empty() {
        return Err(format!("none of the {n} pictures could be read: {}", samples.first().and_then(|s| s["error"].as_str()).unwrap_or("no pictures")));
    }
    let avg = |f: &dyn Fn(&an::FrameStats) -> f64| mean(stats.iter().map(f));
    let temperature = mean(stats.iter().filter_map(|s| s.colour_temperature).map(f64::from));
    let lb = |f: &dyn Fn(&an::image::Letterbox) -> f32| median(stats.iter().map(|s| f(&s.letterbox)).collect());
    let summary = json!({
        "lumaMean": avg(&|s| f64::from(s.luma_mean)),
        "contrast": avg(&|s| f64::from(s.contrast)),
        "saturation": avg(&|s| f64::from(s.saturation)),
        "sharpness": avg(&|s| f64::from(s.sharpness)),
        "grain": avg(&|s| f64::from(s.grain)),
        "colourTemperature": temperature,
        "letterbox": {"top": lb(&|l| l.top), "bottom": lb(&|l| l.bottom), "left": lb(&|l| l.left), "right": lb(&|l| l.right)},
        "palette": to_value(&merged_palette(&stats)),
        "motion": to_value(&an::summarize(&pairs)),
    });
    Ok(json!({
        "width": v.width, "height": v.height, "durationSeconds": secs(w.window.duration),
        "samples": samples, "failed": failed, "summary": summary,
    }))
}

fn audio_report(w: &ItemWork, prog: &filmcraft_export::Progress) -> std::result::Result<Value, String> {
    let Some(a) = w.info.audio() else { return Ok(Value::Null) };
    let sr = a.sample_rate;
    if !(an::audio::MIN_SAMPLE_RATE..=an::audio::MAX_SAMPLE_RATE).contains(&sr) {
        return Ok(json!({"error": format!("a sample rate of {sr} Hz cannot be analysed")}));
    }
    let want = (secs(w.window.duration).clamp(0.0, AUDIO_SECONDS) * f64::from(sr)) as usize;
    let first = (secs(w.window.start).max(0.0) * f64::from(sr)) as i64;
    let chunk = sr as usize * 10;
    let mut pcm: Vec<f32> = Vec::new();
    let mut channels = 0usize;
    let mut got = 0usize;
    while got < want {
        if prog.cancel.load(Ordering::Relaxed) {
            return Err("stopped".into());
        }
        *lock(&prog.status) = format!("Sound {:.0} of {:.0} s", got as f64 / f64::from(sr), want as f64 / f64::from(sr));
        let take = chunk.min(want - got);
        let buf = match w.src.audio(first + got as i64, take, sr) {
            Ok(b) => b,
            Err(e) => {
                if got == 0 {
                    return Ok(json!({"error": e.to_string()}));
                }
                break;
            }
        };
        let frames = buf.frames();
        if frames == 0 {
            break;
        }
        // at most two channels: the first two
        let nch = buf.channel_count().clamp(1, 2);
        if channels == 0 {
            channels = nch;
        }
        for i in 0..frames {
            for c in 0..channels {
                pcm.push(buf.channels.get(c).and_then(|ch| ch.get(i)).copied().unwrap_or(0.0));
            }
        }
        got += frames;
        prog.done.fetch_add(1, Ordering::Relaxed);
        if frames < take {
            break;
        }
    }
    let Ok(channels_u16) = u16::try_from(channels) else { return Ok(Value::Null) };
    let stats = match an::analyze_audio(sr, channels_u16, &pcm) {
        Ok(mut st) => {
            if let Some(t) = st.tempo.as_mut() {
                t.beats.truncate(MAX_BEATS_KEPT);
            }
            st
        }
        Err(e) => return Ok(json!({"error": e.to_string()})),
    };
    let seg = (SEGMENT_SECONDS * f64::from(sr)) as usize;
    let mut segments = Vec::new();
    let mut at = 0usize;
    while at + sr as usize <= pcm.len() / channels.max(1) {
        let end = (at + seg).min(pcm.len() / channels.max(1));
        if let Some(part) = pcm.get(at * channels..end * channels)
            && let Ok(mut st) = an::analyze_audio(sr, channels_u16, part)
        {
            if let Some(t) = st.tempo.as_mut() {
                t.beats.clear();
            }
            segments.push(json!({"start": at as f64 / f64::from(sr), "duration": (end - at) as f64 / f64::from(sr), "stats": to_value(&st)}));
        }
        at = end;
    }
    Ok(json!({
        "sampleRate": sr, "channels": channels, "analysedSeconds": (pcm.len() / channels.max(1)) as f64 / f64::from(sr),
        "truncated": secs(w.window.duration) > AUDIO_SECONDS,
        "stats": to_value(&stats), "segments": segments,
    }))
}

fn item_report(w: &ItemWork, prog: &filmcraft_export::Progress) -> std::result::Result<Value, String> {
    let units = if w.info.video.is_some() { if w.info.kind == MediaKind::Still { 1 } else { w.samples } } else { 0 }
        + if w.info.audio().is_some() { (AUDIO_SECONDS / 10.0) as usize } else { 0 };
    prog.total.store(units.max(1) as u64, Ordering::Relaxed);
    let video = video_report(w, prog)?;
    let audio = audio_report(w, prog)?;
    if video.is_null() && audio.is_null() {
        return Err("there is neither picture nor sound to measure".into());
    }
    Ok(json!({"video": video, "audio": audio}))
}

fn sequence_report(w: &SeqWork, prog: &filmcraft_export::Progress) -> std::result::Result<Value, String> {
    prog.total.store(w.shots.len().max(1) as u64, Ordering::Relaxed);
    let stats = an::analyze_sequence(&w.input).map_err(|e| e.to_string())?;
    let mut shots = Vec::new();
    for (i, sh) in w.shots.iter().enumerate() {
        if prog.cancel.load(Ordering::Relaxed) {
            return Err("stopped".into());
        }
        *lock(&prog.status) = format!("Shot {} of {}", i + 1, w.shots.len());
        let mid = Tick(sh.start.0 / 2 + sh.end.0 / 2);
        let mut entry = json!({"clip": sh.clip, "start": secs(sh.start), "duration": secs(sh.end - sh.start)});
        match &sh.src {
            None => entry["error"] = json!("its media is offline or not footage"),
            Some(src) => {
                let width = src.info().video.as_ref().map_or(1, |v| v.width.max(1));
                let scale = (FRAME_WIDTH / f64::from(width)).min(1.0) as f32;
                let at = sh.item.source_time_at(mid);
                match frame_at(src, at, scale).and_then(|(fw, fh, rgba)| an::analyze_rgba(fw, fh, &rgba).map_err(|e| e.to_string())) {
                    Ok(st) => {
                        entry["stats"] = to_value(&st);
                        // the motion across a fifth of a second around the middle, on the timeline's clock
                        let half = TICKS_PER_SECOND / 10;
                        let (ta, tb) = (Tick((mid.0 - half).max(sh.start.0)), Tick((mid.0 + half).min(sh.end.0)));
                        if tb.0 - ta.0 >= TICKS_PER_SECOND / 25
                            && let Some(m) = motion_between(src, sh.item.source_time_at(ta), sh.item.source_time_at(tb), secs(tb - ta), scale)
                        {
                            entry["motion"] = to_value(&m);
                            entry["camera"] = to_value(&an::summarize(&[m]).camera);
                        }
                    }
                    Err(e) => entry["error"] = json!(e),
                }
            }
        }
        shots.push(entry);
        prog.done.fetch_add(1, Ordering::Relaxed);
    }
    Ok(json!({"sequence": to_value(&stats), "shots": shots, "shotsTotal": w.shots_total, "shotsAnalysed": w.shots.len()}))
}

fn mix_report(w: &MixWork, prog: &filmcraft_export::Progress) -> std::result::Result<Value, String> {
    let Some(q) = w.project.sequence(w.seq) else { return Err("the sequence is gone".into()) };
    let mut meter = an::MixMeter::new(w.rate, 2).map_err(|e| e.to_string())?;
    let rate = i64::from(w.rate);
    prog.total.store(((w.frames + rate - 1) / rate).max(1) as u64, Ordering::Relaxed);
    let mut pos = 0i64;
    let mut inter: Vec<f32> = Vec::new();
    while pos < w.frames {
        if prog.cancel.load(Ordering::Relaxed) {
            return Err("stopped".into());
        }
        *lock(&prog.status) = format!("Mix {} of {} s", pos / rate, w.frames / rate);
        let n = (w.frames - pos).min(rate) as usize;
        let buf = filmcraft_render::audio::mix_sequence(&w.project, q, pos, n, &w.provider);
        inter.clear();
        for i in 0..buf.frames() {
            for c in 0..2 {
                inter.push(buf.channels.get(c).or_else(|| buf.channels.first()).and_then(|ch| ch.get(i)).copied().unwrap_or(0.0));
            }
        }
        meter.push(&inter);
        pos += n as i64;
        prog.done.fetch_add(1, Ordering::Relaxed);
    }
    let mix = meter.finish();
    Ok(json!({"mix": to_value(&mix), "analysedSeconds": w.frames as f64 / f64::from(w.rate), "truncated": w.truncated, "sampleRate": w.rate}))
}

// ---------------------------------------------------------------------------- commands

fn header(plan: &Plan) -> Value {
    json!({
        "reportVersion": REPORT_VERSION,
        "algorithms": algorithms(),
        "kind": plan.kind,
        "key": plan.key,
        "target": {"id": plan.target, "name": plan.name},
        "params": plan.params,
    })
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "analysis.run";
    let plan = plan(s, p, cmd)?;
    let cache = Cache::of(s);
    if !bool_p(p, "force").unwrap_or(false) && cache.read(&plan.key).is_some() {
        return Ok(json!({"key": plan.key, "cached": true, "job": Value::Null}));
    }
    let id = s.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
    let label = format!("Analyse {}", plan.name);
    let job = crate::Job { id, label, progress: Default::default(), result: Default::default() };
    let (prog, res) = (job.progress.clone(), job.result.clone());
    let head = header(&plan);
    let key = plan.key.clone();
    let report: Arc<Mutex<Option<Value>>> = Arc::default();
    let out = report.clone();
    let work = plan.work;
    let run = move || {
        let t0 = web_time::Instant::now();
        let measured = match &work {
            Work::Item(w) => item_report(w, &prog),
            Work::Sequence(w) => sequence_report(w, &prog),
            Work::Mix(w) => mix_report(w, &prog),
        };
        let secs = t0.elapsed().as_secs_f64();
        let done = prog.done.load(Ordering::Relaxed);
        let r = match measured {
            Ok(body) => {
                let mut full = head;
                if let (Some(h), Some(b)) = (full.as_object_mut(), body.as_object()) {
                    h.extend(b.iter().map(|(k, v)| (k.clone(), v.clone())));
                }
                cache.write(&key, &full);
                *lock(&out) = Some(full);
                *lock(&prog.status) = format!("Analysed in {secs:.1}s");
                Ok(filmcraft_export::Report {
                    path: String::new(),
                    frames: done,
                    seconds: secs,
                    bytes: 0,
                    render_fps: done as f64 / secs.max(1e-6),
                    extra_files: Vec::new(),
                })
            }
            Err(e) => {
                *lock(&prog.status) = if e == "stopped" { "Stopped: nothing was kept".into() } else { e.clone() };
                Err(e)
            }
        };
        *lock(&res) = Some(r);
        prog.finished.store(true, Ordering::Relaxed);
    };
    let run = crate::export_tools::guard_job(job.progress.clone(), job.result.clone(), run);
    s.jobs.push(job);
    let mut result = json!({"key": plan.key, "cached": false, "job": id});
    if bool_p(p, "wait").unwrap_or(false) || cfg!(target_arch = "wasm32") {
        run();
        if let Some(Err(e)) = s.jobs.iter().find(|j| j.id == id).and_then(|j| lock(&j.result).clone()) {
            return Err(EngineError::Other(e));
        }
        result["report"] = lock(&report).clone().unwrap_or(Value::Null);
    } else {
        std::thread::Builder::new().name("edith-analysis".into()).spawn(run).map_err(|e| EngineError::Other(e.to_string()))?;
    }
    Ok(result)
}

/// The key a stored report would have today, from its header: `None` when its target is gone.
fn current_key(s: &Session, header: &Value) -> Option<String> {
    let id = header["target"]["id"].as_u64()?;
    let cmd = "analysis.list";
    match header["kind"].as_str()? {
        "item" => plan_item(s, ItemId(id), usize::try_from(header["params"]["samples"].as_u64()?).ok()?.clamp(1, MAX_SAMPLES), cmd).ok().map(|p| p.key),
        "mix" => plan_mix(s, ItemId(id), cmd).ok().map(|p| p.key),
        "sequence" => {
            let shots = header["params"]["shots"].as_bool().unwrap_or(true);
            let max = usize::try_from(header["params"]["maxShots"].as_u64().unwrap_or(DEFAULT_SHOTS as u64)).ok()?.clamp(1, MAX_SHOTS);
            plan_sequence(s, ItemId(id), shots, max, cmd).ok().map(|p| p.key)
        }
        _ => None,
    }
}

fn entry(s: &Session, cache: &Cache, key: &str) -> Option<Value> {
    let report = cache.read(key)?;
    let current = current_key(s, &report).is_some_and(|k| k == key);
    Some(json!({
        "key": key,
        "kind": report["kind"],
        "target": report["target"],
        "params": report["params"],
        "algorithms": report["algorithms"],
        "bytes": cache.bytes(key),
        "current": current,
    }))
}

fn list(s: &mut Session, _p: &Value) -> Result<Value> {
    let cache = Cache::of(s);
    let entries: Vec<Value> = cache.keys().iter().filter_map(|k| entry(s, &cache, k)).collect();
    Ok(json!({"reports": entries, "folder": cache.dir.map(|d| d.to_string_lossy().into_owned())}))
}

fn get(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "analysis.get";
    let cache = Cache::of(s);
    if let Some(key) = str_p(p, "key") {
        return cache.read(key).ok_or_else(|| bad(cmd, format!("no report {key}")));
    }
    let wanted = plan(s, p, cmd)?;
    if let Some(report) = cache.read(&wanted.key) {
        return Ok(report);
    }
    // measured with other options (another number of samples, say): take a current one for the same target
    for key in cache.keys() {
        if let Some(r) = cache.read(&key)
            && r["kind"] == wanted.kind
            && r["target"]["id"] == wanted.target
            && current_key(s, &r).is_some_and(|k| k == key)
        {
            return Ok(r);
        }
    }
    Err(bad(cmd, format!("“{}” has not been analysed: run analysis.run first", wanted.name)))
}

fn clear(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "analysis.clear";
    let cache = Cache::of(s);
    let mut removed = 0u32;
    if bool_p(p, "all").unwrap_or(false) {
        for k in cache.keys() {
            removed += u32::from(cache.remove(&k));
        }
    } else if let Some(key) = str_p(p, "key") {
        removed += u32::from(cache.remove(key));
    } else if bool_p(p, "stale").unwrap_or(false) {
        for k in cache.keys() {
            let stale = match cache.read(&k) {
                Some(r) => current_key(s, &r).is_none_or(|c| c != k),
                None => true,
            };
            if stale {
                removed += u32::from(cache.remove(&k));
            }
        }
    } else if p.get("item").is_some() || p.get("sequence").is_some() {
        let (kind, id) = match (u64_p(p, "item"), p.get("sequence")) {
            (Some(i), _) => ("item", Some(i)),
            (None, Some(Value::Bool(true))) => ("sequence", s.state.active_sequence.map(|i| i.0)),
            (None, Some(v)) => ("sequence", v.as_u64()),
            _ => ("item", None),
        };
        let id = id.ok_or_else(|| bad(cmd, "`item` or `sequence` must be an id (or true for the open sequence)"))?;
        for k in cache.keys() {
            if cache.read(&k).is_some_and(|r| r["kind"] == kind && r["target"]["id"] == id) {
                removed += u32::from(cache.remove(&k));
            }
        }
    } else {
        return Err(bad(cmd, "say what to clear: `key`, `item`, `sequence`, `stale: true` or `all: true`"));
    }
    Ok(json!({"removed": removed}))
}

// ---------------------------------------------------------------------------- the coach

/// What the edit is measured against: the plan's Format Pack targets, and, for each plan section that is
/// laid on the timeline, where it is.
fn coach_targets(s: &Session) -> an::coach::Targets {
    let mut t = an::coach::Targets::default();
    if let Some(plan) = crate::planner::get(&s.project) {
        t.words_per_minute = crate::planner::pack_target(s, &plan, "words_per_minute");
        t.average_shot_seconds = crate::planner::pack_target(s, &plan, "average_shot_seconds");
        for sec in &plan.sections {
            let links: Vec<_> = sec.beats.iter().filter_map(|b| b.link).collect();
            let (Some(start), Some(end)) = (links.iter().map(|l| l.start.0).min(), links.iter().map(|l| l.start.0.saturating_add(l.duration.0)).max()) else {
                continue;
            };
            t.sections.push(an::coach::SectionTarget { name: sec.name.clone(), start, end, average_shot_seconds: None });
        }
    }
    t
}

/// The sequence the coach looks at: `sequence` (an id) or the open one.
fn coach_sequence(s: &Session, p: &Value, cmd: &str) -> Result<ItemId> {
    match p.get("sequence").filter(|v| !v.is_null() && **v != json!(true)) {
        Some(v) => v.as_u64().map(ItemId).ok_or_else(|| bad(cmd, "`sequence` must be a sequence's item id")),
        None => s.state.active_sequence.ok_or(EngineError::NoSequence),
    }
}

/// The mix measurement of this edit, if one has been made and the edit has not changed since.
pub(crate) fn measured_mix(s: &Session, id: ItemId) -> Option<an::Mix> {
    let key = plan_mix(s, id, "coach.findings").ok()?.key;
    let report = Cache::of(s).read(&key)?;
    serde_json::from_value(report["mix"].clone()).ok()
}

fn coach_findings(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "coach.findings";
    let id = coach_sequence(s, p, cmd)?;
    let item = s.project.item(id).ok_or_else(|| bad(cmd, format!("no sequence {}", id.0)))?;
    let ItemKind::Sequence(q) = &item.kind else { return Err(bad(cmd, format!("“{}” is not a sequence", item.name))) };
    let input = sequence_input(s, q);
    let targets = coach_targets(s);
    let mix = measured_mix(s, id);
    let found = an::coach::findings(&input, &targets, mix.as_ref()).map_err(|e| bad(cmd, e.to_string()))?;
    let shots = an::shot_spans(&input).map(|v| v.len()).unwrap_or(0);
    Ok(json!({
        "sequence": {"id": id.0, "name": item.name},
        "findings": found,
        "targets": to_value(&targets),
        "mix": mix.map(|m| to_value(&m)),
        "mixMeasured": mix.is_some(),
        "shots": shots,
        "coachVersion": an::coach::COACH_VERSION,
    }))
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
            "analysis.run",
            "Analyse",
            r#"{"item":id | "sequence":true|id | "mix":true,"samples":n=60,"shots":bool=true,"maxShots":n=300,"wait":bool=false,"force":bool=false}"#,
            run,
        ),
        spec("analysis.get", "Get Analysis", r#"{"item":id | "sequence":true|id | "key":str}"#, get),
        spec("analysis.list", "List Analyses", "{}", list),
        spec("coach.findings", "Coach Findings", r#"{"sequence":id?}"#, coach_findings),
        spec("analysis.clear", "Clear Analyses", r#"{"key":str | "item":id | "sequence":true|id | "stale":bool | "all":bool}"#, clear),
    ]
}
