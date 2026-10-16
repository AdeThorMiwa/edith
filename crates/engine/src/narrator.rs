//! Narrator voices (fork roadmap F06.4): a saved way your narration sounds, kept for every project.
//! A narrator voice is the voice, pace and vocal pitch of a narration plus the effects on its track
//! (up to five mixer inserts: a high-pass, an EQ, a compressor, a de-esser, a touch of room), so a
//! series keeps one sound without setting it up again each time.
//!
//! Applying one **copies** it: its effects are put on the narration's track and its settings on the
//! narration, so a project stays complete without this file. Two starting points are built in,
//! *Documentary* and *Clean* (chosen from the effects' ranges, not tuned by ear: save your own); they
//! carry effects only and leave the narration's voice, pace and pitch alone.
//!
//! The saved voices are in `edith-narrator-voices.json` beside the preferences (in memory only when
//! there is no data directory, as in tests). Everything read from it is untrusted: an entry that
//! does not check out is dropped, an effect that is not an audio effect is dropped, parameters are
//! limited to the effect's own and clamped to its range, and at most [`MAX_VOICES`] are kept.
//!
//! | command | does |
//! |---|---|
//! | `tts.voicePresets {clip?}` | the built-in and saved narrator voices with their effects; with a narration clip (or one selected) also whether its effects are on |
//! | `tts.saveVoicePreset {name, clip?, strip?, voice?, pace?, pitch?}` | saves a narrator voice: settings from the narration clip (or the parameters), effects from that clip's track (or `strip`); the same name replaces yours; a built-in name is refused |
//! | `tts.deleteVoicePreset {name}` | removes one of yours |
//! | `tts.applyVoicePreset {name, clip?}` | puts the voice on a narration clip: replaces its track's effects and renders it again with the voice's settings; one undo step |
//! | `tts.compareVoice {clip?, plain?}` | turns the effects on the narration's track off (`plain: true`) or on, to hear the plain voice; without `plain` it flips; one undo step |
//!
//! `tts.create` also takes `preset`: the voice's settings (unless given) and its effects on the
//! track the narration is placed on.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use filmcraft_project::narration::{MAX_PACE, MIN_PACE};
use filmcraft_project::{EffectKind, ParamKind, ParamValue, TrackId, VocalPitch, find_effect};
use filmcraft_tts as tts;

use crate::commands::{CommandSpec, always, bad, str_p};
use crate::{EngineError, Result, Session};

pub(crate) const FILE: &str = "edith-narrator-voices.json";
const MAX_FILE: u64 = 4 * 1024 * 1024;
/// Most saved voices kept.
pub const MAX_VOICES: usize = 100;
/// Most parameters kept per effect.
const MAX_PARAMS: usize = 64;
const MAX_NAME: usize = 60;

/// One effect on the narration's track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Insert {
    /// Audio effect id (`highpass`, `parametric_eq`, `dynamics`, `deesser`, `studio_reverb`, …).
    pub effect: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub post_fader: bool,
    /// Parameter id → number, or true/false for a switch.
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
}

fn yes() -> bool {
    true
}

/// A narrator voice. A missing voice, pace or pitch means "leave the narration's own".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NarratorVoice {
    pub name: String,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub pace: Option<f64>,
    /// A vocal pitch id (`low`, `default`, …).
    #[serde(default)]
    pub pitch: Option<String>,
    #[serde(default)]
    pub inserts: Vec<Insert>,
}

/// The saved narrator voices, read from disk the first time they are needed.
#[derive(Default)]
pub struct Presets {
    mine: Mutex<Option<Vec<NarratorVoice>>>,
}

fn file_path(s: &Session) -> Option<PathBuf> {
    s.prefs_path.as_ref().and_then(|p| p.parent()).map(|d| d.join(FILE))
}

// ---------------------------------------------------------------------------- checking

fn tidy_name(name: &str) -> Option<String> {
    let n = name.trim();
    (!n.is_empty() && n.chars().count() <= MAX_NAME && !n.chars().any(char::is_control)).then(|| n.to_string())
}

/// An insert with only real parameters of a real audio effect, numbers kept within the effect's own
/// range; None when the effect is not an audio effect.
fn tidy_insert(i: &Insert) -> Option<Insert> {
    let def = find_effect(&i.effect).filter(|d| d.kind == EffectKind::Audio && !d.intrinsic)?;
    let mut params = BTreeMap::new();
    for (k, v) in i.params.iter().take(MAX_PARAMS) {
        let Some(pd) = def.param(k) else { continue };
        let kept = match (&pd.kind, v) {
            (ParamKind::Float { min, max, .. }, Value::Number(n)) => n.as_f64().filter(|x| x.is_finite()).map(|x| json!(x.clamp(*min, *max))),
            (ParamKind::Bool, Value::Bool(b)) => Some(json!(b)),
            (ParamKind::Choice(list), Value::Number(n)) => n.as_u64().filter(|c| (*c as usize) < list.len()).map(|c| json!(c)),
            _ => None,
        };
        if let Some(kept) = kept {
            params.insert(k.clone(), kept);
        }
    }
    Some(Insert { effect: def.id.to_string(), enabled: i.enabled, post_fader: i.post_fader, params })
}

/// The settings checked: a known voice, a pace in range, a vocal pitch id.
fn tidy_settings(voice: &Option<String>, pace: Option<f64>, pitch: &Option<String>) -> std::result::Result<(), String> {
    if let Some(v) = voice
        && !tts::voices_in(None).iter().any(|i| i.id == v)
    {
        return Err(format!("unknown voice `{v}` (see tts.voices)"));
    }
    if let Some(p) = pace
        && !(p.is_finite() && (MIN_PACE..=MAX_PACE).contains(&p))
    {
        return Err(format!("pace must be between {MIN_PACE} and {MAX_PACE}"));
    }
    if let Some(p) = pitch
        && VocalPitch::from_id(p).is_none()
    {
        return Err(format!("unknown pitch `{p}` (extraLow, low, default, high, extraHigh)"));
    }
    Ok(())
}

/// A voice read from a file or built in, kept only if it checks out (bad inserts are dropped).
fn tidy_voice(v: &NarratorVoice) -> Option<NarratorVoice> {
    let name = tidy_name(&v.name)?;
    tidy_settings(&v.voice, v.pace, &v.pitch).ok()?;
    let inserts: Vec<Insert> = v.inserts.iter().filter_map(tidy_insert).take(filmcraft_project::mixer::MAX_INSERTS).collect();
    Some(NarratorVoice { name, voice: v.voice.clone(), pace: v.pace, pitch: v.pitch.clone(), inserts })
}

// ---------------------------------------------------------------------------- the list

fn ins(effect: &str, params: &[(&str, Value)]) -> Insert {
    Insert { effect: effect.to_string(), enabled: true, post_fader: false, params: params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect() }
}

/// The two built-in starting points (effects only).
pub fn builtins() -> Vec<NarratorVoice> {
    let voice = |name: &str, inserts: Vec<Insert>| NarratorVoice { name: name.into(), voice: None, pace: None, pitch: None, inserts };
    vec![
        voice(
            "Documentary",
            vec![
                ins("highpass", &[("cutoff", json!(80.0))]),
                ins(
                    "parametric_eq",
                    &[
                        ("low_on", json!(true)),
                        ("low_freq", json!(110.0)),
                        ("low_gain", json!(1.5)),
                        ("b5_on", json!(true)),
                        ("b5_freq", json!(3500.0)),
                        ("b5_gain", json!(2.0)),
                        ("b5_q", json!(1.0)),
                    ],
                ),
                ins("dynamics", &[("threshold", json!(-22.0)), ("ratio", json!(3.0)), ("attack", json!(12.0)), ("release", json!(140.0))]),
                ins("deesser", &[("frequency", json!(6500.0)), ("threshold", json!(-16.0)), ("reduction", json!(6.0))]),
                ins("studio_reverb", &[("room", json!(25.0)), ("decay", json!(25.0)), ("damping", json!(60.0)), ("dry", json!(100.0)), ("wet", json!(6.0))]),
            ],
        ),
        voice(
            "Clean",
            vec![
                ins("highpass", &[("cutoff", json!(90.0))]),
                ins(
                    "parametric_eq",
                    &[
                        ("b4_on", json!(true)),
                        ("b4_freq", json!(3000.0)),
                        ("b4_gain", json!(1.0)),
                        ("b4_q", json!(1.0)),
                        ("high_on", json!(true)),
                        ("high_freq", json!(9000.0)),
                        ("high_gain", json!(1.0)),
                    ],
                ),
                ins("dynamics", &[("threshold", json!(-20.0)), ("ratio", json!(2.5)), ("attack", json!(10.0)), ("release", json!(120.0))]),
                ins("deesser", &[("frequency", json!(7000.0)), ("threshold", json!(-14.0)), ("reduction", json!(5.0))]),
            ],
        ),
    ]
}

fn read_file(s: &Session) -> Vec<NarratorVoice> {
    let Some(p) = file_path(s) else { return Vec::new() };
    let Ok(meta) = std::fs::metadata(&p) else { return Vec::new() };
    if meta.len() > MAX_FILE {
        return Vec::new();
    }
    let Ok(text) = std::fs::read_to_string(&p) else { return Vec::new() };
    let Ok(list) = serde_json::from_str::<Vec<NarratorVoice>>(&text) else { return Vec::new() };
    let builtin: Vec<String> = builtins().iter().map(|b| b.name.to_lowercase()).collect();
    let mut out: Vec<NarratorVoice> = Vec::new();
    for v in list.iter().take(MAX_VOICES * 4) {
        if out.len() >= MAX_VOICES {
            break;
        }
        let Some(v) = tidy_voice(v) else { continue };
        let key = v.name.to_lowercase();
        if builtin.contains(&key) || out.iter().any(|o| o.name.to_lowercase() == key) {
            continue;
        }
        out.push(v);
    }
    out
}

pub(crate) fn mine(s: &Session) -> Vec<NarratorVoice> {
    s.narrator.mine.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_with(|| read_file(s)).clone()
}

fn set_mine(s: &Session, list: Vec<NarratorVoice>) {
    if let Some(p) = file_path(s)
        && let Ok(json) = serde_json::to_string_pretty(&list)
    {
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
    *s.narrator.mine.lock().unwrap_or_else(PoisonError::into_inner) = Some(list);
}

/// A narrator voice by name (case-insensitive): the built-ins, then yours.
pub fn find(s: &Session, name: &str) -> Option<NarratorVoice> {
    let key = name.trim().to_lowercase();
    builtins().into_iter().chain(mine(s)).find(|v| v.name.to_lowercase() == key)
}

fn is_builtin(name: &str) -> bool {
    let key = name.trim().to_lowercase();
    builtins().iter().any(|b| b.name.to_lowercase() == key)
}

// ---------------------------------------------------------------------------- the narration's track

/// The narration clip a command works on and its track.
fn narration_track(s: &Session, cmd: &str, p: &Value) -> Result<(filmcraft_project::ClipId, TrackId)> {
    let clip = crate::narration::target_clip(s, p).ok_or_else(|| bad(cmd, "select a narration clip (or pass `clip`)"))?;
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (track, _) = seq.find_item(clip).ok_or_else(|| bad(cmd, "no such clip"))?;
    Ok((clip, track))
}

/// The effects now on a strip, as inserts.
fn capture(s: &Session, strip: TrackId) -> Vec<Insert> {
    let Some(seq) = s.active_sequence() else { return Vec::new() };
    let Some(t) = seq.strip(strip) else { return Vec::new() };
    t.effects
        .iter()
        .filter_map(|e| {
            let params = e
                .params
                .iter()
                .filter_map(|(k, p)| {
                    let v = match &p.value {
                        ParamValue::Float(x) if x.is_finite() => json!(x),
                        ParamValue::Bool(b) => json!(b),
                        ParamValue::Choice(c) => json!(c),
                        _ => return None,
                    };
                    Some((k.clone(), v))
                })
                .collect();
            tidy_insert(&Insert { effect: e.effect.clone(), enabled: e.enabled, post_fader: e.post_fader, params })
        })
        .take(filmcraft_project::mixer::MAX_INSERTS)
        .collect()
}

/// Replace the effects on `track` with `inserts` (the caller makes it one undo step).
pub(crate) fn apply_chain(s: &mut Session, inserts: &[Insert], track: TrackId) -> Result<()> {
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let label = crate::mixer::strip_label(seq, track);
    let n = seq.strip(track).map_or(0, |t| t.effects.len());
    for _ in 0..n {
        s.execute("mixer.removeInsert", json!({"strip": label, "slot": 0}))?;
    }
    for (i, ins) in inserts.iter().enumerate() {
        s.execute("mixer.addInsert", json!({"strip": label, "effect": ins.effect, "postFader": ins.post_fader}))?;
        s.execute("mixer.setInsert", json!({"strip": label, "slot": i, "enabled": ins.enabled, "params": ins.params}))?;
    }
    Ok(())
}

/// `p` with the settings of its `preset` filled in where `p` does not give them, for `tts.create`.
/// Returns the voice's effects too, to put on the track once it is placed.
pub(crate) fn with_preset(s: &Session, cmd: &str, p: &Value) -> Result<(Value, Vec<Insert>)> {
    let Some(name) = p.get("preset").filter(|v| !v.is_null()) else { return Ok((p.clone(), Vec::new())) };
    let name = name.as_str().ok_or_else(|| bad(cmd, "`preset` must be the name of a narrator voice"))?;
    let v = find(s, name).ok_or_else(|| bad(cmd, format!("no narrator voice called \"{name}\" (see tts.voicePresets)")))?;
    let mut q = p.clone();
    if let Some(o) = q.as_object_mut() {
        if let Some(x) = &v.voice {
            o.entry("voice").or_insert_with(|| json!(x));
        }
        if let Some(x) = v.pace {
            o.entry("pace").or_insert_with(|| json!(x));
        }
        if let Some(x) = &v.pitch {
            o.entry("pitch").or_insert_with(|| json!(x));
        }
    }
    Ok((q, v.inserts))
}

// ---------------------------------------------------------------------------- commands

fn voice_json(s: &Session, v: &NarratorVoice, builtin: bool) -> Value {
    let _ = s;
    json!({
        "name": v.name,
        "builtIn": builtin,
        "voice": v.voice,
        "pace": v.pace,
        "pitch": v.pitch,
        "inserts": v.inserts.iter().map(|i| json!({
            "effect": i.effect,
            "name": find_effect(&i.effect).map_or("", |d| d.name),
            "enabled": i.enabled,
            "postFader": i.post_fader,
            "params": i.params,
        })).collect::<Vec<_>>(),
    })
}

fn list_json(s: &Session, p: &Value) -> Value {
    let mut voices: Vec<Value> = builtins().iter().map(|v| voice_json(s, v, true)).collect();
    voices.extend(mine(s).iter().map(|v| voice_json(s, v, false)));
    let track = narration_track(s, "tts.voicePresets", p).ok().and_then(|(_, t)| {
        let seq = s.active_sequence()?;
        let strip = seq.strip(t)?;
        let n = strip.effects.len();
        Some(json!({"inserts": n, "plain": n > 0 && strip.effects.iter().all(|e| !e.enabled)}))
    });
    json!({"voices": voices, "track": track})
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    Ok(list_json(s, p))
}

fn save(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "tts.saveVoicePreset";
    let name = str_p(p, "name").and_then(tidy_name).ok_or_else(|| bad(cmd, format!("`name` is 1 to {MAX_NAME} characters")))?;
    if is_builtin(&name) {
        return Err(bad(cmd, format!("\"{name}\" is a built-in narrator voice; pick another name")));
    }
    for key in ["voice", "pitch"] {
        if p.get(key).is_some_and(|v| !v.is_null() && !v.is_string()) {
            return Err(bad(cmd, format!("`{key}` must be text")));
        }
    }
    if p.get("pace").is_some_and(|v| !v.is_null() && !v.is_number()) {
        return Err(bad(cmd, "`pace` must be a number"));
    }
    // settings and effects from the narration clip, if there is one
    let clip = narration_track(s, cmd, p).ok();
    let from = clip.and_then(|(c, _)| {
        let item = s.active_sequence()?.find_item(c)?.1.item;
        s.project.narrations.get(&item).cloned()
    });
    let voice = str_p(p, "voice").map(str::to_string).or_else(|| from.as_ref().map(|n| n.voice.clone()));
    let pace = p.get("pace").and_then(Value::as_f64).or_else(|| from.as_ref().map(|n| n.pace));
    let pitch = str_p(p, "pitch").map(str::to_string).or_else(|| from.as_ref().map(|n| n.pitch.id().to_string()));
    tidy_settings(&voice, pace, &pitch).map_err(|e| bad(cmd, e))?;
    let strip = match p.get("strip").filter(|v| !v.is_null()) {
        Some(v) => {
            let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
            Some(crate::mixer::strip_ref(seq, v).ok_or_else(|| bad(cmd, format!("no strip {v}")))?)
        }
        None => clip.map(|(_, t)| t),
    };
    if strip.is_none() && clip.is_none() && voice.is_none() && pace.is_none() && pitch.is_none() {
        return Err(bad(cmd, "nothing to save: select a narration clip, or give `voice`, `pace`, `pitch` or `strip`"));
    }
    let inserts = strip.map(|t| capture(s, t)).unwrap_or_default();
    let v = NarratorVoice { name: name.clone(), voice, pace, pitch, inserts };
    let mut all = mine(s);
    let key = name.to_lowercase();
    let replaced = all.iter().any(|o| o.name.to_lowercase() == key);
    all.retain(|o| o.name.to_lowercase() != key);
    if all.len() >= MAX_VOICES {
        return Err(bad(cmd, format!("at most {MAX_VOICES} saved narrator voices; delete one first")));
    }
    all.push(v);
    set_mine(s, all);
    let mut r = list_json(s, p);
    r["saved"] = json!(name);
    r["replaced"] = json!(replaced);
    Ok(r)
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "tts.deleteVoicePreset";
    let name = str_p(p, "name").ok_or_else(|| bad(cmd, "`name` is required text"))?;
    if is_builtin(name) {
        return Err(bad(cmd, "built-in narrator voices cannot be deleted"));
    }
    let key = name.trim().to_lowercase();
    let mut all = mine(s);
    let before = all.len();
    all.retain(|o| o.name.to_lowercase() != key);
    if all.len() == before {
        return Err(bad(cmd, format!("no narrator voice called \"{name}\"")));
    }
    set_mine(s, all);
    Ok(list_json(s, p))
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "tts.applyVoicePreset";
    let name = str_p(p, "name").ok_or_else(|| bad(cmd, "`name` is required text"))?;
    let v = find(s, name).ok_or_else(|| bad(cmd, format!("no narrator voice called \"{name}\" (see tts.voicePresets)")))?;
    let (clip, track) = narration_track(s, cmd, p)?;
    if s.active_sequence().ok_or(EngineError::NoSequence)?.audio_tracks.iter().any(|t| t.id == track && t.locked) {
        return Err(bad(cmd, "the narration's track is locked"));
    }
    s.edit_group("Apply Narrator Voice", |s| {
        apply_chain(s, &v.inserts, track)?;
        if v.voice.is_some() || v.pace.is_some() || v.pitch.is_some() {
            let mut q = json!({"clip": clip.0});
            if let Some(x) = &v.voice {
                q["voice"] = json!(x);
            }
            if let Some(x) = v.pace {
                q["pace"] = json!(x);
            }
            if let Some(x) = &v.pitch {
                q["pitch"] = json!(x);
            }
            if let Some(d) = p.get("dir") {
                q["dir"] = d.clone();
            }
            s.execute("tts.edit", q)?;
        }
        Ok(())
    })?;
    Ok(json!({"applied": v.name, "clip": clip.0, "inserts": v.inserts.len()}))
}

fn compare(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "tts.compareVoice";
    if p.get("plain").is_some_and(|v| !v.is_null() && !v.is_boolean()) {
        return Err(bad(cmd, "`plain` must be true or false"));
    }
    let (_, track) = narration_track(s, cmd, p)?;
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let label = crate::mixer::strip_label(seq, track);
    let effects: Vec<bool> = seq.strip(track).map(|t| t.effects.iter().map(|e| e.enabled).collect()).unwrap_or_default();
    if effects.is_empty() {
        return Err(bad(cmd, "this narration's track has no effects to switch off"));
    }
    // without `plain` it flips: plain when any effect is on, with effects when all are off
    let plain = p.get("plain").and_then(Value::as_bool).unwrap_or_else(|| effects.iter().any(|on| *on));
    s.edit_group(if plain { "Hear Plain Voice" } else { "Hear Voice With Effects" }, |s| {
        for (slot, on) in effects.iter().enumerate() {
            if *on == plain {
                s.execute("mixer.setInsert", json!({"strip": label, "slot": slot, "enabled": !plain}))?;
            }
        }
        Ok(())
    })?;
    Ok(json!({"plain": plain, "inserts": effects.len()}))
}

pub fn commands() -> Vec<CommandSpec> {
    let spec = |id, label, params, run: fn(&mut Session, &Value) -> Result<Value>, journal| CommandSpec {
        id,
        label,
        menu: &[],
        shortcut: None,
        params,
        enabled: always,
        run,
        journal,
    };
    vec![
        spec("tts.voicePresets", "List Narrator Voices", r#"{"clip":id?}"#, list, false),
        spec("tts.saveVoicePreset", "Save Narrator Voice", r#"{"name":str,"clip":id?,"strip":"A1"|id?,"voice":str?,"pace":0.5..2?,"pitch":str?}"#, save, false),
        spec("tts.deleteVoicePreset", "Delete Narrator Voice", r#"{"name":str}"#, delete, false),
        spec("tts.applyVoicePreset", "Apply Narrator Voice", r#"{"name":str,"clip":id?,"dir":str?}"#, apply, true),
        spec("tts.compareVoice", "Hear Plain Voice", r#"{"clip":id?,"plain":bool?}"#, compare, true),
    ]
}
