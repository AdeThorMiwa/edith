//! The assistant's analysis and coach tools (fork roadmap F11.2b). They let the model pull the
//! numbers; the numbers themselves are measured and judged by code (`edith-analysis`), never by the
//! model, which is why these tools return facts as compact text and the model is told to quote them.
//!
//! | tool | returns |
//! |---|---|
//! | `coach_findings` | what is worth telling the editor about the open edit: each finding with its place, severity, explanation and numbers |
//! | `analysis_get` | the measurements of the edit (always current: computed from the timeline), of a media item, or of the mix, in words and numbers; says what to run when there are none |
//! | `analysis_list` | what has been measured and whether it is still current |
//! | `analysis_run` | measure a media item, the sequence's shots or the mix; waits for the job (the editor stays usable) and answers with what it found |
//!
//! All four leave the project unchanged, so Ask and Coach both have them. `analysis_run` is the one
//! that cannot be answered at once: its reply waits until the background job finishes (see
//! [`finish_pending`]), and gives up after [`WAIT_SECONDS`] with a note to try `analysis_get` later.

use std::sync::atomic::Ordering;
use std::sync::mpsc::SyncSender;

use edith_ai::ToolOutput;
use serde_json::{Value, json};

use crate::Session;
use crate::analysis::{Cache, plan};
use crate::assistant::ToolRequest;
use crate::assistant_tools::{ToolDef, def};

/// How long `analysis_run` waits for its job before it answers that it is still running.
const WAIT_SECONDS: u64 = 300;
/// Findings and per-shot lines in one answer.
const MAX_LINES: usize = 40;

pub(crate) fn tools() -> Vec<ToolDef> {
    let target = |extra: Value| {
        let mut p = json!({
            "item": {"type": "integer", "description": "A media item id (from project_overview or sequence_view): footage, a still or a sound."},
            "sequence": {"type": "boolean", "description": "true: the open sequence (the edit's own numbers, and a look at each shot)."},
            "mix": {"type": "boolean", "description": "true: the sound of the whole open sequence (loudness, loudness range, peaks, silence)."},
        });
        if let (Some(o), Some(e)) = (p.as_object_mut(), extra.as_object()) {
            o.extend(e.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        p
    };
    vec![
        def(
            "coach_findings",
            "What is worth telling the editor about the open edit, worked out by fixed rules from the measurements and the plan's targets (not by you): flash frames, gaps, a short shot between long ones, long holds, pace against the plan, bursts of cuts, narration pace, J and L cuts, dissolves, and loudness once the mix has been measured. Each has an id, a severity (look = worth a look, problem = probably a problem), a timecode, a plain explanation and the numbers it rests on. Quote these; do not invent findings or numbers. If the mix has not been measured, say so before talking about loudness, and offer analysis_run with mix.",
            json!({"sequence": {"type": "integer", "description": "A sequence id. Default: the open sequence."}}),
            coach_findings,
        ),
        def(
            "analysis_get",
            "The measurements of one thing, as numbers. For the open sequence: shot lengths (mean, median, 10th and 90th percentile, shortest, longest, a histogram), cuts and cuts per minute, J and L cuts, transitions, narration pace, how much of each track is used, and (once analysis_run has looked at the shots) brightness, contrast, saturation, colour warmth and camera movement per shot. For a media item: its pictures over time and its sound. For the mix: loudness, loudness range, true peak, silence. If it has not been measured, the answer says which analysis_run call to make.",
            target(json!({})),
            analysis_get,
        ),
        def(
            "analysis_list",
            "What has been measured so far (items, sequences, mixes), and whether each report still matches the file or edit it was made from.",
            json!({}),
            analysis_list,
        ),
        def(
            "show",
            "Put a view in your answer, right where you call it, so the person reads one continuous reply instead of looking elsewhere. You choose which view and what to point at; the numbers in it are drawn from the measurements, you do not supply them. Views: findings (a card of the coach's findings: severity, a timecode they can click, the title), timeline (the edit as its shots in proportion, with findings marked, plan sections behind, click to jump the playhead), histogram (how many shots are short, medium, long, with the plan's target), pace (cuts a minute along the edit, against the plan), gauge (one number set against its target: narration pace, average shot, loudness, cuts a minute), palette (the colours of a measured media item), pie (screen time by shot length, narration, or silence). Call show after the sentence that introduces the view, then carry on in words; do not read out what the view already shows, say what it means. One or two views per answer, and a view never replaces the explanation. It needs the measurements to exist: for the findings and the timeline they come from the open edit.",
            json!({
                "view": {"type": "string", "enum": ["findings", "timeline", "histogram", "pace", "gauge", "palette", "pie"], "description": "Which view."},
                "metric": {"type": "string", "enum": ["narrationPace", "averageShot", "loudness", "cutsPerMinute"], "description": "gauge view: which number to set against its target."},
                "of": {"type": "string", "enum": ["screenTime", "narration", "silence"], "description": "pie view: what to divide up. screenTime: how much of the running time is in short, medium and long shots; narration: narrated or not; silence: sound or silence (needs the mix measured)."},
                "item": {"type": "integer", "description": "palette view: a media item that has been measured."},
                "ids": {"type": "array", "items": {"type": "string"}, "description": "findings view: which findings, by id from coach_findings. Default: the first eight."},
                "highlight": {"type": "array", "items": {"type": "string"}, "description": "timeline view: finding ids to mark. Default: every finding that has a place on the timeline."},
                "from": {"type": "string", "description": "timeline view: start of the stretch to draw (seconds or timecode). Default: the start."},
                "to": {"type": "string", "description": "timeline view: end of the stretch. Default: the end."},
                "caption": {"type": "string", "description": "A short line over the view (up to 120 characters)."},
            }),
            show,
        ),
        def(
            "analysis_run",
            "Measure something that has not been measured: an item (pictures and sound), the open sequence (a look at each shot), or its mix. It takes seconds to a minute; you get the result when it finishes. Use it when analysis_get says there are no measurements, and tell the person what you are about to measure.",
            target(json!({
                "samples": {"type": "integer", "description": "For an item: how many pictures to measure, 1 to 600. Default 60."},
                "force": {"type": "boolean", "description": "Measure again even if a current report exists."},
            })),
            |_, _| ToolOutput::error("analysis_run is answered by the session once its job finishes"),
        ),
    ]
}

// ---------------------------------------------------------------------------- running

/// An `analysis_run` call whose job has not finished: the reply waits here.
pub(crate) struct PendingTool {
    job: u64,
    key: String,
    reply: SyncSender<ToolOutput>,
    since: web_time::Instant,
}

/// Starts the job for an `analysis_run` call and holds its reply until the job is done.
pub(crate) fn start(s: &mut Session, req: ToolRequest) {
    let mut params = if req.input.is_object() { req.input.clone() } else { json!({}) };
    if let Some(o) = params.as_object_mut() {
        o.remove("wait");
    }
    match crate::analysis::run(s, &params) {
        Err(e) => {
            let _ = req.reply.send(ToolOutput::error(e.to_string()));
        }
        Ok(r) => {
            let key = r["key"].as_str().unwrap_or_default().to_string();
            match r["job"].as_u64() {
                None => {
                    let text = Cache::of(s).read(&key).map_or_else(|| "that was measured, but the report is gone".to_string(), |rep| describe(&rep));
                    let _ = req.reply.send(ToolOutput::ok(text));
                }
                Some(job) => s.assistant.pending_tools.push(PendingTool { job, key, reply: req.reply, since: web_time::Instant::now() }),
            }
        }
    }
}

/// Answers the `analysis_run` calls whose jobs have finished (or have waited too long).
pub(crate) fn finish_pending(s: &mut Session) {
    if s.assistant.pending_tools.is_empty() {
        return;
    }
    let mut keep = Vec::new();
    for p in std::mem::take(&mut s.assistant.pending_tools) {
        let job = s.jobs.iter().find(|j| j.id == p.job);
        let finished = job.is_none_or(|j| j.progress.finished.load(Ordering::Relaxed));
        if finished {
            let result = job.and_then(|j| j.result.lock().unwrap_or_else(|e| e.into_inner()).clone());
            let out = match (Cache::of(s).read(&p.key), result) {
                (Some(rep), _) => ToolOutput::ok(describe(&rep)),
                (None, Some(Err(e))) => ToolOutput::error(format!("the measurement failed: {e}")),
                (None, _) => ToolOutput::error("the measurement finished without a report"),
            };
            let _ = p.reply.send(out);
        } else if p.since.elapsed().as_secs() >= WAIT_SECONDS {
            let _ = p
                .reply
                .send(ToolOutput::ok(format!("The measurement is still running (job {}). Carry on, and call analysis_get later for the result.", p.job)));
        } else {
            keep.push(p);
        }
    }
    s.assistant.pending_tools = keep;
}

// ---------------------------------------------------------------------------- reading

fn text_of(r: crate::Result<String>) -> ToolOutput {
    match r {
        Ok(t) => ToolOutput::ok(t),
        Err(e) => ToolOutput::error(e.to_string()),
    }
}

fn analysis_get(s: &Session, p: &Value) -> ToolOutput {
    text_of(get_text(s, p))
}

fn get_text(s: &Session, p: &Value) -> crate::Result<String> {
    let wanted = plan(s, &normalised(p), "analysis_get")?;
    let cache = Cache::of(s);
    if let Some(report) = cache.read(&wanted.key).or_else(|| crate::analysis::current_report_for(s, &cache, wanted.kind, wanted.target)) {
        return Ok(describe(&report));
    }
    match wanted.kind {
        // the edit's own numbers need no media: always available, always current
        "sequence" => {
            let id = filmcraft_project::ItemId(wanted.target);
            let Some(filmcraft_project::ItemKind::Sequence(q)) = s.project.item(id).map(|i| &i.kind) else {
                return Err(crate::EngineError::Other("no such sequence".into()));
            };
            let stats = edith_analysis::analyze_sequence(&crate::analysis::sequence_input(s, q)).map_err(|e| crate::EngineError::Other(e.to_string()))?;
            let mut text = sequence_text(&wanted.name, &serde_json::to_value(&stats).unwrap_or(Value::Null), None);
            text.push_str("\nThe shots themselves (brightness, colour, camera movement) have not been looked at: analysis_run with sequence true does that.");
            Ok(text)
        }
        "mix" => {
            Ok("The mix has not been measured. analysis_run with mix true measures loudness, loudness range, peaks and silence over the whole edit.".into())
        }
        _ => Ok(format!("“{}” has not been measured. analysis_run with item {} measures its pictures and sound.", wanted.name, wanted.target)),
    }
}

/// `sequence: true` and `mix: true` as the commands take them; anything else is left to refuse.
fn normalised(p: &Value) -> Value {
    let mut q = if p.is_object() { p.clone() } else { json!({}) };
    if let Some(o) = q.as_object_mut() {
        if o.get("sequence") == Some(&json!(false)) {
            o.remove("sequence");
        }
        if o.get("mix") == Some(&json!(false)) {
            o.remove("mix");
        }
        if o.get("item").is_none() && o.get("sequence").is_none() && o.get("mix").is_none() {
            // nothing named: the open sequence is the likeliest meaning
            o.insert("sequence".into(), json!(true));
        }
    }
    q
}

fn analysis_list(s: &Session, _p: &Value) -> ToolOutput {
    let cache = Cache::of(s);
    let entries: Vec<Value> = cache.keys().iter().filter_map(|k| crate::analysis::entry(s, &cache, k)).collect();
    if entries.is_empty() {
        return ToolOutput::ok("Nothing has been measured yet.");
    }
    let mut out = String::new();
    for e in &entries {
        out.push_str(&format!(
            "- {} “{}” (id {}): {}\n",
            e["kind"].as_str().unwrap_or("?"),
            e["target"]["name"].as_str().unwrap_or("?"),
            e["target"]["id"],
            if e["current"] == true { "current" } else { "out of date: the file or the edit has changed since" }
        ));
    }
    ToolOutput::ok(out)
}

fn coach_findings(s: &Session, p: &Value) -> ToolOutput {
    let params = if p.is_object() { p.clone() } else { json!({}) };
    let r = match crate::analysis::coach_report(s, &params) {
        Ok(r) => r,
        Err(e) => return ToolOutput::error(e.to_string()),
    };
    ToolOutput::ok(coach_text(&r))
}

// ---------------------------------------------------------------------------- the words

fn num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|x| x.is_finite())
}

/// `5.2 s`, `0.4 s`, `38 s`.
fn len(seconds: f64) -> String {
    if seconds < 10.0 { format!("{seconds:.1} s") } else { format!("{:.0} s", seconds) }
}

/// `2:41`, `1:05:00`.
fn clock(seconds: f64) -> String {
    let n = seconds.max(0.0).floor() as u64;
    if n >= 3600 { format!("{}:{:02}:{:02}", n / 3600, (n / 60) % 60, n % 60) } else { format!("{}:{:02}", n / 60, n % 60) }
}

fn ticks_clock(ticks: &Value) -> String {
    clock(num(ticks).unwrap_or(0.0) / filmcraft_time::TICKS_PER_SECOND as f64)
}

pub(crate) fn coach_text(r: &Value) -> String {
    let findings = r["findings"].as_array().cloned().unwrap_or_default();
    let mut out = format!(
        "Coach findings for “{}”: {} shots, {} finding{}.\n",
        r["sequence"]["name"].as_str().unwrap_or("?"),
        r["shots"],
        findings.len(),
        if findings.len() == 1 { "" } else { "s" }
    );
    let t = &r["targets"];
    let mut targets = Vec::new();
    if let Some(v) = num(&t["averageShotSeconds"]) {
        targets.push(format!("about {} a shot", len(v)));
    }
    if let Some(v) = num(&t["wordsPerMinute"]) {
        targets.push(format!("{v:.0} words a minute"));
    }
    out.push_str(&if targets.is_empty() {
        "Targets: none (no plan, or its pack sets none).\n".to_string()
    } else {
        format!("Targets from the plan: {}.\n", targets.join(", "))
    });
    out.push_str(&if r["mixMeasured"] == true {
        "The mix has been measured, so loudness findings are included.\n".to_string()
    } else {
        "The mix has not been measured: loudness, silence and peak findings are not included. analysis_run with mix true measures it.\n".to_string()
    });
    if findings.is_empty() {
        out.push_str("Nothing stands out: no rule fired.\n");
    }
    for (i, f) in findings.iter().take(MAX_LINES).enumerate() {
        out.push_str(&format!(
            "{}. [{}] {} (id {}, at {})\n   {}\n",
            i + 1,
            if f["severity"] == "problem" { "problem" } else { "look" },
            f["title"].as_str().unwrap_or(""),
            f["id"].as_str().unwrap_or(""),
            ticks_clock(&f["start"]),
            f["detail"].as_str().unwrap_or("")
        ));
        let numbers: Vec<String> = f["numbers"]
            .as_object()
            .map(|o| o.iter().map(|(k, v)| format!("{k} {}", num(v).map_or("?".into(), |x| format!("{x:.2}")))).collect())
            .unwrap_or_default();
        let idea = f["idea"].as_str().map(|i| format!("idea: {i}; ")).unwrap_or_default();
        out.push_str(&format!("   {idea}numbers: {}\n", numbers.join(", ")));
    }
    if findings.len() > MAX_LINES {
        out.push_str(&format!("…and {} more not shown.\n", findings.len() - MAX_LINES));
    }
    out
}

/// A report in words and numbers.
pub(crate) fn describe(report: &Value) -> String {
    let name = report["target"]["name"].as_str().unwrap_or("?");
    match report["kind"].as_str() {
        Some("sequence") => sequence_text(name, &report["sequence"], Some(report)),
        Some("mix") => mix_text(name, report),
        Some("item") => item_text(name, report),
        _ => "an unrecognised report".into(),
    }
}

fn sequence_text(name: &str, q: &Value, report: Option<&Value>) -> String {
    let s = &q["shots"];
    let mut out = format!("The edit “{name}”: {} long, {} shots.\n", clock(num(&q["durationSeconds"]).unwrap_or(0.0)), s["count"]);
    if s["count"].as_u64().unwrap_or(0) > 0 {
        out.push_str(&format!(
            "Shot length: mean {}, median {}, 10th to 90th percentile {} to {}, shortest {}, longest {}.\n",
            len(num(&s["meanSeconds"]).unwrap_or(0.0)),
            len(num(&s["medianSeconds"]).unwrap_or(0.0)),
            len(num(&s["p10Seconds"]).unwrap_or(0.0)),
            len(num(&s["p90Seconds"]).unwrap_or(0.0)),
            len(num(&s["minSeconds"]).unwrap_or(0.0)),
            len(num(&s["maxSeconds"]).unwrap_or(0.0)),
        ));
        let bars: Vec<String> = s["histogram"]
            .as_array()
            .map(|h| {
                let mut lower = 0.0;
                h.iter()
                    .map(|b| {
                        let label = match num(&b["belowSeconds"]) {
                            Some(up) => {
                                let l = format!("{}–{} s", lower as u32, up as u32);
                                lower = up;
                                l
                            }
                            None => format!("{} s and more", lower as u32),
                        };
                        format!("{label}: {}", b["count"])
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push_str(&format!("Shot lengths by size: {}.\n", bars.join(", ")));
    }
    let c = &q["cuts"];
    out.push_str(&format!("Cuts: {} ({:.1} a minute).\n", c["count"], num(&c["perMinute"]).unwrap_or(0.0)));
    if let Some(series) = c["series"].as_array() {
        let w = num(&c["windowSeconds"]).unwrap_or(30.0);
        let shown: Vec<String> = series.iter().take(MAX_LINES).map(|n| n.to_string()).collect();
        out.push_str(&format!("Cuts in each {w:.0} s stretch, from the start: {}{}.\n", shown.join(", "), if series.len() > MAX_LINES { ", …" } else { "" }));
    }
    let j = &q["splitCuts"];
    if j["cutsWithAudio"].as_u64().unwrap_or(0) > 0 {
        out.push_str(&format!(
            "J and L cuts: {} J and {} L among {} cuts with sound on both sides ({:.0}% are J or L).\n",
            j["jCuts"],
            j["lCuts"],
            j["cutsWithAudio"],
            num(&j["share"]).unwrap_or(0.0) * 100.0
        ));
    } else {
        out.push_str("J and L cuts: none can be told (no cut has linked sound on both sides).\n");
    }
    let t = &q["transitions"];
    let kinds: Vec<String> = t["byKind"].as_object().map(|o| o.iter().map(|(k, v)| format!("{k} {v}")).collect()).unwrap_or_default();
    out.push_str(&format!(
        "Transitions: {}{} ({:.0}% of the joins between shots).\n",
        t["count"],
        if kinds.is_empty() { String::new() } else { format!(" ({})", kinds.join(", ")) },
        num(&t["shareOfJoints"]).unwrap_or(0.0) * 100.0
    ));
    let n = &q["narration"];
    match num(&n["wordsPerMinute"]) {
        Some(wpm) => out.push_str(&format!(
            "Narration: {} words over {} spoken, {wpm:.0} words a minute; it covers {:.0}% of the edit.\n",
            n["words"],
            clock(num(&n["spokenSeconds"]).unwrap_or(0.0)),
            num(&n["share"]).unwrap_or(0.0) * 100.0
        )),
        None => out.push_str("Narration: none on the timeline.\n"),
    }
    let tracks: Vec<String> = q["tracks"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|t| {
                    format!(
                        "{}{} {:.0}%",
                        if t["video"] == true { "V" } else { "A" },
                        t["index"].as_u64().unwrap_or(0) + 1,
                        num(&t["share"]).unwrap_or(0.0) * 100.0
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    out.push_str(&format!("Track use: {}.\n", tracks.join(", ")));
    if let Some(shots) = report.and_then(|r| r["shots"].as_array()).filter(|a| !a.is_empty()) {
        out.push_str(&format!("A look at {} of {} shots:\n", shots.len(), report.map_or(Value::Null, |r| r["shotsTotal"].clone())));
        for sh in shots.iter().take(MAX_LINES) {
            let st = &sh["stats"];
            let at = format!("  {} ({}), clip #{}", clock(num(&sh["start"]).unwrap_or(0.0)), len(num(&sh["duration"]).unwrap_or(0.0)), sh["clip"]);
            if st.is_null() {
                out.push_str(&format!("{at}: could not be looked at ({})\n", sh["error"].as_str().unwrap_or("no picture")));
            } else {
                out.push_str(&format!(
                    "{at}: brightness {:.2}, contrast {:.2}, saturation {:.2}{}{}\n",
                    num(&st["lumaMean"]).unwrap_or(0.0),
                    num(&st["contrast"]).unwrap_or(0.0),
                    num(&st["saturation"]).unwrap_or(0.0),
                    num(&st["colourTemperature"]).map_or(String::new(), |k| format!(", warmth {k:.0} K")),
                    sh["camera"].as_str().map_or(String::new(), |c| format!(", camera {c}"))
                ));
            }
        }
        if shots.len() > MAX_LINES {
            out.push_str(&format!("  …and {} more.\n", shots.len() - MAX_LINES));
        }
    }
    out
}

fn mix_text(name: &str, r: &Value) -> String {
    let m = &r["mix"];
    let opt = |v: &Value, unit: &str| num(v).map_or("not measurable (too quiet or too short)".to_string(), |x| format!("{x:.1} {unit}"));
    format!(
        "The mix of “{name}”, {} measured{}:\nIntegrated loudness {}. Loudness range {}. True peak {}. Silence: {:.0}% of the time is quieter than −60 dBFS.\n",
        clock(num(&r["analysedSeconds"]).unwrap_or(0.0)),
        if r["truncated"] == true { " (the first three hours)" } else { "" },
        opt(&m["integratedLufs"], "LUFS"),
        opt(&m["loudnessRangeLu"], "LU"),
        opt(&m["truePeakDbtp"], "dBTP"),
        num(&m["silenceShare"]).unwrap_or(0.0) * 100.0
    )
}

fn item_text(name: &str, r: &Value) -> String {
    let mut out = format!("“{name}”:\n");
    let v = &r["video"];
    if !v.is_null() {
        let sum = &v["summary"];
        out.push_str(&format!(
            "Pictures ({} measured over {}, {}×{}): brightness {:.2}, contrast {:.2}, saturation {:.2}, sharpness {:.4}, grain {:.3}{}.\n",
            v["samples"].as_array().map_or(0, Vec::len),
            clock(num(&v["durationSeconds"]).unwrap_or(0.0)),
            v["width"],
            v["height"],
            num(&sum["lumaMean"]).unwrap_or(0.0),
            num(&sum["contrast"]).unwrap_or(0.0),
            num(&sum["saturation"]).unwrap_or(0.0),
            num(&sum["sharpness"]).unwrap_or(0.0),
            num(&sum["grain"]).unwrap_or(0.0),
            num(&sum["colourTemperature"]).map_or(String::new(), |k| format!(", warmth {k:.0} K"))
        ));
        let lb = &sum["letterbox"];
        if ["top", "bottom", "left", "right"].iter().any(|k| num(&lb[*k]).unwrap_or(0.0) > 0.0) {
            out.push_str(&format!(
                "Black bars: top {:.0}%, bottom {:.0}%, left {:.0}%, right {:.0}%.\n",
                num(&lb["top"]).unwrap_or(0.0) * 100.0,
                num(&lb["bottom"]).unwrap_or(0.0) * 100.0,
                num(&lb["left"]).unwrap_or(0.0) * 100.0,
                num(&lb["right"]).unwrap_or(0.0) * 100.0
            ));
        }
        let m = &sum["motion"];
        out.push_str(&format!(
            "Camera: {}; how much changes between frames on average {:.3}.\n",
            m["camera"].as_str().unwrap_or("unknown"),
            num(&m["energy"]).unwrap_or(0.0)
        ));
        if let Some(p) = sum["palette"].as_array().filter(|p| !p.is_empty()) {
            let colours: Vec<String> = p
                .iter()
                .map(|c| {
                    let rgb: Vec<u32> =
                        c["rgb"].as_array().map(|a| a.iter().map(|x| (num(x).unwrap_or(0.0) * 255.0).round() as u32).collect()).unwrap_or_default();
                    format!(
                        "#{:02x}{:02x}{:02x} {:.0}%",
                        rgb.first().unwrap_or(&0),
                        rgb.get(1).unwrap_or(&0),
                        rgb.get(2).unwrap_or(&0),
                        num(&c["share"]).unwrap_or(0.0) * 100.0
                    )
                })
                .collect();
            out.push_str(&format!("Palette: {}.\n", colours.join(", ")));
        }
    }
    let a = &r["audio"];
    if a.is_object() && a["stats"].is_object() {
        let st = &a["stats"];
        out.push_str(&format!(
            "Sound ({} measured): loudness {} LUFS, range {} LU, true peak {} dBTP, {:.0}% silent; spectrum {:.0}% low, {:.0}% mid, {:.0}% high; sounds like {}{}.\n",
            clock(num(&a["analysedSeconds"]).unwrap_or(0.0)),
            num(&st["integratedLufs"]).map_or("?".into(), |x| format!("{x:.1}")),
            num(&st["loudnessRangeLu"]).map_or("?".into(), |x| format!("{x:.1}")),
            num(&st["truePeakDbtp"]).map_or("?".into(), |x| format!("{x:.1}")),
            num(&st["silenceShare"]).unwrap_or(0.0) * 100.0,
            num(&st["spectral"]["low"]).unwrap_or(0.0) * 100.0,
            num(&st["spectral"]["mid"]).unwrap_or(0.0) * 100.0,
            num(&st["spectral"]["high"]).unwrap_or(0.0) * 100.0,
            st["kind"].as_str().unwrap_or("?"),
            num(&st["tempo"]["bpm"]).map_or(String::new(), |b| format!(", steady pulse near {b:.0} BPM"))
        ));
        if a["truncated"] == true {
            out.push_str("Only the first five minutes of the sound were measured.\n");
        }
    } else if let Some(e) = a["error"].as_str() {
        out.push_str(&format!("The sound could not be measured: {e}.\n"));
    }
    out
}

/// The coach's starting point, attached to each question in Coach mode: the edit's headline numbers
/// and what the rules found, short, so the model knows where to look. `None` with no sequence open.
pub(crate) fn coach_context(s: &Session) -> Option<String> {
    let report = crate::analysis::coach_report(s, &json!({})).ok()?;
    let id = filmcraft_project::ItemId(report["sequence"]["id"].as_u64()?);
    let filmcraft_project::ItemKind::Sequence(q) = &s.project.item(id)?.kind else { return None };
    let stats = edith_analysis::analyze_sequence(&crate::analysis::sequence_input(s, q)).ok()?;
    let stats = serde_json::to_value(&stats).ok()?;
    let findings = report["findings"].as_array().cloned().unwrap_or_default();
    let problems = findings.iter().filter(|f| f["severity"] == "problem").count();
    let ids: Vec<&str> = findings.iter().take(12).filter_map(|f| f["id"].as_str()).collect();
    let mut out = format!(
        "The open edit “{}” as it is now: {} long, {} shots, mean {}, median {}; {:.1} cuts a minute",
        report["sequence"]["name"].as_str().unwrap_or("?"),
        clock(num(&stats["durationSeconds"]).unwrap_or(0.0)),
        stats["shots"]["count"],
        len(num(&stats["shots"]["meanSeconds"]).unwrap_or(0.0)),
        len(num(&stats["shots"]["medianSeconds"]).unwrap_or(0.0)),
        num(&stats["cuts"]["perMinute"]).unwrap_or(0.0)
    );
    if let Some(w) = num(&stats["narration"]["wordsPerMinute"]) {
        out.push_str(&format!("; narration {w:.0} words a minute"));
    }
    out.push_str(".\n");
    let t = &report["targets"];
    let mut targets = Vec::new();
    if let Some(v) = num(&t["averageShotSeconds"]) {
        targets.push(format!("about {} a shot", len(v)));
    }
    if let Some(v) = num(&t["wordsPerMinute"]) {
        targets.push(format!("{v:.0} words a minute"));
    }
    out.push_str(&if targets.is_empty() {
        "The plan sets no targets (or there is no plan).\n".to_string()
    } else {
        format!("The plan's targets: {}.\n", targets.join(", "))
    });
    out.push_str(&format!(
        "The rules found {} thing{} ({problems} probably a problem){}. {}\n",
        findings.len(),
        if findings.len() == 1 { "" } else { "s" },
        if ids.is_empty() { String::new() } else { format!(": {}", ids.join(", ")) },
        if report["mixMeasured"] == true { "The mix has been measured." } else { "The mix has not been measured." }
    ));
    out.push_str("Pull coach_findings for the explanations and numbers before you say anything about them.");
    Some(out)
}

// ---------------------------------------------------------------------------- views

/// The views `show` can draw.
const VIEWS: [&str; 7] = ["findings", "timeline", "histogram", "pace", "gauge", "palette", "pie"];
/// Shots drawn in one timeline view.
const MAX_SHOTS_DRAWN: usize = 1200;
/// Findings marked on one timeline view.
const MAX_MARKS: usize = 40;
/// Kinds of finding that sit at a place on the timeline (the others are about the whole edit).
const PLACED: [&str; 6] = ["flashFrame", "emptyGap", "shortBetweenLong", "longHold", "sectionPace", "cutBurst"];

fn show(s: &Session, p: &Value) -> ToolOutput {
    match show_view(s, p) {
        Ok(out) => out,
        Err(e) => ToolOutput::error(e),
    }
}

fn caption_of(o: &serde_json::Map<String, Value>) -> Option<String> {
    let c: String = o.get("caption")?.as_str()?.chars().filter(|c| !c.is_control()).take(120).collect();
    let c = c.trim().to_string();
    (!c.is_empty()).then_some(c)
}

fn id_list(o: &serde_json::Map<String, Value>, key: &str, max: usize) -> Result<Option<Vec<String>>, String> {
    let Some(v) = o.get(key).filter(|v| !v.is_null()) else { return Ok(None) };
    let a = v.as_array().ok_or_else(|| format!("`{key}` must be a list of finding ids"))?;
    if a.len() > max {
        return Err(format!("`{key}` may name at most {max} findings"));
    }
    a.iter()
        .map(|x| x.as_str().map(str::to_string).ok_or_else(|| format!("`{key}` must be a list of finding ids (text)")))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// The findings with these ids, or an error that lists the ones there are.
fn pick<'a>(all: &'a [Value], ids: &[String]) -> Result<Vec<&'a Value>, String> {
    ids.iter()
        .map(|id| {
            all.iter().find(|f| f["id"].as_str() == Some(id.as_str())).ok_or_else(|| {
                let known: Vec<&str> = all.iter().take(10).filter_map(|f| f["id"].as_str()).collect();
                format!(
                    "there is no finding {id:?}. The ids are: {}.",
                    if known.is_empty() { "none (nothing was found)".to_string() } else { known.join(", ") }
                )
            })
        })
        .collect()
}

fn show_view(s: &Session, p: &Value) -> Result<ToolOutput, String> {
    let o = p.as_object().ok_or("show takes an object such as {\"view\": \"timeline\"}")?;
    let view = o.get("view").and_then(Value::as_str).ok_or_else(|| format!("`view` is required: one of {}", VIEWS.join(", ")))?;
    if !VIEWS.contains(&view) {
        return Err(format!("there is no view {:?}; use one of {}", view.chars().take(30).collect::<String>(), VIEWS.join(", ")));
    }
    let report = crate::analysis::coach_report(s, &json!({})).map_err(|e| e.to_string())?;
    let all: Vec<Value> = report["findings"].as_array().cloned().unwrap_or_default();
    let name = report["sequence"]["name"].as_str().unwrap_or("").to_string();
    let caption = caption_of(o);
    match view {
        "findings" => {
            let ids = id_list(o, "ids", 12)?;
            let chosen: Vec<&Value> = match &ids {
                Some(ids) => pick(&all, ids)?,
                None => all.iter().take(8).collect(),
            };
            if chosen.is_empty() {
                return Ok(ToolOutput::ok("There are no findings to show: no rule fired. Say that nothing stands out, and why that is good news."));
            }
            let rows: Vec<Value> = chosen
                .iter()
                .map(|f| json!({"id": f["id"], "kind": f["kind"], "severity": f["severity"], "start": f["start"], "end": f["end"], "title": f["title"]}))
                .collect();
            let names: Vec<&str> = chosen.iter().filter_map(|f| f["id"].as_str()).collect();
            let v = json!({"view": "findings", "caption": caption, "sequence": name, "total": all.len(), "findings": rows});
            Ok(ToolOutput::ok(format!("Shown to the person: a card with {} of {} findings ({}). It lists each one's timecode (they can click it) and title, so add the explanation and what to try; do not repeat the titles.", chosen.len(), all.len(), names.join(", "))).with_view(v))
        }
        "timeline" => {
            let id = s.state.active_sequence.ok_or("no sequence is open")?;
            let Some(filmcraft_project::ItemKind::Sequence(q)) = s.project.item(id).map(|i| &i.kind) else { return Err("no sequence is open".into()) };
            let input = crate::analysis::sequence_input(s, q);
            let shots = edith_analysis::shot_spans(&input).map_err(|e| e.to_string())?;
            let end = shots.iter().map(|x| x.end).max().unwrap_or(0);
            if shots.is_empty() || end <= 0 {
                return Err("the open edit has no shots on its video tracks to draw".into());
            }
            let from = match o.get("from").filter(|v| !v.is_null()) {
                Some(v) => crate::assistant_tools::parse_time(v, q, "`from`")?.0.clamp(0, end),
                None => 0,
            };
            let to = match o.get("to").filter(|v| !v.is_null()) {
                Some(v) => crate::assistant_tools::parse_time(v, q, "`to`")?.0.clamp(0, end),
                None => end,
            };
            if to <= from {
                return Err("`to` must be after `from`".into());
            }
            let in_range: Vec<&edith_analysis::ShotSpan> = shots.iter().filter(|x| x.end > from && x.start < to).collect();
            let truncated = in_range.len() > MAX_SHOTS_DRAWN;
            let drawn: Vec<Value> = in_range.iter().take(MAX_SHOTS_DRAWN).map(|x| json!([x.start, x.end, x.clip])).collect();
            let highlight = id_list(o, "highlight", MAX_MARKS)?;
            let marks: Vec<&Value> = match &highlight {
                Some(ids) => pick(&all, ids)?,
                None => all.iter().filter(|f| f["kind"].as_str().is_some_and(|k| PLACED.contains(&k))).take(MAX_MARKS).collect(),
            };
            let marks: Vec<Value> = marks
                .iter()
                .filter(|f| f["start"].as_i64().is_some_and(|t| t >= from && t < to))
                .map(|f| json!({"id": f["id"], "severity": f["severity"], "start": f["start"], "title": f["title"]}))
                .collect();
            let sections: Vec<Value> = report["targets"]["sections"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|x| x["end"].as_i64().unwrap_or(0) > from && x["start"].as_i64().unwrap_or(0) < to)
                        .take(12)
                        .map(|x| json!({"name": x["name"], "start": x["start"], "end": x["end"]}))
                        .collect()
                })
                .unwrap_or_default();
            let marked: Vec<&str> = marks.iter().filter_map(|m| m["id"].as_str()).collect();
            let v = json!({"view": "timeline", "caption": caption, "sequence": name, "from": from, "to": to, "shots": drawn, "marks": marks, "sections": sections, "truncated": truncated});
            Ok(ToolOutput::ok(format!(
                "Shown to the person: a timeline of {} shots from {} to {} with {} finding{} marked{}. They can click it to jump the playhead there. Say what it means; do not read out the shots.",
                in_range.len().min(MAX_SHOTS_DRAWN),
                ticks_clock(&json!(from)),
                ticks_clock(&json!(to)),
                marked.len(),
                if marked.len() == 1 { "" } else { "s" },
                if marked.is_empty() { String::new() } else { format!(" ({})", marked.join(", ")) }
            ))
            .with_view(v))
        }
        "pace" => pace_view(s, &report, caption),
        "gauge" => gauge_view(s, o, &report, caption),
        "palette" => palette_view(s, o, caption),
        "pie" => pie_view(s, o, &report, caption),
        _ => {
            let id = s.state.active_sequence.ok_or("no sequence is open")?;
            let Some(filmcraft_project::ItemKind::Sequence(q)) = s.project.item(id).map(|i| &i.kind) else { return Err("no sequence is open".into()) };
            let stats = edith_analysis::analyze_sequence(&crate::analysis::sequence_input(s, q)).map_err(|e| e.to_string())?;
            if stats.shots.count == 0 {
                return Err("the open edit has no shots to count".into());
            }
            let mut lower = 0.0;
            let buckets: Vec<Value> = stats
                .shots
                .histogram
                .iter()
                .map(|b| {
                    let (label, up) = match b.below_seconds {
                        Some(up) => (format!("{}–{} s", lower as u32, up as u32), Some(up)),
                        None => (format!("{} s +", lower as u32), None),
                    };
                    let v = json!({"label": label, "count": b.count, "lowerSeconds": lower, "upperSeconds": up});
                    if let Some(u) = up {
                        lower = u;
                    }
                    v
                })
                .collect();
            let v = json!({
                "view": "histogram", "caption": caption, "sequence": name, "shots": stats.shots.count,
                "buckets": buckets, "meanSeconds": stats.shots.mean_seconds, "medianSeconds": stats.shots.median_seconds,
                "targetSeconds": report["targets"]["averageShotSeconds"],
            });
            Ok(ToolOutput::ok(format!(
                "Shown to the person: a histogram of the {} shots by length (mean {}, median {}){}. Say what the shape means for the pace.",
                stats.shots.count,
                len(stats.shots.mean_seconds),
                len(stats.shots.median_seconds),
                num(&report["targets"]["averageShotSeconds"]).map_or(String::new(), |t| format!(", with the plan's target of about {} marked", len(t)))
            ))
            .with_view(v))
        }
    }
}

/// The open sequence's measurements (no media needed), or why there are none.
fn open_stats(s: &Session) -> Result<(String, edith_analysis::SequenceStats), String> {
    let id = s.state.active_sequence.ok_or("no sequence is open")?;
    let item = s.project.item(id).ok_or("no sequence is open")?;
    let filmcraft_project::ItemKind::Sequence(q) = &item.kind else { return Err("no sequence is open".into()) };
    let stats = edith_analysis::analyze_sequence(&crate::analysis::sequence_input(s, q)).map_err(|e| e.to_string())?;
    Ok((item.name.clone(), stats))
}

/// The most bars a pace view holds: longer edits are averaged into this many stretches.
const MAX_PACE_BARS: usize = 120;

fn pace_view(s: &Session, report: &Value, caption: Option<String>) -> Result<ToolOutput, String> {
    let (name, stats) = open_stats(s)?;
    if stats.shots.count < 2 {
        return Err("the open edit has fewer than two shots, so there is no pace to draw".into());
    }
    let w = stats.cuts.window_seconds;
    let per_min: Vec<f64> = stats.cuts.series.iter().map(|c| f64::from(*c) * 60.0 / w).collect();
    let group = per_min.len().div_ceil(MAX_PACE_BARS).max(1);
    let bars: Vec<f64> = per_min.chunks(group).map(|c| c.iter().sum::<f64>() / c.len() as f64).collect();
    let target = num(&report["targets"]["averageShotSeconds"]).filter(|t| *t > 0.0).map(|t| 60.0 / t);
    let v = json!({"view": "pace", "caption": caption, "sequence": name, "barSeconds": w * group as f64, "cutsPerMinute": bars, "overall": stats.cuts.per_minute, "targetPerMinute": target});
    Ok(ToolOutput::ok(format!(
        "Shown to the person: cuts a minute along the edit, {} stretches of {:.0} s (overall {:.1} a minute){}. Say where it speeds up or slows down and whether that suits the moment.",
        bars.len(),
        w * group as f64,
        stats.cuts.per_minute,
        target.map_or(String::new(), |t| format!(", with the plan's rate of about {t:.1} marked"))
    ))
    .with_view(v))
}

fn gauge_view(s: &Session, o: &serde_json::Map<String, Value>, report: &Value, caption: Option<String>) -> Result<ToolOutput, String> {
    let metric = o.get("metric").and_then(Value::as_str).ok_or("a gauge needs `metric`: narrationPace, averageShot, loudness or cutsPerMinute")?;
    let (name, stats) = open_stats(s)?;
    let targets = &report["targets"];
    // (label, value, unit, target, band as fractions of the target or ± units, scale)
    let (label, value, unit, target, band, scale): (&str, f64, &str, Option<f64>, Option<(f64, f64)>, Option<(f64, f64)>) = match metric {
        "narrationPace" => {
            let v = stats.narration.words_per_minute.ok_or("there is no narration on the timeline to measure")?;
            let t = num(&targets["wordsPerMinute"]);
            ("Narration pace", v, "words a minute", t, t.map(|t| (0.8 * t, 1.15 * t)), None)
        }
        "averageShot" => {
            if stats.shots.count == 0 {
                return Err("the open edit has no shots".into());
            }
            let t = num(&targets["averageShotSeconds"]);
            ("Average shot", stats.shots.mean_seconds, "s", t, t.map(|t| (0.65 * t, 1.35 * t)), None)
        }
        "cutsPerMinute" => {
            let t = num(&targets["averageShotSeconds"]).filter(|t| *t > 0.0).map(|t| 60.0 / t);
            ("Cuts a minute", stats.cuts.per_minute, "a minute", t, t.map(|t| (0.65 * t, 1.35 * t)), None)
        }
        "loudness" => {
            let id = s.state.active_sequence.ok_or("no sequence is open")?;
            let mix = crate::analysis::measured_mix(s, id).ok_or("the mix has not been measured: call analysis_run with mix true first")?;
            let v = mix.integrated_lufs.ok_or("the mix is too quiet or too short to have a loudness")?;
            let t = num(&targets["loudnessLufs"]).unwrap_or(-14.0);
            ("Loudness", v, "LUFS", Some(t), Some((t - 3.0, t + 3.0)), Some((-40.0, 0.0)))
        }
        other => {
            return Err(format!(
                "there is no metric {:?}; use narrationPace, averageShot, loudness or cutsPerMinute",
                other.chars().take(30).collect::<String>()
            ));
        }
    };
    let (lo, hi) = scale.unwrap_or_else(|| (0.0, (value.abs().max(target.unwrap_or(0.0)) * 1.4).max(1.0)));
    let v = json!({"view": "gauge", "caption": caption, "sequence": name, "metric": metric, "label": label, "value": value, "unit": unit, "target": target, "low": band.map(|b| b.0), "high": band.map(|b| b.1), "scaleMin": lo, "scaleMax": hi});
    Ok(ToolOutput::ok(format!(
        "Shown to the person: a gauge of {} at {value:.1} {unit}{}. Say what it means for this video.",
        label.to_lowercase(),
        target.map_or(String::new(), |t| format!(" against a target of {t:.1}"))
    ))
    .with_view(v))
}

fn palette_view(s: &Session, o: &serde_json::Map<String, Value>, caption: Option<String>) -> Result<ToolOutput, String> {
    let id = o.get("item").and_then(Value::as_u64).ok_or("a palette needs `item`: the id of a media item that has been measured")?;
    let cache = Cache::of(s);
    let report = crate::analysis::current_report_for(s, &cache, "item", id)
        .ok_or_else(|| format!("item {id} has not been measured: call analysis_run with item {id} first"))?;
    let colours = report["video"]["summary"]["palette"].as_array().cloned().unwrap_or_default();
    if colours.is_empty() {
        return Err(format!("item {id} has no pictures, so no palette"));
    }
    let rows: Vec<Value> = colours.iter().take(5).map(|c| json!({"rgb": c["rgb"], "share": c["share"]})).collect();
    let name = report["target"]["name"].as_str().unwrap_or("").to_string();
    let v = json!({"view": "palette", "caption": caption, "name": name, "colors": rows});
    Ok(ToolOutput::ok(format!(
        "Shown to the person: the {} main colours of “{name}” as swatches in proportion. Say what the palette feels like and how it sits with the rest.",
        rows.len()
    ))
    .with_view(v))
}

fn pie_view(s: &Session, o: &serde_json::Map<String, Value>, report: &Value, caption: Option<String>) -> Result<ToolOutput, String> {
    let of = o.get("of").and_then(Value::as_str).ok_or("a pie needs `of`: screenTime, narration or silence")?;
    let (name, stats) = open_stats(s)?;
    let slices: Vec<(String, f64, String)> = match of {
        "screenTime" => {
            let id = s.state.active_sequence.ok_or("no sequence is open")?;
            let Some(filmcraft_project::ItemKind::Sequence(q)) = s.project.item(id).map(|i| &i.kind) else { return Err("no sequence is open".into()) };
            let shots = edith_analysis::shot_spans(&crate::analysis::sequence_input(s, q)).map_err(|e| e.to_string())?;
            if shots.is_empty() {
                return Err("the open edit has no shots".into());
            }
            let bands: [(&str, f64); 4] = [("under 2 s", 2.0), ("2–6 s", 6.0), ("6–15 s", 15.0), ("15 s and more", f64::INFINITY)];
            let (mut time, mut count) = ([0f64; 4], [0u32; 4]);
            for sh in &shots {
                let l = (sh.end - sh.start) as f64 / filmcraft_time::TICKS_PER_SECOND as f64;
                let i = bands.iter().position(|b| l < b.1).unwrap_or(3);
                time[i] += l;
                count[i] += 1;
            }
            let total: f64 = time.iter().sum();
            bands
                .iter()
                .enumerate()
                .filter(|(i, _)| count[*i] > 0)
                .map(|(i, b)| (b.0.to_string(), time[i] / total.max(f64::MIN_POSITIVE), format!("{} shot{}", count[i], if count[i] == 1 { "" } else { "s" })))
                .collect()
        }
        "narration" => {
            let share = stats.narration.share.clamp(0.0, 1.0);
            if stats.narration.words == 0 {
                return Err("there is no narration on the timeline".into());
            }
            vec![("narrated".to_string(), share, String::new()), ("not narrated".to_string(), 1.0 - share, String::new())]
        }
        _ if of == "silence" => {
            let id = s.state.active_sequence.ok_or("no sequence is open")?;
            let mix = crate::analysis::measured_mix(s, id).ok_or("the mix has not been measured: call analysis_run with mix true first")?;
            let q = f64::from(mix.silence_share).clamp(0.0, 1.0);
            vec![("sound".to_string(), 1.0 - q, String::new()), ("silence".to_string(), q, String::new())]
        }
        other => return Err(format!("there is no `of` {:?}; use screenTime, narration or silence", other.chars().take(30).collect::<String>())),
    };
    let _ = report;
    let rows: Vec<Value> = slices.iter().map(|(l, sh, d)| json!({"label": l, "share": sh, "detail": d})).collect();
    let v = json!({"view": "pie", "caption": caption, "sequence": name, "of": of, "slices": rows});
    let words: Vec<String> = slices.iter().map(|(l, sh, _)| format!("{l} {:.0}%", sh * 100.0)).collect();
    Ok(ToolOutput::ok(format!("Shown to the person: a pie of {of}: {}. Say what the split means.", words.join(", "))).with_view(v))
}
