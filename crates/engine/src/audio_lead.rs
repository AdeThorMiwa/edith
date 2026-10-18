//! `clip.audioLead` (fork roadmap F16.3): a J cut or an L cut on a shot that has its own sound.
//!
//! A shot placed from footage with sound is a picture clip and an audio clip linked together. Cutting
//! the two at the same moment sounds abrupt; editors let the sound lead (a **J cut**: the next shot's sound
//! begins `frames` before its picture) or trail (an **L cut**: this shot's sound carries on `frames` past its
//! picture). This does that for a clip already on the timeline, as one undo step, and keeps the audio track
//! free of overlaps by taking the same amount off the neighbouring audio clip:
//!
//! - `edge: "in"` (default): the audio clip starts `frames` earlier and shows that much more of its source; the
//!   audio clip before it on the track ends that much earlier.
//! - `edge: "out"`: the audio clip ends `frames` later; the audio clip after it starts that much later.
//!
//! It refuses, with the reason, when the source has no spare sound to use (the audio is already at the start or end
//! of its file), when the move would run past the start of the timeline, when it would swallow the neighbouring clip, or
//! when the shot has no sound of its own (a picture placed by `timeline.placeCutList` has none: narration is
//! its own clip).
//!
//! `clip.audioLead {clip, frames, edge?}`: `clip` is the picture clip or its audio clip; `frames` is 1 to 240.

use filmcraft_project::{ClipId, ItemKind, TrackItem};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, has_seq, str_p, u64_p};
use crate::{EngineError, Result, Session};

const MAX_FRAMES: i64 = 240;

/// How much source an audio clip has at its tail past what it plays now.
fn tail_handle(s: &Session, a: &TrackItem) -> Tick {
    let Some(pi) = s.project.item(a.item) else { return Tick::ZERO };
    let end = match &pi.kind {
        ItemKind::Subclip { range, .. } => range.end(),
        _ => pi.duration(),
    };
    end - (a.source_in + Tick(((a.duration.0 as f64) * a.speed.abs()).round() as i64))
}

fn audio_lead(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "clip.audioLead";
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let given = u64_p(p, "clip").map(ClipId).ok_or_else(|| bad(cmd, "`clip` must be a clip id (the picture clip or its audio clip)"))?;
    let frames = p
        .get("frames")
        .and_then(Value::as_i64)
        .filter(|n| (1..=MAX_FRAMES).contains(n))
        .ok_or_else(|| bad(cmd, format!("`frames` must be a whole number from 1 to {MAX_FRAMES}")))?;
    let edge = match str_p(p, "edge") {
        None | Some("in") => "in",
        Some("out") => "out",
        Some(_) => return Err(bad(cmd, "`edge` must be \"in\" (the sound starts early: a J cut) or \"out\" (the sound carries on: an L cut)")),
    };
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, clip) = q.find_item(given).ok_or_else(|| bad(cmd, format!("there is no clip {}", given.0)))?;
    let is_audio = q.audio_tracks.iter().any(|t| t.item(given).is_some());
    let audio_id = if is_audio {
        given
    } else {
        let link = clip.link.ok_or_else(|| {
            bad(cmd, "that shot has no sound of its own to lead or trail (a picture placed by timeline.placeCutList has none: the narration is its own clip)")
        })?;
        q.audio_tracks
            .iter()
            .flat_map(|t| t.items.iter())
            .find(|i| i.link == Some(link) && i.id != clip.id)
            .map(|i| i.id)
            .ok_or_else(|| bad(cmd, "that shot's linked audio is not on the timeline"))?
    };
    let track = q.audio_tracks.iter().find(|t| t.item(audio_id).is_some()).ok_or_else(|| bad(cmd, "the audio clip is not on an audio track"))?;
    if track.locked {
        return Err(bad(cmd, "the audio track is locked"));
    }
    let a = track.item(audio_id).ok_or_else(|| bad(cmd, "the audio clip is gone"))?.clone();
    let rate = q.settings.frame_rate;
    let n = rate.tick_of(frames);
    let speed = a.speed.abs().max(1e-6);
    let media_n = Tick(((n.0 as f64) * speed).round() as i64);
    // the neighbour whose edge gives way
    let neighbour = if edge == "in" {
        track.items.iter().filter(|i| i.id != a.id && i.end() <= a.start + Tick(1)).max_by_key(|i| i.end()).cloned()
    } else {
        track.items.iter().filter(|i| i.id != a.id && i.start + Tick(1) >= a.end()).min_by_key(|i| i.start).cloned()
    };
    if edge == "in" {
        if a.source_in < media_n {
            return Err(bad(
                cmd,
                format!(
                    "only {:.2} s of sound before this shot exists in its file, so it cannot lead by {:.2} s",
                    a.source_in.seconds().max(0.0),
                    media_n.seconds()
                ),
            ));
        }
        if a.start < n {
            return Err(bad(cmd, "the sound would start before the beginning of the timeline"));
        }
    } else {
        let have = tail_handle(s, &a);
        if have < media_n {
            return Err(bad(
                cmd,
                format!("only {:.2} s of sound after this shot exists in its file, so it cannot trail by {:.2} s", have.seconds().max(0.0), media_n.seconds()),
            ));
        }
    }
    if let Some(nb) = &neighbour {
        // the part of the neighbour that the extension covers
        let touching = if edge == "in" { nb.end() > a.start - n } else { nb.start < a.end() + n };
        if touching && nb.duration <= n {
            return Err(bad(cmd, format!("“{}” is only {:.2} s long and would be swallowed; use a shorter lead", nb.name, nb.duration.seconds())));
        }
    }
    let (aid, nb_id) = (a.id, neighbour.as_ref().map(|i| i.id));
    s.edit("Lead Audio", |pr, _| {
        let seq = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        let track = seq.audio_tracks.iter_mut().find(|t| t.item(aid).is_some()).ok_or_else(|| bad(cmd, "the audio clip is gone"))?;
        if edge == "in" {
            if let Some(nid) = nb_id
                && let Some(nb) = track.item_mut(nid)
                && nb.end() > a.start - n
            {
                let cut = nb.end() - (a.start - n);
                nb.duration -= cut;
            }
            let it = track.item_mut(aid).ok_or_else(|| bad(cmd, "the audio clip is gone"))?;
            it.start -= n;
            it.duration += n;
            it.source_in -= media_n;
        } else {
            if let Some(nid) = nb_id
                && let Some(nb) = track.item_mut(nid)
                && nb.start < a.end() + n
            {
                let cut = (a.end() + n) - nb.start;
                nb.start += cut;
                nb.duration -= cut;
                nb.source_in += Tick(((cut.0 as f64) * nb.speed.abs()).round() as i64);
            }
            let it = track.item_mut(aid).ok_or_else(|| bad(cmd, "the audio clip is gone"))?;
            it.duration += n;
        }
        track.items.sort_by_key(|i| i.start);
        seq.check().map_err(EngineError::Other)?;
        Ok(())
    })?;
    Ok(json!({"clip": aid.0, "edge": edge, "frames": frames, "seconds": n.seconds(), "neighbourTrimmed": nb_id.map(|i| i.0)}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "clip.audioLead",
        label: "Lead or Trail a Clip's Sound",
        menu: &[],
        shortcut: None,
        params: r#"{"clip":id,"frames":1..240,"edge":"in|out"?}"#,
        enabled: has_seq,
        run: audio_lead,
        journal: true,
    }]
}
