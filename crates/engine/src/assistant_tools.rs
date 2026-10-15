//! The assistant's tools: what the Edith agent can look at. All of them only read the project.
//!
//! Results are compact text written for a model (and readable by a person): times are timecodes
//! in the sequence's own rate, clips carry their id (`#29`) so the model can ask about one, and
//! every result is bounded. The design, and the measurement behind choosing a compact edit list
//! over OpenTimelineIO, is in `docs/upcoming-features.md` (F03.2).

use crate::Session;
use edith_ai::{ToolOutput, ToolSpec};
use filmcraft_project::{ClipId, EffectInstance, ItemId, ItemKind, Label, Marker, Param, ParamValue, Sequence, Track, TrackItem};
use filmcraft_time::{TICKS_PER_SECOND, Tick, TimeRange, format_timecode_frames};
use serde_json::{Value, json};

/// Clips listed by `sequence_view` before it asks the model to narrow the range.
const MAX_CLIPS: usize = 300;
/// Words returned by `transcript`.
const MAX_WORDS: usize = 600;
/// Effects whose presence on every clip says nothing; `clip_detail` shows their values.
const BASE_EFFECTS: [&str; 6] = ["motion", "opacity", "time_remap", "volume", "channel_volume", "panner"];

pub(crate) struct ToolDef {
    pub spec: ToolSpec,
    /// True when the tool leaves the project unchanged. The engine re-checks this before running.
    pub read_only: bool,
    pub run: fn(&Session, &Value) -> ToolOutput,
}

fn def(name: &str, description: &str, properties: Value, run: fn(&Session, &Value) -> ToolOutput) -> ToolDef {
    ToolDef {
        spec: ToolSpec {
            name: name.into(),
            description: description.into(),
            input_schema: json!({"type": "object", "properties": properties, "additionalProperties": false}),
        },
        read_only: true,
        run,
    }
}

pub(crate) fn tools() -> Vec<ToolDef> {
    let time = "A time: seconds (12.5) or timecode (\"00:00:12:03\") in the sequence's rate.";
    vec![
        def(
            "project_overview",
            "The project at a glance: name, the sequences with their lengths, the open sequence, the playhead, what is selected, and how much media there is. Start here.",
            json!({}),
            overview,
        ),
        def(
            "sequence_view",
            "The edit as a compact list: one line per clip with its track, timecodes, name and source start, then non-default speed, gain and effect names, transitions and markers. Clip ids look like #29; pass one to clip_detail for everything about it. Use from and to to look at one part of a long edit.",
            json!({
                "sequence": {"type": "integer", "description": "Sequence id from project_overview. Default: the open sequence."},
                "from": {"type": "string", "description": time},
                "to": {"type": "string", "description": time},
            }),
            sequence_view,
        ),
        def(
            "clip_detail",
            "Everything about one clip: its source file and media details, where it sits, its source range, speed, gain, every effect with its parameter values, markers on it, transitions touching it, and the start of its transcript.",
            json!({"clip": {"type": "integer", "description": "Clip id, the number after # in sequence_view."}, "sequence": {"type": "integer"}}),
            clip_detail,
        ),
        def(
            "transcript",
            "The spoken words of a clip or media item with timecodes, grouped into lines at pauses. For a clip the times are timeline times and only the part of the media the clip shows is included. Returns nothing if no transcript has been made.",
            json!({
                "clip": {"type": "integer", "description": "A clip id: its visible words, in timeline time."},
                "item": {"type": "integer", "description": "A media item id: the whole transcript, in media time."},
                "from": {"type": "string", "description": time},
                "to": {"type": "string", "description": time},
            }),
            transcript,
        ),
    ]
}

// ---------------------------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------------------------

fn tc(q: &Sequence, t: Tick) -> String {
    format_timecode_frames(q.settings.frame_rate.frame_at(t), q.settings.frame_rate, q.settings.drop_frame)
}

fn secs(t: Tick) -> f64 {
    t.0 as f64 / TICKS_PER_SECOND as f64
}

/// A time argument: seconds as a number or string, or `HH:MM:SS:FF`, `MM:SS:FF`, `SS:FF`.
fn parse_time(v: &Value, q: &Sequence, what: &str) -> Result<Tick, String> {
    let seconds = match v {
        Value::Number(n) => n.as_f64().ok_or_else(|| format!("{what} is not a usable number"))?,
        Value::String(s) if s.contains([':', ';']) => {
            let parts: Vec<&str> = s.trim().split([':', ';']).collect();
            if parts.len() > 4 || parts.iter().any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit())) {
                return Err(format!("{what} {s:?} is not a timecode like 00:00:12:03"));
            }
            let nums: Vec<f64> = parts.iter().map(|p| p.parse::<f64>().unwrap_or(0.0)).collect();
            let fps = q.settings.frame_rate.as_f64().max(1.0);
            let (h, m, s2, f) = match nums.as_slice() {
                [f] => (0.0, 0.0, *f, 0.0),
                [s, f] => (0.0, 0.0, *s, *f),
                [m, s, f] => (0.0, *m, *s, *f),
                [h, m, s, f] => (*h, *m, *s, *f),
                _ => (0.0, 0.0, 0.0, 0.0),
            };
            h * 3600.0 + m * 60.0 + s2 + f / fps
        }
        Value::String(s) => s.trim().parse::<f64>().map_err(|_| format!("{what} {s:?} is not a time"))?,
        _ => return Err(format!("{what} must be seconds or a timecode")),
    };
    if !seconds.is_finite() || !(0.0..=1.0e7).contains(&seconds) {
        return Err(format!("{what} is out of range"));
    }
    Ok(Tick((seconds * TICKS_PER_SECOND as f64).round() as i64))
}

fn id_arg(input: &Value, key: &str) -> Result<Option<u64>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| format!("{key} must be a whole number")),
    }
}

/// The sequence asked for (or the open one) and its project item id.
fn sequence_arg<'a>(s: &'a Session, input: &Value) -> Result<(ItemId, &'a Sequence), String> {
    let id = match id_arg(input, "sequence")? {
        Some(n) => ItemId(n),
        None => s.state.active_sequence.ok_or("no sequence is open")?,
    };
    s.project.sequence(id).map(|q| (id, q)).ok_or_else(|| format!("there is no sequence {}", id.0))
}

fn track_tag(q: &Sequence, t: &Track) -> String {
    if let Some(i) = q.video_tracks.iter().position(|x| x.id == t.id) {
        return format!("V{}", i + 1);
    }
    if let Some(i) = q.audio_tracks.iter().position(|x| x.id == t.id) {
        return format!("A{}", i + 1);
    }
    t.name.clone()
}

fn param_text(p: &Param) -> String {
    let base = match &p.value {
        ParamValue::Float(f) => format!("{:.3}", f).trim_end_matches('0').trim_end_matches('.').to_string(),
        ParamValue::Vec2(v) => {
            let c = |x: f64| if x.is_nan() { "auto".to_string() } else { format!("{x:.1}") };
            format!("({}, {})", c(v.x), c(v.y))
        }
        ParamValue::Color(c) => format!("#{:02x}{:02x}{:02x}{:02x}", (c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, (c[3] * 255.0) as u8),
        ParamValue::Bool(b) => b.to_string(),
        ParamValue::Choice(n) => format!("choice {n}"),
        ParamValue::Text(t) => format!("{:?}", t.chars().take(60).collect::<String>()),
        ParamValue::Curve(c) => format!("curve, {} points", c.len()),
        ParamValue::Path(_) => "mask path".to_string(),
    };
    if p.keyframes.is_empty() { base } else { format!("{base} ({} keyframes)", p.keyframes.len()) }
}

fn effect_line(e: &EffectInstance) -> String {
    let params: Vec<String> = e.params.iter().map(|(k, p)| format!("{k}={}", param_text(p))).collect();
    let mut s = format!("{}{}", e.effect, if e.enabled { "" } else { " (off)" });
    if !params.is_empty() {
        s.push_str(&format!(": {}", params.join(", ")));
    }
    if !e.masks.is_empty() {
        s.push_str(&format!(" [{} mask{}]", e.masks.len(), if e.masks.len() == 1 { "" } else { "s" }));
    }
    s
}

fn marker_line(q: &Sequence, m: &Marker) -> String {
    let mut s = format!("{} {} ({:?}, {})", tc(q, m.start), if m.name.is_empty() { "(unnamed)" } else { &m.name }, m.kind, Label::name(m.color));
    if m.duration > Tick::ZERO {
        s.push_str(&format!(", {:.1}s", secs(m.duration)));
    }
    if !m.comment.is_empty() {
        s.push_str(&format!(": {}", m.comment.chars().take(200).collect::<String>()));
    }
    s
}

fn clip_line(q: &Sequence, t: &Track, i: &TrackItem) -> String {
    let mut s = format!("{} #{} {}-{} {} src {}", track_tag(q, t), i.id.0, tc(q, i.start), tc(q, i.end()), i.name, tc(q, i.source_in));
    let mut extra: Vec<String> = Vec::new();
    if (i.speed.abs() - 1.0).abs() > 1e-9 {
        extra.push(format!("speed {:.0}%", i.speed.abs() * 100.0));
    }
    if i.reverse || i.speed < 0.0 {
        extra.push("reverse".into());
    }
    if i.gain_db.abs() > 1e-9 {
        extra.push(format!("gain {:+.1}dB", i.gain_db));
    }
    if !i.enabled {
        extra.push("disabled".into());
    }
    let fx: Vec<&str> = i.effects.iter().filter(|e| !BASE_EFFECTS.contains(&e.effect.as_str())).map(|e| e.effect.as_str()).collect();
    if !fx.is_empty() {
        extra.push(format!("fx {}", fx.join(",")));
    }
    if !extra.is_empty() {
        s.push_str(&format!("  [{}]", extra.join("; ")));
    }
    s
}

// ---------------------------------------------------------------------------------------------
// the tools
// ---------------------------------------------------------------------------------------------

fn overview(s: &Session, _input: &Value) -> ToolOutput {
    let p = &s.project;
    let mut out = format!("project {:?} ({})\n", p.name, if s.is_dirty() { "unsaved changes" } else { "saved" });
    let (mut movies, mut other) = (0, 0);
    for it in p.items.values() {
        match &it.kind {
            ItemKind::Media(_) => movies += 1,
            ItemKind::Sequence(_) => {}
            _ => other += 1,
        }
    }
    let active = s.state.active_sequence;
    match active.and_then(|id| p.sequence(id).map(|q| (id, q))) {
        Some((id, q)) => {
            let sel: Vec<String> = s.state.selection.iter().take(20).map(|c| format!("#{}", c.0)).collect();
            out.push_str(&format!(
                "open sequence: {:?} (id {}), playhead {}, selection: {}\n",
                p.item(id).map(|i| i.name.as_str()).unwrap_or("?"),
                id.0,
                tc(q, s.playhead()),
                if sel.is_empty() { "none".to_string() } else { sel.join(" ") }
            ));
        }
        None => out.push_str("no sequence is open\n"),
    }
    out.push_str("sequences:\n");
    for it in p.sequences().take(50) {
        if let ItemKind::Sequence(q) = &it.kind {
            out.push_str(&format!(
                "  {:?} (id {}): {}, {}x{} {:.3}fps, {} video and {} audio tracks, {} markers\n",
                it.name,
                it.id.0,
                tc(q, q.duration()),
                q.settings.width,
                q.settings.height,
                q.settings.frame_rate.as_f64(),
                q.video_tracks.len(),
                q.audio_tracks.len(),
                q.markers.len()
            ));
        }
    }
    out.push_str(&format!("media: {movies} items in the pool, {} other items (graphics, subclips, adjustment layers)\n", other));
    if !p.notes.trim().is_empty() {
        out.push_str(&format!("project notes: {}\n", p.notes.chars().take(300).collect::<String>().replace('\n', " ")));
    }
    ToolOutput::ok(out)
}

fn sequence_view(s: &Session, input: &Value) -> ToolOutput {
    let (id, q) = match sequence_arg(s, input) {
        Ok(v) => v,
        Err(e) => return ToolOutput::error(e),
    };
    let from = match input.get("from").filter(|v| !v.is_null()).map(|v| parse_time(v, q, "from")).transpose() {
        Ok(v) => v.unwrap_or(Tick::ZERO),
        Err(e) => return ToolOutput::error(e),
    };
    let dur = q.duration();
    // Without `to`, up to the end of the edit (or `from`, if that is already past the end).
    let to = match input.get("to").filter(|v| !v.is_null()).map(|v| parse_time(v, q, "to")).transpose() {
        Ok(v) => v.unwrap_or(dur.max(from)),
        Err(e) => return ToolOutput::error(e),
    };
    if to < from {
        return ToolOutput::error("`to` is before `from`");
    }
    let range = TimeRange::from_bounds(from, to);
    let fr = q.settings.frame_rate;
    let mut out = format!(
        "sequence {:?} (id {}) {}x{} {:.3}fps, length {}; showing {} to {}\n",
        s.project.item(id).map(|i| i.name.as_str()).unwrap_or("?"),
        id.0,
        q.settings.width,
        q.settings.height,
        fr.as_f64(),
        tc(q, dur),
        tc(q, from),
        tc(q, to)
    );
    let mut shown = 0usize;
    let mut hidden = 0usize;
    let tracks = q.video_tracks.iter().rev().chain(q.audio_tracks.iter());
    let mut transitions: Vec<String> = Vec::new();
    for t in tracks {
        for i in t.items.iter().filter(|i| i.start < range.end() && i.end() > range.start) {
            if shown >= MAX_CLIPS {
                hidden += 1;
                continue;
            }
            out.push_str(&clip_line(q, t, i));
            out.push('\n');
            shown += 1;
        }
        for x in t.transitions.iter().filter(|x| x.start < range.end() && x.start + x.duration > range.start) {
            let side = |c: Option<ClipId>| c.map(|c| format!("#{}", c.0)).unwrap_or_else(|| "nothing".into());
            transitions.push(format!(
                "{} {} {} for {} frames, from {} to {}",
                track_tag(q, t),
                tc(q, x.start),
                x.effect.effect,
                fr.frame_at(x.duration),
                side(x.from),
                side(x.to)
            ));
        }
    }
    if hidden > 0 {
        out.push_str(&format!("[{hidden} more clips not shown; ask for a narrower range with from and to]\n"));
    }
    if shown == 0 {
        out.push_str("(no clips in this range)\n");
    }
    if !transitions.is_empty() {
        out.push_str("transitions:\n");
        for t in transitions.iter().take(100) {
            out.push_str(&format!("  {t}\n"));
        }
    }
    let markers: Vec<&Marker> = q.markers.iter().filter(|m| m.start >= range.start && m.start <= range.end()).collect();
    if !markers.is_empty() {
        out.push_str("markers:\n");
        for m in markers.iter().take(100) {
            out.push_str(&format!("  {}\n", marker_line(q, m)));
        }
    }
    ToolOutput::ok(out)
}

/// The clip with this id in `q`, and its track.
fn find_clip(q: &Sequence, id: ClipId) -> Option<(&Track, &TrackItem)> {
    q.video_tracks.iter().chain(q.audio_tracks.iter()).find_map(|t| t.items.iter().find(|i| i.id == id).map(|i| (t, i)))
}

fn clip_detail(s: &Session, input: &Value) -> ToolOutput {
    let (_, q) = match sequence_arg(s, input) {
        Ok(v) => v,
        Err(e) => return ToolOutput::error(e),
    };
    let clip = match id_arg(input, "clip") {
        Ok(Some(n)) => ClipId(n),
        Ok(None) => return ToolOutput::error("clip is required: the number after # in sequence_view"),
        Err(e) => return ToolOutput::error(e),
    };
    let Some((t, i)) = find_clip(q, clip) else { return ToolOutput::error(format!("there is no clip #{} in this sequence", clip.0)) };
    let mut out = format!("clip #{} {:?} on {}\n", i.id.0, i.name, track_tag(q, t));
    out.push_str(&format!("  on the timeline: {} to {} ({:.2}s)\n", tc(q, i.start), tc(q, i.end()), secs(i.duration)));
    out.push_str(&format!(
        "  source range: {} to {}, speed {:.0}%{}, gain {:+.1}dB, {}\n",
        tc(q, i.source_in),
        tc(q, i.source_out()),
        i.speed.abs() * 100.0,
        if i.reverse || i.speed < 0.0 { " reversed" } else { "" },
        i.gain_db,
        if i.enabled { "enabled" } else { "disabled" }
    ));
    if let Some(item) = s.project.item(i.item) {
        match &item.kind {
            ItemKind::Media(m) => {
                let file = match &m.media {
                    filmcraft_project::MediaRef::File { path: p } => p.rsplit(['/', '\\']).next().unwrap_or("").to_string(),
                    other => format!("{other:?}").chars().take(60).collect(),
                };
                out.push_str(&format!("  source: {file} ({:?}, {:.2}s)", m.info.kind, secs(m.info.duration)));
                if let Some(v) = &m.info.video {
                    out.push_str(&format!(", {}x{}", v.width, v.height));
                }
                out.push('\n');
            }
            ItemKind::Sequence(_) => out.push_str("  source: a nested sequence\n"),
            _ => out.push_str(&format!("  source: {}\n", item.type_label())),
        }
    }
    if let Some(l) = i.link {
        out.push_str(&format!("  linked with clip #{l}\n"));
    }
    out.push_str("  effects:\n");
    if i.effects.is_empty() {
        out.push_str("    none\n");
    }
    for e in i.effects.iter().take(40) {
        out.push_str(&format!("    {}\n", effect_line(e)));
    }
    if !i.markers.is_empty() {
        out.push_str("  markers on the clip:\n");
        for m in i.markers.iter().take(20) {
            out.push_str(&format!("    {}\n", marker_line(q, m)));
        }
    }
    let touching: Vec<String> = t
        .transitions
        .iter()
        .filter(|x| x.from == Some(i.id) || x.to == Some(i.id))
        .map(|x| format!("{} at {} ({} frames)", x.effect.effect, tc(q, x.start), q.settings.frame_rate.frame_at(x.duration)))
        .collect();
    if !touching.is_empty() {
        out.push_str(&format!("  transitions: {}\n", touching.join("; ")));
    }
    if let Some(tr) = s.project.transcripts.get(&i.item) {
        let r = tr.words_in(TimeRange::from_bounds(i.source_in, i.source_out()));
        let words: Vec<&str> = tr.words.get(r.clone()).unwrap_or(&[]).iter().take(60).map(|w| w.text.as_str()).collect();
        if !words.is_empty() {
            out.push_str(&format!("  transcript starts: {}{}\n", words.join(" "), if r.len() > 60 { " ..." } else { "" }));
        }
    }
    ToolOutput::ok(out)
}

fn transcript(s: &Session, input: &Value) -> ToolOutput {
    let clip = match id_arg(input, "clip") {
        Ok(v) => v,
        Err(e) => return ToolOutput::error(e),
    };
    let item = match id_arg(input, "item") {
        Ok(v) => v,
        Err(e) => return ToolOutput::error(e),
    };
    let (_, q) = match sequence_arg(s, input) {
        Ok(v) => v,
        Err(e) => return ToolOutput::error(e),
    };
    let from = match input.get("from").filter(|v| !v.is_null()).map(|v| parse_time(v, q, "from")).transpose() {
        Ok(v) => v,
        Err(e) => return ToolOutput::error(e),
    };
    let to = match input.get("to").filter(|v| !v.is_null()).map(|v| parse_time(v, q, "to")).transpose() {
        Ok(v) => v,
        Err(e) => return ToolOutput::error(e),
    };
    // (the transcript, the media item, offset to timeline time: timeline = (media - origin) / speed + start)
    let (tr, label, map): (_, String, Box<dyn Fn(Tick) -> Tick>) = match (clip, item) {
        (Some(c), _) => {
            let Some((_, i)) = find_clip(q, ClipId(c)) else { return ToolOutput::error(format!("there is no clip #{c} in this sequence")) };
            let Some(tr) = s.project.transcripts.get(&i.item) else {
                return ToolOutput::ok(format!("clip #{c} has no transcript. A transcript can be made in the Text panel."));
            };
            let (start, origin, speed) = (i.start, i.source_in, i.speed.abs().max(1e-6));
            (tr, format!("clip #{c} {:?}", i.name), Box::new(move |m: Tick| Tick(start.0 + (((m.0 - origin.0) as f64) / speed).round() as i64)))
        }
        (None, Some(n)) => {
            let Some(tr) = s.project.transcripts.get(&ItemId(n)) else { return ToolOutput::ok(format!("media item {n} has no transcript.")) };
            (tr, format!("media item {n}"), Box::new(|m: Tick| m))
        }
        (None, None) => return ToolOutput::error("give a clip or an item"),
    };
    // Which words: for a clip, those inside its visible range; then the optional from and to in the output's own time.
    let visible: Option<TimeRange> = clip.and_then(|c| find_clip(q, ClipId(c))).map(|(_, i)| TimeRange::from_bounds(i.source_in, i.source_out()));
    let mut lines: Vec<String> = Vec::new();
    let (mut cur, mut cur_start, mut last_end, mut count) = (String::new(), Tick::ZERO, Tick::ZERO, 0usize);
    let pause = Tick(TICKS_PER_SECOND * 8 / 10);
    let mut cut = false;
    for w in &tr.words {
        if let Some(v) = visible
            && (w.end <= v.start || w.start >= v.end())
        {
            continue;
        }
        let t = map(w.start);
        if from.is_some_and(|f| t < f) || to.is_some_and(|x| t >= x) {
            continue;
        }
        if count >= MAX_WORDS {
            cut = true;
            break;
        }
        if !cur.is_empty() && (w.start - last_end > pause || cur.split(' ').count() >= 25) {
            lines.push(format!("[{}] {}", tc(q, cur_start), cur));
            cur = String::new();
        }
        if cur.is_empty() {
            cur_start = t;
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(&w.text);
        last_end = w.end;
        count += 1;
    }
    if !cur.is_empty() {
        lines.push(format!("[{}] {}", tc(q, cur_start), cur));
    }
    if lines.is_empty() {
        return ToolOutput::ok(format!("{label}: no spoken words in that range."));
    }
    let mut out =
        format!("transcript of {label} ({}, {}): times are {}\n", tr.language, tr.source, if clip.is_some() { "timeline time" } else { "media time" });
    out.push_str(&lines.join("\n"));
    if cut {
        out.push_str(&format!("\n[stopped after {MAX_WORDS} words; ask for a narrower range with from and to]"));
    }
    ToolOutput::ok(out)
}
