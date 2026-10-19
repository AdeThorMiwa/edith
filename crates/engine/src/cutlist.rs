//! `timeline.placeCutList` (fork roadmap F16.3): place a run of shots on one track in a single call.
//!
//! The assistant used about ten edit calls to cut one idea (place, trim, speed, move, note). A cut list says
//! it once: each entry is a stretch of the timeline, the project clip that fills it, where in the clip to start,
//! optionally a speed and a camera move, and **why** this shot is there. Any number of entries; one undo step.
//!
//! Everything is checked before anything changes, and a bad entry refuses the whole call and says which entry
//! and why, so the project is never left half done:
//! - each stretch is at least a frame, whole frames, and stretches do not overlap;
//! - the clip is a project media item with a picture, online, and long enough from `sourceIn` for the stretch at
//!   the speed asked (a still is as long as needed);
//! - the track exists and is not locked;
//! - when something on the timeline speaks (a narration, a transcribed recording), no cut falls **inside a word**:
//!   a start or end must sit in a pause or at a word's start or end, and the refusal names the word and the
//!   nearest places that would do. With nothing speaking (a montage) there is nothing to check.
//!
//! Shots go in as an overwrite on the track (they replace what is under them), picture only. The reason is kept
//! on the clip as a marker named "Why this shot".
//!
//! `transition` says how a shot **comes in** (or, for `fadeOut`, goes out): `"dissolve"`, `"dipToBlack"`, `"fadeIn"`,
//! `"fadeOut"` or `"none"` (the default: a hard cut), or an object `{kind, seconds?}`. A dissolve or a dip is centred
//! on the cut with the shot before it when that touches, and fades up from nothing when nothing touches; `fadeIn` and
//! `fadeOut` are for a shot with nothing touching on that side (use a dip between two shots). The default length
//! comes from the plan's Format Pack (0.8 s a dissolve, 1 s a dip or fade). Refused, naming the entry, when the
//! transition is longer than half a shot it joins, or a shot has not the spare picture it would use.
//!
//! `timeline.placeCutList {entries: [{start, end, item, sourceIn?, speed?, move?, amount?, seed?, transition?, reason?}], track?}`:
//! times are seconds, `"1:25"` or a timecode; `speed` is a multiple (0.5 plays at half speed); `move` is one of
//! the camera moves (`motion.cameraMove`).

use filmcraft_project::{ClipId, ItemId, ItemKind, Label, Marker, MarkerId, MarkerKind};
use filmcraft_time::{TICKS_PER_SECOND, Tick, TimeRange};
use serde_json::{Value, json};

use crate::assistant_tools::parse_time;
use crate::commands::{CommandSpec, bad, has_seq, place_item};
use crate::{EngineError, Result, Session};

/// Entries in one call.
pub const MAX_ENTRIES: usize = 2_000;
const MAX_REASON: usize = 300;

/// One entry, read and checked, in ticks on the frame grid.
struct Shot {
    n: usize,
    start: Tick,
    end: Tick,
    item: ItemId,
    source_in: Tick,
    speed: f64,
    camera: Option<(String, Option<f64>, Option<u64>)>,
    reason: String,
    transition: Option<Trans>,
    /// Picture to spare before the first frame and after the last, in the clip's own time.
    before: Tick,
    after: Tick,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TKind {
    Dissolve,
    Dip,
    FadeIn,
    FadeOut,
}

#[derive(Clone, Copy)]
struct Trans {
    kind: TKind,
    seconds: f64,
}

/// A `transition` entry: a kind as a word, or `{kind, seconds?}`.
fn read_transition(v: &Value, pol: &crate::polish::Policy) -> std::result::Result<Option<Trans>, String> {
    let (word, seconds) = match v {
        Value::String(w) => (w.as_str(), None),
        Value::Object(o) => (
            o.get("kind").and_then(Value::as_str).ok_or("`transition.kind` must be dissolve, dipToBlack, fadeIn, fadeOut or none")?,
            match o.get("seconds").filter(|x| !x.is_null()) {
                None => None,
                Some(x) => Some(x.as_f64().filter(|d| d.is_finite() && (0.1..=5.0).contains(d)).ok_or("`transition.seconds` must be from 0.1 to 5")?),
            },
        ),
        _ => return Err("`transition` must be a word such as \"dissolve\" or {kind, seconds?}".into()),
    };
    let kind = match word.trim().to_ascii_lowercase().as_str() {
        "none" | "cut" | "" => return Ok(None),
        "dissolve" | "crossdissolve" => TKind::Dissolve,
        "diptoblack" | "dip" => TKind::Dip,
        "fadein" => TKind::FadeIn,
        "fadeout" => TKind::FadeOut,
        _ => return Err("`transition` must be dissolve, dipToBlack, fadeIn, fadeOut or none".into()),
    };
    let default = if kind == TKind::Dissolve { pol.dissolve } else { pol.dip };
    Ok(Some(Trans { kind, seconds: seconds.unwrap_or(default) }))
}

fn secs(t: Tick) -> f64 {
    t.0 as f64 / TICKS_PER_SECOND as f64
}

fn read_shots(s: &Session, p: &Value) -> Result<Vec<Shot>> {
    let cmd = "timeline.placeCutList";
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rate = q.settings.frame_rate;
    let list = p.get("entries").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`entries` must be a list of shots"))?;
    if list.is_empty() || list.len() > MAX_ENTRIES {
        return Err(bad(cmd, format!("`entries` needs 1 to {MAX_ENTRIES} shots")));
    }
    let frame = rate.frame_duration();
    let mut shots = Vec::with_capacity(list.len());
    for (i, e) in list.iter().enumerate() {
        let n = i + 1;
        let no = |why: String| bad(cmd, format!("entry {n}: {why}"));
        if !e.is_object() {
            return Err(no("is not an object with start, end and item".into()));
        }
        let time = |key: &str| -> Result<Tick> {
            let v = e.get(key).filter(|v| !v.is_null()).ok_or_else(|| no(format!("`{key}` is required")))?;
            parse_time(v, q, key).map(|t| rate.snap_nearest(t)).map_err(no)
        };
        let (start, end) = (time("start")?, time("end")?);
        if end - start < frame {
            return Err(no(format!("runs {:.3} s to {:.3} s, which is less than one frame; `end` must be after `start`", secs(start), secs(end))));
        }
        let item = e
            .get("item")
            .and_then(Value::as_u64)
            .map(ItemId)
            .ok_or_else(|| no("`item` must be the number of a media item in the project (project_items lists them)".into()))?;
        let pi = s.project.item(item).ok_or_else(|| no(format!("there is no item {}", item.0)))?;
        let media = matches!(pi.kind, ItemKind::Media(_) | ItemKind::Subclip { .. });
        if !media || !s.project.source_streams(item).0 {
            return Err(no(format!("“{}” is not a picture or footage in the project (audio and sequences cannot be placed as a shot)", pi.name)));
        }
        if let ItemKind::Media(m) = &pi.kind
            && m.offline
        {
            return Err(no(format!("“{}” is offline: its file is missing", pi.name)));
        }
        let speed = match e.get("speed").filter(|v| !v.is_null()) {
            None => 1.0,
            Some(v) => v
                .as_f64()
                .filter(|x| x.is_finite() && (0.1..=4.0).contains(x))
                .ok_or_else(|| no("`speed` must be a multiple from 0.1 to 4 (0.5 is half speed)".into()))?,
        };
        let pol = crate::polish::policy(s);
        let transition = match e.get("transition").filter(|v| !v.is_null()) {
            None => None,
            Some(v) => read_transition(v, &pol).map_err(&no)?,
        };
        let (mark_in, still) = match &pi.kind {
            ItemKind::Media(m) => (m.mark_in.unwrap_or_default(), matches!(m.info.kind, filmcraft_media::MediaKind::Still)),
            ItemKind::Subclip { range, .. } => (range.start, false),
            _ => (Tick::ZERO, false),
        };
        let source_in = match e.get("sourceIn").filter(|v| !v.is_null()) {
            None => mark_in,
            Some(v) => rate.snap_nearest(parse_time(v, q, "sourceIn").map_err(no)?),
        };
        // the picture to spare on either side of what the shot shows, for a transition to use
        let (media_start, media_end) = match &pi.kind {
            ItemKind::Subclip { range, .. } => (range.start, range.end()),
            _ => (Tick::ZERO, crate::commands::full_duration(s, pi)),
        };
        let used = Tick((((end - start).0 as f64) * speed).round() as i64);
        let (before, after) = if still {
            (Tick(i64::MAX / 4), Tick(i64::MAX / 4))
        } else {
            ((source_in - media_start).max(Tick::ZERO), (media_end - (source_in + used)).max(Tick::ZERO))
        };
        // a still is as long as the stretch needs; footage must have enough left from `sourceIn`
        if !still {
            let full = crate::commands::full_duration(s, pi);
            let end_of_media = match &pi.kind {
                ItemKind::Subclip { range, .. } => range.end(),
                _ => full,
            };
            let needed = Tick((((end - start).0 as f64) * speed).round() as i64);
            let have = end_of_media - source_in;
            if source_in < Tick::ZERO || have + frame < needed {
                return Err(no(format!(
                    "“{}” has {:.2} s left from {:.2} s but the shot needs {:.2} s of it at speed {speed}; start earlier in it (`sourceIn`), slow it down, or use another clip",
                    pi.name,
                    secs(have.max(Tick::ZERO)),
                    secs(source_in),
                    secs(needed)
                )));
            }
        }
        let camera = match e.get("move").filter(|v| !v.is_null()) {
            None => None,
            Some(v) => {
                let name = v
                    .as_str()
                    .filter(|m| crate::camera_moves::MOVES.contains(m))
                    .ok_or_else(|| no(format!("`move` must be one of {}", crate::camera_moves::MOVES.join(", "))))?;
                let amount = match e.get("amount").filter(|v| !v.is_null()) {
                    None => None,
                    Some(a) => Some(a.as_f64().filter(|x| x.is_finite() && (0.0..=100.0).contains(x)).ok_or_else(|| no("`amount` must be 0 to 100".into()))?),
                };
                Some((name.to_string(), amount, e.get("seed").and_then(Value::as_u64)))
            }
        };
        let reason: String = match e.get("reason") {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(r)) => r.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string(),
            Some(_) => return Err(no("`reason` must be text".into())),
        };
        if reason.chars().count() > MAX_REASON {
            return Err(no(format!("`reason` is longer than {MAX_REASON} characters; keep it to a sentence")));
        }
        shots.push(Shot { n, start, end, item, source_in, speed, camera, reason, transition, before, after });
    }
    Ok(shots)
}

/// Each transition fits: no longer than half a shot it joins, with spare picture to use, and a fade only where nothing touches.
fn check_transitions(s: &Session, shots: &[Shot]) -> Result<()> {
    let tol = s.sequence_rate().frame_duration().0 / 2;
    let touching = |a: &Shot, b: &Shot| (a.end - b.start).0.abs() <= tol;
    for shot in shots {
        let Some(t) = shot.transition else { continue };
        let no = |why: String| bad("timeline.placeCutList", format!("entry {}: {why}", shot.n));
        let prev = shots.iter().find(|p| p.n != shot.n && touching(p, shot));
        let next = shots.iter().find(|x| x.n != shot.n && touching(shot, x));
        let long = |len: Tick| t.seconds > secs(len) / 2.0 + 1e-9;
        if long(shot.end - shot.start) {
            return Err(no(format!(
                "a {:.1} s transition is longer than half of this {:.1} s shot; shorten it (`transition.seconds`) or keep the cut hard",
                t.seconds,
                secs(shot.end - shot.start)
            )));
        }
        match t.kind {
            TKind::FadeIn if prev.is_some() => {
                return Err(no("`fadeIn` is for a shot with nothing touching before it; use `dipToBlack` between two shots".into()));
            }
            TKind::FadeOut if next.is_some() => {
                return Err(no("`fadeOut` is for a shot with nothing touching after it; use `dipToBlack` on the next shot".into()));
            }
            TKind::Dissolve | TKind::Dip => {
                if let Some(p) = prev {
                    if long(p.end - p.start) {
                        return Err(no(format!(
                            "a {:.1} s transition is longer than half of the shot before it ({:.1} s, entry {})",
                            t.seconds,
                            secs(p.end - p.start),
                            p.n
                        )));
                    }
                    let half = Tick::from_seconds_f64(t.seconds / 2.0);
                    let need = |sp: f64| Tick((half.0 as f64 * sp).round() as i64);
                    if p.after < need(p.speed) {
                        return Err(no(format!(
                            "a {:.1} s {} needs {:.2} s of spare picture after entry {}, which has {:.2} s; end it earlier in the clip (`sourceIn`) or keep the cut hard",
                            t.seconds,
                            if t.kind == TKind::Dip { "dip" } else { "dissolve" },
                            secs(need(p.speed)),
                            p.n,
                            secs(p.after)
                        )));
                    }
                    if shot.before < need(shot.speed) {
                        return Err(no(format!(
                            "a {:.1} s {} needs {:.2} s of spare picture before this shot, which has {:.2} s; start it later in the clip (`sourceIn`) or keep the cut hard",
                            t.seconds,
                            if t.kind == TKind::Dip { "dip" } else { "dissolve" },
                            secs(need(shot.speed)),
                            secs(shot.before)
                        )));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// No two shots overlap.
fn check_overlaps(shots: &[Shot]) -> Result<()> {
    let mut order: Vec<&Shot> = shots.iter().collect();
    order.sort_by_key(|x| (x.start, x.end));
    for w in order.windows(2) {
        if w[1].start < w[0].end {
            return Err(bad(
                "timeline.placeCutList",
                format!(
                    "entries {} and {} overlap: {:.3}–{:.3} s and {:.3}–{:.3} s; one shot ends where the next begins",
                    w[0].n,
                    w[1].n,
                    secs(w[0].start),
                    secs(w[0].end),
                    secs(w[1].start),
                    secs(w[1].end)
                ),
            ));
        }
    }
    Ok(())
}

/// Cuts that fall inside a spoken word, each as a sentence naming the word and where it could go instead.
fn mid_word_cuts(s: &Session, shots: &[Shot]) -> Vec<String> {
    let spoken = crate::narration_timings::spoken_spans(s);
    if spoken.is_empty() {
        return Vec::new();
    }
    let frame = s.sequence_rate().frame_duration();
    let mut out = Vec::new();
    for shot in shots {
        for (label, t) in [("starts", shot.start), ("ends", shot.end)] {
            // the word the cut would split: the cut is more than a frame inside it
            if let Some((a, b, text)) = spoken.iter().find(|(a, b, _)| t > *a + frame && t + frame < *b) {
                out.push(format!(
                    "entry {} {label} at {:.2} s, inside the word “{}” ({:.2}–{:.2} s): move it to {:.2} s (the word's start), {:.2} s (its end), or a pause",
                    shot.n,
                    secs(t),
                    text,
                    secs(*a),
                    secs(*b),
                    secs(*a),
                    secs(*b)
                ));
            }
        }
    }
    out
}

fn place_cut_list(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.placeCutList";
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let shots = read_shots(s, p)?;
    check_overlaps(&shots)?;
    check_transitions(s, &shots)?;
    let track = match crate::commands::track_p(s, p, "track", cmd)? {
        Some(t) => t,
        None => {
            let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
            q.video_tracks.iter().find(|t| !t.locked).map(|t| t.id).ok_or_else(|| bad(cmd, "there is no unlocked video track to put the shots on"))?
        }
    };
    {
        let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
        match q.video_tracks.iter().find(|t| t.id == track) {
            Some(t) if t.locked => return Err(bad(cmd, "that video track is locked")),
            Some(_) => {}
            None => return Err(bad(cmd, "`track` must be a video track such as V1")),
        }
    }
    let wrong = mid_word_cuts(s, &shots);
    if !wrong.is_empty() {
        let shown: Vec<String> = wrong.iter().take(8).cloned().collect();
        return Err(bad(
            cmd,
            format!(
                "nothing was placed: {} cut{} would split a spoken word. {}{}",
                wrong.len(),
                if wrong.len() == 1 { "" } else { "s" },
                shown.join("; "),
                if wrong.len() > shown.len() { format!("; and {} more", wrong.len() - shown.len()) } else { String::new() }
            ),
        ));
    }
    let checked = !crate::narration_timings::spoken_spans(s).is_empty();
    let count = shots.len();
    let total: Tick = shots.iter().fold(Tick::ZERO, |a, x| a + (x.end - x.start));
    let ids = s.edit_group(&format!("Place Cut List ({count} shot{})", if count == 1 { "" } else { "s" }), |s| {
        let mut placed: Vec<ClipId> = Vec::with_capacity(shots.len());
        for shot in &shots {
            let range = TimeRange::new(shot.source_in, shot.end - shot.start);
            let clips = place_item(s, shot.item, range, shot.start, Some(track), None, false, "Place Clip", None)?;
            let clip = clips.first().copied().ok_or_else(|| bad(cmd, format!("entry {}: nothing was placed", shot.n)))?;
            let (speed, reason) = (shot.speed, shot.reason.clone());
            s.edit("Shot Details", |pr, _| {
                let id = MarkerId(pr.alloc_id());
                let seq = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
                let (_, it) = seq.find_item_mut(clip).ok_or_else(|| bad(cmd, "the shot was removed while it was being placed"))?;
                it.speed = speed;
                if !reason.is_empty() {
                    it.markers.push(Marker {
                        id,
                        start: Tick::ZERO,
                        duration: Tick::ZERO,
                        name: "Why this shot".into(),
                        comment: reason,
                        kind: MarkerKind::Comment,
                        color: Label::Green,
                    });
                }
                Ok(())
            })?;
            if let Some((mv, amount, seed)) = &shot.camera {
                let mut params = json!({"clip": clip.0, "move": mv});
                if let Some(a) = amount {
                    params["amount"] = json!(a);
                }
                if let Some(sd) = seed {
                    params["seed"] = json!(sd);
                }
                s.execute("motion.cameraMove", params).map_err(|e| bad(cmd, format!("entry {}: {e}", shot.n)))?;
            }
            placed.push(clip);
        }
        // transitions last: they need the shots on either side already in place
        for (shot, clip) in shots.iter().zip(&placed) {
            let Some(t) = shot.transition else { continue };
            let (edge, effect) = match t.kind {
                TKind::Dissolve => ("in", "cross_dissolve"),
                TKind::Dip | TKind::FadeIn => ("in", "dip_to_black"),
                TKind::FadeOut => ("out", "dip_to_black"),
            };
            let rate = s.sequence_rate();
            let frames = (rate.snap_nearest(Tick::from_seconds_f64(t.seconds)).0 / rate.frame_duration().0.max(1)).max(1);
            s.execute("sequence.applyVideoTransition", json!({"clip": clip.0, "edge": edge, "frames": frames, "effect": effect}))
                .map_err(|e| bad(cmd, format!("entry {}: the transition could not be made: {e}", shot.n)))?;
        }
        Ok(placed)
    })?;
    let mut notes: Vec<&str> = Vec::new();
    if !checked {
        notes.push("nothing on the timeline speaks, so cuts were not checked against words");
    }
    Ok(json!({"placed": count, "clips": ids.iter().map(|c| c.0).collect::<Vec<_>>(), "seconds": secs(total), "wordsChecked": checked, "notes": notes}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "timeline.placeCutList",
        label: "Place a Cut List",
        menu: &[],
        shortcut: None,
        params: r#"{"entries":[{"start":time,"end":time,"item":id,"sourceIn":time?,"speed":0.1..4?,"move":"pushIn|pullOut|driftLeft|driftRight|sway"?,"amount":0..100?,"seed":u64?,"transition":"dissolve|dipToBlack|fadeIn|fadeOut|none"|{"kind":str,"seconds":0.1..5?}?,"reason":str?}],"track":"V1"?}"#,
        enabled: has_seq,
        run: place_cut_list,
        journal: true,
    }]
}
