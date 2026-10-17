//! The spotting sheet and the cues on the timeline (fork roadmap F13.3).
//!
//! | command | does |
//! |---|---|
//! | `sound.sheet {path?, beat?}` | the sound plan as a spotting sheet: per beat, where it is on the timeline, the ambience bed, the music (mood, in, out), the effects and the deliberate silences, in order, with times from the start of the video; as structured `beats` and as plain `text` (written to `path` when one is given) |
//! | `sound.placeCues {beat?, clear?}` | put the sound plan on the timeline as markers (one undo step): a range for the ambience and for the music, a point for each effect, a range for each silence, in their own colours and named `Sound · …`. Asked again they are replaced, not doubled; `beat` does one beat; `clear: true` takes them away |
//!
//! Where a beat is: its marker from `planner.buildSkeleton`, else the plan laid end to end from the
//! start. The markers made here are recognised by their name and by the note they carry (`From the
//! sound plan [beat N]`), so no marker of yours is ever touched.

use edith_planner::{Beat, CueKind};
use filmcraft_project::{Label, Marker, MarkerId, MarkerKind};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, u64_p};
use crate::{EngineError, Result, Session};

const PREFIX: &str = "Sound · ";
const TAG: &str = "From the sound plan";
const MAX_NAME: usize = 100;

/// A beat and where it is on the timeline.
pub(crate) struct Place {
    pub beat: Beat,
    pub section: String,
    pub start: i64,
    pub length: i64,
}

/// Every beat of the plan with its place: its marker's, else the plan laid end to end.
pub(crate) fn places(s: &Session) -> Vec<Place> {
    let Some(plan) = crate::planner::get(&s.project) else { return Vec::new() };
    let laid = edith_planner::ranges(&plan, Tick::ZERO);
    let mut out = Vec::new();
    for (sec, beats) in plan.sections.iter().map(|sec| (&sec.name, &sec.beats)) {
        for b in beats {
            let (start, length) = match b.link {
                Some(l) => (l.start.0, l.duration.0),
                None => laid.iter().find(|r| r.beat == b.id).map_or((0, b.target.0), |r| (r.start.0, r.end.0.saturating_sub(r.start.0))),
            };
            out.push(Place { beat: b.clone(), section: sec.clone(), start, length: length.max(0) });
        }
    }
    out
}

fn clock(ticks: i64) -> String {
    let n = (ticks.max(0) / TICKS_PER_SECOND).max(0) as u64;
    if n >= 3600 { format!("{}:{:02}:{:02}", n / 3600, (n / 60) % 60, n % 60) } else { format!("{}:{:02}", n / 60, n % 60) }
}

fn seconds_text(ticks: i64) -> String {
    let s = ticks.max(0) as f64 / TICKS_PER_SECOND as f64;
    if s < 10.0 && (s * 10.0).fract().abs() > 1e-9 { format!("{s:.1} s") } else { format!("{} s", s.round() as i64) }
}

/// Whether a beat has a sound plan: a bed, music or cues. A free-text note (the Format Pack writes one
/// for every beat) is shown on the sheet but is not a plan.
fn planned(b: &Beat) -> bool {
    !b.sound.ambience.trim().is_empty() || b.sound.music.is_some() || !b.sound.cues.is_empty()
}

fn beat_json(p: &Place) -> Value {
    let b = &p.beat;
    let at = |t: Tick| p.start.saturating_add(t.0);
    json!({
        "id": b.id, "name": b.name, "section": p.section, "start": p.start, "length": p.length, "intensity": f64::from(b.intensity) / 10.0,
        "status": b.status.id(), "soundNote": b.sound_note, "ambience": b.sound.ambience,
        "music": b.sound.music.as_ref().map(|m| json!({
            "mood": m.mood, "enters": at(m.enters),
            "leaves": at(m.leaves.unwrap_or(Tick(p.length.max(m.enters.0)))), "toEnd": m.leaves.is_none(),
        })),
        "cues": b.sound.cues.iter().map(|c| json!({
            "id": c.id, "kind": c.kind.id(), "start": at(c.at), "length": c.length.0, "inBeat": c.at.0, "text": c.text,
        })).collect::<Vec<_>>(),
        "planned": planned(b),
    })
}

/// The spotting sheet as plain text.
fn sheet_text(title: &str, places: &[&Place]) -> String {
    let mut out = String::new();
    out.push_str(&format!("SPOTTING SHEET{}\n", if title.trim().is_empty() { String::new() } else { format!(": {}", title.trim()) }));
    out.push_str("Times are from the start of the video.\n");
    let mut section = String::new();
    let mut empty: Vec<&str> = Vec::new();
    for p in places {
        if p.section != section {
            section = p.section.clone();
            out.push_str(&format!("\n== {section} ==\n"));
        }
        let b = &p.beat;
        out.push_str(&format!(
            "\n{}  {}-{}  (intensity {:.1})\n",
            b.name,
            clock(p.start),
            clock(p.start.saturating_add(p.length)),
            f64::from(b.intensity) / 10.0
        ));
        if !planned(b) {
            out.push_str("  nothing planned yet\n");
            empty.push(&b.name);
            if !b.sound_note.trim().is_empty() {
                out.push_str(&format!("  Note: {}\n", b.sound_note.trim().replace('\n', " ")));
            }
            continue;
        }
        let at = |t: Tick| p.start.saturating_add(t.0);
        if !b.sound.ambience.trim().is_empty() {
            out.push_str(&format!("  Ambience: {}\n", b.sound.ambience.trim().replace('\n', " ")));
        }
        if let Some(m) = &b.sound.music {
            let out_at = m.leaves.map_or_else(|| "the end of the beat".to_string(), |t| clock(at(t)));
            out.push_str(&format!(
                "  Music: {}, in at {}, out at {}\n",
                if m.mood.is_empty() { "(no mood given)" } else { &m.mood },
                clock(at(m.enters)),
                out_at
            ));
        }
        for c in &b.sound.cues {
            match c.kind {
                CueKind::Effect => out.push_str(&format!("  {}  EFFECT   {}\n", clock(at(c.at)), if c.text.is_empty() { "(unnamed)" } else { &c.text })),
                CueKind::Silence => out.push_str(&format!(
                    "  {}  SILENCE  {}{}\n",
                    clock(at(c.at)),
                    seconds_text(c.length.0),
                    if c.text.is_empty() { String::new() } else { format!(" ({})", c.text) }
                )),
            }
        }
        if !b.sound_note.trim().is_empty() {
            out.push_str(&format!("  Note: {}\n", b.sound_note.trim().replace('\n', " ")));
        }
    }
    if !empty.is_empty() {
        out.push_str(&format!("\nNothing planned yet for: {}.\n", empty.join(", ")));
    }
    out
}

fn sheet_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sound.sheet";
    let plan = crate::planner::get(&s.project).ok_or_else(|| bad(cmd, "there is no plan yet: planner.create makes one"))?;
    let all = places(s);
    let wanted = u64_p(p, "beat");
    let chosen: Vec<&Place> = match wanted {
        Some(id) => vec![all.iter().find(|x| x.beat.id == id).ok_or_else(|| bad(cmd, format!("no beat {id}")))?],
        None => all.iter().collect(),
    };
    let text = sheet_text(&plan.title, &chosen);
    let mut out =
        json!({"beats": chosen.iter().map(|x| beat_json(x)).collect::<Vec<_>>(), "text": text, "planned": chosen.iter().filter(|x| planned(&x.beat)).count()});
    if let Some(path) = p.get("path").filter(|v| !v.is_null()) {
        let path =
            path.as_str().filter(|x| !x.trim().is_empty() && x.len() <= 1000 && !x.contains('\0')).ok_or_else(|| bad(cmd, "`path` must be a file path"))?;
        s.services.write_file(path, text.as_bytes()).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
        out["path"] = json!(path);
    }
    Ok(out)
}

fn tag(beat: u64) -> String {
    format!("{TAG} [beat {beat}]")
}

/// Whether a marker was made here (and, with `beat`, for that beat).
fn is_ours(m: &Marker, beat: Option<u64>) -> bool {
    m.name.starts_with(PREFIX) && m.comment.starts_with(&beat.map_or_else(|| format!("{TAG} [beat "), tag))
}

fn name(kind: &str, text: &str) -> String {
    let t: String = text.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    format!("{PREFIX}{kind}{}", if t.trim().is_empty() { String::new() } else { format!(": {}", t.trim()) })
}

/// What the markers of one beat's sound plan are: start, length, name, note, colour.
fn beat_markers(p: &Place) -> Vec<(i64, i64, String, String, Label)> {
    let b = &p.beat;
    let note = |extra: &str| format!("{}\n{extra}", tag(b.id));
    let mut out = Vec::new();
    if !b.sound.ambience.trim().is_empty() {
        out.push((p.start, p.length, name("Ambience", &b.sound.ambience), note(&b.name), Label::Teal));
    }
    if let Some(m) = &b.sound.music {
        let from = p.start.saturating_add(m.enters.0);
        let to = m.leaves.map_or(p.start.saturating_add(p.length), |t| p.start.saturating_add(t.0));
        out.push((from, (to - from).max(0), name("Music", &m.mood), note(&b.name), Label::Purple));
    }
    for c in &b.sound.cues {
        let at = p.start.saturating_add(c.at.0);
        match c.kind {
            CueKind::Effect => out.push((at, 0, name("Effect", &c.text), note(&b.name), Label::Mango)),
            CueKind::Silence => out.push((at, c.length.0, name("Silence", &c.text), note(&b.name), Label::Rose)),
        }
    }
    out
}

fn place_cues(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sound.placeCues";
    if crate::planner::get(&s.project).is_none() {
        return Err(bad(cmd, "there is no plan yet: planner.create makes one"));
    }
    s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let beat = u64_p(p, "beat");
    let clear = bool_p(p, "clear").unwrap_or(false);
    let all = places(s);
    if let Some(id) = beat
        && !all.iter().any(|x| x.beat.id == id)
    {
        return Err(bad(cmd, format!("no beat {id}")));
    }
    let wanted: Vec<(i64, i64, String, String, Label)> =
        if clear { Vec::new() } else { all.iter().filter(|x| beat.is_none_or(|b| x.beat.id == b)).flat_map(beat_markers).collect() };
    if !clear && wanted.is_empty() {
        return Err(bad(cmd, "no beat has a sound plan yet: planner.setSound and planner.addCue write one"));
    }
    let (removed, made) = s.edit_sequence("Place Sound Cues", |q, ctx, _| {
        let before = q.markers.len();
        q.markers.retain(|m| !is_ours(m, beat));
        let removed = before - q.markers.len();
        for (start, length, name, comment, color) in &wanted {
            q.markers.push(Marker {
                id: MarkerId(ctx.alloc()),
                start: Tick((*start).max(0)),
                duration: Tick((*length).max(0)),
                name: name.clone(),
                comment: comment.clone(),
                kind: MarkerKind::Comment,
                color: *color,
            });
        }
        q.markers.sort_by_key(|m| m.start);
        Ok((removed, wanted.len()))
    })?;
    Ok(json!({"placed": made, "removed": removed}))
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
        spec("sound.sheet", "Spotting Sheet", r#"{"path":str?,"beat":id?}"#, sheet_command, false),
        spec("sound.placeCues", "Place Sound Cues on the Timeline", r#"{"beat":id?,"clear":bool=false}"#, place_cues, true),
    ]
}
