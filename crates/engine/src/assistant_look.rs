//! `media_look` and `media_listen` (fork roadmap F29): Edith sees a stock clip, and hears a stock sound, before she
//! offers it.
//!
//! - `media_look {candidates, frames?}` (only offered to a model that can see): up to six results from `media_search`,
//!   each as one to three small pictures taken from inside the clip (the site's own stills, else its thumbnail), with a
//!   line saying what it is. Fetched in the background, a few hundred kilobytes at most per picture, kept for the
//!   session, and never put in the project.
//! - `media_listen {candidate}`: she cannot hear, so the sound is measured: its preview is fetched to a temporary file
//!   (never the project, deleted straight after), decoded and run through the same sound analysis the app uses on a
//!   clip, and described in plain words (what kind of sound it is, whether it has a steady pulse, how loud, how it
//!   starts and ends).
//!
//! Both are read-only. They run on the session's thread only as far as is quick; fetching is on a worker thread that
//! answers the model itself, and the one slow step that touches the session (decoding the sound) is done by [`pump`].

// Without the `net` feature the fetching is not built; what remains is unused.
#![cfg_attr(not(feature = "net"), allow(dead_code, unused_imports))]

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};

use edith_ai::ToolOutput;
use edith_analysis as an;
use serde_json::{Value, json};

use crate::Session;
use crate::assistant::ToolRequest;
use crate::assistant_cards::candidate_line;
use crate::assistant_tools::{ToolDef, def};

/// Results looked at in one call.
const MAX_LOOK: usize = 6;
/// Pictures taken from inside one clip.
const MAX_FRAMES: usize = 3;
/// Seconds a look may take in all before what has come is sent.
const LOOK_WAIT: u64 = 40;
/// Seconds a sound may take to fetch before it is given up on.
const LISTEN_WAIT: u64 = 90;
/// The most of a sound that is fetched, bytes.
const LISTEN_MAX_BYTES: u64 = 12 * 1024 * 1024;
/// The most of a sound that is measured, seconds.
const LISTEN_MAX_SECONDS: f64 = 90.0;
/// Looks kept for the session.
const CACHE_MAX: usize = 64;

/// Pictures already fetched this session (by result and frame count), and the sounds being fetched.
#[derive(Default)]
pub(crate) struct Looks {
    #[cfg(feature = "net")]
    cache: Arc<Mutex<std::collections::BTreeMap<String, Vec<edith_sources::Picture>>>>,
    listens: Vec<PendingListen>,
}

pub(crate) struct PendingListen {
    pub(crate) reply: SyncSender<ToolOutput>,
    pub(crate) since: web_time::Instant,
    /// The result in a line, said again with what it sounds like.
    pub(crate) line: String,
    pub(crate) rx: Receiver<std::result::Result<PathBuf, String>>,
    /// The file is a copy made for listening: delete it afterwards (a sound in the person's folders is never touched).
    pub(crate) delete: bool,
}

#[cfg(test)]
impl Looks {
    pub(crate) fn push_listen_for_tests(&mut self, p: PendingListen) {
        self.listens.push(p);
    }

    pub(crate) fn waiting_for_tests(&self) -> usize {
        self.listens.len()
    }
}

fn not_here(_: &Session, _: &Value) -> ToolOutput {
    ToolOutput::error("this tool works inside a conversation only")
}

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![
        def(
            "media_look",
            "See results from media_search before you offer them: up to 6 at once, each as 1 to 3 small pictures taken from inside the clip, with a line saying what it is. A picture is one moment, so judge what the clip shows, not how it moves. Look only at the shortlist for the idea you are working on, not at every result; if none fits, search again in other words rather than offering the nearest. Pictures are shown to you only: nothing is downloaded or added to the project.",
            json!({
                "candidates": {"type": "array", "items": {"type": "string"}, "description": "1 to 6 result ids from media_search, such as pexels:123."},
                "frames": {"type": "integer", "description": "Pictures per clip, 1 to 3 (default 2); more only when the clip's content changes a lot."},
            }),
            not_here,
        ),
        def(
            "media_listen",
            "Hear a sound from media_search, which you cannot do by ear: it is fetched and measured, and you are told in words and numbers what it is like: music, ambience or speech; whether it has a steady pulse and at what tempo; how loud and how steady; how it starts and ends; whether it could be repeated to fill a stretch. Use it on the one or two sounds you are about to offer, not on every result. Nothing is downloaded into the project.",
            json!({
                "candidate": {"type": "string", "description": "A sound result id from media_search, such as openverse:... or freesound:77."},
            }),
            not_here,
        ),
    ]
}

/// Starts `media_look` or `media_listen`. `Some` is the answer when it is known now (a refusal); `None` when a worker
/// will answer.
pub(crate) fn serve(s: &mut Session, req: &ToolRequest) -> Option<ToolOutput> {
    match req.name.as_str() {
        "media_look" => look(s, req),
        _ => listen(s, req),
    }
}

// ---------------------------------------------------------------------------- media_look

fn ids_of(input: &Value) -> std::result::Result<Vec<String>, String> {
    let list: Vec<&str> = match input.get("candidates") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => input.get("candidate").and_then(Value::as_str).into_iter().collect(),
    };
    let mut ids: Vec<String> = Vec::new();
    for id in list {
        let id = id.trim();
        if !id.is_empty() && id.chars().count() <= 200 && !ids.iter().any(|x| x == id) {
            ids.push(id.to_string());
        }
    }
    if ids.is_empty() {
        return Err("`candidates` must list 1 to 6 result ids from media_search (such as pexels:123)".into());
    }
    if ids.len() > MAX_LOOK {
        return Err(format!("look at at most {MAX_LOOK} results at a time: pick the shortlist for this idea"));
    }
    Ok(ids)
}

fn look(s: &mut Session, req: &ToolRequest) -> Option<ToolOutput> {
    let ids = match ids_of(&req.input) {
        Ok(v) => v,
        Err(e) => return Some(ToolOutput::error(e)),
    };
    let frames = req.input.get("frames").and_then(Value::as_u64).map_or(2, |n| n.clamp(1, MAX_FRAMES as u64) as usize);
    let mut list = Vec::new();
    for id in &ids {
        match s.hub.candidate(id) {
            Some(c) => list.push(c),
            None => return Some(ToolOutput::error(format!("no result {id} in this session: search with media_search first, and use ids from its results"))),
        }
    }
    #[cfg(feature = "net")]
    {
        let sources = s.hub.sources();
        let cache = s.assistant.cards.looks.cache.clone();
        let reply = req.reply.clone();
        let spawned = std::thread::Builder::new().name("assistant-look".into()).spawn(move || {
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| take_looks(&sources, &list, frames, &cache)))
                .unwrap_or_else(|_| ToolOutput::error("the pictures could not be fetched"));
            let _ = reply.send(out);
        });
        match spawned {
            Ok(_) => None,
            Err(_) => Some(ToolOutput::error("the pictures could not be fetched just now; try again")),
        }
    }
    #[cfg(not(feature = "net"))]
    {
        let _ = (list, frames);
        Some(ToolOutput::error("the stock sites cannot be reached in this build"))
    }
}

#[cfg(feature = "net")]
fn take_looks(
    sources: &edith_sources::Sources,
    list: &[edith_sources::Candidate],
    frames: usize,
    cache: &Mutex<std::collections::BTreeMap<String, Vec<edith_sources::Picture>>>,
) -> ToolOutput {
    let started = web_time::Instant::now();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut images: Vec<edith_ai::Image> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let mut missed: Vec<String> = Vec::new();
    for c in list {
        if started.elapsed().as_secs() >= LOOK_WAIT {
            missed.push(format!("{}: ran out of time", c.id));
            continue;
        }
        if c.thumb.starts_with("file://") {
            missed.push(format!("{}: a file in the person's own folders; it can be looked at once it is in the project", c.id));
            continue;
        }
        let key = format!("{}#{frames}", c.id);
        let cached = crate::assistant::lock(cache).get(&key).cloned();
        let pictures = match cached {
            Some(p) => Ok(p),
            None => sources.look_pictures(c, frames, &cancel).inspect(|p| {
                let mut map = crate::assistant::lock(cache);
                if map.len() >= CACHE_MAX {
                    map.clear();
                }
                map.insert(key.clone(), p.clone());
            }),
        };
        match pictures {
            Ok(p) if !p.is_empty() => {
                let (first, last) = (images.len() + 1, images.len() + p.len());
                images.extend(p.iter().map(|x| edith_ai::Image::from_bytes(x.mime, &x.bytes)));
                let which = if first == last { format!("Picture {first}") } else { format!("Pictures {first} to {last}") };
                lines.push(format!("{which}: {}", candidate_line(c)));
            }
            Ok(_) => missed.push(format!("{}: it has no picture to show", c.id)),
            Err(e) => missed.push(format!("{}: {e}", c.id)),
        }
    }
    if images.is_empty() {
        return ToolOutput::error(format!("No pictures could be had. {}", missed.join("; ")));
    }
    let mut text = format!("{} picture(s) follow, in this order, from inside each clip, earliest first.\n{}", images.len(), lines.join("\n"));
    if !missed.is_empty() {
        text.push_str(&format!("\nNo picture for: {}.", missed.join("; ")));
    }
    text.push_str("\nA picture is one moment: judge what the clip shows, not how it moves. Nothing has been downloaded.");
    ToolOutput { content: text, is_error: false, images, view: None }
}

// ---------------------------------------------------------------------------- media_listen

fn listen(s: &mut Session, req: &ToolRequest) -> Option<ToolOutput> {
    let ids = match ids_of(&req.input) {
        Ok(v) if v.len() == 1 => v,
        Ok(_) => return Some(ToolOutput::error("listen to one sound at a time: `candidate` is one result id")),
        Err(_) => return Some(ToolOutput::error("`candidate` must be a sound result id from media_search (such as freesound:77)")),
    };
    let Some(c) = ids.first().and_then(|id| s.hub.candidate(id)) else {
        return Some(ToolOutput::error(format!("no result {} in this session: search with media_search first, and use an id from its results", ids.join(""))));
    };
    if c.kind != edith_sources::Kind::Audio {
        return Some(ToolOutput::error(format!("{} is {}, not a sound; media_look is for pictures and video", c.id, c.kind.name())));
    }
    let Some(file) = c.download.clone() else { return Some(ToolOutput::error("this result has no file to listen to")) };
    let (tx, rx) = std::sync::mpsc::channel();
    let line = if c.tags.is_empty() { candidate_line(&c) } else { format!("{} | tags: {}", candidate_line(&c), c.tags.join(", ")) };
    // a sound in the person's own folders is measured where it is
    if c.source == "local" {
        let _ = tx.send(Ok(PathBuf::from(&file.url)));
        s.assistant.cards.looks.listens.push(PendingListen { reply: req.reply.clone(), since: web_time::Instant::now(), line, rx, delete: false });
        return None;
    }
    #[cfg(not(feature = "net"))]
    {
        let _ = (tx, rx, file);
        Some(ToolOutput::error("the stock sites cannot be reached in this build"))
    }
    #[cfg(feature = "net")]
    {
        fetch_and_wait(s, req, c, file, line, tx, rx)
    }
}

#[cfg(feature = "net")]
fn fetch_and_wait(
    s: &mut Session,
    req: &ToolRequest,
    c: edith_sources::Candidate,
    file: edith_sources::Download,
    line: String,
    tx: std::sync::mpsc::Sender<std::result::Result<PathBuf, String>>,
    rx: Receiver<std::result::Result<PathBuf, String>>,
) -> Option<ToolOutput> {
    let sources = s.hub.sources();
    let dir = std::env::temp_dir().join(format!("edith-listen-{}", std::process::id()));
    let name =
        format!("{}.{}", crate::assistant::lock(&LISTEN_COUNTER).bump(), file.ext.chars().filter(char::is_ascii_alphanumeric).take(5).collect::<String>());
    let path = dir.join(name);
    let spawned = std::thread::Builder::new().name("assistant-listen".into()).spawn(move || {
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            std::fs::create_dir_all(&dir).map_err(|e| format!("a place to keep the sound while it is measured could not be made ({e})"))?;
            sources.download_up_to(&c, &path, &cancel, Some(LISTEN_MAX_BYTES), |_, _| {}).map_err(|e| e.to_string())?;
            Ok::<PathBuf, String>(path.clone())
        }))
        .unwrap_or_else(|_| Err("the sound could not be fetched".to_string()));
        if tx.send(done).is_err() {
            // nobody is waiting any more: leave nothing behind
            let _ = std::fs::remove_file(&path);
        }
    });
    if spawned.is_err() {
        return Some(ToolOutput::error("the sound could not be fetched just now; try again"));
    }
    s.assistant.cards.looks.listens.push(PendingListen { reply: req.reply.clone(), since: web_time::Instant::now(), line, rx, delete: true });
    None
}

/// Names for the temporary files of sounds being measured.
struct Counter(u64);

impl Counter {
    fn bump(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(1);
        self.0
    }
}

static LISTEN_COUNTER: Mutex<Counter> = Mutex::new(Counter(0));

/// Answers the `media_listen` calls whose sound has arrived: decodes and measures it here, on the session's thread,
/// then deletes the file.
pub(crate) fn pump(s: &mut Session) {
    if s.assistant.cards.looks.listens.is_empty() {
        return;
    }
    let mut keep = Vec::new();
    for p in std::mem::take(&mut s.assistant.cards.looks.listens) {
        match p.rx.try_recv() {
            Ok(Ok(path)) => {
                let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listen_to(s, &path, &p.line)))
                    .unwrap_or_else(|_| ToolOutput::error("the sound could not be measured"));
                if p.delete {
                    let _ = std::fs::remove_file(&path);
                }
                let _ = p.reply.send(out);
            }
            Ok(Err(e)) => {
                let _ = p.reply.send(ToolOutput::error(format!("The sound could not be fetched to listen to: {e}")));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) if p.since.elapsed().as_secs() < LISTEN_WAIT => keep.push(p),
            Err(_) => {
                let _ = p.reply.send(ToolOutput::error("The sound took too long to fetch. Offer it without listening, or try another."));
            }
        }
    }
    s.assistant.cards.looks.listens.extend(keep);
}

/// Opens the sound at `path`, measures up to [`LISTEN_MAX_SECONDS`] of it and says what it is like.
fn listen_to(s: &Session, path: &Path, line: &str) -> ToolOutput {
    let Some(p) = path.to_str() else { return ToolOutput::error("the sound's file name could not be read") };
    let src = match s.media.probe_file(p, &*s.services) {
        Ok(src) => src,
        Err(e) => return ToolOutput::error(format!("The sound could not be opened to measure ({e}). Offer it only if the person's own ears can judge it.")),
    };
    let Some(sr) = src.info().audio().map(|a| a.sample_rate) else { return ToolOutput::error("there is no sound in that file") };
    if !(an::audio::MIN_SAMPLE_RATE..=an::audio::MAX_SAMPLE_RATE).contains(&sr) {
        return ToolOutput::error(format!("a sound at {sr} Hz cannot be measured"));
    }
    let total = crate::narration_timings::secs(src.info().duration);
    let want = (total.clamp(0.0, LISTEN_MAX_SECONDS) * f64::from(sr)) as usize;
    let mut pcm: Vec<f32> = Vec::new();
    let mut channels = 0usize;
    let mut got = 0usize;
    let chunk = sr as usize * 10;
    while got < want {
        let take = chunk.min(want - got);
        let Ok(buf) = src.audio(got as i64, take, sr) else { break };
        let frames = buf.frames();
        if frames == 0 {
            break;
        }
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
        if frames < take {
            break;
        }
    }
    let Ok(ch) = u16::try_from(channels) else { return ToolOutput::error("that sound has no channels to measure") };
    if pcm.is_empty() {
        return ToolOutput::error("nothing could be read from that sound");
    }
    let stats = match an::analyze_audio(sr, ch, &pcm) {
        Ok(st) => st,
        Err(e) => return ToolOutput::error(format!("the sound could not be measured: {e}")),
    };
    let measured = pcm.len() as f64 / channels.max(1) as f64 / f64::from(sr);
    ToolOutput::ok(format!("{line}\n{}", describe_sound(&stats, &pcm, channels, sr, measured, total)))
}

// ---------------------------------------------------------------------------- saying what it is like

/// The level of each half second, dB (all channels together).
fn levels(pcm: &[f32], channels: usize, sr: u32) -> Vec<f64> {
    let window = (sr as usize / 2).max(1) * channels.max(1);
    pcm.chunks(window)
        .filter(|w| w.len() >= window)
        .map(|w| {
            let ms = w.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / w.len() as f64;
            10.0 * ms.max(1e-12).log10()
        })
        .collect()
}

/// A sound's measurements in plain words. Pure: the numbers in, the sentences out.
pub(crate) fn describe_sound(st: &an::audio::AudioStats, pcm: &[f32], channels: usize, sr: u32, measured: f64, total: f64) -> String {
    use an::audio::AudioKind;
    // the measure is a guess from the sound's shape: a solo instrument can look like a voice, so when the two are close
    // it says it cannot tell, and the title and tags decide
    let close = (st.scores.music - st.scores.speech).abs() < 0.4 && matches!(st.kind, AudioKind::Music | AudioKind::Speech);
    let kind = match st.kind {
        _ if close => "either music or speech; the measure cannot tell, so go by the title and tags".to_string(),
        AudioKind::Music => "music".to_string(),
        AudioKind::Speech => "speech".to_string(),
        AudioKind::Ambience => "ambience (a bed of sound, no tune and no words)".to_string(),
        AudioKind::Silence => "silence".to_string(),
    };
    let mut out = format!(
        "Measured from {}: it reads as {kind} (music {:.2}, speech {:.2}, ambience {:.2}).",
        if total > measured + 1.0 { format!("its first {measured:.0} s of {total:.0} s") } else { format!("all {measured:.0} s") },
        st.scores.music,
        st.scores.speech,
        st.scores.ambience
    );
    match &st.tempo {
        Some(t) => out.push_str(&format!(" It has a steady pulse at about {:.0} BPM (confidence {:.0}%).", t.bpm, f64::from(t.confidence) * 100.0)),
        None => out.push_str(" It has no steady pulse."),
    }
    match st.integrated_lufs {
        Some(l) => {
            let steadiness = match st.loudness_range_lu {
                Some(r) if r <= 3.0 => format!("very steady (within {r:.1} LU)"),
                Some(r) if r <= 7.0 => format!("moving moderately (within {r:.1} LU)"),
                Some(r) => format!("swelling and falling a lot (within {r:.1} LU)"),
                None => "too short to say how steady".to_string(),
            };
            out.push_str(&format!(" Loudness about {l:.1} LUFS, {steadiness}"));
            if let Some(p) = st.sample_peak_dbfs {
                out.push_str(&format!("; peak {p:.1} dBFS"));
            }
            out.push('.');
        }
        None => out.push_str(" It is too quiet or too short to measure its loudness."),
    }
    let sp = &st.spectral;
    let tone = match sp.centroid_hz {
        c if c <= 0.0 => "no tone to speak of",
        c if c < 700.0 => "dark and low",
        c if c < 2000.0 => "balanced",
        _ => "bright",
    };
    out.push_str(&format!(
        " Tone: {tone} (low {:.0}%, middle {:.0}%, high {:.0}% of the energy; centre {:.0} Hz).",
        sp.low * 100.0,
        sp.mid * 100.0,
        sp.high * 100.0,
        sp.centroid_hz
    ));
    if st.silence_share >= 0.1 {
        out.push_str(&format!(" It has quiet gaps ({:.0}% of it is silent).", st.silence_share * 100.0));
    }
    // how it starts and ends, from the level of each half second
    let l = levels(pcm, channels, sr);
    if l.len() >= 8 {
        let mut sorted = l.clone();
        sorted.sort_by(f64::total_cmp);
        let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
        let fade_in = l.first().is_some_and(|v| *v < median - 12.0);
        let fade_out = l.last().is_some_and(|v| *v < median - 12.0);
        let steady = st.loudness_range_lu.is_some_and(|r| r <= 4.0);
        out.push_str(match (fade_in, fade_out) {
            (true, true) => " It fades in and fades out, so it suits one stretch but will not repeat cleanly.",
            (true, false) => " It fades in and ends at full level.",
            (false, true) => " It starts at full level and fades out.",
            (false, false) if steady => " It holds one level with no fade at either end, so it can be repeated or crossfaded to fill a longer stretch.",
            (false, false) => " It has no fade at either end, but its level moves, so repeating it would show the seam.",
        });
    }
    out.push_str(" This is measured, not heard: the person's ear decides whether it is right.");
    out
}
