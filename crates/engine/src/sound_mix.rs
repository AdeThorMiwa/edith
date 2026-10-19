//! `sound.autoMix` (fork roadmap F17.2): a first mix of narration, music, ambience and effects in one call.
//!
//! What it does, in this order, as one undo step:
//! 1. The **narration** (and any clip you gave the Dialogue type) is Dialogue; the clips in the Music, Ambience and
//!    Effects lanes (`timeline.placeSoundList` makes them) get their lane's audio type if they have none. Footage sound
//!    with no type is left alone and reported.
//! 2. Each type is **matched to a loudness** with Essential Sound's auto-match (BS.1770 integrated loudness of the
//!    clip, then the match gain is set): dialogue -16 LUFS, music -22, ambience -28, effects -20 by default, so the
//!    voice leads and the score and bed sit under it. A target can be given.
//! 3. Music and ambience are **ducked under the voice** (`essentialSound.generateDucking`: the level dips while the
//!    narration speaks, with the pack's fade, and returns in the pauses), unless `duck` is false.
//! 4. The whole mix is **measured before and after** (integrated loudness through the real mixer) and said in words.
//!
//! The mix itself (a bus limiter, the sequence's master level) is left to the person. Asked again it gives the same
//! result: matching and ducking are recomputed, not stacked.
//!
//! `sound.autoMix {dialogueLufs?, musicLufs?, ambienceLufs?, effectsLufs?, duck?}`.

use filmcraft_audio_dsp::LoudnessMeter;
use filmcraft_project::ClipId;
use filmcraft_project::essential::AudioType;
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p, f64_p, has_seq};
use crate::sound_list::{Lane, lane_of};
use crate::{EngineError, Result, Session};

const CMD: &str = "sound.autoMix";
/// The loudness each type is matched to when none is given, LUFS.
pub(crate) const DIALOGUE_LUFS: f64 = -16.0;
const MUSIC_LUFS: f64 = -22.0;
const AMBIENCE_LUFS: f64 = -28.0;
const EFFECTS_LUFS: f64 = -20.0;
/// The longest mix measured, seconds (a longer one is measured to here).
const MAX_MEASURE_SECONDS: f64 = 1800.0;

/// The integrated loudness of the sequence's mix between `from` and `to`, through the mixer exactly as playback and
/// export render it. `None` when it is silent. Capped at [`MAX_MEASURE_SECONDS`].
pub(crate) fn mix_lufs(s: &Session, from: Tick, to: Tick) -> Option<f64> {
    let q = s.active_sequence()?;
    let sr = q.settings.sample_rate.max(1);
    let provider = s.media.full_res_provider(s.project.clone(), s.services.clone());
    let a = from.to_units_floor(i64::from(sr)).max(0);
    let b = to.to_units_floor(i64::from(sr)).min(a.saturating_add((MAX_MEASURE_SECONDS * f64::from(sr)) as i64));
    let mut meter = LoudnessMeter::new(f64::from(sr), 2);
    let mut pos = a;
    while pos < b {
        let n = (b - pos).min(i64::from(sr)) as usize;
        let mix = filmcraft_render::audio::mix_sequence(&s.project, q, pos, n, &provider);
        let (l, r) = (mix.channels.first()?, mix.channels.get(1).or(mix.channels.first())?);
        meter.process(&[l, r]);
        pos += n as i64;
    }
    Some(meter.integrated()).filter(|v| v.is_finite())
}

/// The audio clips of the active sequence by what they are: the narration and other dialogue, then each lane.
#[derive(Default)]
struct Sorted {
    /// Untyped narration clips to be typed Dialogue.
    to_type_dialogue: Vec<ClipId>,
    dialogue: Vec<ClipId>,
    /// Untyped clips in a lane's track, to be given the lane's type.
    to_type: Vec<(Lane, ClipId)>,
    lanes: [Vec<ClipId>; 3],
    /// Footage sound with no type, left alone.
    left_alone: usize,
}

fn sort_clips(s: &Session) -> Sorted {
    let mut out = Sorted::default();
    let Some(q) = s.active_sequence() else { return out };
    for t in q.audio_tracks.iter().filter(|t| t.enabled) {
        for it in t.items.iter().filter(|i| i.enabled) {
            let narration = s.project.narrations.contains_key(&it.item);
            match (it.essential.as_ref().map(|e| e.kind), narration) {
                (Some(AudioType::Dialogue), _) => out.dialogue.push(it.id),
                (None, true) => {
                    out.to_type_dialogue.push(it.id);
                    out.dialogue.push(it.id);
                }
                _ => match lane_of(s, it, &t.name) {
                    Some(lane) => {
                        if it.essential.is_none() {
                            out.to_type.push((lane, it.id));
                        }
                        let k = Lane::ALL.iter().position(|l| *l == lane).unwrap_or(0);
                        if let Some(v) = out.lanes.get_mut(k) {
                            v.push(it.id);
                        }
                    }
                    None => out.left_alone += usize::from(it.essential.is_none()),
                },
            }
        }
    }
    out
}

fn target(p: &Value, key: &str, default: f64) -> Result<f64> {
    match p.get(key).filter(|v| !v.is_null()) {
        None => Ok(default),
        Some(_) => f64_p(p, key)
            .filter(|x| x.is_finite() && (-60.0..=0.0).contains(x))
            .ok_or_else(|| bad(CMD, format!("`{key}` must be a loudness from -60 to 0 LUFS"))),
    }
}

fn lufs_text(v: Option<f64>) -> String {
    v.map_or_else(|| "silent".to_string(), |x| format!("{x:.1} LUFS"))
}

fn auto_mix(s: &mut Session, p: &Value) -> Result<Value> {
    let targets = [target(p, "musicLufs", MUSIC_LUFS)?, target(p, "ambienceLufs", AMBIENCE_LUFS)?, target(p, "effectsLufs", EFFECTS_LUFS)?];
    let dialogue_target = target(p, "dialogueLufs", DIALOGUE_LUFS)?;
    let duck = bool_p(p, "duck").unwrap_or(true);
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let end = q.duration();
    let sorted = sort_clips(s);
    let any = !sorted.dialogue.is_empty() || sorted.lanes.iter().any(|l| !l.is_empty());
    if !any {
        return Err(bad(
            CMD,
            "there is no narration and no sound in the Music, Ambience or Effects lanes to mix (timeline.placeSoundList puts sound in lanes)",
        ));
    }
    let before = mix_lufs(s, Tick::ZERO, end);
    let mut notes: Vec<String> = Vec::new();
    let mut groups: Vec<Value> = Vec::new();
    s.edit_group("Auto-Mix Sound", |s| {
        // 1. the types
        if !sorted.to_type_dialogue.is_empty() {
            let ids: Vec<u64> = sorted.to_type_dialogue.iter().map(|c| c.0).collect();
            s.execute("essentialSound.setType", json!({"clips": ids, "type": "dialogue"}))?;
        }
        for lane in Lane::ALL {
            let ids: Vec<u64> = sorted.to_type.iter().filter(|(l, _)| *l == lane).map(|(_, c)| c.0).collect();
            if !ids.is_empty() {
                let word = match lane {
                    Lane::Music => "music",
                    Lane::Ambience => "ambience",
                    Lane::Effects => "sfx",
                };
                s.execute("essentialSound.setType", json!({"clips": ids, "type": word}))?;
            }
        }
        // 2. the loudness of each type
        let mut matched = |label: &str, ids: &[ClipId], lufs: f64| -> Result<()> {
            if ids.is_empty() {
                return Ok(());
            }
            let ids_json: Vec<u64> = ids.iter().map(|c| c.0).collect();
            match s.execute("essentialSound.autoMatch", json!({"clips": ids_json, "target": lufs})) {
                Ok(r) => {
                    let done = r["clips"].as_array().map_or(0, |a| a.iter().filter(|c| c.get("gainDb").is_some()).count());
                    let silent = ids.len() - done.min(ids.len());
                    if silent > 0 {
                        notes.push(format!(
                            "{silent} {label} clip{} silent, left as {} {}",
                            if silent == 1 { " is" } else { "s are" },
                            if silent == 1 { "it" } else { "they" },
                            if silent == 1 { "is" } else { "are" }
                        ));
                    }
                    groups.push(json!({"type": label, "clips": ids.len(), "targetLufs": lufs, "matched": done}));
                }
                Err(e) => notes.push(format!("the {label} could not be matched: {e}")),
            }
            Ok(())
        };
        matched("narration", &sorted.dialogue, dialogue_target)?;
        for (k, lane) in Lane::ALL.iter().enumerate() {
            if let Some(ids) = sorted.lanes.get(k) {
                matched(&lane.name().to_ascii_lowercase(), ids, targets.get(k).copied().unwrap_or(MUSIC_LUFS))?;
            }
        }
        // 3. music and ambience duck under the voice
        let bed: Vec<u64> = [Lane::Music, Lane::Ambience]
            .iter()
            .filter_map(|l| Lane::ALL.iter().position(|x| x == l))
            .filter_map(|k| sorted.lanes.get(k))
            .flatten()
            .map(|c| c.0)
            .collect();
        if duck && !bed.is_empty() {
            if sorted.dialogue.is_empty() {
                notes.push("there is no narration to duck the music and ambience under".into());
            } else {
                s.execute("essentialSound.generateDucking", json!({"clips": bed}))?;
                groups.push(json!({"type": "ducking", "clips": bed.len()}));
            }
        }
        Ok(())
    })?;
    let after = mix_lufs(s, Tick::ZERO, end);
    let ducked = groups.iter().find(|g| g["type"] == "ducking").and_then(|g| g["clips"].as_u64()).unwrap_or(0);
    let mut said = Vec::new();
    for g in &groups {
        if g["type"] != "ducking" {
            said.push(format!("{} to {:.0} LUFS", g["type"].as_str().unwrap_or("?"), g["targetLufs"].as_f64().unwrap_or(0.0)));
        }
    }
    let mut summary = format!("Matched {}.", said.join(", "));
    if ducked > 0 {
        summary.push_str(&format!(
            " The music and ambience ({ducked} clip{}) dip under the voice and come back in the pauses.",
            if ducked == 1 { "" } else { "s" }
        ));
    }
    summary.push_str(&format!(" The whole mix measured {} before and {} after.", lufs_text(before), lufs_text(after)));
    if sorted.left_alone > 0 {
        notes.push(format!(
            "{} clip{} of footage sound with no audio type {} left as {} {}",
            sorted.left_alone,
            if sorted.left_alone == 1 { "" } else { "s" },
            if sorted.left_alone == 1 { "was" } else { "were" },
            if sorted.left_alone == 1 { "it" } else { "they" },
            if sorted.left_alone == 1 { "is" } else { "are" }
        ));
    }
    Ok(json!({
        "summary": summary,
        "beforeLufs": before, "afterLufs": after,
        "groups": groups, "notes": notes,
        "footageSoundLeftAlone": sorted.left_alone,
    }))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "sound.autoMix",
        label: "Auto-Mix the Sound",
        menu: &[],
        shortcut: None,
        params: r#"{"dialogueLufs":-60..0?,"musicLufs":-60..0?,"ambienceLufs":-60..0?,"effectsLufs":-60..0?,"duck":bool?}"#,
        enabled: has_seq,
        run: auto_mix,
        journal: true,
    }]
}
