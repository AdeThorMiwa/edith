//! The sound map (fork roadmap F13.2): the timeline's audio in lanes, with what is missing or off.
//!
//! | command | does |
//! |---|---|
//! | `sound.map {sequence?, levels?}` | the lanes (narration, music, ambience, effects, and unsorted when something is in it) with the clips in each, where each sounds, its loudness over time once measured (`levels: false` leaves the readings out), and the sound findings: no ambience, an ambience that stops, dead air, a room tone that jumps |
//! | `analysis.run {sound: true}` | measure the loudness of each lane over time, as a background job (cached like the mix; stale as soon as the audio or a clip's lane changes) |
//!
//! A clip goes in a lane by, in this order: its Essential Sound tag (Dialogue is narration, SFX is
//! effects), being a narration made here, what analysing its media found it to be (speech, music or
//! ambience), else the unsorted lane. The map exists from the clips alone; the measurement adds the
//! levels and the findings that need them. Deliberate silences in the sound plan are not dead air.

use edith_analysis::coach::Finding;
use edith_analysis::soundmap as sm;
use filmcraft_project::{AudioType, ItemId, ItemKind, Sequence, TrackItem, TrackKind};
use serde_json::{Value, json};

use crate::analysis::{secs, to_value};
use crate::commands::{CommandSpec, always, bad, bool_p};
use crate::{EngineError, Result, Session};

/// Clips listed per lane in `sound.map`.
const MAX_CLIPS_LISTED: usize = 2000;

/// What analysing a media item found its sound to be, if it has been analysed.
fn guessed_lane(s: &Session, c: &TrackItem) -> Option<sm::Lane> {
    let report = crate::analysis::item_report_for(s, c.item)?;
    match report["audio"]["stats"]["kind"].as_str()? {
        "speech" => Some(sm::Lane::Narration),
        "music" => Some(sm::Lane::Music),
        "ambience" => Some(sm::Lane::Ambience),
        _ => None,
    }
}

/// The lane of a clip on an audio track. `guesses` remembers, per media item, what analysing it found.
pub(crate) fn lane_of(s: &Session, c: &TrackItem, guesses: &mut std::collections::HashMap<u64, Option<sm::Lane>>) -> sm::Lane {
    if let Some(e) = &c.essential {
        return match e.kind {
            AudioType::Dialogue => sm::Lane::Narration,
            AudioType::Music => sm::Lane::Music,
            AudioType::Sfx => sm::Lane::Effects,
            AudioType::Ambience => sm::Lane::Ambience,
        };
    }
    if s.project.narrations.contains_key(&c.item) {
        return sm::Lane::Narration;
    }
    (*guesses.entry(c.item.0).or_insert_with(|| guessed_lane(s, c))).unwrap_or(sm::Lane::Other)
}

/// The sound clips of a sequence in their lanes: a muted track and a disabled clip sound like nothing.
pub(crate) fn lane_clips(s: &Session, q: &Sequence) -> Vec<sm::LaneClip> {
    let mut guesses = std::collections::HashMap::new();
    q.audio_tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio && !t.muted)
        .flat_map(|t| t.items.iter().filter(|c| c.enabled && c.duration.0 > 0))
        .map(|c| sm::LaneClip { lane: lane_of(s, c, &mut guesses), id: c.id.0, start: c.start.0, end: c.end().0 })
        .collect()
}

/// Where the programme ends: the last picture or the last sound.
fn programme_end(q: &Sequence) -> i64 {
    let video = q.video_tracks.iter().filter(|t| t.enabled).flat_map(|t| t.items.iter().filter(|c| c.enabled)).map(|c| c.end().0);
    let audio = q.audio_tracks.iter().filter(|t| !t.muted).flat_map(|t| t.items.iter().filter(|c| c.enabled)).map(|c| c.end().0);
    video.chain(audio).max().unwrap_or(0)
}

/// The silences the sound plan asks for, on the timeline: where each beat is (its marker, else the plan
/// laid end to end) plus the silence's time in the beat.
pub(crate) fn planned_silences(s: &Session) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    for p in crate::soundplan::places(s) {
        for c in p.beat.sound.cues.iter().filter(|c| c.kind == edith_planner::CueKind::Silence) {
            let from = p.start.saturating_add(c.at.0);
            out.push((from, from.saturating_add(c.length.0)));
        }
    }
    out
}

/// The loudness readings of a measurement, as the map takes them.
fn levels_from(report: &Value) -> Vec<sm::LaneLevels> {
    let Some(map) = report["levels"].as_object() else { return Vec::new() };
    sm::Lane::ALL
        .into_iter()
        .filter_map(|lane| {
            let a = map.get(lane.name())?.as_array()?;
            let values = a.iter().take(sm::MAX_READINGS).map(|v| v.as_f64().filter(|x| x.is_finite()).map(|x| x as f32)).collect();
            Some(sm::LaneLevels { lane, values })
        })
        .collect()
}

/// What the map reads: the clips in lanes, the planned silences and, if the sound has been measured
/// and nothing has changed since, its levels.
pub(crate) fn input_for(s: &Session, id: ItemId, levels: bool) -> Result<(sm::SoundMapInput, Option<Value>)> {
    let item = s.project.item(id).ok_or_else(|| bad("sound.map", format!("no sequence {}", id.0)))?;
    let ItemKind::Sequence(q) = &item.kind else { return Err(bad("sound.map", format!("“{}” is not a sequence", item.name))) };
    let duration = programme_end(q);
    let report = if levels { crate::analysis::measured_sound(s, id) } else { None };
    let input =
        sm::SoundMapInput { duration, clips: lane_clips(s, q), silences: planned_silences(s), levels: report.as_ref().map(levels_from).unwrap_or_default() };
    Ok((input, report))
}

/// The sound map's findings for a sequence; none when it has no sound or no length.
pub(crate) fn sound_findings(s: &Session, id: ItemId) -> Vec<Finding> {
    input_for(s, id, true).ok().and_then(|(i, _)| sm::findings(&i).ok()).unwrap_or_default()
}

fn clip_name(s: &Session, q: &Sequence, id: u64) -> String {
    q.audio_tracks.iter().flat_map(|t| &t.items).find(|c| c.id.0 == id).and_then(|c| s.project.item(c.item)).map(|i| i.name.clone()).unwrap_or_default()
}

fn map_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sound.map";
    let id = match p.get("sequence").filter(|v| !v.is_null() && **v != json!(true)) {
        Some(v) => v.as_u64().map(ItemId).ok_or_else(|| bad(cmd, "`sequence` must be a sequence's item id"))?,
        None => s.state.active_sequence.ok_or(EngineError::NoSequence)?,
    };
    build_map(s, id, bool_p(p, "levels").unwrap_or(true))
}

/// The map of a sequence as `sound.map` gives it.
pub(crate) fn build_map(s: &Session, id: ItemId, want_levels: bool) -> Result<Value> {
    let cmd = "sound.map";
    let (input, report) = input_for(s, id, true)?;
    if input.duration <= 0 {
        return Err(bad(cmd, "the sequence has nothing on its tracks yet"));
    }
    let Some(ItemKind::Sequence(q)) = s.project.item(id).map(|i| &i.kind) else { return Err(bad(cmd, "not a sequence")) };
    let map = sm::lane_map(&input).map_err(|e| bad(cmd, e.to_string()))?;
    let found = sm::findings(&input).map_err(|e| bad(cmd, e.to_string()))?;
    let lanes: Vec<Value> = map
        .lanes
        .iter()
        .map(|l| {
            let clips: Vec<Value> = input
                .clips
                .iter()
                .filter(|c| c.lane == l.lane)
                .take(MAX_CLIPS_LISTED)
                .map(|c| json!({"id": c.id, "start": c.start, "end": c.end, "name": clip_name(s, q, c.id)}))
                .collect();
            let mut v = json!({
                "lane": l.lane.name(), "label": l.lane.label(), "spans": l.spans, "clips": clips,
                "coveredSeconds": l.covered_seconds, "share": l.share, "medianLufs": l.median_lufs, "loudestLufs": l.loudest_lufs,
            });
            if want_levels && map.measured {
                v["levels"] = input.levels.iter().find(|x| x.lane == l.lane).map_or(Value::Null, |x| json!(x.values));
            }
            v
        })
        .collect();
    let name = s.project.item(id).map(|i| i.name.clone()).unwrap_or_default();
    Ok(json!({
        "sequence": {"id": id.0, "name": name},
        "duration": map.duration, "durationSeconds": secs(filmcraft_time::Tick(map.duration)),
        "measured": map.measured, "hopSeconds": sm::HOP_SECONDS,
        "analysedSeconds": report.as_ref().map(|r| r["analysedSeconds"].clone()),
        "lanes": lanes, "findings": to_value(&found),
        "plannedSilences": input.silences.len(),
    }))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "sound.map",
        label: "Sound Map",
        menu: &[],
        shortcut: None,
        params: r#"{"sequence":id?,"levels":bool=true}"#,
        enabled: always,
        run: map_command,
        journal: false,
    }]
}

// ---------------------------------------------------------------------------- the assistant

use crate::assistant_tools::ToolDef;
use edith_ai::ToolOutput;

/// Points a `soundmap` view draws per lane.
const VIEW_POINTS: usize = 240;
/// Spans per lane and findings a view carries.
const VIEW_SPANS: usize = 200;
const VIEW_FINDINGS: usize = 12;

fn sequence_of(s: &Session, p: &Value) -> std::result::Result<ItemId, String> {
    match p.get("sequence").filter(|v| !v.is_null()) {
        Some(v) => v.as_u64().map(ItemId).ok_or_else(|| "`sequence` must be a sequence's item id".to_string()),
        None => s.state.active_sequence.ok_or_else(|| "no sequence is open".to_string()),
    }
}

fn clock_of(ticks: i64) -> String {
    let n = (ticks.max(0) / filmcraft_time::TICKS_PER_SECOND).max(0) as u64;
    if n >= 3600 { format!("{}:{:02}:{:02}", n / 3600, (n / 60) % 60, n % 60) } else { format!("{}:{:02}", n / 60, n % 60) }
}

fn sound_map_tool(s: &Session, p: &Value) -> ToolOutput {
    let p = if p.is_object() { p.clone() } else { json!({}) };
    let id = match sequence_of(s, &p) {
        Ok(id) => id,
        Err(e) => return ToolOutput::error(e),
    };
    let m = match build_map(s, id, false) {
        Ok(m) => m,
        Err(e) => return ToolOutput::error(e.to_string()),
    };
    let measured = m["measured"].as_bool().unwrap_or(false);
    let mut out = format!(
        "Sound map of “{}” ({} long), {}.\n",
        m["sequence"]["name"].as_str().unwrap_or(""),
        clock_of(m["duration"].as_i64().unwrap_or(0)),
        if measured {
            "loudness measured"
        } else {
            "loudness NOT measured yet (the lanes show only where each sound is; analysis_run with sound true measures how loud each lane is and finds more)"
        }
    );
    for l in m["lanes"].as_array().into_iter().flatten() {
        let (label, covered) = (l["label"].as_str().unwrap_or(""), l["coveredSeconds"].as_f64().unwrap_or(0.0));
        let clips = l["clips"].as_array().map_or(0, Vec::len);
        if covered <= 0.0 {
            out.push_str(&format!("- {label}: nothing\n"));
            continue;
        }
        out.push_str(&format!(
            "- {label}: {clips} clip{} covering {covered:.0} s ({:.0}% of the video){}{}\n",
            if clips == 1 { "" } else { "s" },
            l["share"].as_f64().unwrap_or(0.0) * 100.0,
            l["medianLufs"].as_f64().map_or(String::new(), |v| format!(", typically {v:.0} LUFS")),
            l["loudestLufs"].as_f64().map_or(String::new(), |v| format!(", loudest {v:.0} LUFS")),
        ));
    }
    let unsorted = m["lanes"].as_array().into_iter().flatten().find(|l| l["lane"] == "other").and_then(|l| l["clips"].as_array().map(Vec::len)).unwrap_or(0);
    if unsorted > 0 {
        out.push_str(&format!(
            "{unsorted} clip{} not sorted into a lane: the map cannot say what is missing while that is so. The person sorts them by marking each as Dialogue, Music, SFX or Ambience in Essential Sound.\n",
            if unsorted == 1 { " is" } else { "s are" }
        ));
    }
    if m["plannedSilences"].as_u64().unwrap_or(0) > 0 {
        out.push_str(&format!("The sound plan asks for {} deliberate silence(s); they are not counted as dead air.\n", m["plannedSilences"]));
    }
    let findings = m["findings"].as_array().cloned().unwrap_or_default();
    if findings.is_empty() {
        out.push_str("Findings: none.\n");
    } else {
        out.push_str("Findings (worked out by fixed rules; quote them, do not invent others):\n");
        for f in findings.iter().take(20) {
            out.push_str(&format!(
                "- [{}] {} at {}: {} {}\n",
                f["severity"].as_str().unwrap_or(""),
                f["title"].as_str().unwrap_or(""),
                clock_of(f["start"].as_i64().unwrap_or(0)),
                f["detail"].as_str().unwrap_or(""),
                f["idea"].as_str().map_or(String::new(), |i| format!("(The idea: {i}.)"))
            ));
        }
    }
    ToolOutput::ok(out)
}

fn sound_plan_tool(s: &Session, p: &Value) -> ToolOutput {
    let p = if p.is_object() { p.clone() } else { json!({}) };
    let mut params = json!({});
    if let Some(b) = p.get("beat").filter(|v| !v.is_null()) {
        match b.as_u64() {
            Some(b) => params["beat"] = json!(b),
            None => return ToolOutput::error("`beat` must be a beat's id from planner_get"),
        }
    }
    if crate::planner::get(&s.project).is_none() {
        return ToolOutput::error("there is no plan yet, so no sound plan: the person makes one in the Planner");
    }
    let all = crate::soundplan::places(s);
    let wanted: Option<u64> = params["beat"].as_u64();
    let chosen: Vec<&crate::soundplan::Place> = match wanted {
        Some(id) => all.iter().filter(|x| x.beat.id == id).collect(),
        None => all.iter().collect(),
    };
    if wanted.is_some() && chosen.is_empty() {
        return ToolOutput::error("there is no such beat");
    }
    let title = crate::planner::get(&s.project).map(|p| p.title).unwrap_or_default();
    ToolOutput::ok(crate::soundplan::sheet_text(&title, &chosen))
}

/// The `soundmap` view of `show`: the lanes where they sound, their loudness if measured, the planned
/// silences and the findings, from the measurements as they are now.
pub(crate) fn view(s: &Session, o: &serde_json::Map<String, Value>, caption: Option<String>) -> std::result::Result<ToolOutput, String> {
    let id = sequence_of(s, &Value::Object(o.clone()))?;
    let m = build_map(s, id, true).map_err(|e| e.to_string())?;
    let lanes: Vec<Value> = m["lanes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|l| {
            let spans: Vec<Value> = l["spans"].as_array().into_iter().flatten().take(VIEW_SPANS).cloned().collect();
            // the readings, thinned to the points a strip can show: the loudest of each group
            let levels = l["levels"].as_array().map(|a| {
                let group = a.len().div_ceil(VIEW_POINTS).max(1);
                a.chunks(group)
                    .map(|c| c.iter().filter_map(Value::as_f64).fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v)))).map_or(Value::Null, |v| json!(v)))
                    .collect::<Vec<Value>>()
            });
            json!({"lane": l["lane"], "label": l["label"], "spans": spans, "levels": levels, "hopSeconds": m["hopSeconds"].as_f64().unwrap_or(0.5) * (l["levels"].as_array().map_or(1, |a| a.len().div_ceil(VIEW_POINTS).max(1)) as f64), "medianLufs": l["medianLufs"]})
        })
        .collect();
    let findings: Vec<Value> = m["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .take(VIEW_FINDINGS)
        .map(|f| json!({"start": f["start"], "end": f["end"], "severity": f["severity"], "title": f["title"], "kind": f["kind"]}))
        .collect();
    let silences: Vec<Value> = planned_silences(s).into_iter().take(VIEW_SPANS).map(|(a, b)| json!([a, b])).collect();
    let n = findings.len();
    let v = json!({
        "view": "soundmap", "caption": caption, "sequence": m["sequence"]["name"], "duration": m["duration"], "measured": m["measured"],
        "lanes": lanes, "silences": silences, "findings": findings,
    });
    Ok(ToolOutput::ok(format!(
        "Shown to the person: the sound map of “{}”, the lanes where each sound is{}{}, with {n} finding{} marked on the ruler (they can click a mark to jump there). Say what is missing or off and what it does to a viewer; do not read the lanes out.",
        m["sequence"]["name"].as_str().unwrap_or(""),
        if m["measured"].as_bool().unwrap_or(false) { " and how loud each is" } else { " (loudness has not been measured)" },
        if silences.is_empty() { "" } else { ", and the planned silences shaded" },
        if n == 1 { "" } else { "s" },
    ))
    .with_view(v))
}

pub(crate) fn tools() -> Vec<ToolDef> {
    use crate::assistant_tools::def;
    let seq = json!({"sequence": {"type": "integer", "description": "A sequence id. Default: the open sequence."}});
    vec![
        def(
            "sound_map",
            "Where each kind of sound is on the timeline: lanes for narration, music, ambience and effects (and unsorted clips), how much of the video each covers, how loud each is once measured, and the sound findings worked out by fixed rules: no ambience or room tone, an ambience that stops, dead air, a room tone that jumps. A deliberate silence in the sound plan is never dead air. If the loudness has not been measured the answer says so; offer analysis_run with sound true. Quote the findings; do not invent others. Never guess why a sound is there or what it is made of.",
            seq,
            sound_map_tool,
        ),
        def(
            "sound_plan",
            "The spotting sheet: the sound plan of each beat, in order, with times from the start of the video: the ambience bed, the music (mood, in, out), the effects and the deliberate silences, and the beats with nothing planned yet. Use it to talk about what the sound of the video is meant to do, and to compare it with the sound map.",
            json!({"beat": {"type": "integer", "description": "One beat's id from planner_get. Default: every beat."}}),
            sound_plan_tool,
        ),
    ]
}
