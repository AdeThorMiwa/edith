//! `edit.checks` and the `edit_checks` tool (fork roadmap F16.3): when is a cut done?
//!
//! Three layers, never one score:
//! 1. **Must pass.** Things code can decide, and "done" is blocked while one fails: every moment has picture
//!    (no gaps), no flash frames, no cut inside a spoken word, no shot straddling the boundary between two of the
//!    plan's ideas, every shot's length within the video type's range, no clip with a licence that forbids the
//!    use, and no dead air the sound plan did not ask for.
//! 2. **Should.** Measured and reported, not blocking: the pace against the plan, footage used twice in a row or
//!    too soon again, stills with no camera move.
//! 3. **Taste.** Whether a picture illustrates the idea, whether something carries across the cut, whether it
//!    feels right: only the person decides, so the report says so and does not score it.
//!
//! A check has one of five states: `pass`, `fail`, `needsYou` (the person has to confirm something, such as a
//! clip whose origin is not recorded: flagged, never a reason to stop), `note` (a should-check worth a look) and
//! `na` (nothing to check: no narration, no plan placed on the timeline).
//!
//! `edit.checks {from?, to?, beat?}` looks at the whole edit, or only the stretch between `from` and `to`, or
//! one idea of the plan (`beat`: its id), so a single idea can be checked before the next is cut.
//!
//! Read-only. The numbers are the timeline's own; nothing here measures pixels or audio except the sound map's
//! dead-air finding, which only reads where clips sit.

use edith_ai::ToolOutput;
use filmcraft_project::{ItemId, ItemKind};
use filmcraft_time::TICKS_PER_SECOND;
use serde_json::{Value, json};

use crate::assistant_tools::{ToolDef, def, parse_time};
use crate::commands::{CommandSpec, bad, has_seq};
use crate::provenance::Status;
use crate::{EngineError, Result, Session};

/// A shot shorter than this is a flash frame, seconds (the coach uses the same).
const FLASH_SECONDS: f64 = 0.12;
/// Issues listed per check; the rest are counted.
const MAX_ISSUES: usize = 12;
/// A shot is "reused too soon" when the same footage comes back within this many seconds.
const REUSE_SECONDS: f64 = 30.0;
/// A still shorter than this does not need a camera move, seconds.
const STILL_MOVE_SECONDS: f64 = 2.0;

fn secs(t: i64) -> f64 {
    t as f64 / TICKS_PER_SECOND as f64
}

fn clock(t: i64) -> String {
    let s = secs(t.max(0));
    let (m, rest) = ((s / 60.0).floor() as u64, s % 60.0);
    format!("{m}:{rest:04.1}")
}

/// A picture shot of the edit: a media clip (not a graphic or an adjustment layer), in time.
struct Shot {
    item: ItemId,
    name: String,
    start: i64,
    end: i64,
    still: bool,
    moves: bool,
}

fn shots(s: &Session) -> Vec<Shot> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    let mut out: Vec<Shot> = Vec::new();
    for t in q.video_tracks.iter().filter(|t| t.enabled) {
        for it in t.items.iter().filter(|it| it.enabled && it.graphic.is_none() && it.duration.0 > 0) {
            let Some(pi) = s.project.item(it.item) else { continue };
            let (media, still) = match &pi.kind {
                ItemKind::Media(m) => (true, matches!(m.info.kind, filmcraft_media::MediaKind::Still)),
                ItemKind::Subclip { .. } | ItemKind::Sequence(_) => (true, false),
                _ => (false, false),
            };
            if !media {
                continue;
            }
            out.push(Shot {
                item: it.item,
                name: pi.name.clone(),
                start: it.start.0,
                end: it.end().0,
                still,
                moves: it.effects.iter().any(|e| e.params.iter().any(|(_, p)| !p.keyframes.is_empty())),
            });
        }
    }
    out.sort_by_key(|x| (x.start, x.end));
    out
}

/// The part of the timeline being looked at.
struct Scope {
    from: i64,
    to: i64,
    label: String,
}

impl Scope {
    fn holds(&self, start: i64, end: i64) -> bool {
        start < self.to && end > self.from
    }
}

fn issue(at: i64, text: String) -> Value {
    json!({"at": at, "seconds": secs(at), "text": text})
}

/// A check's record. `issues` is cut to what is listed; `total` says how many there were.
fn check(id: &str, layer: &str, status: &str, title: &str, summary: String, issues: Vec<Value>) -> Value {
    let total = issues.len();
    let shown: Vec<Value> = issues.into_iter().take(MAX_ISSUES).collect();
    json!({"id": id, "layer": layer, "status": status, "title": title, "summary": summary, "issues": shown, "total": total})
}

fn status_of(issues: &[Value], fail: &str) -> &'static str {
    if issues.is_empty() {
        "pass"
    } else if fail == "fail" {
        "fail"
    } else {
        "note"
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

// ---------------------------------------------------------------------------- the checks

fn coverage(s: &Session, shots: &[Shot], end: i64, scope: &Scope) -> Value {
    let (from, to) = (scope.from, scope.to.min(end.max(scope.from)));
    let frame = s.sequence_rate().frame_duration().0;
    let mut gaps: Vec<(i64, i64)> = Vec::new();
    let mut at = from;
    for sh in shots.iter().filter(|x| x.end > from && x.start < to) {
        if sh.start > at + frame / 2 {
            gaps.push((at, sh.start.min(to)));
        }
        at = at.max(sh.end);
    }
    if at + frame / 2 < to {
        gaps.push((at, to));
    }
    let issues: Vec<Value> = gaps.iter().map(|(a, b)| issue(*a, format!("no picture from {} to {} ({:.1} s)", clock(*a), clock(*b), secs(b - a)))).collect();
    let summary = if issues.is_empty() {
        format!("picture all the way, {} to {}", clock(from), clock(to))
    } else {
        format!("{} without picture", plural(issues.len(), "stretch", "stretches"))
    };
    check("coverage", "must", status_of(&issues, "fail"), "Every moment has picture", summary, issues)
}

fn flash(shots: &[Shot], scope: &Scope) -> Value {
    let flash = (FLASH_SECONDS * TICKS_PER_SECOND as f64) as i64;
    let issues: Vec<Value> = shots
        .iter()
        .filter(|x| scope.holds(x.start, x.end) && x.end - x.start < flash)
        .map(|x| issue(x.start, format!("“{}” is on screen for only {:.2} s at {}", x.name, secs(x.end - x.start), clock(x.start))))
        .collect();
    let summary = if issues.is_empty() {
        "no shot is too brief to see".to_string()
    } else {
        format!("{} too brief to see", plural(issues.len(), "shot is", "shots are"))
    };
    check("flashFrames", "must", status_of(&issues, "fail"), "No flash frames", summary, issues)
}

fn cuts_on_words(s: &Session, shots: &[Shot], scope: &Scope) -> Value {
    let spoken = crate::narration_timings::spoken_spans(s);
    if spoken.is_empty() {
        return check(
            "cutsOnWords",
            "must",
            "na",
            "No cut inside a word",
            "nothing on the timeline speaks, so there is no word to cut into".into(),
            Vec::new(),
        );
    }
    let frame = s.sequence_rate().frame_duration().0;
    let mut issues = Vec::new();
    for sh in shots.iter().filter(|x| scope.holds(x.start, x.end)) {
        for (what, t) in [("starts", sh.start), ("ends", sh.end)] {
            if t < scope.from || t > scope.to {
                continue;
            }
            if let Some((a, b, text)) = spoken.iter().find(|(a, b, _)| t > a.0 + frame && t + frame < b.0) {
                issues.push(issue(t, format!("“{}” {what} at {} inside the word “{text}” ({}–{})", sh.name, clock(t), clock(a.0), clock(b.0))));
            }
        }
    }
    let summary = if issues.is_empty() {
        "every cut is in a pause or at a word's edge".to_string()
    } else {
        format!("{} inside a word", plural(issues.len(), "cut is", "cuts are"))
    };
    check("cutsOnWords", "must", status_of(&issues, "fail"), "No cut inside a word", summary, issues)
}

fn idea_boundaries(s: &Session, shots: &[Shot], scope: &Scope) -> Value {
    let title = "No shot spans two ideas";
    let places: Vec<crate::soundplan::Place> = crate::soundplan::places(s).into_iter().filter(|p| p.beat.link.is_some()).collect();
    if places.len() < 2 {
        return check(
            "ideaBoundaries",
            "must",
            "na",
            title,
            "the plan's ideas are not placed on the timeline (planner.link), so there are no boundaries to keep".into(),
            Vec::new(),
        );
    }
    let tol = 2 * s.sequence_rate().frame_duration().0;
    let mut order: Vec<&crate::soundplan::Place> = places.iter().collect();
    order.sort_by_key(|p| p.start);
    let mut issues = Vec::new();
    for pair in order.windows(2) {
        let edge = pair[1].start;
        if edge < scope.from || edge > scope.to {
            continue;
        }
        for sh in shots.iter().filter(|x| x.start + tol < edge && edge < x.end - tol) {
            issues.push(issue(edge, format!("“{}” runs across the start of “{}” ({}); cut at the boundary", sh.name, pair[1].beat.name, clock(edge))));
        }
    }
    let summary = if issues.is_empty() {
        "every idea starts on a cut".to_string()
    } else {
        format!("{} across an idea boundary", plural(issues.len(), "shot runs", "shots run"))
    };
    check("ideaBoundaries", "must", status_of(&issues, "fail"), title, summary, issues)
}

/// The shortest and longest a shot should be, from the plan's Format Pack (`shot_seconds_min` and `shot_seconds_max`
/// in `[targets]` when it sets them; otherwise worked out from `average_shot_seconds`, the way the coach does).
fn shot_range(s: &Session) -> Option<(f64, f64, f64)> {
    let plan = crate::planner::get(&s.project)?;
    let avg = crate::planner::pack_target(s, &plan, "average_shot_seconds").filter(|a| *a > 0.0)?;
    let min = crate::planner::pack_target(s, &plan, "shot_seconds_min").filter(|m| *m > 0.0).unwrap_or_else(|| (0.15 * avg).max(0.5));
    let max = crate::planner::pack_target(s, &plan, "shot_seconds_max").filter(|m| *m > 0.0).unwrap_or_else(|| (3.0 * avg).max(12.0));
    Some((min, max, avg))
}

fn shot_lengths(s: &Session, shots: &[Shot], scope: &Scope) -> Value {
    let title = "Shot lengths in range";
    let Some((min, max, avg)) = shot_range(s) else {
        return check("shotLengths", "must", "na", title, "the plan's Format Pack sets no shot length, so there is no range to hold to".into(), Vec::new());
    };
    let flash = FLASH_SECONDS;
    let issues: Vec<Value> = shots
        .iter()
        .filter(|x| scope.holds(x.start, x.end))
        .filter_map(|x| {
            let l = secs(x.end - x.start);
            // a flash frame is its own check
            if l < flash {
                None
            } else if l < min {
                Some(issue(x.start, format!("“{}” is {l:.1} s at {}: shorter than the {min:.1} s the plan's shots usually run", x.name, clock(x.start))))
            } else if l > max {
                Some(issue(x.start, format!("“{}” holds {l:.1} s at {}: longer than the {max:.0} s the plan's shots usually run", x.name, clock(x.start))))
            } else {
                None
            }
        })
        .collect();
    let summary = if issues.is_empty() {
        format!("every shot is between {min:.1} and {max:.0} s (the plan averages {avg:.0} s)")
    } else {
        format!("{} outside {min:.1} to {max:.0} s", plural(issues.len(), "shot is", "shots are"))
    };
    check("shotLengths", "must", status_of(&issues, "fail"), title, summary, issues)
}

fn rights(s: &Session, shots: &[Shot], scope: &Scope) -> Value {
    let title = "Every clip's rights are known";
    let mut seen: Vec<ItemId> = Vec::new();
    let mut forbidden = Vec::new();
    let mut unknown = Vec::new();
    for sh in shots.iter().filter(|x| scope.holds(x.start, x.end)) {
        if seen.contains(&sh.item) {
            continue;
        }
        seen.push(sh.item);
        match crate::provenance::get(&s.project, sh.item) {
            None => unknown.push(issue(
                sh.start,
                format!("“{}” has no record of where it came from: the person confirms it is theirs or licensed (Credits and Rights)", sh.name),
            )),
            Some(r) if r.status() == Status::Risk => forbidden.push(issue(sh.start, format!("“{}”: {}", sh.name, r.summary()))),
            Some(_) => {}
        }
    }
    if !forbidden.is_empty() {
        let n = forbidden.len();
        forbidden.extend(unknown);
        return check(
            "rights",
            "must",
            "fail",
            title,
            format!("{} licensed in a way that does not allow this use, or on terms that are not known", plural(n, "clip is", "clips are")),
            forbidden,
        );
    }
    if !unknown.is_empty() {
        let n = unknown.len();
        return check(
            "rights",
            "must",
            "needsYou",
            title,
            format!("{} with no record of its origin: for the person to confirm (never a reason to stop cutting)", plural(n, "clip", "clips")),
            unknown,
        );
    }
    check("rights", "must", "pass", title, format!("{} used, each with a recorded licence that allows it", plural(seen.len(), "clip", "clips")), Vec::new())
}

fn dead_air(s: &Session, scope: &Scope) -> Value {
    let title = "No unplanned dead air";
    let Some(id) = s.state.active_sequence else { return check("deadAir", "must", "na", title, "no sequence is open".into(), Vec::new()) };
    let Ok(map) = crate::soundmap::build_map(s, id, false) else {
        return check("deadAir", "must", "na", title, "the sequence has nothing on its tracks yet".into(), Vec::new());
    };
    let issues: Vec<Value> = map["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["kind"] == "deadAir")
        .filter(|f| {
            let a = f["start"].as_i64().unwrap_or(0);
            let b = f["end"].as_i64().unwrap_or(a);
            scope.holds(a, b.max(a + 1))
        })
        .map(|f| issue(f["start"].as_i64().unwrap_or(0), f["title"].as_str().unwrap_or("dead air").to_string()))
        .collect();
    let summary = if issues.is_empty() {
        "no stretch of nothing the plan did not ask for".to_string()
    } else {
        format!("{} of dead air", plural(issues.len(), "stretch", "stretches"))
    };
    check("deadAir", "must", status_of(&issues, "fail"), title, summary, issues)
}

fn pace(s: &Session, shots: &[Shot], scope: &Scope) -> Value {
    let title = "Pace against the plan";
    let mine: Vec<&Shot> = shots.iter().filter(|x| scope.holds(x.start, x.end)).collect();
    if mine.is_empty() {
        return check("pace", "should", "na", title, "no shots to measure".into(), Vec::new());
    }
    let avg = mine.iter().map(|x| secs(x.end - x.start)).sum::<f64>() / mine.len() as f64;
    let Some((_, _, target)) = shot_range(s) else {
        return check("pace", "should", "pass", title, format!("shots average {avg:.1} s; the plan's Format Pack sets no target to compare with"), Vec::new());
    };
    let off = avg < 0.6 * target || avg > 1.6 * target;
    let summary = format!("shots average {avg:.1} s; the plan aims for about {target:.0} s{}", if off { ": a long way from it" } else { "" });
    check("pace", "should", if off { "note" } else { "pass" }, title, summary, Vec::new())
}

fn variety(shots: &[Shot], scope: &Scope) -> Value {
    let title = "Variety: footage not repeated too soon";
    let mut issues = Vec::new();
    let mine: Vec<&Shot> = shots.iter().filter(|x| scope.holds(x.start, x.end)).collect();
    for (i, a) in mine.iter().enumerate() {
        if let Some(b) = mine.iter().skip(i + 1).find(|b| b.item == a.item && secs(b.start - a.end) < REUSE_SECONDS && b.start >= a.end) {
            issues.push(issue(
                b.start,
                format!("“{}” comes back at {} after {:.0} s ({} the first time)", b.name, clock(b.start), secs(b.start - a.end).max(0.0), clock(a.start)),
            ));
        }
    }
    let summary = if issues.is_empty() {
        "no footage comes back within half a minute".to_string()
    } else {
        format!("{} reused soon", plural(issues.len(), "clip is", "clips are"))
    };
    check("variety", "should", status_of(&issues, "note"), title, summary, issues)
}

fn stills_move(shots: &[Shot], scope: &Scope) -> Value {
    let title = "Stills have a camera move";
    let issues: Vec<Value> = shots
        .iter()
        .filter(|x| x.still && !x.moves && scope.holds(x.start, x.end) && secs(x.end - x.start) >= STILL_MOVE_SECONDS)
        .map(|x| {
            issue(
                x.start,
                format!(
                    "“{}” sits still for {:.1} s at {}: a slow push or drift (motion.cameraMove) keeps it alive",
                    x.name,
                    secs(x.end - x.start),
                    clock(x.start)
                ),
            )
        })
        .collect();
    let summary =
        if issues.is_empty() { "every still that is held has a move".to_string() } else { format!("{} with no move", plural(issues.len(), "still", "stills")) };
    check("stillsMove", "should", status_of(&issues, "note"), title, summary, issues)
}

// ---------------------------------------------------------------------------- the command

fn scope_of(s: &Session, p: &Value, cmd: &str, end: i64) -> Result<Scope> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if let Some(b) = p.get("beat").filter(|v| !v.is_null()) {
        let id = b.as_u64().ok_or_else(|| bad(cmd, "`beat` must be a beat's id (planner_get lists them)"))?;
        let place = crate::soundplan::places(s).into_iter().find(|x| x.beat.id == id).ok_or_else(|| bad(cmd, format!("there is no beat {id} in the plan")))?;
        if place.beat.link.is_none() {
            return Err(bad(cmd, format!("beat “{}” is not placed on the timeline yet (planner.link), so its stretch is not known", place.beat.name)));
        }
        return Ok(Scope { from: place.start, to: place.start + place.length, label: format!("the idea “{}”", place.beat.name) });
    }
    let time = |key: &str| -> Result<Option<i64>> {
        match p.get(key).filter(|v| !v.is_null()) {
            None => Ok(None),
            Some(v) => parse_time(v, q, key).map(|t| Some(t.0)).map_err(|e| bad(cmd, e)),
        }
    };
    let (from, to) = (time("from")?.unwrap_or(0), time("to")?.unwrap_or(end));
    if to <= from {
        return Err(bad(cmd, "`to` must be after `from`"));
    }
    let label = if from == 0 && to == end { "the whole edit".to_string() } else { format!("{} to {}", clock(from), clock(to)) };
    Ok(Scope { from, to, label })
}

pub(crate) fn run_checks(s: &Session, p: &Value) -> Result<Value> {
    let cmd = "edit.checks";
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let all = shots(s);
    let spoken_end = crate::narration_timings::spoken_spans(s).last().map_or(0, |w| w.1.0);
    let end = all.iter().map(|x| x.end).max().unwrap_or(0).max(spoken_end);
    if end <= 0 {
        return Err(bad(cmd, "the edit is empty: there is nothing to check yet"));
    }
    let scope = scope_of(s, p, cmd, end)?;
    let checks = vec![
        coverage(s, &all, end, &scope),
        flash(&all, &scope),
        cuts_on_words(s, &all, &scope),
        idea_boundaries(s, &all, &scope),
        shot_lengths(s, &all, &scope),
        rights(s, &all, &scope),
        dead_air(s, &scope),
        pace(s, &all, &scope),
        variety(&all, &scope),
        stills_move(&all, &scope),
    ];
    let count = |layer: &str, status: &str| checks.iter().filter(|c| c["layer"] == layer && c["status"] == status).count();
    let failed = count("must", "fail");
    let needs = count("must", "needsYou");
    let passed = count("must", "pass");
    let na = count("must", "na");
    let _ = q;
    Ok(json!({
        "sequence": q_name(s), "scope": scope.label, "from": scope.from, "to": scope.to, "end": end,
        "mustPass": {"passed": passed, "failed": failed, "needsYou": needs, "notApplicable": na, "done": failed == 0},
        "checks": checks,
    }))
}

fn q_name(s: &Session) -> Value {
    s.state.active_sequence.and_then(|id| s.project.item(id)).map_or(Value::Null, |i| json!({"id": i.id.0, "name": i.name}))
}

fn checks_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    run_checks(s, p)
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "edit.checks",
        label: "Check the Edit",
        menu: &[],
        shortcut: None,
        params: r#"{"from":time?,"to":time?,"beat":id?}"#,
        enabled: has_seq,
        run: checks_cmd,
        journal: false,
    }]
}

// ---------------------------------------------------------------------------- the tool

/// The report as words for the model: the verdict first, then each check, then what is the person's alone.
pub(crate) fn render(r: &Value) -> String {
    let mark = |status: &str| match status {
        "pass" => "PASS",
        "fail" => "FAIL",
        "needsYou" => "NEEDS THE PERSON",
        "note" => "worth a look",
        _ => "n/a",
    };
    let m = &r["mustPass"];
    let mut out = String::new();
    let (failed, needs) = (m["failed"].as_u64().unwrap_or(0), m["needsYou"].as_u64().unwrap_or(0));
    out.push_str(&format!(
        "Checks for {} of “{}”: {}.\n",
        r["scope"].as_str().unwrap_or("the edit"),
        r["sequence"]["name"].as_str().unwrap_or(""),
        if failed > 0 {
            format!("NOT DONE: {failed} must-pass check{} fail{}", if failed == 1 { "" } else { "s" }, if failed == 1 { "s" } else { "" })
        } else if needs > 0 {
            format!("every must-pass check that can be decided passes; {needs} need{} the person to confirm", if needs == 1 { "s" } else { "" })
        } else {
            "every must-pass check passes".to_string()
        }
    ));
    for layer in ["must", "should"] {
        out.push_str(if layer == "must" { "\nMust pass (done is blocked while one fails):\n" } else { "\nShould (measured, not blocking):\n" });
        for c in r["checks"].as_array().into_iter().flatten().filter(|c| c["layer"] == layer) {
            out.push_str(&format!(
                "- [{}] {}: {}\n",
                mark(c["status"].as_str().unwrap_or("")),
                c["title"].as_str().unwrap_or(""),
                c["summary"].as_str().unwrap_or("")
            ));
            for i in c["issues"].as_array().into_iter().flatten() {
                out.push_str(&format!("    · {}\n", i["text"].as_str().unwrap_or("")));
            }
            let total = c["total"].as_u64().unwrap_or(0) as usize;
            let shown = c["issues"].as_array().map_or(0, Vec::len);
            if total > shown {
                out.push_str(&format!("    · …and {} more\n", total - shown));
            }
        }
    }
    out.push_str("\nTaste (only the person decides; give your reasons, do not score it): does each picture illustrate or contrast with its idea; does something carry across each cut; does it feel right.\n");
    if failed > 0 {
        out.push_str("Do not say the cut is done while a must-pass check fails: fix it (undo_steps, or edit), then check again.\n");
    }
    out
}

fn edit_checks(s: &Session, input: &Value) -> ToolOutput {
    let p = if input.is_object() { input.clone() } else { json!({}) };
    match run_checks(s, &p) {
        Ok(r) => ToolOutput::ok(render(&r)),
        Err(e) => ToolOutput::error(e.to_string()),
    }
}

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![def(
        "edit_checks",
        "Check the cut against what \"done\" means. Must pass (blocks done): every moment has picture, no flash frames, no cut inside a spoken word, no shot spanning two ideas, shot lengths in the Format Pack's range, every clip's rights known and allowing the use, no unplanned dead air. Should (measured only): pace against the plan, footage not reused too soon, stills with a camera move. Taste is the person's. Run it after each idea you cut and before you say you are done; narrow it with `beat` (one idea of the plan) or `from` and `to`. Never claim done while a must-pass check fails. A clip with no recorded origin is for the person to confirm, not a reason to stop.",
        json!({
            "beat": {"type": "integer", "description": "Check only this idea of the plan (its beat id, placed on the timeline)."},
            "from": {"type": "string", "description": "Check only from this timeline time (seconds, \"1:25\", or a timecode)."},
            "to": {"type": "string", "description": "Check only up to this timeline time."},
        }),
        edit_checks,
    )]
}
