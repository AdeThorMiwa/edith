//! The planner (fork roadmap F08): the structure of a video (sections, beats, shots) kept in the
//! project next to the edit. It is stored in `Project::ext["edith.planner"]`, so it is undone, saved
//! and recovered with the project. The data types, limits and Format Packs are in `edith-planner`;
//! this is the commands.
//!
//! | command | does |
//! |---|---|
//! | `planner.get` | the plan, with its totals (planned against target length, beats by status) |
//! | `planner.packs` | the Format Packs available (shipped and yours), and any pack file that was skipped |
//! | `planner.create {pack?, minutes\|seconds\|ticks, title?, clues?, replace?}` | a plan, empty or from a pack |
//! | `planner.applyPack {pack, minutes\|seconds\|ticks?, clues?, keepNarration?}` | lay a pack's structure over the plan (keeping the writing of beats with the same name when asked) |
//! | `planner.clear` | remove the plan |
//! | `planner.addSection {name, at?}`, `planner.updateSection {section, name}` | sections |
//! | `planner.addBeat {section, name, at?, …fields}`, `planner.updateBeat {beat, …fields}` | beats; fields: `name`, `purpose`, `intensity` (0 to 10), `minutes`/`seconds`/`ticks`, `narration`, `visualIntent`, `soundNote`, `status` |
//! | `planner.addShot {beat, intent?, shotType?, cameraMove?, seconds?, at?}`, `planner.updateShot {shot, …, source?}` | shots |
//! | `planner.move {beat\|section\|shot, to?, at}` | reorder, or move a beat to another section or a shot to another beat |
//! | `planner.delete {section\|beat\|shot}` | remove one |
//! | `planner.link {beat, start, duration \| clear}` | tie a beat to a range of the timeline |
//! | `planner.buildSkeleton {start?, track?}` | lay the plan on the timeline: a range marker for every beat and an offline placeholder clip for every shot that has no source yet |
//! | `planner.export {path?}`, `planner.import {path\|plan}` | the plan as JSON, and back |
//!
//! Every change is one undo step. Everything from a parameter, a pack file, an import or a project
//! is untrusted: text is checked against its limit (refused, not cut) and every count and length
//! against the planner's limits.
//!
//! Format Packs: the shipped ones are built in; yours are `*.toml` files in a `format-packs` folder
//! beside the preferences (see `docs/format-packs.md`). One with the same id replaces the shipped
//! one; a file that does not check out is skipped and named in `planner.packs`.

use std::path::PathBuf;

use edith_planner::model::{MAX_INTENSITY, MAX_NAME, MAX_NARRATION, MAX_NOTE, MAX_PURPOSE, MAX_TARGET, MAX_TITLE};
use edith_planner::{Beat, FormatPack, Link, Planner, Section, Shot, Status, builtin_packs};
use filmcraft_project::{ClipId, Label, Marker, MarkerId, MarkerKind, Project};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, str_p};
use crate::{EngineError, Result, Session};

/// The key in `Project::ext` that holds the plan.
pub(crate) const KEY: &str = "edith.planner";
const PACKS_DIR: &str = "format-packs";
/// Most pack files looked at, and the largest import read.
const MAX_PACK_FILES: usize = 100;
const MAX_IMPORT_BYTES: u64 = 8 * 1024 * 1024;

// ---------------------------------------------------------------------------- storage

/// The project's plan, if it has a usable one (a damaged value counts as none).
pub fn get(p: &Project) -> Option<Planner> {
    let v = p.ext.get(KEY)?;
    serde_json::from_value::<Planner>(v.clone()).ok().map(Planner::sanitized)
}

fn put(p: &mut Project, plan: Option<Planner>) {
    match plan {
        Some(pl) => {
            p.ext.insert(KEY.to_string(), serde_json::to_value(pl).unwrap_or(Value::Null));
        }
        None => {
            p.ext.remove(KEY);
        }
    }
}

fn plan_of(s: &Session, cmd: &str) -> Result<Planner> {
    get(&s.project).ok_or_else(|| bad(cmd, "there is no plan yet: planner.create makes one"))
}

/// Run `f` on a copy of the plan, check it, and store it as one undo step. Nothing changes when `f`
/// or the check fails.
fn edit<R>(s: &mut Session, cmd: &str, label: &str, f: impl FnOnce(&mut Planner) -> Result<R>) -> Result<R> {
    let mut plan = plan_of(s, cmd)?;
    let r = f(&mut plan)?;
    plan.check().map_err(|e| bad(cmd, e))?;
    s.edit(label, |pr, _| {
        put(pr, Some(plan));
        Ok(())
    })?;
    Ok(r)
}

// ---------------------------------------------------------------------------- packs

fn packs_dir(s: &Session) -> Option<PathBuf> {
    s.prefs_path.as_ref().and_then(|p| p.parent()).map(|d| d.join(PACKS_DIR))
}

/// A pack with where it came from.
pub struct Listed {
    pub pack: FormatPack,
    pub yours: bool,
}

/// The packs available, and the files that were skipped (with why).
pub fn library(s: &Session) -> (Vec<Listed>, Vec<String>) {
    let mut out: Vec<Listed> = builtin_packs().into_iter().map(|pack| Listed { pack, yours: false }).collect();
    let mut problems = Vec::new();
    let Some(dir) = packs_dir(s) else { return (out, problems) };
    let Ok(rd) = std::fs::read_dir(&dir) else { return (out, problems) };
    let mut files: Vec<PathBuf> =
        rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("toml"))).collect();
    files.sort();
    for path in files.into_iter().take(MAX_PACK_FILES) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let text = match std::fs::metadata(&path) {
            Ok(m) if m.len() as usize <= edith_planner::pack::MAX_PACK_BYTES => std::fs::read_to_string(&path).map_err(|e| e.to_string()),
            Ok(_) => Err("the file is too large for a pack".into()),
            Err(e) => Err(e.to_string()),
        };
        match text.and_then(|t| FormatPack::parse(&t)) {
            Ok(pack) => {
                out.retain(|l| l.pack.id != pack.id);
                out.push(Listed { pack, yours: true });
            }
            Err(e) => problems.push(format!("{name}: {e}")),
        }
    }
    (out, problems)
}

fn find_pack(s: &Session, id: &str, cmd: &str) -> Result<FormatPack> {
    let (packs, _) = library(s);
    packs
        .into_iter()
        .find(|l| l.pack.id.eq_ignore_ascii_case(id.trim()))
        .map(|l| l.pack)
        .ok_or_else(|| bad(cmd, format!("no pack called \"{id}\" (see planner.packs)")))
}

// ---------------------------------------------------------------------------- parameters

/// A whole number parameter, strictly: a negative, fractional or non-numeric value is an error.
fn uint(p: &Value, key: &str, cmd: &str) -> Result<Option<u64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .or_else(|| v.as_f64().filter(|f| f.is_finite() && *f >= 0.0 && f.fract() == 0.0 && *f < 9.0e15).map(|f| f as u64))
            .map(Some)
            .ok_or_else(|| bad(cmd, format!("`{key}` must be a whole number, 0 or more"))),
    }
}

fn required_id(p: &Value, key: &str, cmd: &str) -> Result<u64> {
    uint(p, key, cmd)?.filter(|i| *i > 0).ok_or_else(|| bad(cmd, format!("`{key}` must be an id from planner.get")))
}

/// A text parameter within `max` characters: refused when too long or holding control characters.
fn text(p: &Value, key: &str, max: usize, multiline: bool, cmd: &str) -> Result<Option<String>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            if s.chars().count() > max || s.chars().any(|c| c.is_control() && !(multiline && matches!(c, '\n' | '\t'))) {
                return Err(bad(cmd, format!("`{key}` must be text of at most {max} characters without control characters")));
            }
            Ok(Some(s.clone()))
        }
        Some(_) => Err(bad(cmd, format!("`{key}` must be text"))),
    }
}

/// A length from `ticks`, `seconds` or `minutes` (given with an optional prefix, e.g. `startSeconds`).
fn length(p: &Value, prefix: &str, cmd: &str) -> Result<Option<Tick>> {
    let key = |k: &str| if prefix.is_empty() { k.to_string() } else { format!("{prefix}{}{}", k[..1].to_uppercase(), &k[1..]) };
    if let Some(t) = uint(p, &key("ticks"), cmd)? {
        return i64::try_from(t).map(|t| Some(Tick(t))).map_err(|_| bad(cmd, "that length is too large"));
    }
    for (k, per) in [("seconds", 1.0), ("minutes", 60.0)] {
        let name = key(k);
        match p.get(&name) {
            None | Some(Value::Null) => {}
            Some(v) => {
                let x = v
                    .as_f64()
                    .filter(|x| x.is_finite() && *x >= 0.0 && *x <= 24.0 * 3600.0 * 10.0)
                    .ok_or_else(|| bad(cmd, format!("`{name}` must be a number, 0 or more")))?;
                return Ok(Some(Tick((x * per * TICKS_PER_SECOND as f64).round() as i64)));
            }
        }
    }
    Ok(None)
}

fn target_length(p: &Value, cmd: &str) -> Result<Option<Tick>> {
    let t = length(p, "", cmd)?;
    if let Some(t) = t
        && t > MAX_TARGET
    {
        return Err(bad(cmd, "a video can be planned up to 12 hours"));
    }
    Ok(t)
}

fn intensity(p: &Value, cmd: &str) -> Result<Option<u8>> {
    match p.get("intensity") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite() && (0.0..=10.0).contains(x))
            .map(|x| Some((x * 10.0).round() as u8))
            .ok_or_else(|| bad(cmd, "`intensity` must be a number from 0 to 10")),
    }
}

fn status(p: &Value, cmd: &str) -> Result<Option<Status>> {
    match p.get("status") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_str().and_then(Status::from_id).map(Some).ok_or_else(|| bad(cmd, "`status` must be idea, drafted, sourced, cut or done")),
    }
}

/// Put the beat fields in `p` onto `b`.
fn beat_fields(b: &mut Beat, p: &Value, cmd: &str) -> Result<()> {
    if let Some(v) = text(p, "name", MAX_NAME, false, cmd)? {
        if v.trim().is_empty() {
            return Err(bad(cmd, "`name` cannot be empty"));
        }
        b.name = v;
    }
    if let Some(v) = text(p, "purpose", MAX_PURPOSE, true, cmd)? {
        b.purpose = v;
    }
    if let Some(v) = text(p, "narration", MAX_NARRATION, true, cmd)? {
        b.narration = v;
    }
    if let Some(v) = text(p, "visualIntent", MAX_PURPOSE, true, cmd)? {
        b.visual_intent = v;
    }
    if let Some(v) = text(p, "soundNote", MAX_NOTE, true, cmd)? {
        b.sound_note = v;
    }
    if let Some(v) = intensity(p, cmd)? {
        b.intensity = v.min(MAX_INTENSITY);
    }
    if let Some(v) = status(p, cmd)? {
        b.status = v;
    }
    if let Some(v) = target_length(p, cmd)? {
        b.target = v;
    }
    Ok(())
}

fn at_index(p: &Value, len: usize, cmd: &str) -> Result<usize> {
    Ok(uint(p, "at", cmd)?.map_or(len, |a| usize::try_from(a).unwrap_or(usize::MAX).min(len)))
}

// ---------------------------------------------------------------------------- reading

fn secs(t: Tick) -> f64 {
    t.0 as f64 / TICKS_PER_SECOND as f64
}

fn summary(s: &Session, plan: &Planner) -> Value {
    let planned = plan.planned();
    let mut by_status = serde_json::Map::new();
    for st in Status::ALL {
        by_status.insert(st.id().into(), json!(plan.beats().filter(|b| b.status == st).count()));
    }
    let look = plan.pack.as_deref().and_then(|id| find_pack(s, id, "planner.get").ok()).map(|p| p.look_preset).filter(|l| !l.is_empty());
    json!({
        "sections": plan.sections.len(),
        "beats": plan.beat_count(),
        "plannedTicks": planned.0,
        "targetTicks": plan.target.0,
        "plannedSeconds": secs(planned),
        "targetSeconds": secs(plan.target),
        "differenceTicks": planned.0.saturating_sub(plan.target.0),
        "byStatus": by_status,
        "lookPreset": look,
    })
}

fn plan_json(s: &Session, plan: &Planner) -> Value {
    json!({"exists": true, "plan": plan, "summary": summary(s, plan)})
}

fn get_cmd(s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(match get(&s.project) {
        Some(plan) => plan_json(s, &plan),
        None => json!({"exists": false}),
    })
}

fn packs_cmd(s: &mut Session, _p: &Value) -> Result<Value> {
    let (packs, problems) = library(s);
    let rows: Vec<Value> = packs
        .iter()
        .map(|l| {
            let p = &l.pack;
            let beats: usize = p.sections.iter().map(|x| x.beats.len()).sum();
            json!({
                "id": p.id,
                "name": p.name,
                "summary": p.summary,
                "source": if l.yours { "yours" } else { "shipped" },
                "sections": p.sections.len(),
                "beats": beats,
                "typicalMinutes": p.typical_minutes.as_ref().map(|m| json!({"min": m.min, "max": m.max})),
                "lookPreset": p.look_preset,
                "titleStyle": p.title_style,
                "repeat": p.repeat_range().map(|(min, max, default)| json!({"min": min, "max": max, "default": default})),
                "targets": p.targets,
            })
        })
        .collect();
    Ok(json!({"packs": rows, "problems": problems, "folder": packs_dir(s).map(|d| d.to_string_lossy().into_owned())}))
}

// ---------------------------------------------------------------------------- skeleton

/// Where each beat and each shot without a source goes, from `start`, end to end.
struct Layout {
    beats: Vec<(u64, Tick, Tick)>,
    /// (shot id, start, length) for shots with no source.
    shots: Vec<(u64, Tick, Tick)>,
}

fn layout(plan: &Planner, start: Tick) -> Option<Layout> {
    let mut at = start.0;
    let mut out = Layout { beats: Vec::new(), shots: Vec::new() };
    for b in plan.beats() {
        let len = b.target.0.max(0);
        out.beats.push((b.id, Tick(at), Tick(len)));
        if len > 0 && !b.shots.is_empty() {
            // shots share the beat: by their own lengths when every one has one, else equally
            let all_set = b.shots.iter().all(|x| x.target.0 > 0);
            let weights: Vec<u64> = b.shots.iter().map(|x| if all_set { x.target.0 as u64 } else { 1 }).collect();
            let parts = edith_planner::split(len, &weights);
            let mut cursor = at;
            for (x, part) in b.shots.iter().zip(parts) {
                if x.source.is_none() && part > 0 {
                    out.shots.push((x.id, Tick(cursor), Tick(part)));
                }
                cursor = cursor.checked_add(part)?;
            }
        }
        at = at.checked_add(len)?;
    }
    Some(out)
}

fn build_skeleton(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.buildSkeleton";
    let plan = plan_of(s, cmd)?;
    if plan.beat_count() == 0 {
        return Err(bad(cmd, "the plan has no beats yet"));
    }
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let start = length(p, "start", cmd)?.unwrap_or(Tick::ZERO);
    if start > MAX_TARGET {
        return Err(bad(cmd, "`start` is too far along the timeline"));
    }
    let lay = layout(&plan, start)
        .filter(|l| l.beats.last().is_none_or(|(_, st, len)| st.0.saturating_add(len.0) <= 2 * MAX_TARGET.0))
        .ok_or_else(|| bad(cmd, "the plan is longer than the timeline can hold"))?;
    // the placeholders go on one video track that is free where they land
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    // shots that already have a placeholder still on the timeline are left where they are
    let missing: Vec<(u64, Tick, Tick)> = lay
        .shots
        .iter()
        .copied()
        .filter(|(id, _, _)| {
            let clip = plan.find_shot(*id).and_then(|(a, b, c)| plan.sections.get(a)?.beats.get(b)?.shots.get(c)?.clip);
            !clip.is_some_and(|c| seq.find_item(ClipId(c)).is_some())
        })
        .collect();
    let free = |t: &filmcraft_project::Track| {
        !t.locked && !t.items.iter().any(|it| missing.iter().any(|(_, st, len)| it.start < Tick(st.0.saturating_add(len.0)) && *st < it.end()))
    };
    let chosen: Option<String> = match p.get("track").filter(|v| !v.is_null()) {
        Some(v) => {
            let name = v.as_str().ok_or_else(|| bad(cmd, "`track` must be a video track name such as \"V2\""))?;
            let i = (1..=seq.video_tracks.len()).find(|n| format!("V{n}").eq_ignore_ascii_case(name.trim()));
            let t = i.and_then(|n| seq.video_tracks.get(n - 1)).ok_or_else(|| bad(cmd, format!("no video track {name}")))?;
            if t.locked {
                return Err(bad(cmd, format!("{name} is locked")));
            }
            if !missing.is_empty() && !free(t) {
                return Err(bad(cmd, format!("{name} already has clips where the placeholders would go; choose another track, or leave `track` out")));
            }
            Some(format!("V{}", i.unwrap_or(1)))
        }
        None => seq.video_tracks.iter().position(&free).map(|i| format!("V{}", i + 1)),
    };
    let old_markers: Vec<u64> = plan.beats().filter_map(|b| b.link.and_then(|l| l.marker)).collect();
    let beats = lay.beats.clone();
    let shots = lay.shots.clone();
    let made = s.edit_group("Build Plan Skeleton", |s| {
        // 1. one range marker per beat: the ones made last time are moved (replaced), not doubled
        let ids: Vec<u64> = s.edit_sequence("Plan Markers", |q, ctx, _| {
            q.markers.retain(|m| !old_markers.contains(&m.id.0));
            let mut ids = Vec::new();
            for (bid, st, len) in &beats {
                let id = MarkerId(ctx.alloc());
                let (name, comment) = plan.beat_ref(*bid).map(|b| (format!("Beat: {}", b.name), b.purpose.clone())).unwrap_or_default();
                q.markers.push(Marker { id, start: *st, duration: *len, name, comment, kind: MarkerKind::Comment, color: Label::Green });
                ids.push(id.0);
            }
            q.markers.sort_by_key(|m| m.start);
            Ok(ids)
        })?;
        // 2. an offline placeholder for each shot that has none on the timeline
        let mut clips: Vec<(u64, u64)> = Vec::new();
        if !missing.is_empty() {
            let track = match &chosen {
                Some(t) => t.clone(),
                None => {
                    let before = s.active_sequence().map_or(0, |q| q.video_tracks.len());
                    s.execute("sequence.addTracks", json!({"video": 1, "audio": 0}))?;
                    format!("V{}", before + 1)
                }
            };
            for (sid, st, len) in shots.iter().filter(|(id, _, _)| missing.iter().any(|(m, _, _)| m == id)) {
                let intent =
                    plan.find_shot(*sid).and_then(|(a, b, c)| plan.sections.get(a)?.beats.get(b)?.shots.get(c)).map(|x| x.intent.clone()).unwrap_or_default();
                let name: String = if intent.trim().is_empty() { "Shot".into() } else { intent.chars().take(60).collect() };
                let item =
                    s.execute("file.newOfflineFile", json!({"fileName": format!("plan-shot-{sid}.mov"), "name": name, "seconds": secs(*len), "audio": false}))?;
                let item = item["item"].as_u64().ok_or_else(|| EngineError::Other("the placeholder was not made".into()))?;
                let placed = s.execute("timeline.place", json!({"item": item, "track": track, "time": st.0, "audio": false}))?;
                let clip = placed["clips"][0].as_u64().ok_or_else(|| EngineError::Other("the placeholder was not placed".into()))?;
                clips.push((*sid, clip));
            }
        }
        // 3. the plan remembers what was made
        edit(s, cmd, "Plan Skeleton Links", |pl| {
            for ((bid, st, len), mid) in beats.iter().zip(&ids) {
                if let Some(b) = pl.beat_mut(*bid) {
                    b.link = Some(Link { start: *st, duration: *len, marker: Some(*mid) });
                }
            }
            for (sid, clip) in &clips {
                if let Some((a, b, c)) = pl.find_shot(*sid)
                    && let Some(x) = pl.sections.get_mut(a).and_then(|s| s.beats.get_mut(b)).and_then(|b| b.shots.get_mut(c))
                {
                    x.clip = Some(*clip);
                }
            }
            Ok(())
        })?;
        Ok((ids.len(), clips.len()))
    })?;
    let _ = seq_id;
    Ok(json!({"markers": made.0, "placeholders": made.1, "start": start.0, "end": lay.beats.last().map(|(_, st, len)| st.0 + len.0), "track": chosen}))
}

// ---------------------------------------------------------------------------- commands

fn store(s: &mut Session, label: &str, plan: Planner, cmd: &str) -> Result<Value> {
    plan.check().map_err(|e| bad(cmd, e))?;
    let out = plan_json(s, &plan);
    s.edit(label, |pr, _| {
        put(pr, Some(plan));
        Ok(())
    })?;
    Ok(out)
}

fn create(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.create";
    if get(&s.project).is_some() && p.get("replace").and_then(Value::as_bool) != Some(true) {
        return Err(bad(cmd, "this project already has a plan; pass `replace: true` to start over, or use planner.applyPack"));
    }
    let title = text(p, "title", MAX_TITLE, false, cmd)?;
    let target = target_length(p, cmd)?;
    let clues = uint(p, "clues", cmd)?.map(|c| u32::try_from(c).unwrap_or(u32::MAX));
    let plan = match str_p(p, "pack").filter(|_| !p.get("pack").is_some_and(Value::is_null)) {
        Some(id) => {
            let pack = find_pack(s, id, cmd)?;
            let target = target.filter(|t| t.0 > 0).ok_or_else(|| bad(cmd, "a plan from a pack needs a length: `minutes`, `seconds` or `ticks`"))?;
            let mut plan = pack.instantiate(target, clues).map_err(|e| bad(cmd, e))?;
            plan.title = title.unwrap_or_else(|| pack.name.clone());
            plan
        }
        None => {
            if p.get("pack").is_some_and(|v| !v.is_null() && !v.is_string()) {
                return Err(bad(cmd, "`pack` must be a pack id"));
            }
            Planner::new(&title.unwrap_or_default(), target.unwrap_or_default())
        }
    };
    store(s, "Create Plan", plan, cmd)
}

fn apply_pack(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.applyPack";
    let id = str_p(p, "pack").ok_or_else(|| bad(cmd, "`pack` is required: a pack id from planner.packs"))?;
    let pack = find_pack(s, id, cmd)?;
    let old = get(&s.project);
    let target = target_length(p, cmd)?
        .filter(|t| t.0 > 0)
        .or_else(|| old.as_ref().map(|o| o.target).filter(|t| t.0 > 0))
        .ok_or_else(|| bad(cmd, "give a length: `minutes`, `seconds` or `ticks`"))?;
    let clues = uint(p, "clues", cmd)?.map(|c| u32::try_from(c).unwrap_or(u32::MAX));
    let keep = match p.get("keepNarration") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(cmd, "`keepNarration` must be true or false")),
    };
    let mut plan = pack.instantiate(target, clues).map_err(|e| bad(cmd, e))?;
    plan.title = old.as_ref().map(|o| o.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| pack.name.clone());
    if keep && let Some(old) = &old {
        // the writing of a beat with the same name carries over (its shots with it)
        let found: Vec<Beat> = old.beats().cloned().collect();
        let mut ids: Vec<u64> = Vec::new();
        for sec in &plan.sections {
            ids.extend(sec.beats.iter().map(|b| b.id));
        }
        let mut next = plan.next_id;
        for sec in &mut plan.sections {
            for b in &mut sec.beats {
                if let Some(o) = found.iter().find(|o| o.name.eq_ignore_ascii_case(&b.name)) {
                    b.narration = o.narration.clone();
                    b.visual_intent = o.visual_intent.clone();
                    b.status = o.status;
                    b.shots = o
                        .shots
                        .iter()
                        .map(|x| {
                            let id = next;
                            next = next.saturating_add(1);
                            Shot { id, ..x.clone() }
                        })
                        .collect();
                }
            }
        }
        plan.next_id = next;
    }
    store(s, "Apply Format Pack", plan, cmd)
}

fn clear(s: &mut Session, _p: &Value) -> Result<Value> {
    if get(&s.project).is_none() && !s.project.ext.contains_key(KEY) {
        return Err(bad("planner.clear", "there is no plan"));
    }
    s.edit("Remove Plan", |pr, _| {
        put(pr, None);
        Ok(())
    })?;
    Ok(json!({"exists": false}))
}

fn add_section(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.addSection";
    let name = text(p, "name", MAX_NAME, false, cmd)?.filter(|n| !n.trim().is_empty()).ok_or_else(|| bad(cmd, "`name` is required"))?;
    let id = edit(s, cmd, "Add Section", |pl| {
        let at = at_index(p, pl.sections.len(), cmd)?;
        let id = pl.new_id();
        pl.sections.insert(at, Section { id, name, beats: Vec::new() });
        Ok(id)
    })?;
    Ok(json!({"section": id}))
}

fn update_section(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.updateSection";
    let sid = required_id(p, "section", cmd)?;
    let name = text(p, "name", MAX_NAME, false, cmd)?.filter(|n| !n.trim().is_empty()).ok_or_else(|| bad(cmd, "`name` is required"))?;
    edit(s, cmd, "Rename Section", |pl| {
        let i = pl.section_index(sid).ok_or_else(|| bad(cmd, format!("no section {sid}")))?;
        if let Some(sec) = pl.sections.get_mut(i) {
            sec.name = name;
        }
        Ok(())
    })?;
    Ok(json!({"section": sid}))
}

fn add_beat(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.addBeat";
    let sid = required_id(p, "section", cmd)?;
    if str_p(p, "name").is_none_or(|n| n.trim().is_empty()) {
        return Err(bad(cmd, "`name` is required"));
    }
    let id = edit(s, cmd, "Add Beat", |pl| {
        let si = pl.section_index(sid).ok_or_else(|| bad(cmd, format!("no section {sid}")))?;
        let mut beat = Beat { intensity: 50, ..Default::default() };
        beat_fields(&mut beat, p, cmd)?;
        beat.id = pl.new_id();
        let id = beat.id;
        let sec = pl.sections.get_mut(si).ok_or_else(|| bad(cmd, "no such section"))?;
        let at = at_index(p, sec.beats.len(), cmd)?;
        sec.beats.insert(at, beat);
        Ok(id)
    })?;
    Ok(json!({"beat": id}))
}

fn update_beat(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.updateBeat";
    let bid = required_id(p, "beat", cmd)?;
    edit(s, cmd, "Edit Beat", |pl| {
        let b = pl.beat_mut(bid).ok_or_else(|| bad(cmd, format!("no beat {bid}")))?;
        beat_fields(b, p, cmd)
    })?;
    Ok(json!({"beat": bid}))
}

fn shot_fields(x: &mut Shot, p: &Value, cmd: &str) -> Result<()> {
    if let Some(v) = text(p, "intent", MAX_PURPOSE, true, cmd)? {
        x.intent = v;
    }
    if let Some(v) = text(p, "shotType", MAX_NAME, false, cmd)? {
        x.shot_type = v;
    }
    if let Some(v) = text(p, "cameraMove", MAX_NAME, false, cmd)? {
        x.camera_move = v;
    }
    if let Some(v) = target_length(p, cmd)? {
        x.target = v;
    }
    match p.get("source") {
        None => {}
        Some(Value::Null) => x.source = None,
        Some(_) => x.source = Some(uint(p, "source", cmd)?.ok_or_else(|| bad(cmd, "`source` must be a project item id or null"))?),
    }
    Ok(())
}

fn add_shot(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.addShot";
    let bid = required_id(p, "beat", cmd)?;
    let id = edit(s, cmd, "Add Shot", |pl| {
        let mut shot = Shot::default();
        shot_fields(&mut shot, p, cmd)?;
        shot.id = pl.new_id();
        let id = shot.id;
        let b = pl.beat_mut(bid).ok_or_else(|| bad(cmd, format!("no beat {bid}")))?;
        let at = at_index(p, b.shots.len(), cmd)?;
        b.shots.insert(at, shot);
        Ok(id)
    })?;
    Ok(json!({"shot": id}))
}

fn update_shot(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.updateShot";
    let xid = required_id(p, "shot", cmd)?;
    edit(s, cmd, "Edit Shot", |pl| {
        let (si, bi, xi) = pl.find_shot(xid).ok_or_else(|| bad(cmd, format!("no shot {xid}")))?;
        let x = pl.sections.get_mut(si).and_then(|s| s.beats.get_mut(bi)).and_then(|b| b.shots.get_mut(xi)).ok_or_else(|| bad(cmd, "no such shot"))?;
        shot_fields(x, p, cmd)
    })?;
    Ok(json!({"shot": xid}))
}

fn which(p: &Value, cmd: &str) -> Result<(&'static str, u64)> {
    let mut found = Vec::new();
    for k in ["section", "beat", "shot"] {
        if p.get(k).is_some_and(|v| !v.is_null()) {
            found.push((k, required_id(p, k, cmd)?));
        }
    }
    match found.as_slice() {
        [one] => Ok(*one),
        _ => Err(bad(cmd, "give exactly one of `section`, `beat` or `shot`")),
    }
}

fn move_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.move";
    let (kind, id) = which(p, cmd)?;
    let to = uint(p, "to", cmd)?;
    edit(s, cmd, "Move in Plan", |pl| {
        match kind {
            "section" => {
                let i = pl.section_index(id).ok_or_else(|| bad(cmd, format!("no section {id}")))?;
                let sec = pl.sections.remove(i);
                let at = at_index(p, pl.sections.len(), cmd)?;
                pl.sections.insert(at, sec);
            }
            "beat" => {
                let (si, bi) = pl.find_beat(id).ok_or_else(|| bad(cmd, format!("no beat {id}")))?;
                let dest = match to {
                    Some(t) => pl.section_index(t).ok_or_else(|| bad(cmd, format!("no section {t}")))?,
                    None => si,
                };
                let beat = pl.sections.get_mut(si).map(|s| s.beats.remove(bi)).ok_or_else(|| bad(cmd, "no such beat"))?;
                let sec = pl.sections.get_mut(dest).ok_or_else(|| bad(cmd, "no such section"))?;
                let at = at_index(p, sec.beats.len(), cmd)?;
                sec.beats.insert(at, beat);
            }
            _ => {
                let (si, bi, xi) = pl.find_shot(id).ok_or_else(|| bad(cmd, format!("no shot {id}")))?;
                let dest = match to {
                    Some(t) => pl.find_beat(t).ok_or_else(|| bad(cmd, format!("no beat {t}")))?,
                    None => (si, bi),
                };
                let shot = pl.sections.get_mut(si).and_then(|s| s.beats.get_mut(bi)).map(|b| b.shots.remove(xi)).ok_or_else(|| bad(cmd, "no such shot"))?;
                let b = pl.sections.get_mut(dest.0).and_then(|s| s.beats.get_mut(dest.1)).ok_or_else(|| bad(cmd, "no such beat"))?;
                let at = at_index(p, b.shots.len(), cmd)?;
                b.shots.insert(at, shot);
            }
        }
        Ok(())
    })?;
    Ok(json!({kind: id}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.delete";
    let (kind, id) = which(p, cmd)?;
    edit(s, cmd, "Delete from Plan", |pl| {
        match kind {
            "section" => {
                let i = pl.section_index(id).ok_or_else(|| bad(cmd, format!("no section {id}")))?;
                pl.sections.remove(i);
            }
            "beat" => {
                let (si, bi) = pl.find_beat(id).ok_or_else(|| bad(cmd, format!("no beat {id}")))?;
                if let Some(sec) = pl.sections.get_mut(si) {
                    sec.beats.remove(bi);
                }
            }
            _ => {
                let (si, bi, xi) = pl.find_shot(id).ok_or_else(|| bad(cmd, format!("no shot {id}")))?;
                if let Some(b) = pl.sections.get_mut(si).and_then(|s| s.beats.get_mut(bi)) {
                    b.shots.remove(xi);
                }
            }
        }
        Ok(())
    })?;
    Ok(json!({"deleted": kind, "id": id}))
}

fn link(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.link";
    let bid = required_id(p, "beat", cmd)?;
    let clear = match p.get("clear") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(cmd, "`clear` must be true or false")),
    };
    let new = if clear {
        None
    } else {
        let start = length(p, "start", cmd)?.ok_or_else(|| bad(cmd, "give `startSeconds` or `startTicks` (or `clear: true`)"))?;
        let duration = length(p, "duration", cmd)?.ok_or_else(|| bad(cmd, "give `durationSeconds` or `durationTicks`"))?;
        if start.0.checked_add(duration.0).is_none() || start > MAX_TARGET || duration > MAX_TARGET {
            return Err(bad(cmd, "that range is outside the supported time"));
        }
        Some((start, duration))
    };
    edit(s, cmd, if clear { "Unlink Beat" } else { "Link Beat" }, |pl| {
        let b = pl.beat_mut(bid).ok_or_else(|| bad(cmd, format!("no beat {bid}")))?;
        b.link = new.map(|(start, duration)| Link { start, duration, marker: b.link.and_then(|l| l.marker) });
        Ok(())
    })?;
    Ok(json!({"beat": bid, "linked": !clear}))
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.export";
    let plan = plan_of(s, cmd)?;
    let mut out = json!({"plan": plan});
    if let Some(path) = text(p, "path", 1000, false, cmd)?.filter(|x| !x.is_empty()) {
        let bytes = serde_json::to_vec_pretty(&json!({"format": "edith.plan", "version": 1, "plan": plan})).map_err(|e| EngineError::Other(e.to_string()))?;
        s.services.write_file(&path, &bytes).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
        out["path"] = json!(path);
    }
    Ok(out)
}

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.import";
    let value: Value = match (p.get("plan").filter(|v| !v.is_null()), text(p, "path", 1000, false, cmd)?.filter(|x| !x.is_empty())) {
        (Some(v), None) => v.clone(),
        (None, Some(path)) => {
            if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_IMPORT_BYTES) {
                return Err(bad(cmd, "that file is too large to be a plan"));
            }
            let bytes = s.services.read_file(&path).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
            if bytes.len() as u64 > MAX_IMPORT_BYTES {
                return Err(bad(cmd, "that file is too large to be a plan"));
            }
            serde_json::from_slice(&bytes).map_err(|_| bad(cmd, "that file is not a plan"))?
        }
        _ => return Err(bad(cmd, "give either `plan` (the plan as JSON) or `path` (a file)")),
    };
    // a file made by planner.export wraps the plan; a bare plan is accepted too
    let inner = value.get("plan").filter(|_| value.get("format").is_some()).cloned().unwrap_or(value);
    let plan: Planner = serde_json::from_value(inner).map_err(|e| bad(cmd, format!("that is not a plan: {e}")))?;
    plan.check().map_err(|e| bad(cmd, format!("the plan does not fit the limits: {e}")))?;
    store(s, "Import Plan", plan, cmd)
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
    macro_rules! fields {
        () => {
            r#""name":str?,"purpose":str?,"intensity":0..10?,"minutes"|"seconds"|"ticks":n?,"narration":str?,"visualIntent":str?,"soundNote":str?,"status":"idea|drafted|sourced|cut|done"?"#
        };
    }
    let beat_params: &'static str = concat!(r#"{"beat":id,"#, fields!(), "}");
    let add_beat_params: &'static str = concat!(r#"{"section":id,"at":n?,"#, fields!(), "}");
    vec![
        spec("planner.get", "Get Plan", "{}", get_cmd, false),
        spec("planner.packs", "List Format Packs", "{}", packs_cmd, false),
        spec("planner.create", "Create Plan", r#"{"pack":str?,"minutes"|"seconds"|"ticks":n?,"title":str?,"clues":n?,"replace":bool?}"#, create, true),
        spec("planner.applyPack", "Apply Format Pack", r#"{"pack":str,"minutes"|"seconds"|"ticks":n?,"clues":n?,"keepNarration":bool?}"#, apply_pack, true),
        spec("planner.clear", "Remove Plan", "{}", clear, true),
        spec("planner.addSection", "Add Plan Section", r#"{"name":str,"at":n?}"#, add_section, true),
        spec("planner.updateSection", "Rename Plan Section", r#"{"section":id,"name":str}"#, update_section, true),
        spec("planner.addBeat", "Add Beat", add_beat_params, add_beat, true),
        spec("planner.updateBeat", "Edit Beat", beat_params, update_beat, true),
        spec(
            "planner.addShot",
            "Add Shot",
            r#"{"beat":id,"intent":str?,"shotType":str?,"cameraMove":str?,"seconds"|"ticks":n?,"source":id|null?,"at":n?}"#,
            add_shot,
            true,
        ),
        spec(
            "planner.updateShot",
            "Edit Shot",
            r#"{"shot":id,"intent":str?,"shotType":str?,"cameraMove":str?,"seconds"|"ticks":n?,"source":id|null?}"#,
            update_shot,
            true,
        ),
        spec("planner.move", "Move in Plan", r#"{"section"|"beat"|"shot":id,"to":id?,"at":n?}"#, move_cmd, true),
        spec("planner.delete", "Delete from Plan", r#"{"section"|"beat"|"shot":id}"#, delete, true),
        spec(
            "planner.link",
            "Link Beat to Timeline",
            r#"{"beat":id,"startSeconds"|"startTicks":n?,"durationSeconds"|"durationTicks":n?,"clear":bool?}"#,
            link,
            true,
        ),
        spec("planner.buildSkeleton", "Build Plan Skeleton", r#"{"startSeconds"|"startTicks":n?,"track":"V2"?}"#, build_skeleton, true),
        spec("planner.export", "Export Plan", r#"{"path":str?}"#, export, false),
        spec("planner.import", "Import Plan", r#"{"path":str?,"plan":object?}"#, import, true),
    ]
}
