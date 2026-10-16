//! Camera moves for stills and footage (fork roadmap F07.8): `motion.cameraMove` puts keyframes on
//! a clip's Motion position and scale so the picture slowly pushes in, pulls out, drifts or sways,
//! the kind of move often called a Ken Burns move.
//!
//! The move is sized to the clip: it starts at the clip's first frame and ends at its last, however
//! long the clip is. Every move enlarges the picture enough that its edges stay outside the frame
//! (so no black edges appear for media that fills the frame), and the amount is a share of the
//! frame, not pixels, so it looks the same at any size. It replaces the clip's Motion position and
//! scale keyframes (its other Motion settings are left alone) and is one undo step.
//!
//! | `move` | does |
//! |---|---|
//! | `pushIn` | slowly gets closer: scale from 100 % up to 100 % + 0.3 × `amount` |
//! | `pullOut` | the opposite: starts close and ends at 100 % |
//! | `driftLeft`, `driftRight` | a slow slide sideways by up to 8 % of the frame width × `amount`/100, enlarged to cover |
//! | `sway` | a gentle handheld wander (small moves in both directions every half second or so, seeded by `seed`), up to 3 % of the frame × `amount`/100 |
//!
//! `amount` is 0 to 100 (default 50). `motion.cameraMove {clip?, move, amount?, seed?}`; without
//! `clip` it takes the first selected video clip.

use filmcraft_geom::Vec2;
use filmcraft_project::{ClipId, EffectInstance, Interpolation, Keyframe, Param, ParamValue};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, clip_p, has_seq, str_p, u64_p};
use crate::{EngineError, Result, Session};

/// The moves, by the name `move` takes.
pub const MOVES: [&str; 5] = ["pushIn", "pullOut", "driftLeft", "driftRight", "sway"];

/// One keyframe of a move: fraction through the clip, scale in percent, position as an offset from
/// the frame centre in fractions of the frame width and height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub at: f64,
    pub scale: f64,
    pub dx: f64,
    pub dy: f64,
}

/// A small deterministic number in −1..1 from `seed`, `i` and `k`.
fn noise(seed: u64, i: u64, k: u64) -> f64 {
    let mut h = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ i.wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ k.wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 29;
    ((h >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
}

/// The keyframes of `mv` at `amount` (0 to 100) for a clip of `seconds`, or None for an unknown move.
pub fn plan(mv: &str, amount: f64, seed: u64, seconds: f64) -> Option<Vec<Key>> {
    let a = if amount.is_finite() { amount.clamp(0.0, 100.0) / 100.0 } else { 0.0 };
    let key = |at: f64, scale: f64, dx: f64, dy: f64| Key { at, scale, dx, dy };
    Some(match mv {
        "pushIn" => vec![key(0.0, 100.0, 0.0, 0.0), key(1.0, 100.0 + 30.0 * a, 0.0, 0.0)],
        "pullOut" => vec![key(0.0, 100.0 + 30.0 * a, 0.0, 0.0), key(1.0, 100.0, 0.0, 0.0)],
        "driftLeft" | "driftRight" => {
            let d = 0.08 * a;
            let sign = if mv == "driftLeft" { -1.0 } else { 1.0 };
            // the picture slides from one side of the centre to the other (right: its centre moves
            // right); the enlargement covers the whole travel
            let scale = 100.0 * (1.0 + 2.0 * d);
            vec![key(0.0, scale, -sign * d, 0.0), key(1.0, scale, sign * d, 0.0)]
        }
        "sway" => {
            let m = 0.03 * a;
            let scale = 100.0 * (1.0 + 2.0 * m);
            // a point about every 0.6 s, at most 60 of them, always one at each end
            let n = ((seconds.max(0.0) / 0.6).round() as usize).clamp(1, 60);
            (0..=n).map(|i| key(i as f64 / n as f64, scale, noise(seed, i as u64, 1) * m, noise(seed, i as u64, 2) * m * 0.6)).collect()
        }
        _ => return None,
    })
}

fn selected_video_clip(s: &Session) -> Option<ClipId> {
    let seq = s.active_sequence()?;
    s.state.selection.iter().copied().find(|c| seq.video_tracks.iter().any(|t| t.items.iter().any(|i| i.id == *c)))
}

fn camera_move(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "motion.cameraMove";
    let mv = str_p(p, "move").ok_or_else(|| bad(cmd, format!("`move` is required: one of {}", MOVES.join(", "))))?;
    if !MOVES.contains(&mv) {
        return Err(bad(cmd, format!("unknown move `{mv}` (one of {})", MOVES.join(", "))));
    }
    let amount = match p.get("amount") {
        None | Some(Value::Null) => 50.0,
        Some(v) => v.as_f64().filter(|a| a.is_finite() && (0.0..=100.0).contains(a)).ok_or_else(|| bad(cmd, "`amount` must be a number from 0 to 100"))?,
    };
    if p.get("seed").is_some_and(|v| !v.is_null() && v.as_u64().is_none()) {
        return Err(bad(cmd, "`seed` must be a whole number"));
    }
    let seed = u64_p(p, "seed").unwrap_or(0);
    let clip = match p.get("clip").filter(|v| !v.is_null()) {
        Some(_) => clip_p(p, "clip").ok_or_else(|| bad(cmd, "`clip` must be a clip id"))?,
        None => selected_video_clip(s).ok_or_else(|| bad(cmd, "select a video clip (or pass `clip`)"))?,
    };
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (track, item) = seq.find_item(clip).ok_or_else(|| bad(cmd, format!("no clip {}", clip.0)))?;
    let video = seq.video_tracks.iter().find(|t| t.id == track).ok_or_else(|| bad(cmd, "that clip is not on a video track"))?;
    if video.locked {
        return Err(bad(cmd, "the clip's track is locked"));
    }
    let (w, h) = (f64::from(seq.settings.width), f64::from(seq.settings.height));
    if !(w >= 1.0 && h >= 1.0) {
        return Err(bad(cmd, "the sequence has no frame size"));
    }
    let seconds = item.duration.seconds();
    if seconds.is_nan() || seconds <= 0.0 {
        return Err(bad(cmd, "the clip has no length"));
    }
    let keys = plan(mv, amount, seed, seconds).ok_or_else(|| bad(cmd, "unknown move"))?;
    // each key's media time: where the clip is at that fraction of its length
    let (start, dur) = (item.start, item.duration);
    let times: Vec<Tick> = keys
        .iter()
        .map(|k| {
            let at = Tick(start.0.saturating_add((dur.0 as f64 * k.at.clamp(0.0, 1.0)) as i64));
            item.effect_time_at(at)
        })
        .collect();
    let count = keys.len();
    s.edit_sequence("Camera Move", move |q, _, _| {
        let (_, it) = q.find_item_mut(clip).ok_or_else(|| bad(cmd, "the clip was deleted"))?;
        if it.effect("motion").is_none() {
            it.effects.insert(0, filmcraft_project::find_effect("motion").map(|d| d.instance()).ok_or_else(|| EngineError::Other("no Motion effect".into()))?);
        }
        let m: &mut EffectInstance = it.effects.iter_mut().find(|e| e.effect == "motion").ok_or_else(|| EngineError::Other("no Motion effect".into()))?;
        let mut pos: Vec<Keyframe> = Vec::new();
        let mut scale: Vec<Keyframe> = Vec::new();
        for (k, t) in keys.iter().zip(&times) {
            let mut kp = Keyframe::new(*t, ParamValue::Vec2(Vec2::new(w / 2.0 + k.dx * w, h / 2.0 + k.dy * h)));
            let mut ks = Keyframe::new(*t, ParamValue::Float(k.scale));
            // pushes and pulls ease in and out; drifts and sways stay smooth
            kp.interp = Interpolation::AutoBezier;
            ks.interp = Interpolation::AutoBezier;
            pos.push(kp);
            scale.push(ks);
        }
        // a reversed clip plays its keys backwards in media time: keep them sorted
        pos.sort_by_key(|k| k.time);
        scale.sort_by_key(|k| k.time);
        pos.dedup_by_key(|k| k.time);
        scale.dedup_by_key(|k| k.time);
        for (id, list) in [("position", pos), ("scale", scale)] {
            let first = list.first().map(|k| k.value.clone()).unwrap_or(ParamValue::Float(100.0));
            let param = m.params.entry(id.to_string()).or_insert_with(|| Param::new(first.clone()));
            param.value = first;
            param.keyframes = list;
        }
        // the move scales both ways together
        if let Some(u) = m.params.get_mut("uniform_scale") {
            u.value = ParamValue::Bool(true);
        }
        Ok(())
    })?;
    let _ = seq_id;
    Ok(json!({"clip": clip.0, "move": mv, "amount": amount, "keyframes": count, "seconds": seconds}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "motion.cameraMove",
        label: "Camera Move",
        menu: &[],
        shortcut: None,
        params: r#"{"clip":id?,"move":"pushIn|pullOut|driftLeft|driftRight|sway","amount":0..100?,"seed":u64?}"#,
        enabled: has_seq,
        run: camera_move,
        journal: true,
    }]
}
