//! References (fork roadmap F12): films you admire, measured into style fingerprints.
//!
//! | command | does |
//! |---|---|
//! | `reference.measure {path, name?, sampleFps?, pictures?, sensitivity?, wait?}` | measure a film file into a fingerprint (`edith-analysis`), as a background job you can watch and stop; `wait: true` returns the fingerprint |
//! | `reference.result {job}` | where a measuring job is: running, done (with the fingerprint) or failed |
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
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use edith_analysis as an;
use filmcraft_media::{FrameRequest, SharedSource};
use filmcraft_render::scene::{SceneDetector, SceneOptions};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use crate::analysis::{frame_at, lock, motion_between, secs, to_value};
use crate::commands::{CommandSpec, always, bad, bool_p, f64_p, str_p, u64_p};
use crate::{EngineError, Result, Session};

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

/// What the session keeps: where finished and failed measuring jobs left their answers.
#[derive(Clone, Default)]
pub struct Hub {
    done: Arc<Mutex<BTreeMap<u64, std::result::Result<Value, String>>>>,
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

fn measure_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "reference.measure";
    let path = path_param(p, cmd)?.to_string();
    let name = name_of(p, &path);
    let options = Options::from_params(p);
    let id = s.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
    let job = crate::Job { id, label: format!("Measure reference {name}"), progress: Default::default(), result: Default::default() };
    let (prog, res) = (job.progress.clone(), job.result.clone());
    let (media, services) = (s.media.clone(), s.services.clone());
    let done = s.reference.done.clone();
    let run = move || {
        let t0 = web_time::Instant::now();
        *lock(&prog.status) = "Opening the file".into();
        let outcome = media
            .open_file(&path, &*services)
            .map_err(|e| {
                format!("could not open {}: {e}", std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
            })
            .and_then(|src| measure(&src, &name, &options, &prog));
        let seconds = t0.elapsed().as_secs_f64();
        let r = match outcome {
            Ok(fp) => {
                lock(&done).insert(id, Ok(to_value(&fp)));
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
            Some(Ok(fp)) => out["fingerprint"] = fp,
            Some(Err(e)) => return Err(EngineError::Other(e)),
            None => return Err(EngineError::Other("the measurement ended without an answer".into())),
        }
    } else {
        std::thread::Builder::new().name("edith-reference".into()).spawn(run).map_err(|e| EngineError::Other(e.to_string()))?;
    }
    Ok(out)
}

fn result_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "reference.result";
    let id = u64_p(p, "job").ok_or_else(|| bad(cmd, "`job` is required: the number `reference.measure` returned"))?;
    if !s.jobs.iter().any(|j| j.id == id) && !lock(&s.reference.done).contains_key(&id) {
        return Err(bad(cmd, format!("there is no job {id}")));
    }
    Ok(match lock(&s.reference.done).get(&id) {
        Some(Ok(fp)) => json!({"state": "done", "fingerprint": fp}),
        Some(Err(e)) => json!({"state": "failed", "error": e}),
        None => json!({"state": "running"}),
    })
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
        spec("reference.result", "Reference Result", r#"{"job":id}"#, result_command),
    ]
}
