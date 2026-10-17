//! Looks (fork roadmap F12.7): a look bible is a small board of pictures that say how a film should
//! feel, kept with a note of why you may use each one, and measured.
//!
//! | command | does |
//! |---|---|
//! | `look.create {name, notes?}` | a new, empty look |
//! | `look.list`, `look.get {id}` | the looks, and one in full (its pictures' numbers, not the pictures) |
//! | `look.set {id, name?, notes?, aspect?, grain?, lut?}` | rename it and write down what you want: the picture shape, the grain, the LUT |
//! | `look.remove {id}` | delete a look and its pictures |
//! | `look.addStill {look, path?, seconds?, rights?, caption?, wait?, time?}` | keep a picture: a frame of an image or film file (`path`, a note of `rights` is required), or the Program frame (the playhead, or `time`). A file is read as a background job (watch it in `jobs.list`, stop it with `jobs.cancel`, ask for its answer with `look.result {job}`); `wait: true` returns the picture's id instead |
//! | `look.result {job}` | where a file picture's job is: running, done (with the picture's id) or failed |
//! | `look.setStill {look, still, caption}`, `look.removeStill {look, still}` | tidy a look's pictures |
//! | `look.picture {look, still}` | one picture as a JPEG in base64, for drawing |
//! | `look.compare {look, time?}` | the Program frame against the look's numbers, in plain words |
//! | `look.match {look, still?, clip?, faceDetection?}` | grade the clip at the playhead to match one of the look's pictures (Lumetri wheels and saturation, one undo step) |
//!
//! The library is global: `edith-looks.json` beside the preferences, for every project (in memory only
//! where there is no data folder, as in tests). Pictures are kept small (at most 640 px on the long
//! side, JPEG) and every one carries the note of why it may be used and where it came from (a file's
//! name, never its folder). Everything read back from the file is untrusted: sizes, counts and text are
//! held to their limits, and a damaged file is an empty library.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use edith_analysis as an;
use filmcraft_media::FrameRequest;
use filmcraft_project::gtemplate::{base64_decode, base64_encode};
use filmcraft_render::Image;
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::analysis::{lock, secs, to_value};
use crate::commands::{CommandSpec, always, bad, str_p, time_p};
use crate::{EngineError, Result, Session};

pub(crate) const FILE: &str = "edith-looks.json";
const MAX_FILE: u64 = 40 * 1024 * 1024;
/// Most looks kept, most pictures in one, and most pictures in all of them.
pub const MAX_LOOKS: usize = 20;
pub const MAX_STILLS: usize = 24;
pub const MAX_STILLS_TOTAL: usize = 150;
/// The long side of a kept picture.
pub const STILL_SIDE: u32 = 640;
/// The most characters of a picture's JPEG in base64 that are believed (about 450 KB of JPEG).
const MAX_JPEG_B64: usize = 600_000;
const MAX_NAME: usize = 80;
const MAX_NOTE: usize = 600;
const MAX_SHORT: usize = 120;
const MAX_RIGHTS: usize = 300;
const MAX_PATH: usize = 4096;

/// One picture of a look: small, with its note of rights and where it came from, and its numbers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Still {
    pub id: String,
    pub caption: String,
    pub rights: String,
    /// A file's name, or "Program frame": never a folder.
    pub source: String,
    /// Where in the film it was taken, when that is known.
    pub time_seconds: Option<f64>,
    pub width: u32,
    pub height: u32,
    /// The picture as a JPEG, base64.
    pub jpeg: String,
    pub stats: an::FrameStats,
    pub added_unix: i64,
}

/// A named board of pictures and what you want: the picture shape, the grain, the LUT.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Look {
    pub id: String,
    pub name: String,
    pub notes: String,
    pub aspect: String,
    pub grain: String,
    pub lut: String,
    pub stills: Vec<Still>,
    pub added_unix: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Library {
    pub version: u32,
    pub next_id: u64,
    pub looks: Vec<Look>,
}

type SharedLibrary = Arc<Mutex<Option<Library>>>;

/// What the session keeps: the library, read from its file the first time it is needed.
#[derive(Clone, Default)]
pub struct Hub {
    library: SharedLibrary,
    /// Where finished and failed `look.addStill` jobs left their answers.
    done: Arc<Mutex<BTreeMap<u64, std::result::Result<Value, String>>>>,
}

fn clean(t: &str, max: usize) -> String {
    t.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}

fn clean_multiline(t: &str, max: usize) -> String {
    t.chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect::<String>().trim().to_string()
}

fn file_path(s: &Session) -> Option<PathBuf> {
    s.prefs_path.as_ref().and_then(|p| p.parent()).map(|d| d.join(FILE))
}

fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// A picture read back from the file is kept only when everything about it can be believed.
fn still_ok(st: &Still) -> bool {
    id_ok(&st.id)
        && (1..=STILL_SIDE).contains(&st.width)
        && (1..=STILL_SIDE).contains(&st.height)
        && !st.jpeg.is_empty()
        && st.jpeg.len() <= MAX_JPEG_B64
        && st.time_seconds.is_none_or(f64::is_finite)
}

fn read_library(file: Option<&Path>) -> Library {
    let empty = Library { version: 1, ..Default::default() };
    let Some(p) = file else { return empty };
    let Ok(meta) = std::fs::metadata(p) else { return empty };
    if meta.len() > MAX_FILE {
        return empty;
    }
    let Some(mut lib) = std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<Library>(&t).ok()) else { return empty };
    let mut seen: Vec<String> = Vec::new();
    lib.looks.retain(|l| {
        let ok = id_ok(&l.id) && !seen.contains(&l.id);
        if ok {
            seen.push(l.id.clone());
        }
        ok
    });
    lib.looks.truncate(MAX_LOOKS);
    let mut total = 0usize;
    for l in &mut lib.looks {
        l.name = clean(&l.name, MAX_NAME);
        l.notes = clean_multiline(&l.notes, MAX_NOTE);
        l.aspect = clean(&l.aspect, MAX_SHORT);
        l.grain = clean(&l.grain, MAX_SHORT);
        l.lut = clean(&l.lut, MAX_SHORT);
        let mut ids: Vec<String> = Vec::new();
        l.stills.retain(|st| {
            let ok = still_ok(st) && !ids.contains(&st.id);
            if ok {
                ids.push(st.id.clone());
            }
            ok
        });
        l.stills.truncate(MAX_STILLS.min(MAX_STILLS_TOTAL.saturating_sub(total)));
        total += l.stills.len();
        for st in &mut l.stills {
            st.caption = clean(&st.caption, MAX_SHORT);
            st.rights = clean(&st.rights, MAX_RIGHTS);
            st.source = clean(&st.source, MAX_SHORT);
        }
    }
    lib.version = 1;
    lib
}

fn save_library(file: Option<&Path>, lib: &Library) {
    let Some(p) = file else { return };
    if let Ok(json) = serde_json::to_string(lib) {
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, p).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// Runs `f` on the library without saving: for reading (the panel asks every frame).
fn read_only<T>(s: &Session, f: impl FnOnce(&Library) -> T) -> T {
    let file = file_path(s);
    let mut g = lock(&s.look.library);
    let lib = g.get_or_insert_with(|| read_library(file.as_deref()));
    f(lib)
}

/// Runs `f` on the library and keeps what it did (saved to the file when `f` says it changed something).
fn write<T>(s: &Session, f: impl FnOnce(&mut Library) -> (T, bool)) -> T {
    write_to(&s.look.library, file_path(s).as_deref(), f)
}

/// [`write`] without a session, for the worker thread of a job.
fn write_to<T>(shared: &SharedLibrary, file: Option<&Path>, f: impl FnOnce(&mut Library) -> (T, bool)) -> T {
    let mut g = lock(shared);
    let lib = g.get_or_insert_with(|| read_library(file));
    let (out, changed) = f(lib);
    if changed {
        save_library(file, lib);
    }
    out
}

// ---------------------------------------------------------------------------- pictures

/// A picture of at most [`STILL_SIDE`] on its long side, by averaging blocks of pixels.
fn shrink(w: u32, h: u32, rgba: &[u8]) -> std::result::Result<(u32, u32, Vec<u8>), String> {
    shrink_to(w, h, rgba, STILL_SIDE)
}

/// [`shrink`] to `side` on the long side.
fn shrink_to(w: u32, h: u32, rgba: &[u8], side_max: u32) -> std::result::Result<(u32, u32, Vec<u8>), String> {
    let (wu, hu) = (w as usize, h as usize);
    let need = wu.checked_mul(hu).and_then(|n| n.checked_mul(4)).ok_or("the picture is too large")?;
    if w == 0 || h == 0 || rgba.len() < need {
        return Err("the picture has no pixels".into());
    }
    let side = wu.max(hu);
    let f = side.div_ceil(side_max.max(1) as usize).max(1);
    let (ow, oh) = (wu.div_ceil(f), hu.div_ceil(f));
    let mut out = Vec::with_capacity(ow * oh * 4);
    for oy in 0..oh {
        for ox in 0..ow {
            let mut acc = [0u32; 3];
            let mut n = 0u32;
            for y in oy * f..((oy + 1) * f).min(hu) {
                for x in ox * f..((ox + 1) * f).min(wu) {
                    let i = (y * wu + x) * 4;
                    if let Some(px) = rgba.get(i..i + 3) {
                        for (a, v) in acc.iter_mut().zip(px) {
                            *a += u32::from(*v);
                        }
                        n += 1;
                    }
                }
            }
            let n = n.max(1);
            out.extend([(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8, 255]);
        }
    }
    Ok((ow as u32, oh as u32, out))
}

/// The working image the colour matcher reads: linear light, opaque.
fn to_image(w: u32, h: u32, rgba: &[u8]) -> Image {
    let table: Vec<f32> = (0..=255u32).map(|v| filmcraft_color::srgb_to_linear(v as f32 / 255.0)).collect();
    let mut px = Vec::with_capacity(w as usize * h as usize * 4);
    for p in rgba.as_chunks::<4>().0.iter().take(w as usize * h as usize) {
        px.extend([table[usize::from(p[0])], table[usize::from(p[1])], table[usize::from(p[2])], 1.0]);
    }
    Image { w: w as usize, h: h as usize, px }
}

fn encode_jpeg(w: u32, h: u32, rgba: &[u8]) -> std::result::Result<Vec<u8>, String> {
    filmcraft_export::still::encode(&to_image(w, h, rgba), filmcraft_export::still::StillFormat::Jpeg, 8).map_err(|e| e.to_string())
}

/// A kept picture back as pixels: width, height, sRGB RGBA8.
fn decode_still(st: &Still) -> std::result::Result<(u32, u32, Vec<u8>), String> {
    let bytes = base64_decode(&st.jpeg).ok_or("the picture is damaged")?;
    let r = filmcraft_media::still::decode_raster("still.jpg", &bytes).map_err(|e| e.to_string())?;
    if r.width == 0 || r.height == 0 || r.width > STILL_SIDE || r.height > STILL_SIDE {
        return Err("the picture is damaged".into());
    }
    Ok((r.width, r.height, r.rgba))
}

/// The Program frame at `t`, small, as sRGB RGBA8.
fn program_rgba(s: &Session, t: Tick) -> std::result::Result<(u32, u32, Vec<u8>), String> {
    let q = s.active_sequence().ok_or("no sequence is open")?;
    let (w, h) = (q.settings.width.max(1), q.settings.height.max(1));
    if t < Tick::ZERO || t > q.duration() {
        return Err("that moment is outside the edit".into());
    }
    let scale = (STILL_SIDE as f32 / w.max(h) as f32).min(1.0);
    let _ = filmcraft_media::pending::take();
    let img = s.try_render_program_at(scale, t).map_err(|e| e.to_string())?;
    if filmcraft_media::pending::take() {
        return Err("the frame is still loading; try again in a moment".into());
    }
    let (iw, ih) = (u32::try_from(img.w).map_err(|e| e.to_string())?, u32::try_from(img.h).map_err(|e| e.to_string())?);
    Ok((iw, ih, img.over_black_rgba8()))
}

/// The numbers of a look's pictures, as a fingerprint of the picture only, so a frame and a look are
/// compared in the same words as an edit and a film.
fn picture_fingerprint(name: &str, stats: Vec<an::FrameStats>) -> std::result::Result<an::Fingerprint, String> {
    let n = stats.len().max(1);
    let (w, h) = stats.first().map_or((1, 1), |f| (f.width, f.height));
    let pictures = stats
        .into_iter()
        .enumerate()
        .map(|(i, stats)| an::fingerprint::PictureSample { time_seconds: i as f64 + 0.5, stats, camera: None, motion_energy: None })
        .collect();
    an::fingerprint(&an::ReferenceInput { name: name.to_string(), duration_seconds: n as f64, width: w, height: h, cuts: Vec::new(), pictures, sound: None })
        .map_err(|e| e.to_string())
}

/// What the pictures of a look measure on average, and the colours that recur in them.
fn measured_json(look: &Look) -> Value {
    let Ok(fp) = picture_fingerprint(&look.name, look.stills.iter().map(|s| s.stats.clone()).collect()) else { return Value::Null };
    if look.stills.is_empty() {
        return Value::Null;
    }
    to_value(&fp.picture)
}

fn still_json(st: &Still) -> Value {
    json!({
        "id": st.id, "caption": st.caption, "rights": st.rights, "source": st.source, "timeSeconds": st.time_seconds,
        "width": st.width, "height": st.height,
        "brightness": st.stats.luma_mean, "contrast": st.stats.contrast, "saturation": st.stats.saturation,
        "warmthKelvin": st.stats.colour_temperature, "grain": st.stats.grain, "aspect": st.stats.active_aspect,
        "palette": to_value(&st.stats.palette),
    })
}

fn summary(l: &Look) -> Value {
    json!({
        "id": l.id, "name": l.name, "notes": l.notes, "aspect": l.aspect, "grain": l.grain, "lut": l.lut,
        "stills": l.stills.len(), "addedUnix": l.added_unix,
        "firstStill": l.stills.first().map(|s| s.id.clone()),
        "measured": measured_json(l),
    })
}

fn full(l: &Look) -> Value {
    let mut v = summary(l);
    v["pictures"] = Value::Array(l.stills.iter().map(still_json).collect());
    v
}

pub(crate) fn find(s: &Session, id: &str) -> Option<Look> {
    read_only(s, |lib| lib.looks.iter().find(|l| l.id == id).cloned())
}

// ---------------------------------------------------------------------------- commands

fn look_id<'a>(p: &'a Value, cmd: &str) -> Result<&'a str> {
    str_p(p, "look").or_else(|| str_p(p, "id")).filter(|i| id_ok(i)).ok_or_else(|| bad(cmd, "`look` is required: a look's id from `look.list`"))
}

fn no_look(cmd: &str, id: &str) -> EngineError {
    bad(cmd, format!("there is no look {:?}; `look.list` shows what there is", id.chars().take(20).collect::<String>()))
}

fn create_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "look.create";
    let name = str_p(p, "name").map(|n| clean(n, MAX_NAME)).filter(|n| !n.is_empty()).ok_or_else(|| bad(cmd, "`name` is required"))?;
    let notes = str_p(p, "notes").map(|n| clean_multiline(n, MAX_NOTE)).unwrap_or_default();
    let made = write(s, |lib| {
        if lib.looks.len() >= MAX_LOOKS {
            return (Err(format!("the library already holds {MAX_LOOKS} looks; remove one first")), false);
        }
        lib.next_id = lib.next_id.max(lib.looks.len() as u64) + 1;
        let id = format!("l{}", lib.next_id);
        lib.looks.push(Look { id: id.clone(), name: name.clone(), notes, added_unix: crate::autosave::unix_now(), ..Default::default() });
        (Ok(id), true)
    });
    let id = made.map_err(|e| bad(cmd, e))?;
    Ok(json!({"look": id, "name": name}))
}

fn list_command(s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(read_only(s, |lib| json!({"looks": lib.looks.iter().map(summary).collect::<Vec<_>>(), "max": MAX_LOOKS, "maxStills": MAX_STILLS})))
}

fn get_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "look.get";
    let id = look_id(p, cmd)?;
    find(s, id).map(|l| full(&l)).ok_or_else(|| no_look(cmd, id))
}

fn set_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "look.set";
    let id = look_id(p, cmd)?.to_string();
    let name = match p.get("name") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_str().map(|n| clean(n, MAX_NAME)).filter(|n| !n.is_empty()).ok_or_else(|| bad(cmd, "`name` must be some text"))?),
    };
    let text = |k: &str, max: usize| -> Result<Option<String>> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_str()
                .map(|t| Some(if k == "notes" { clean_multiline(t, max) } else { clean(t, max) }))
                .ok_or_else(|| bad(cmd, format!("`{k}` must be text"))),
        }
    };
    let (notes, aspect, grain, lut) = (text("notes", MAX_NOTE)?, text("aspect", MAX_SHORT)?, text("grain", MAX_SHORT)?, text("lut", MAX_SHORT)?);
    let found = write(s, |lib| match lib.looks.iter_mut().find(|l| l.id == id) {
        Some(l) => {
            if let Some(v) = name {
                l.name = v;
            }
            if let Some(v) = notes {
                l.notes = v;
            }
            if let Some(v) = aspect {
                l.aspect = v;
            }
            if let Some(v) = grain {
                l.grain = v;
            }
            if let Some(v) = lut {
                l.lut = v;
            }
            (true, true)
        }
        None => (false, false),
    });
    if !found {
        return Err(no_look(cmd, &id));
    }
    find(s, &id).map(|l| summary(&l)).ok_or_else(|| no_look(cmd, &id))
}

fn remove_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "look.remove";
    let id = look_id(p, cmd)?.to_string();
    let removed = write(s, |lib| {
        let before = lib.looks.len();
        lib.looks.retain(|l| l.id != id);
        let gone = before != lib.looks.len();
        (gone, gone)
    });
    if removed { Ok(json!({"removed": id})) } else { Err(no_look(cmd, &id)) }
}

fn file_name_of(path: &str) -> String {
    Path::new(path).file_name().map(|n| clean(&n.to_string_lossy(), MAX_SHORT)).unwrap_or_default()
}

/// A picture on its way to being kept: its pixels, where it came from, and when in it.
struct Picture {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    source: String,
    time_seconds: Option<f64>,
    rights: String,
}

/// A file picture as asked for, checked before any work starts.
struct FileRequest {
    path: String,
    rights: String,
    seconds: Option<f64>,
}

fn file_request(p: &Value, path: &str, cmd: &str) -> Result<FileRequest> {
    if path.len() > MAX_PATH || path.contains('\0') {
        return Err(bad(cmd, "`path` is not a usable file path"));
    }
    let rights = str_p(p, "rights")
        .map(|r| clean(r, MAX_RIGHTS + 1))
        .filter(|r| !r.is_empty())
        .ok_or_else(|| bad(cmd, "`rights` is required for a file: a short note of why you may use this picture (public domain, your own, a licence)"))?;
    if rights.chars().count() > MAX_RIGHTS {
        return Err(bad(cmd, format!("`rights` is at most {MAX_RIGHTS} characters")));
    }
    let seconds = match p.get("seconds").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => match v.as_f64().filter(|f| f.is_finite() && *f >= 0.0) {
            Some(f) => Some(f),
            None => return Err(bad(cmd, "`seconds` must be a time in the file, 0 or more")),
        },
    };
    Ok(FileRequest { path: path.to_string(), rights, seconds })
}

/// Opens the file and reads one picture from it: the frame at the moment asked for (the middle of a
/// film by default; a still image has one). This is the slow part, so it runs on a worker thread.
fn read_file_picture(media: &crate::media_pool::MediaPool, services: &dyn crate::Services, req: &FileRequest) -> std::result::Result<Picture, String> {
    let name = file_name_of(&req.path);
    let src = media.open_file(&req.path, services).map_err(|e| format!("could not open {name}: {e}"))?;
    let info = src.info().clone();
    let video = info.video.clone().ok_or("that file has no picture")?;
    let duration = secs(info.duration);
    let at = match req.seconds {
        Some(v) => v.min((duration - 0.04).max(0.0)),
        None if duration > 1.0 => duration / 2.0,
        None => 0.0,
    };
    // a still image has one frame; a film is read at the moment asked for
    let moment = (duration > 0.0 && info.duration > Tick::ZERO).then_some(at);
    let scale = (f64::from(STILL_SIDE) / f64::from(video.width.max(video.height).max(1))).min(1.0) as f32;
    let f = src
        .video_frame(FrameRequest { time: Tick::from_seconds_f64(moment.unwrap_or(0.0)), scale })
        .map_err(|e| format!("could not read a picture from {name}: {e}"))?;
    Ok(Picture {
        w: f.width,
        h: f.height,
        rgba: f.to_rgba8(),
        source: name,
        time_seconds: moment.filter(|_| info.duration > Tick(TICKS_PER_SECOND / 10)),
        rights: req.rights.clone(),
    })
}

/// A picture made small, measured and encoded: a [`Still`] with no id yet.
fn make_still(pic: Picture, caption: String) -> std::result::Result<Still, String> {
    let (w, h, rgba) = shrink(pic.w, pic.h, &pic.rgba)?;
    let stats = an::analyze_rgba(w, h, &rgba).map_err(|e| e.to_string())?;
    let jpeg = base64_encode(&encode_jpeg(w, h, &rgba)?);
    if jpeg.len() > MAX_JPEG_B64 {
        return Err("that picture is too detailed to keep at this size".into());
    }
    Ok(Still {
        id: String::new(),
        caption,
        rights: pic.rights,
        source: pic.source,
        time_seconds: pic.time_seconds,
        width: w,
        height: h,
        jpeg,
        stats,
        added_unix: crate::autosave::unix_now(),
    })
}

/// Puts a picture on a look and gives it its id. The limits are checked again here: the look may have
/// changed while a job was reading the file.
fn store_still(shared: &SharedLibrary, file: Option<&Path>, look: &str, mut still: Still) -> std::result::Result<String, String> {
    write_to(shared, file, |lib| {
        let total: usize = lib.looks.iter().map(|l| l.stills.len()).sum();
        if total >= MAX_STILLS_TOTAL {
            return (Err(format!("the library already holds {MAX_STILLS_TOTAL} pictures; remove some first")), false);
        }
        lib.next_id = lib.next_id.max(lib.looks.len() as u64 + total as u64) + 1;
        let sid = format!("s{}", lib.next_id);
        match lib.looks.iter_mut().find(|l| l.id == look) {
            Some(l) if l.stills.len() < MAX_STILLS => {
                still.id = sid.clone();
                l.stills.push(still);
                (Ok(sid), true)
            }
            Some(_) => (Err(format!("a look holds at most {MAX_STILLS} pictures; remove one first")), false),
            None => (Err("that look was removed".to_string()), false),
        }
    })
}

/// Most finished jobs' answers kept.
const MAX_DONE: usize = 50;

fn add_still_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "look.addStill";
    let id = look_id(p, cmd)?.to_string();
    let (count, total) =
        read_only(s, |lib| (lib.looks.iter().find(|l| l.id == id).map(|l| l.stills.len()), lib.looks.iter().map(|l| l.stills.len()).sum::<usize>()));
    let count = count.ok_or_else(|| no_look(cmd, &id))?;
    if count >= MAX_STILLS {
        return Err(bad(cmd, format!("a look holds at most {MAX_STILLS} pictures; remove one first")));
    }
    if total >= MAX_STILLS_TOTAL {
        return Err(bad(cmd, format!("the library already holds {MAX_STILLS_TOTAL} pictures; remove some first")));
    }
    let caption = str_p(p, "caption").map(|c| clean(c, MAX_SHORT)).unwrap_or_default();
    let file = file_path(s);
    match str_p(p, "path").map(str::trim).filter(|t| !t.is_empty()) {
        // the Program frame is quick and needs the session: done here
        None => {
            let t = time_p(s, p, "").unwrap_or_else(|| s.playhead());
            let (w, h, rgba) = program_rgba(s, t).map_err(|e| bad(cmd, e))?;
            let rights = str_p(p, "rights").map(|r| clean(r, MAX_RIGHTS)).filter(|r| !r.is_empty()).unwrap_or_else(|| "A frame of my own edit".to_string());
            let still = make_still(Picture { w, h, rgba, source: "Program frame".to_string(), time_seconds: Some(secs(t)), rights }, caption)
                .map_err(|e| bad(cmd, e))?;
            let sid = store_still(&s.look.library, file.as_deref(), &id, still).map_err(|e| bad(cmd, e))?;
            Ok(json!({"look": id, "still": sid}))
        }
        // a file is opened and read on a worker thread, so the app never waits for it
        Some(path) => {
            let req = file_request(p, path, cmd)?;
            start_file_job(s, p, id, req, caption, file)
        }
    }
}

/// Starts the job that reads a file picture and keeps it. With `wait: true` (and on the web, which has
/// no threads) it runs here and returns the picture's id.
fn start_file_job(s: &mut Session, p: &Value, look: String, req: FileRequest, caption: String, file: Option<PathBuf>) -> Result<Value> {
    let cmd = "look.addStill";
    let job_id = s.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
    let job = crate::Job { id: job_id, label: format!("Add picture {}", file_name_of(&req.path)), progress: Default::default(), result: Default::default() };
    let (prog, res) = (job.progress.clone(), job.result.clone());
    let (media, services) = (s.media.clone(), s.services.clone());
    let (done, library) = (s.look.done.clone(), s.look.library.clone());
    prog.total.store(3, std::sync::atomic::Ordering::Relaxed);
    let run = move || {
        use std::sync::atomic::Ordering::Relaxed;
        let stopped = || prog.cancel.load(Relaxed);
        let step = |text: &str| {
            *lock(&prog.status) = text.to_string();
        };
        let t0 = web_time::Instant::now();
        step("Opening the file");
        let outcome = read_file_picture(&media, &*services, &req).and_then(|pic| {
            prog.done.fetch_add(2, Relaxed);
            if stopped() {
                return Err("stopped".to_string());
            }
            step("Measuring the picture");
            let still = make_still(pic, caption.clone())?;
            if stopped() {
                return Err("stopped".to_string());
            }
            store_still(&library, file.as_deref(), &look, still)
        });
        let r = match outcome {
            Ok(sid) => {
                prog.done.store(3, Relaxed);
                step("Added");
                lock(&done).insert(job_id, Ok(json!({"look": look, "still": sid})));
                Ok(filmcraft_export::Report {
                    path: String::new(),
                    frames: 1,
                    seconds: t0.elapsed().as_secs_f64(),
                    bytes: 0,
                    render_fps: 0.0,
                    extra_files: Vec::new(),
                })
            }
            Err(e) => {
                step(if e == "stopped" { "Stopped: nothing was kept" } else { &e });
                lock(&done).insert(job_id, Err(e.clone()));
                Err(e)
            }
        };
        {
            let mut d = lock(&done);
            while d.len() > MAX_DONE {
                let Some(first) = d.keys().next().copied() else { break };
                d.remove(&first);
            }
        }
        *lock(&res) = Some(r);
        prog.finished.store(true, Relaxed);
    };
    let run = crate::export_tools::guard_job(job.progress.clone(), job.result.clone(), run);
    s.jobs.push(job);
    if crate::commands::bool_p(p, "wait").unwrap_or(false) || cfg!(target_arch = "wasm32") {
        run();
        return match lock(&s.look.done).get(&job_id).cloned() {
            Some(Ok(v)) => Ok(json!({"job": job_id, "look": v["look"], "still": v["still"]})),
            Some(Err(e)) => Err(bad(cmd, e)),
            None => Err(bad(cmd, "the job ended without an answer")),
        };
    }
    std::thread::Builder::new().name("edith-look".into()).spawn(run).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(json!({"job": job_id}))
}

/// Where a `look.addStill` job is: running, done (with the picture's id) or failed.
fn result_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "look.result";
    let id = crate::commands::u64_p(p, "job").ok_or_else(|| bad(cmd, "`job` is required: the number `look.addStill` returned"))?;
    if !s.jobs.iter().any(|j| j.id == id) && !lock(&s.look.done).contains_key(&id) {
        return Err(bad(cmd, format!("there is no job {id}")));
    }
    Ok(match lock(&s.look.done).get(&id) {
        Some(Ok(v)) => json!({"state": "done", "look": v["look"], "still": v["still"]}),
        Some(Err(e)) => json!({"state": "failed", "error": e}),
        None => json!({"state": "running"}),
    })
}

fn set_still_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "look.setStill";
    let id = look_id(p, cmd)?.to_string();
    let still = str_p(p, "still").filter(|i| id_ok(i)).ok_or_else(|| bad(cmd, "`still` is required: a picture's id from `look.get`"))?.to_string();
    let caption = str_p(p, "caption").map(|c| clean(c, MAX_SHORT)).ok_or_else(|| bad(cmd, "`caption` is required (it may be empty)"))?;
    let done = write(s, |lib| {
        let st = lib.looks.iter_mut().find(|l| l.id == id).and_then(|l| l.stills.iter_mut().find(|x| x.id == still));
        match st {
            Some(st) => {
                st.caption = caption.clone();
                (true, true)
            }
            None => (false, false),
        }
    });
    if done { Ok(json!({"look": id, "still": still, "caption": caption})) } else { Err(bad(cmd, "there is no such picture in that look")) }
}

fn remove_still_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "look.removeStill";
    let id = look_id(p, cmd)?.to_string();
    let still = str_p(p, "still").filter(|i| id_ok(i)).ok_or_else(|| bad(cmd, "`still` is required: a picture's id from `look.get`"))?.to_string();
    let done = write(s, |lib| match lib.looks.iter_mut().find(|l| l.id == id) {
        Some(l) => {
            let before = l.stills.len();
            l.stills.retain(|x| x.id != still);
            let gone = before != l.stills.len();
            (gone, gone)
        }
        None => (false, false),
    });
    if done { Ok(json!({"removed": still})) } else { Err(bad(cmd, "there is no such picture in that look")) }
}

fn find_still(s: &Session, look: &str, still: &str) -> Option<Still> {
    read_only(s, |lib| lib.looks.iter().find(|l| l.id == look).and_then(|l| l.stills.iter().find(|x| x.id == still)).cloned())
}

fn picture_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "look.picture";
    let id = look_id(p, cmd)?;
    let still = str_p(p, "still").filter(|i| id_ok(i)).ok_or_else(|| bad(cmd, "`still` is required"))?;
    let st = find_still(s, id, still).ok_or_else(|| bad(cmd, "there is no such picture in that look"))?;
    Ok(json!({"look": id, "still": still, "width": st.width, "height": st.height, "jpegBase64": st.jpeg}))
}

/// The Program frame against a look, in the words a comparison of an edit and a film uses. The frame
/// is measured the way the look's pictures were: small.
pub(crate) fn compare(s: &Session, look: &Look, t: Tick, cmd: &str) -> Result<Value> {
    if look.stills.is_empty() {
        return Err(bad(cmd, "that look has no pictures yet"));
    }
    let (w, h, rgba) = program_rgba(s, t).map_err(|e| bad(cmd, e))?;
    let (w, h, rgba) = shrink(w, h, &rgba).map_err(|e| bad(cmd, e))?;
    let yours = an::analyze_rgba(w, h, &rgba).map_err(|e| bad(cmd, e.to_string()))?;
    let yours = picture_fingerprint("Program frame", vec![yours]).map_err(|e| bad(cmd, e))?;
    let theirs = picture_fingerprint(&look.name, look.stills.iter().map(|x| x.stats.clone()).collect()).map_err(|e| bad(cmd, e))?;
    let c = an::compare(&yours, &theirs);
    let rows: Vec<&an::compare::Row> = c.rows.iter().filter(|r| r.group == "picture").collect();
    let close = rows.iter().filter(|r| r.verdict == an::compare::Verdict::Close).count();
    Ok(json!({
        "look": {"id": look.id, "name": look.name},
        "time": secs(t),
        "close": close, "differ": rows.len() - close,
        "rows": to_value(&rows),
        "notes": {"aspect": look.aspect, "grain": look.grain, "lut": look.lut},
    }))
}

fn compare_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "look.compare";
    let id = look_id(p, cmd)?;
    let look = find(s, id).ok_or_else(|| no_look(cmd, id))?;
    let t = time_p(s, p, "").unwrap_or_else(|| s.playhead());
    compare(s, &look, t, cmd)
}

fn match_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "look.match";
    let id = look_id(p, cmd)?.to_string();
    let look = find(s, &id).ok_or_else(|| no_look(cmd, &id))?;
    let st = match str_p(p, "still") {
        Some(sid) => look.stills.iter().find(|x| x.id == sid).ok_or_else(|| bad(cmd, "there is no such picture in that look"))?,
        None => match look.stills.as_slice() {
            [only] => only,
            [] => return Err(bad(cmd, "that look has no pictures yet")),
            _ => return Err(bad(cmd, "this look has several pictures: pass `still`, one of the ids `look.get` lists")),
        },
    };
    let (w, h, rgba) = decode_still(st).map_err(|e| bad(cmd, e))?;
    let mut out = crate::color::match_clip(s, p, cmd, "Match to Look", Some(to_image(w, h, &rgba)))?;
    out["look"] = json!(id);
    out["still"] = json!(st.id);
    Ok(out)
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
        spec("look.create", "New Look", r#"{"name":str,"notes":str?}"#, create_command),
        spec("look.list", "List Looks", "{}", list_command),
        spec("look.get", "Get Look", r#"{"id":str}"#, get_command),
        spec("look.set", "Edit Look", r#"{"id":str,"name":str?,"notes":str?,"aspect":str?,"grain":str?,"lut":str?}"#, set_command),
        spec("look.remove", "Remove Look", r#"{"id":str}"#, remove_command),
        spec(
            "look.addStill",
            "Add Picture to Look",
            r#"{"look":str,"path":str?,"seconds":n?,"rights":str?,"caption":str?,"wait":bool=false,"time":ticks?|"frame":n?|"timecode":str?}"#,
            add_still_command,
        ),
        spec("look.result", "Look Picture Job Result", r#"{"job":id}"#, result_command),
        spec("look.setStill", "Caption Look Picture", r#"{"look":str,"still":str,"caption":str}"#, set_still_command),
        spec("look.removeStill", "Remove Look Picture", r#"{"look":str,"still":str}"#, remove_still_command),
        spec("look.picture", "Look Picture", r#"{"look":str,"still":str}"#, picture_command),
        spec("look.compare", "Compare With Look", r#"{"look":str,"time":ticks?|"frame":n?|"seconds":n?|"timecode":str?}"#, compare_command),
        spec("look.match", "Match to Look", r#"{"look":str,"still":str?,"clip":id?,"faceDetection":bool=true}"#, match_command),
    ]
}

// ---------------------------------------------------------------------------- the assistant

use crate::assistant_tools::ToolDef;
use edith_ai::ToolOutput;

/// Pictures `look_see` sends in one answer.
const MAX_SEEN: usize = 6;
/// Pictures a `look` view draws.
const MAX_DRAWN: usize = 8;
/// Rows a `look` view's comparison shows.
const MAX_VIEW_ROWS: usize = 6;
/// The long side of the Program frame kept inside a `look` view.
const VIEW_SIDE: u32 = 256;

fn hex(rgb: [f32; 3]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(rgb[0]), b(rgb[1]), b(rgb[2]))
}

fn percent(v: f32) -> String {
    format!("{:.0}%", f64::from(v) * 100.0)
}

fn warmth_word(kelvin: Option<f32>) -> String {
    match kelvin {
        Some(k) if k < 4500.0 => format!("warm ({k:.0} K)"),
        Some(k) if k > 6500.0 => format!("cool ({k:.0} K)"),
        Some(k) => format!("neutral ({k:.0} K)"),
        None => "no colour cast".into(),
    }
}

fn picture_line(st: &Still) -> String {
    let f = &st.stats;
    format!(
        "- {}{}: from {}{}; brightness {}, contrast {}, saturation {}, {}, grain {:.3}; palette {}; rights note: {}",
        st.id,
        if st.caption.is_empty() { String::new() } else { format!(" “{}”", st.caption) },
        st.source,
        st.time_seconds.map_or(String::new(), |t| format!(" at {}", clock(t))),
        percent(f.luma_mean),
        percent(f.contrast),
        percent(f.saturation),
        warmth_word(f.colour_temperature),
        f.grain,
        f.palette.iter().map(|c| format!("{} {}", hex(c.rgb), percent(c.share))).collect::<Vec<_>>().join(" "),
        st.rights
    )
}

fn clock(seconds: f64) -> String {
    let n = seconds.max(0.0) as u64;
    if n >= 3600 { format!("{}:{:02}:{:02}", n / 3600, (n / 60) % 60, n % 60) } else { format!("{}:{:02}", n / 60, n % 60) }
}

fn look_text(l: &Look) -> String {
    let mut out = format!("Look {} “{}”: {} picture{}.\n", l.id, l.name, l.stills.len(), if l.stills.len() == 1 { "" } else { "s" });
    for (what, v) in [("Notes", &l.notes), ("Picture shape wanted", &l.aspect), ("Grain wanted", &l.grain), ("LUT", &l.lut)] {
        if !v.is_empty() {
            out.push_str(&format!("{what}: {v}\n"));
        }
    }
    if let Ok(fp) = picture_fingerprint(&l.name, l.stills.iter().map(|x| x.stats.clone()).collect())
        && let Some(p) = fp.picture.as_ref().filter(|_| !l.stills.is_empty())
    {
        out.push_str(&format!(
            "On average: brightness {}, contrast {}, saturation {}, {}, grain {:.3}, picture shape {:.2}:1; recurring colours {}.\n",
            percent(p.luma_mean),
            percent(p.contrast),
            percent(p.saturation),
            warmth_word(p.colour_temperature),
            p.grain,
            p.active_aspect,
            p.palette.iter().map(|c| format!("{} {}", hex(c.rgb), percent(c.share))).collect::<Vec<_>>().join(" ")
        ));
        out.push_str("(Measured on pictures kept at most 640 px wide, so grain reads lower than in the original.)\n");
    }
    for st in &l.stills {
        out.push_str(&picture_line(st));
        out.push('\n');
    }
    out
}

fn input_object(p: &Value) -> Value {
    if p.is_object() { p.clone() } else { json!({}) }
}

fn pick_look(s: &Session, p: &Value, tool: &str) -> std::result::Result<Look, String> {
    let id = str_p(p, "id").or_else(|| str_p(p, "look")).filter(|i| id_ok(i));
    match id {
        Some(id) => find(s, id).ok_or_else(|| format!("there is no look {id:?}; look_list shows what there is")),
        None => {
            let all = read_only(s, |lib| lib.looks.clone());
            match all.as_slice() {
                [only] => Ok(only.clone()),
                [] => Err("there are no looks yet. The person makes one in the Look Bible panel from pictures they have the right to use.".into()),
                _ => Err(format!("{tool} needs `id`: one of the looks look_list shows")),
            }
        }
    }
}

fn look_list_tool(s: &Session, _p: &Value) -> ToolOutput {
    let all = read_only(s, |lib| lib.looks.clone());
    if all.is_empty() {
        return ToolOutput::ok(
            "There are no looks yet. A look is a small board of pictures the person keeps to say how a film should feel (each with a note of why it may be used), with its numbers measured.",
        );
    }
    let mut out = String::new();
    for l in &all {
        out.push_str(&format!("- {} “{}”: {} picture{}", l.id, l.name, l.stills.len(), if l.stills.len() == 1 { "" } else { "s" }));
        for (what, v) in [("aspect", &l.aspect), ("grain", &l.grain), ("LUT", &l.lut)] {
            if !v.is_empty() {
                out.push_str(&format!("; {what}: {v}"));
            }
        }
        if !l.notes.is_empty() {
            out.push_str(&format!("; notes: {}", l.notes.lines().next().unwrap_or("")));
        }
        out.push('\n');
    }
    ToolOutput::ok(out)
}

fn look_get_tool(s: &Session, p: &Value) -> ToolOutput {
    match pick_look(s, &input_object(p), "look_get") {
        Ok(l) => ToolOutput::ok(look_text(&l)),
        Err(e) => ToolOutput::error(e),
    }
}

fn look_see_tool(s: &Session, p: &Value) -> ToolOutput {
    let p = input_object(p);
    let l = match pick_look(s, &p, "look_see") {
        Ok(l) => l,
        Err(e) => return ToolOutput::error(e),
    };
    if l.stills.is_empty() {
        return ToolOutput::error("that look has no pictures yet");
    }
    let chosen: Vec<&Still> = match p.get("still").filter(|v| !v.is_null()) {
        Some(v) => match v.as_str().and_then(|sid| l.stills.iter().find(|x| x.id == sid)) {
            Some(st) => vec![st],
            None => {
                return ToolOutput::error(format!(
                    "there is no such picture; the look's pictures are {}",
                    l.stills.iter().map(|x| x.id.as_str()).collect::<Vec<_>>().join(", ")
                ));
            }
        },
        // up to six, spread over the look when it has more
        None => {
            let n = l.stills.len();
            let take = n.min(MAX_SEEN);
            (0..take).filter_map(|i| l.stills.get(i * n / take)).collect()
        }
    };
    let mut images = Vec::new();
    let mut lines = Vec::new();
    for st in &chosen {
        let Some(bytes) = base64_decode(&st.jpeg) else { continue };
        images.push(edith_ai::Image::from_bytes("image/jpeg", &bytes));
        lines.push(format!(
            "picture {} of {}: {}{}",
            images.len(),
            chosen.len(),
            st.id,
            if st.caption.is_empty() { String::new() } else { format!(" “{}”", st.caption) }
        ));
    }
    if images.is_empty() {
        return ToolOutput::error("those pictures are damaged");
    }
    let mut out = ToolOutput::ok(format!("The look “{}”, in order: {}. The pictures follow.", l.name, lines.join("; ")));
    out.images = images;
    out
}

fn compare_text(c: &Value) -> String {
    let mut out = format!(
        "The Program frame at {} against the look “{}”: {} of the picture rows are close, {} differ.\n",
        clock(c["time"].as_f64().unwrap_or(0.0)),
        c["look"]["name"].as_str().unwrap_or(""),
        c["close"].as_u64().unwrap_or(0),
        c["differ"].as_u64().unwrap_or(0)
    );
    for r in c["rows"].as_array().into_iter().flatten() {
        out.push_str(&format!("- [{}] {}\n", r["verdict"].as_str().unwrap_or(""), r["words"].as_str().unwrap_or("")));
    }
    for (what, k) in [("picture shape wanted", "aspect"), ("grain wanted", "grain"), ("LUT", "lut")] {
        if let Some(v) = c["notes"][k].as_str().filter(|v| !v.is_empty()) {
            out.push_str(&format!("The look's note on {what}: {v}\n"));
        }
    }
    out
}

fn look_compare_tool(s: &Session, p: &Value) -> ToolOutput {
    let p = input_object(p);
    let l = match pick_look(s, &p, "look_compare") {
        Ok(l) => l,
        Err(e) => return ToolOutput::error(e),
    };
    let Some(q) = s.active_sequence() else { return ToolOutput::error("no sequence is open") };
    let t = match p.get("time").filter(|v| !v.is_null()).map(|v| crate::assistant_tools::parse_time(v, q, "time")).transpose() {
        Ok(v) => v.unwrap_or_else(|| s.playhead()),
        Err(e) => return ToolOutput::error(e),
    };
    match compare(s, &l, t, "look_compare") {
        Ok(c) => ToolOutput::ok(compare_text(&c)),
        Err(e) => ToolOutput::error(e.to_string()),
    }
}

/// The `look` view of `show`: the look's pictures, its colours and numbers, and, when an edit is open,
/// the Program frame beside it with what differs. The pictures are drawn from the library by id; the
/// Program frame is kept small inside the view, since it changes as the edit does.
pub(crate) fn view(s: &Session, o: &serde_json::Map<String, Value>, caption: Option<String>) -> std::result::Result<ToolOutput, String> {
    let params = Value::Object(o.clone());
    let l = pick_look(s, &json!({"id": o.get("look").cloned().unwrap_or(Value::Null)}), "show")?;
    if l.stills.is_empty() {
        return Err("that look has no pictures to show yet".into());
    }
    let wanted: Option<Vec<String>> = match o.get("stills").filter(|v| !v.is_null()) {
        Some(v) => Some(
            v.as_array()
                .ok_or("`stills` must be a list of picture ids")?
                .iter()
                .map(|x| x.as_str().map(str::to_string).ok_or("`stills` must be a list of picture ids (text)"))
                .collect::<std::result::Result<_, _>>()?,
        ),
        None => None,
    };
    let chosen: Vec<&Still> = match &wanted {
        Some(ids) => ids
            .iter()
            .take(MAX_DRAWN)
            .map(|id| {
                l.stills.iter().find(|x| &x.id == id).ok_or_else(|| {
                    format!(
                        "there is no picture {id:?} in that look; its pictures are {}",
                        l.stills.iter().map(|x| x.id.as_str()).collect::<Vec<_>>().join(", ")
                    )
                })
            })
            .collect::<std::result::Result<_, _>>()?,
        None => l.stills.iter().take(MAX_DRAWN).collect(),
    };
    let measured = picture_fingerprint(&l.name, l.stills.iter().map(|x| x.stats.clone()).collect()).ok().and_then(|f| f.picture);
    // the Program frame, if there is an edit to look at (a look is still worth showing without one)
    let yours = if s.active_sequence().is_some() {
        let t = match params.get("time").filter(|v| !v.is_null()) {
            Some(v) => s.active_sequence().and_then(|q| crate::assistant_tools::parse_time(v, q, "time").ok()).unwrap_or_else(|| s.playhead()),
            None => s.playhead(),
        };
        match compare(s, &l, t, "show") {
            Ok(c) => {
                let jpeg = program_rgba(s, t)
                    .and_then(|(w, h, rgba)| shrink_to(w, h, &rgba, VIEW_SIDE))
                    .and_then(|(w, h, rgba)| encode_jpeg(w, h, &rgba))
                    .map(|b| base64_encode(&b))
                    .ok();
                let mut rows: Vec<Value> = c["rows"].as_array().cloned().unwrap_or_default();
                rows.sort_by_key(|r| r["verdict"] == "close");
                rows.truncate(MAX_VIEW_ROWS);
                Some(
                    json!({"time": Tick::from_seconds_f64(c["time"].as_f64().unwrap_or(0.0)).0, "jpegBase64": jpeg, "rows": rows, "close": c["close"], "differ": c["differ"], "text": compare_text(&c)}),
                )
            }
            Err(_) => None,
        }
    } else {
        None
    };
    let v = json!({
        "view": "look", "caption": caption,
        "look": {"id": l.id, "name": l.name, "notes": l.notes, "aspect": l.aspect, "grain": l.grain, "lut": l.lut, "count": l.stills.len()},
        "pictures": chosen.iter().map(|x| json!({"id": x.id, "caption": x.caption, "source": x.source})).collect::<Vec<_>>(),
        "palette": measured.as_ref().map(|m| m.palette.iter().map(|c| json!({"rgb": c.rgb, "share": c.share})).collect::<Vec<_>>()).unwrap_or_default(),
        "measured": measured.as_ref().map(|m| json!({"brightness": m.luma_mean, "contrast": m.contrast, "saturation": m.saturation, "warmthKelvin": m.colour_temperature, "grain": m.grain, "aspect": m.active_aspect})),
        "yours": yours,
    });
    let compared = yours_summary(&v);
    Ok(ToolOutput::ok(format!(
        "Shown to the person: {} of the {} picture{} of the look “{}”, its recurring colours and numbers{}. Say what the look is about and what it asks of the edit; do not read the numbers out.{}",
        chosen.len(),
        l.stills.len(),
        if l.stills.len() == 1 { "" } else { "s" },
        l.name,
        if v["yours"].is_null() { String::new() } else { ", and the Program frame beside it with what differs".to_string() },
        compared
    ))
    .with_view(v))
}

fn yours_summary(v: &Value) -> String {
    match v["yours"]["text"].as_str() {
        Some(t) => format!("\nWhat differs, from the measurements:\n{t}"),
        None => String::new(),
    }
}

pub(crate) fn tools() -> Vec<ToolDef> {
    use crate::assistant_tools::def;
    let id = json!({"id": {"type": "string", "description": "A look id from look_list, such as l1. Default: the only look, if there is just one."}});
    vec![
        def(
            "look_list",
            "The looks the person keeps: boards of small pictures that say how a film should feel (each picture with a note of why it may be used), with what they want written down (picture shape, grain, LUT). Empty if there are none.",
            json!({}),
            look_list_tool,
        ),
        def(
            "look_get",
            "One look in words and numbers: the person's notes, the picture shape, grain and LUT they want, the average brightness, contrast, saturation, colour warmth and recurring colours of its pictures, and each picture's own numbers, caption and where it came from. To actually see the pictures use look_see.",
            id.clone(),
            look_get_tool,
        ),
        def(
            "look_see",
            "The pictures of a look, so you can see them: up to six, spread over the look, or one with still. Costs more than text. Look at them before you judge a frame against the look, and describe what you see (light, colour, framing, mood), not only the numbers. Not available to models that cannot see pictures.",
            {
                let mut p = id.clone();
                if let Some(o) = p.as_object_mut() {
                    o.insert("still".into(), json!({"type": "string", "description": "One picture's id from look_get. Default: up to six pictures."}));
                }
                p
            },
            look_see_tool,
        ),
        def(
            "look_compare",
            "The Program frame at one moment against a look, row by row with a verdict and a sentence each (brightness, contrast, saturation, colour warmth, grain, picture shape), plus the look's notes on grain, shape and LUT. Combine it with frame_view and look_see to describe how the shot differs from the look, and say what would bring it closer; the person applies a match with Match to Look.",
            {
                let mut p = id;
                if let Some(o) = p.as_object_mut() {
                    o.insert(
                        "time".into(),
                        json!({"type": "string", "description": "The moment: seconds (12.5) or timecode in the sequence's rate. Default: the playhead."}),
                    );
                }
                p
            },
            look_compare_tool,
        ),
    ]
}
