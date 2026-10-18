//! `music_beats` and `silence_map` (fork roadmap F16.2): where a score's beats fall on the timeline, and where the
//! timeline has no sound at all.
//!
//! Both read what is already measured and say so when it is not: the beat grid comes from the audio report of a
//! music clip's media (`analysis_run` with the item), the silences from where clips sit on the sound map's lanes.
//! Read-only, in every mode. Times are seconds on the timeline.

use edith_ai::ToolOutput;
use filmcraft_project::TrackItem;
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use crate::Session;
use crate::analysis::{Cache, current_report_for};
use crate::assistant_tools::{ToolDef, def, parse_time};
use crate::narration_timings::{secs, ticks, to_timeline};

/// Beats in one answer.
const MAX_BEATS: usize = 200;
/// Gaps in one answer.
const MAX_GAPS: usize = 60;
/// A gap shorter than this is not listed unless asked for, seconds.
const DEFAULT_MIN_GAP: f64 = 0.5;

// ---------------------------------------------------------------------------- music_beats

/// The beat grid of a clip's media: tempo, confidence and beats in seconds from the start of the measured part.
struct Grid {
    bpm: f64,
    confidence: f64,
    beats: Vec<f64>,
    /// Where the measured part starts in the media (a subclip's start), seconds.
    base: f64,
    truncated: bool,
}

fn grid_of(s: &Session, it: &TrackItem) -> Result<Grid, String> {
    let cache = Cache::of(s);
    let Some(report) = current_report_for(s, &cache, "item", it.item.0) else {
        return Err(format!(
            "this clip's music has not been measured: analysis_run with item {} measures its pictures and sound, including any steady beat",
            it.item.0
        ));
    };
    let audio = &report["audio"];
    let Some(tempo) = audio["stats"]["tempo"].as_object() else {
        let kind = audio["stats"]["kind"].as_str().unwrap_or("this sound");
        return Err(format!(
            "no steady beat was found in it (it reads as {kind}, and a beat grid needs a clear, constant pulse), so there is no beat grid to cut to. Cut on phrases, on the edit's markers, or ask the person where the beats are"
        ));
    };
    let beats: Vec<f64> = tempo
        .get("beats")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).filter(|b| b.is_finite() && *b >= 0.0).collect())
        .unwrap_or_default();
    let base = s.project.resolve_media(it.item).and_then(|(_, _, range)| range).map_or(0.0, |r| secs(r.start));
    Ok(Grid {
        bpm: tempo.get("bpm").and_then(Value::as_f64).unwrap_or(0.0),
        confidence: tempo.get("confidence").and_then(Value::as_f64).unwrap_or(0.0),
        beats,
        base,
        truncated: audio["truncated"] == true,
    })
}

fn music_beats(s: &Session, input: &Value) -> ToolOutput {
    let Some(q) = s.active_sequence() else { return ToolOutput::error("no sequence is open") };
    let narrations = &s.project.narrations;
    let all_audio: Vec<&TrackItem> = q.audio_tracks.iter().flat_map(|t| t.items.iter()).filter(|it| it.enabled && !narrations.contains_key(&it.item)).collect();
    let it = match input.get("clip").filter(|v| !v.is_null()) {
        Some(v) => {
            let Some(c) = v.as_u64() else { return ToolOutput::error("`clip` must be a clip id (from sequence_view)") };
            match q.find_item(filmcraft_project::ClipId(c)) {
                Some((_, it)) => it,
                None => return ToolOutput::error(format!("no clip {c} in this sequence")),
            }
        }
        None => {
            let with_grid: Vec<&TrackItem> = all_audio.iter().copied().filter(|it| grid_of(s, it).is_ok()).collect();
            match (with_grid.as_slice(), all_audio.as_slice()) {
                ([one], _) => *one,
                ([], []) => return ToolOutput::error("there is no music or other sound on the timeline to find beats in"),
                ([], [one]) => *one,
                ([], many) => {
                    let list: Vec<String> = many.iter().take(10).map(|it| format!("clip {} “{}”", it.id.0, it.name)).collect();
                    return ToolOutput::error(format!(
                        "none of the audio clips has a measured beat grid ({}). analysis_run with the item of the music clip measures it; then pass `clip`",
                        list.join(", ")
                    ));
                }
                (many, _) => {
                    let list: Vec<String> = many.iter().take(10).map(|it| format!("clip {} “{}”", it.id.0, it.name)).collect();
                    return ToolOutput::error(format!("{} clips have a beat grid: {}. Pass `clip`.", many.len(), list.join(", ")));
                }
            }
        }
    };
    if it.reverse || it.frame_hold.is_some() || !(it.speed.is_finite() && it.speed > 0.0) {
        return ToolOutput::error("that clip is reversed, frozen or at a speed of zero, so its beats cannot be placed on the timeline");
    }
    let grid = match grid_of(s, it) {
        Ok(g) => g,
        Err(e) => return ToolOutput::error(e),
    };
    let from = match input.get("from").filter(|v| !v.is_null()).map(|v| parse_time(v, q, "from")) {
        Some(Err(e)) => return ToolOutput::error(e),
        Some(Ok(t)) => Some(t),
        None => None,
    };
    let to = match input.get("to").filter(|v| !v.is_null()).map(|v| parse_time(v, q, "to")) {
        Some(Err(e)) => return ToolOutput::error(e),
        Some(Ok(t)) => Some(t),
        None => None,
    };
    let every = input.get("every").and_then(Value::as_u64).unwrap_or(1).clamp(1, 64) as usize;
    // beats that are heard through the clip, on the timeline
    let heard: Vec<Tick> = grid
        .beats
        .iter()
        .map(|b| to_timeline(it, ticks((grid.base + b) * TICKS_PER_SECOND as f64)))
        .filter(|t| *t >= it.start && *t < it.end() && from.is_none_or(|f| *t >= f) && to.is_none_or(|e| *t < e))
        .collect();
    let name = s.project.items.get(&it.item).map_or(it.name.as_str(), |i| i.name.as_str());
    let mut out = format!(
        "Beats of clip {} “{}”: a steady pulse near {:.1} BPM (one beat every {:.2} s), confidence {:.0}%{}.\n",
        it.id.0,
        name,
        grid.bpm,
        if grid.bpm > 0.0 { 60.0 / grid.bpm } else { 0.0 },
        grid.confidence * 100.0,
        if grid.confidence < 0.4 { " (low: treat the grid as a rough guide and check it against the sound)" } else { "" }
    );
    out.push_str("The grid is one constant tempo found by measurement, so it follows a score that does not speed up or slow down. Each beat is good to roughly a tenth of a second (the tempo can be off by a fraction of a BPM, which adds up over a long score), so for a cut that must land exactly on a hit, check a frame near it. Which beat starts a bar is not known (every 4th from the first is a common guess for 4/4, not a fact).\n");
    if grid.truncated {
        out.push_str("Only the first five minutes of the sound were measured.\n");
    }
    if heard.is_empty() {
        out.push_str("No beats fall inside the part of this clip that is on the timeline (or inside the window asked for).\n");
        return ToolOutput::ok(out);
    }
    let picked: Vec<&Tick> = heard.iter().step_by(every).collect();
    out.push_str(&format!(
        "{} beat{} on the timeline{}; seconds on the timeline:\n",
        heard.len(),
        if heard.len() == 1 { "" } else { "s" },
        if every > 1 { format!(", every {every}th shown") } else { String::new() }
    ));
    let shown: Vec<String> = picked.iter().take(MAX_BEATS).map(|t| format!("{:.2}", secs(**t))).collect();
    out.push_str(&shown.join(" "));
    if picked.len() > MAX_BEATS {
        out.push_str(&format!("\n({} more not listed: narrow with `from` and `to`, or raise `every`)", picked.len() - MAX_BEATS));
    }
    ToolOutput::ok(out)
}

// ---------------------------------------------------------------------------- silence_map

fn silence_map(s: &Session, input: &Value) -> ToolOutput {
    let Some(q) = s.active_sequence() else { return ToolOutput::error("no sequence is open") };
    let Some(id) = s.state.active_sequence else { return ToolOutput::error("no sequence is open") };
    let min = input.get("minSeconds").and_then(Value::as_f64).filter(|m| m.is_finite()).unwrap_or(DEFAULT_MIN_GAP).clamp(0.05, 3600.0);
    let from = match input.get("from").filter(|v| !v.is_null()).map(|v| parse_time(v, q, "from")) {
        Some(Err(e)) => return ToolOutput::error(e),
        Some(Ok(t)) => t.0,
        None => 0,
    };
    let (map_input, _) = match crate::soundmap::input_for(s, id, false) {
        Ok(m) => m,
        Err(e) => return ToolOutput::error(e.to_string()),
    };
    let to = match input.get("to").filter(|v| !v.is_null()).map(|v| parse_time(v, q, "to")) {
        Some(Err(e)) => return ToolOutput::error(e),
        Some(Ok(t)) => t.0.min(map_input.duration),
        None => map_input.duration,
    };
    if map_input.duration <= 0 || map_input.clips.is_empty() {
        return ToolOutput::ok("There is no sound on the timeline at all, so the whole programme is silent.");
    }
    // everything that sounds, merged
    let mut spans: Vec<(i64, i64)> = map_input.clips.iter().map(|c| (c.start, c.end)).filter(|(a, b)| b > a).collect();
    spans.sort_unstable();
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (a, b) in spans {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    let mut gaps: Vec<(i64, i64)> = Vec::new();
    let mut at = from.max(0);
    for (a, b) in &merged {
        if *a > at {
            gaps.push((at, (*a).min(to)));
        }
        at = at.max(*b);
    }
    if at < to {
        gaps.push((at, to));
    }
    let min_ticks = (min * TICKS_PER_SECOND as f64) as i64;
    gaps.retain(|(a, b)| b - a >= min_ticks);
    let planned = |a: i64, b: i64| map_input.silences.iter().any(|(pa, pb)| a >= *pa - TICKS_PER_SECOND / 10 && b <= *pb + TICKS_PER_SECOND / 10);
    let mut out = format!(
        "Stretches of at least {min:.2} s where nothing sounds on any track (muted tracks and disabled clips do not count), from {:.2} s to {:.2} s:\n",
        secs(Tick(from)),
        secs(Tick(to))
    );
    if gaps.is_empty() {
        out.push_str("None: something is sounding the whole way.\n");
    }
    for (a, b) in gaps.iter().take(MAX_GAPS) {
        out.push_str(&format!(
            "- {:.2}–{:.2} s ({:.2} s) {}\n",
            secs(Tick(*a)),
            secs(Tick(*b)),
            secs(Tick(b - a)),
            if planned(*a, *b) { "planned: the sound plan asks for this silence" } else { "unplanned" }
        ));
    }
    if gaps.len() > MAX_GAPS {
        out.push_str(&format!("({} more not listed: narrow with `from` and `to`, or raise `minSeconds`)\n", gaps.len() - MAX_GAPS));
    }
    out.push_str("This is where no clip sounds. It does not find quiet moments inside a clip, and the pauses between spoken words are in narration_timings.");
    ToolOutput::ok(out)
}

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![
        def(
            "music_beats",
            "Where the beats of a music clip fall on the timeline, in seconds, from the steady pulse found when its sound was measured (analysis_run with its item). Says the tempo, how confident the measurement is, and honestly says when there is no steady beat to find. Use it to cut or change shot on the beat. `every` thins the list (4 shows one beat in four). With one measured music clip, `clip` can be left out.",
            json!({
                "clip": {"type": "integer", "description": "The music clip's id (from sequence_view)."},
                "from": {"type": "string", "description": "Only beats after this timeline time."},
                "to": {"type": "string", "description": "Only beats before this timeline time."},
                "every": {"type": "integer", "description": "Show every Nth beat (1 to 64). Default 1."},
            }),
            music_beats,
        ),
        def(
            "silence_map",
            "The stretches of the timeline where nothing sounds on any track, each marked as planned (the sound plan asks for the silence) or unplanned. Cheap and exact: it reads where clips sit, not the audio, so it does not find quiet moments inside a clip (narration_timings has the pauses between words).",
            json!({
                "minSeconds": {"type": "number", "description": "Shortest gap to list, seconds. Default 0.5."},
                "from": {"type": "string", "description": "Start of the stretch to look at."},
                "to": {"type": "string", "description": "End of the stretch to look at."},
            }),
            silence_map,
        ),
    ]
}
