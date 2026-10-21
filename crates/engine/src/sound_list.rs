//! `timeline.placeSoundList` (fork roadmap F17.1): put a score, a bed of ambience and sound effects on the timeline in
//! one call, the way `timeline.placeCutList` puts the pictures there.
//!
//! Each entry is a stretch of the timeline, the project clip that fills it, the lane it belongs to (`music`,
//! `ambience` or `effects`), where in the clip to start, how loud, how long to fade in and out, and **why** it is
//! there. A lane is a track of its own, named for it ("Music", "Ambience", "Effects"); it is made when it is first
//! needed and found again by its name after that. The clips are given the audio type of their lane (Music, Ambience
//! or SFX, as in Essential Sound), so `sound.autoMix` and the ducking know what each is.
//!
//! A sound shorter than its stretch can be **looped** (`loop: true`): it is laid end to end, each seam crossfaded
//! (a centred Constant Power crossfade of up to a second, using the clip's own spare sound on either side), so a
//! thirty-second bed fills ten minutes without a click. Without `loop` a sound that is too short is refused.
//!
//! Everything is checked before anything changes; a bad entry refuses the whole call and says which entry and why. One
//! undo step. A lane's stretches may not overlap each other, and may not land on sound already in that lane unless
//! `replace` is true (then what is under them is overwritten, as a normal overwrite edit does).
//!
//! `timeline.placeSoundList {entries: [{start, end, item, lane, sourceIn?, gainDb?, fadeInSeconds?, fadeOutSeconds?,
//! loop?, reason?}], replace?}`: times are seconds, `"1:25"` or a timecode. Fades default to 1.5 s in and 3 s out for
//! music, 1.5 s each for ambience and none for effects. `gainDb` is the clip's own gain, -60 to +12 (the level of the
//! whole mix is `sound.autoMix`'s job).

use filmcraft_project::essential::AudioType;
use filmcraft_project::{ClipId, ItemId, ItemKind, Label, Marker, MarkerId, MarkerKind, TrackId};
use filmcraft_time::{TICKS_PER_SECOND, Tick, TimeRange};
use serde_json::{Value, json};

use crate::assistant_tools::parse_time;
use crate::commands::{CommandSpec, bad, bool_p, has_seq, place_item};
use crate::{EngineError, Result, Session};

/// Entries in one call.
pub const MAX_ENTRIES: usize = 500;
/// Clips one call may make in all (a long bed looped from a short sound).
const MAX_PIECES: usize = 600;
const MAX_REASON: usize = 300;
/// The longest crossfade at a loop seam, seconds.
const SEAM_SECONDS: f64 = 1.0;
/// A sound this short cannot be looped without the seam being the sound.
const MIN_LOOP_SECONDS: f64 = 2.0;
const CMD: &str = "timeline.placeSoundList";

fn secs(t: Tick) -> f64 {
    t.0 as f64 / TICKS_PER_SECOND as f64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lane {
    Music,
    Ambience,
    Effects,
}

impl Lane {
    pub(crate) const ALL: [Lane; 3] = [Lane::Music, Lane::Ambience, Lane::Effects];

    fn parse(s: &str) -> Option<Lane> {
        match s.trim().to_ascii_lowercase().as_str() {
            "music" | "score" => Some(Lane::Music),
            "ambience" | "ambient" | "atmosphere" | "bed" => Some(Lane::Ambience),
            "effects" | "effect" | "sfx" | "fx" => Some(Lane::Effects),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Lane::Music => "Music",
            Lane::Ambience => "Ambience",
            Lane::Effects => "Effects",
        }
    }

    /// The type word `essentialSound.setType` takes.
    fn type_word(self) -> &'static str {
        match self {
            Lane::Music => "music",
            Lane::Ambience => "ambience",
            Lane::Effects => "sfx",
        }
    }

    /// Fade in and out when the entry says nothing, seconds.
    fn default_fades(self) -> (f64, f64) {
        match self {
            Lane::Music => (1.5, 3.0),
            Lane::Ambience => (1.5, 1.5),
            Lane::Effects => (0.0, 0.0),
        }
    }

    /// Whether `name` is this lane's track.
    fn is_named(self, name: &str) -> bool {
        Lane::parse(name) == Some(self)
    }
}

/// The audio track that is `lane`'s, by its name.
pub(crate) fn lane_track(s: &Session, lane: Lane) -> Option<TrackId> {
    s.active_sequence()?.audio_tracks.iter().find(|t| lane.is_named(&t.name)).map(|t| t.id)
}

/// The lane an audio clip belongs to: its Essential Sound type, else the track it is on.
pub(crate) fn lane_of(s: &Session, clip: &filmcraft_project::TrackItem, track_name: &str) -> Option<Lane> {
    match clip.essential.as_ref().map(|e| e.kind) {
        Some(AudioType::Music) => return Some(Lane::Music),
        Some(AudioType::Ambience) => return Some(Lane::Ambience),
        Some(AudioType::Sfx) => return Some(Lane::Effects),
        Some(AudioType::Dialogue) => return None,
        None => {}
    }
    let _ = s;
    Lane::ALL.into_iter().find(|l| l.is_named(track_name))
}

/// One entry, read and checked, in ticks on the frame grid.
struct Cue {
    n: usize,
    start: Tick,
    end: Tick,
    item: ItemId,
    lane: Lane,
    source_in: Tick,
    /// What is left of the sound from `source_in`.
    available: Tick,
    gain_db: f64,
    fade_in: Tick,
    fade_out: Tick,
    looped: bool,
    reason: String,
}

/// One clip to put down: where, how long, from where in the sound.
#[derive(Debug, PartialEq, Eq)]
struct Piece {
    at: Tick,
    length: Tick,
    source: Tick,
}

fn floor_frames(t: Tick, frame: Tick) -> Tick {
    if frame.0 <= 0 { t } else { Tick(t.0.div_euclid(frame.0) * frame.0) }
}

/// The clips that fill `[start, end)` from a sound with `available` left from `source`. One clip when it is long
/// enough; otherwise (looped) the sound end to end with each seam a centred crossfade of `seam` that uses `seam / 2` of
/// the sound's own spare end on either side. `None` when it cannot be done.
fn tile(start: Tick, end: Tick, source: Tick, available: Tick, frame: Tick, looped: bool) -> Option<(Vec<Piece>, Tick)> {
    let wanted = end - start;
    let available = floor_frames(available, frame);
    if available.0 <= 0 || wanted.0 <= 0 {
        return None;
    }
    if wanted <= available + frame {
        return Some((vec![Piece { at: start, length: wanted.min(available.max(frame)), source }], Tick::ZERO));
    }
    if !looped {
        return None;
    }
    // a crossfade of up to a second, at most a quarter of the sound, in whole frames and an even number of them
    let frames = ((available.0 / 4).min((SEAM_SECONDS * TICKS_PER_SECOND as f64) as i64) / frame.0.max(1)).max(2) & !1;
    let seam = Tick(frames * frame.0);
    let half = Tick(seam.0 / 2);
    let body = available - seam;
    if body.0 < seam.0 * 2 {
        return None;
    }
    let mut pieces = vec![Piece { at: start, length: available - half, source }];
    let mut pos = start + available - half;
    while pos < end {
        let left = end - pos;
        if left < seam {
            // too little for its own crossfade: the clip before gives it up, so this one is a whole seam long
            let last = pieces.last_mut()?;
            last.length -= seam - left;
            pos -= seam - left;
            pieces.push(Piece { at: pos, length: seam, source: source + half });
            break;
        }
        let length = left.min(body);
        pieces.push(Piece { at: pos, length, source: source + half });
        pos += length;
        if pieces.len() > MAX_PIECES {
            return None;
        }
    }
    Some((pieces, seam))
}

fn frames_of(s: &Session, t: Tick) -> i64 {
    let frame = s.sequence_rate().frame_duration().0.max(1);
    (t.0 / frame).max(1)
}

fn read_cues(s: &Session, p: &Value) -> Result<Vec<Cue>> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rate = q.settings.frame_rate;
    let list = p.get("entries").and_then(Value::as_array).ok_or_else(|| bad(CMD, "`entries` must be a list of sounds"))?;
    if list.is_empty() || list.len() > MAX_ENTRIES {
        return Err(bad(CMD, format!("`entries` needs 1 to {MAX_ENTRIES} sounds")));
    }
    let frame = rate.frame_duration();
    let mut cues = Vec::with_capacity(list.len());
    for (i, e) in list.iter().enumerate() {
        let n = i + 1;
        let no = |why: String| bad(CMD, format!("entry {n}: {why}"));
        if !e.is_object() {
            return Err(no("is not an object with start, end, item and lane".into()));
        }
        let time = |key: &str| -> Result<Tick> {
            let v = e.get(key).filter(|v| !v.is_null()).ok_or_else(|| no(format!("`{key}` is required")))?;
            parse_time(v, q, key).map(|t| rate.snap_nearest(t)).map_err(no)
        };
        let (start, end) = (time("start")?, time("end")?);
        if start < Tick::ZERO {
            return Err(no("starts before the beginning of the timeline".into()));
        }
        if end - start < frame {
            return Err(no(format!("runs {:.3} s to {:.3} s, which is less than one frame; `end` must be after `start`", secs(start), secs(end))));
        }
        let lane = e.get("lane").and_then(Value::as_str).and_then(Lane::parse).ok_or_else(|| no("`lane` must be music, ambience or effects".into()))?;
        let item = e
            .get("item")
            .and_then(Value::as_u64)
            .map(ItemId)
            .ok_or_else(|| no("`item` must be the number of a media item in the project (project_items lists them)".into()))?;
        let pi = s.project.item(item).ok_or_else(|| no(format!("there is no item {}", item.0)))?;
        let media = matches!(pi.kind, ItemKind::Media(_) | ItemKind::Subclip { .. });
        if !media || !s.project.source_streams(item).1 {
            return Err(no(format!("“{}” has no sound in it (sequences, pictures and graphics cannot be placed as sound)", pi.name)));
        }
        if let ItemKind::Media(m) = &pi.kind
            && m.offline
        {
            return Err(no(format!("“{}” is offline: its file is missing", pi.name)));
        }
        let mark_in = match &pi.kind {
            ItemKind::Media(m) => m.mark_in.unwrap_or_default(),
            ItemKind::Subclip { range, .. } => range.start,
            _ => Tick::ZERO,
        };
        let source_in = match e.get("sourceIn").filter(|v| !v.is_null()) {
            None => mark_in,
            Some(v) => rate.snap_nearest(parse_time(v, q, "sourceIn").map_err(no)?),
        };
        let end_of_media = match &pi.kind {
            ItemKind::Subclip { range, .. } => range.end(),
            _ => crate::commands::full_duration(s, pi),
        };
        let available = end_of_media - source_in;
        if source_in < Tick::ZERO || available < frame {
            return Err(no(format!("“{}” has no sound left from {:.2} s; start earlier in it (`sourceIn`) or use another sound", pi.name, secs(source_in))));
        }
        let looped = match e.get("loop").filter(|v| !v.is_null()) {
            None => false,
            Some(v) => v.as_bool().ok_or_else(|| no("`loop` must be true or false".into()))?,
        };
        let wanted = end - start;
        if wanted > available + frame {
            if !looped {
                return Err(no(format!(
                    "“{}” has {:.1} s of sound from {:.1} s but the stretch is {:.1} s; set `loop: true` to repeat it with crossfaded seams, start earlier in it, or use a longer sound",
                    pi.name,
                    secs(available),
                    secs(source_in),
                    secs(wanted)
                )));
            }
            if secs(available) < MIN_LOOP_SECONDS {
                return Err(no(format!(
                    "“{}” is only {:.1} s of sound: too short to repeat without the repeat being heard; use a longer sound (at least {MIN_LOOP_SECONDS:.0} s)",
                    pi.name,
                    secs(available)
                )));
            }
        }
        let gain_db = match e.get("gainDb").filter(|v| !v.is_null()) {
            None => 0.0,
            Some(v) => v.as_f64().filter(|x| x.is_finite() && (-60.0..=12.0).contains(x)).ok_or_else(|| no("`gainDb` must be from -60 to +12".into()))?,
        };
        let (d_in, d_out) = lane.default_fades();
        let fade = |key: &str, default: f64| -> Result<Tick> {
            let secs_ = match e.get(key).filter(|v| !v.is_null()) {
                None => default,
                Some(v) => {
                    v.as_f64().filter(|x| x.is_finite() && (0.0..=60.0).contains(x)).ok_or_else(|| no(format!("`{key}` must be from 0 to 60 seconds")))?
                }
            };
            Ok(rate.snap_nearest(Tick::from_seconds_f64(secs_)))
        };
        let (mut fade_in, mut fade_out) = (fade("fadeInSeconds", d_in)?, fade("fadeOutSeconds", d_out)?);
        // the two fades may not be longer than the stretch between them
        let room = floor_frames(Tick(wanted.0 - frame.0), frame);
        if fade_in + fade_out > room {
            let scale = room.0 as f64 / (fade_in + fade_out).0.max(1) as f64;
            fade_in = floor_frames(Tick((fade_in.0 as f64 * scale) as i64), frame);
            fade_out = floor_frames(Tick((fade_out.0 as f64 * scale) as i64), frame);
        }
        let reason: String = match e.get("reason") {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(r)) => r.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string(),
            Some(_) => return Err(no("`reason` must be text".into())),
        };
        if reason.chars().count() > MAX_REASON {
            return Err(no(format!("`reason` is longer than {MAX_REASON} characters; keep it to a sentence")));
        }
        cues.push(Cue { n, start, end, item, lane, source_in, available, gain_db, fade_in, fade_out, looped, reason });
    }
    Ok(cues)
}

/// No two sounds of one lane overlap.
fn check_overlaps(cues: &[Cue]) -> Result<()> {
    for lane in Lane::ALL {
        let mut order: Vec<&Cue> = cues.iter().filter(|c| c.lane == lane).collect();
        order.sort_by_key(|c| (c.start, c.end));
        for w in order.windows(2) {
            if w[1].start < w[0].end {
                return Err(bad(
                    CMD,
                    format!(
                        "entries {} and {} are both {} and overlap: {:.2}–{:.2} s and {:.2}–{:.2} s; one sound of a lane ends where the next begins (or put one in another lane)",
                        w[0].n,
                        w[1].n,
                        lane.name().to_ascii_lowercase(),
                        secs(w[0].start),
                        secs(w[0].end),
                        secs(w[1].start),
                        secs(w[1].end)
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// A lane's track, made when there is none.
fn ensure_lane(s: &mut Session, lane: Lane, made: &mut Vec<&'static str>) -> Result<TrackId> {
    if let Some(t) = lane_track(s, lane) {
        return Ok(t);
    }
    let r = s.execute("sequence.addTracks", json!({"video": 0, "audio": 1, "audioAfter": "last"}))?;
    let id = r["audio"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(Value::as_u64)
        .map(TrackId)
        .ok_or_else(|| bad(CMD, "a track for the sound could not be made"))?;
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    s.edit("Name Track", |p, _| {
        let seq = p.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        if let Some(t) = seq.audio_tracks.iter_mut().find(|t| t.id == id) {
            t.name = lane.name().to_string();
        }
        Ok(())
    })?;
    made.push(lane.name());
    Ok(id)
}

fn place_sound_list(s: &mut Session, p: &Value) -> Result<Value> {
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let cues = read_cues(s, p)?;
    check_overlaps(&cues)?;
    let replace = bool_p(p, "replace").unwrap_or(false);
    let frame = s.sequence_rate().frame_duration();
    // what each entry would be made of, before anything is touched
    let mut plans: Vec<(Vec<Piece>, Tick)> = Vec::with_capacity(cues.len());
    let mut total_pieces = 0usize;
    for c in &cues {
        let name = s.project.item(c.item).map(|i| i.name.clone()).unwrap_or_default();
        let (pieces, seam) = tile(c.start, c.end, c.source_in, c.available, frame, c.looped).ok_or_else(|| {
            bad(
                CMD,
                format!(
                    "entry {}: “{name}” cannot fill {:.1} s (it is too short to repeat, or would need more than {MAX_PIECES} repeats)",
                    c.n,
                    secs(c.end - c.start)
                ),
            )
        })?;
        total_pieces += pieces.len();
        if total_pieces > MAX_PIECES {
            return Err(bad(CMD, format!("that would make more than {MAX_PIECES} clips; use longer sounds for the beds")));
        }
        plans.push((pieces, seam));
    }
    // sound already in a lane is not overwritten unless asked
    if !replace {
        let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
        for c in &cues {
            let Some(track) = lane_track(s, c.lane).and_then(|id| q.audio_tracks.iter().find(|t| t.id == id)) else { continue };
            if track.locked {
                return Err(bad(CMD, format!("entry {}: the {} track is locked", c.n, c.lane.name())));
            }
            if let Some(other) = track.items.iter().find(|i| i.start < c.end && i.end() > c.start) {
                return Err(bad(
                    CMD,
                    format!(
                        "entry {}: the {} track already has “{}” from {:.1} to {:.1} s; pass `replace: true` to overwrite it, or choose another stretch",
                        c.n,
                        c.lane.name(),
                        other.name,
                        secs(other.start),
                        secs(other.end())
                    ),
                ));
            }
        }
    }
    let label = format!("Place Sound List ({} sound{})", cues.len(), if cues.len() == 1 { "" } else { "s" });
    let (placed, made, looped) = s.edit_group(&label, |s| {
        let mut made: Vec<&'static str> = Vec::new();
        let mut all: Vec<ClipId> = Vec::new();
        let mut looped = 0usize;
        for (c, (pieces, seam)) in cues.iter().zip(&plans) {
            let track = ensure_lane(s, c.lane, &mut made)?;
            let mut clips: Vec<ClipId> = Vec::with_capacity(pieces.len());
            for piece in pieces {
                let range = TimeRange::new(piece.source, piece.length);
                let placed = place_item(s, c.item, range, piece.at, None, Some(track), false, "Place Sound", None)?;
                let clip = placed.first().copied().ok_or_else(|| bad(CMD, format!("entry {}: nothing was placed", c.n)))?;
                clips.push(clip);
            }
            // the lane's audio type, the clip's own gain and why it is there
            let ids: Vec<u64> = clips.iter().map(|c| c.0).collect();
            s.execute("essentialSound.setType", json!({"clips": ids, "type": c.lane.type_word()}))?;
            let (gain, reason) = (c.gain_db, c.reason.clone());
            let first = clips.first().copied();
            s.edit("Sound Details", |pr, _| {
                let marker = MarkerId(pr.alloc_id());
                let seq = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
                for id in &clips {
                    if let Some((_, it)) = seq.find_item_mut(*id) {
                        it.gain_db = gain;
                    }
                }
                if let (false, Some(id)) = (reason.is_empty(), first)
                    && let Some((_, it)) = seq.find_item_mut(id)
                {
                    it.markers.push(Marker {
                        id: marker,
                        start: Tick::ZERO,
                        duration: Tick::ZERO,
                        name: "Why this sound".into(),
                        comment: reason,
                        kind: MarkerKind::Comment,
                        color: Label::Green,
                    });
                }
                Ok(())
            })?;
            let fail = |what: &str, e: EngineError| bad(CMD, format!("entry {}: {what}: {e}", c.n));
            // crossfades at the seams of a looped sound, then the fades at its ends
            if *seam > Tick::ZERO {
                looped += 1;
                for clip in clips.iter().skip(1) {
                    let frames = frames_of(s, *seam);
                    s.execute("sequence.applyAudioTransition", json!({"clip": clip.0, "edge": "in", "frames": frames, "effect": "constant_power"}))
                        .map_err(|e| fail("the repeat could not be crossfaded", e))?;
                }
            }
            if c.fade_in > Tick::ZERO
                && let Some(clip) = clips.first()
            {
                let frames = frames_of(s, c.fade_in);
                s.execute("sequence.applyAudioTransition", json!({"clip": clip.0, "edge": "in", "frames": frames, "effect": "constant_power"}))
                    .map_err(|e| fail("the fade in could not be made", e))?;
            }
            if c.fade_out > Tick::ZERO
                && let Some(clip) = clips.last()
            {
                let frames = frames_of(s, c.fade_out);
                s.execute("sequence.applyAudioTransition", json!({"clip": clip.0, "edge": "out", "frames": frames, "effect": "constant_power"}))
                    .map_err(|e| fail("the fade out could not be made", e))?;
            }
            all.extend(clips);
        }
        Ok((all, made, looped))
    })?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let lanes: Vec<Value> = Lane::ALL
        .into_iter()
        .filter(|l| cues.iter().any(|c| c.lane == *l))
        .map(|l| {
            let name = lane_track(s, l).and_then(|id| q.audio_tracks.iter().position(|t| t.id == id)).map_or_else(String::new, |i| format!("A{}", i + 1));
            json!({"lane": l.name(), "track": name, "sounds": cues.iter().filter(|c| c.lane == l).count()})
        })
        .collect();
    Ok(json!({"sounds": cues.len(), "clips": placed.iter().map(|c| c.0).collect::<Vec<_>>(), "lanes": lanes, "tracksMade": made, "looped": looped}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "timeline.placeSoundList",
        label: "Place a Sound List",
        menu: &[],
        shortcut: None,
        params: r#"{"entries":[{"start":time,"end":time,"item":id,"lane":"music|ambience|effects","sourceIn":time?,"gainDb":-60..12?,"fadeInSeconds":0..60?,"fadeOutSeconds":0..60?,"loop":bool?,"reason":str?}],"replace":bool?}"#,
        enabled: has_seq,
        run: place_sound_list,
        journal: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(seconds: f64) -> Tick {
        Tick::from_seconds_f64(seconds)
    }

    const FRAME: Tick = Tick(254_016_000_000 / 25);

    #[test]
    fn a_sound_long_enough_is_one_clip_and_a_short_one_without_loop_is_not_tiled() {
        let (p, seam) = tile(t(2.0), t(12.0), t(1.0), t(30.0), FRAME, false).unwrap();
        assert_eq!((p.len(), seam), (1, Tick::ZERO));
        assert_eq!((p[0].at, p[0].length, p[0].source), (t(2.0), t(10.0), t(1.0)));
        assert!(tile(t(0.0), t(60.0), t(0.0), t(20.0), FRAME, false).is_none());
    }

    #[test]
    fn a_short_sound_is_laid_end_to_end_with_each_seam_inside_the_sounds_own_spare_ends() {
        let start = t(5.0);
        let end = t(65.0);
        let (pieces, seam) = tile(start, end, t(0.0), t(10.0), FRAME, true).unwrap();
        // a seam of about a second: half of it from the sound's end and half from its start are used by each
        assert_eq!(seam, Tick(FRAME.0 * 24), "a seam of about a second, in an even number of frames");
        assert!(pieces.len() >= 6, "{}", pieces.len());
        let half = Tick(seam.0 / 2);
        assert_eq!((pieces[0].at, pieces[0].source, pieces[0].length), (start, t(0.0), t(10.0) - half));
        for pair in pieces.windows(2) {
            assert_eq!(pair[0].at + pair[0].length, pair[1].at, "pieces touch: the crossfade is centred on the cut");
            // the outgoing clip has `half` of the sound left after its end, the incoming `half` before its start
            assert!(pair[0].source + pair[0].length + half <= t(10.0), "{pair:?}");
            assert!(pair[1].source >= half, "{pair:?}");
            assert!(pair[1].length >= seam, "{pair:?}");
        }
        let last = pieces.last().unwrap();
        assert_eq!(last.at + last.length, end, "the bed ends where the stretch does");
        assert!(pieces.iter().all(|p| p.length.0 % FRAME.0 == 0 && p.at.0 % FRAME.0 == 0));
    }

    #[test]
    fn a_tail_too_short_for_its_own_seam_is_made_a_whole_seam_by_the_clip_before() {
        // 10 s sounds, a 0.96 s seam, bodies of 9.04 s: a stretch that leaves 0.36 s over
        let end = t(18.92);
        let (pieces, seam) = tile(t(0.0), end, t(0.0), t(10.0), FRAME, true).unwrap();
        let last = pieces.last().unwrap();
        assert_eq!(last.length, seam);
        assert_eq!(last.at + last.length, end);
        let before = &pieces[pieces.len() - 2];
        assert_eq!(before.at + before.length, last.at);
        assert!(before.length >= seam);
    }

    #[test]
    fn what_cannot_be_looped_is_refused() {
        // too short to hold a seam and a body
        assert!(tile(t(0.0), t(30.0), t(0.0), t(0.1), FRAME, true).is_none());
        // far too many repeats
        assert!(tile(t(0.0), t(100_000.0), t(0.0), t(3.0), FRAME, true).is_none());
        // nothing left of the sound
        assert!(tile(t(0.0), t(5.0), t(0.0), Tick::ZERO, FRAME, true).is_none());
    }

    #[test]
    fn lanes_are_found_by_their_names() {
        assert_eq!(Lane::parse("Score"), Some(Lane::Music));
        assert_eq!(Lane::parse(" SFX "), Some(Lane::Effects));
        assert_eq!(Lane::parse("bed"), Some(Lane::Ambience));
        assert_eq!(Lane::parse("dialogue"), None);
        assert!(Lane::Effects.is_named("SFX") && Lane::Effects.is_named("effects") && !Lane::Effects.is_named("Music"));
        assert!(Lane::Music.is_named("music") && !Lane::Music.is_named("Audio 1"));
    }
}
