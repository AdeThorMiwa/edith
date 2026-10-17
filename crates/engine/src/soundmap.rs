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
    let Some(plan) = crate::planner::get(&s.project) else { return Vec::new() };
    let laid = edith_planner::ranges(&plan, filmcraft_time::Tick::ZERO);
    let mut out = Vec::new();
    for (b, r) in plan.beats().zip(laid) {
        let start = b.link.map_or(r.start.0, |l| l.start.0);
        for c in b.sound.cues.iter().filter(|c| c.kind == edith_planner::CueKind::Silence) {
            let from = start.saturating_add(c.at.0);
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
    let want_levels = bool_p(p, "levels").unwrap_or(true);
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
