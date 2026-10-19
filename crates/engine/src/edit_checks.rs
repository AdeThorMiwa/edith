//! `edit.checks` and the `edit_checks` tool (fork roadmap F16.3): when is a cut done?
//!
//! Three layers, never one score:
//! 1. **Must pass.** Things code can decide, and "done" is blocked while one fails: every moment has picture
//!    (no gaps), no flash frames, no cut inside a spoken word, no shot straddling the boundary between two of the
//!    plan's ideas, every shot's length within the video type's range, no clip with a licence that forbids the
//!    use, and no dead air the sound plan did not ask for.
//! 2. **Should.** Measured and reported, not blocking: the pace against the plan, footage used twice in a row or
//!    too soon again, stills with no camera move, sound under the ideas that want it, music and ambience lowered
//!    under the voice, (when asked with `measure`) the loudness of the whole mix near the target, and on-screen text:
//!    up long enough to read, a title near the start, a chapter title where each section after the first begins, and
//!    transitions: few (within the pack's share of the cuts), none longer than half a shot, dissolves all one length.
//! 3. **Taste.** Whether a picture illustrates the idea, whether something carries across the cut, whether it
//!    feels right: only the person decides, so the report says so and does not score it.
//!
//! A check has one of five states: `pass`, `fail`, `needsYou` (the person has to confirm something, such as a
//! clip whose origin is not recorded: flagged, never a reason to stop), `note` (a should-check worth a look) and
//! `na` (nothing to check: no narration, no plan placed on the timeline).
//!
//! `edit.checks {from?, to?, beat?, measure?, targetLufs?}` looks at the whole edit, or only the stretch between `from`
//! and `to`, or one idea of the plan (`beat`: its id), so a single idea can be checked before the next is cut. The
//! mix's loudness is measured only when `measure` is true (it plays the mix through the mixer, which takes a moment);
//! the aim is `targetLufs` (default -16, a YouTube delivery level) within 2 LU.
//!
//! Read-only. The numbers are the timeline's own; nothing here measures pixels, and audio only when `measure` is
//! asked for. Every clip with a licence on record counts for the rights check, pictures and sound alike.

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

/// A clip of the music, ambience or effects lane, in time.
struct Sound {
    item: ItemId,
    name: String,
    start: i64,
    end: i64,
    lane: crate::sound_list::Lane,
    /// Its level is animated (the ducking writes keyframes there).
    ducked: bool,
}

fn sounds(s: &Session) -> Vec<Sound> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    let mut out: Vec<Sound> = Vec::new();
    for t in q.audio_tracks.iter().filter(|t| t.enabled) {
        for it in t.items.iter().filter(|it| it.enabled && it.duration.0 > 0) {
            if s.project.narrations.contains_key(&it.item) {
                continue;
            }
            let Some(lane) = crate::sound_list::lane_of(s, it, &t.name) else { continue };
            let ducked = it.effect("volume").and_then(|e| e.param("level")).is_some_and(|p| p.is_animated());
            out.push(Sound { item: it.item, name: it.name.clone(), start: it.start.0, end: it.end().0, lane, ducked });
        }
    }
    out.sort_by_key(|x| (x.start, x.end));
    out
}

/// A title or other on-screen text: a graphic clip with words in it.
struct Text {
    words: usize,
    shown: String,
    start: i64,
    end: i64,
}

fn texts(s: &Session) -> Vec<Text> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    let mut out: Vec<Text> = Vec::new();
    for t in q.video_tracks.iter().filter(|t| t.enabled) {
        for it in t.items.iter().filter(|it| it.enabled && it.duration.0 > 0) {
            let lines: Vec<String> = it
                .effects
                .iter()
                .filter(|e| filmcraft_project::graphic::is_layer(e))
                .filter_map(|e| {
                    e.param("text").and_then(|p| if let filmcraft_project::ParamValue::Text(x) = &p.value { Some(x.trim().to_string()) } else { None })
                })
                .filter(|x| !x.is_empty())
                .collect();
            if lines.is_empty() {
                continue;
            }
            let shown = lines.join(" · ");
            out.push(Text { words: lines.iter().map(|l| crate::text_list::word_count(l)).sum(), shown, start: it.start.0, end: it.end().0 });
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

fn rights(s: &Session, shots: &[Shot], sounds: &[Sound], scope: &Scope) -> Value {
    let title = "Every clip's rights are known";
    let mut seen: Vec<ItemId> = Vec::new();
    let mut forbidden = Vec::new();
    let mut unknown = Vec::new();
    // pictures and sound alike: music from a stock site has a licence too
    let mut used: Vec<(ItemId, &str, i64, i64)> = shots
        .iter()
        .map(|x| (x.item, x.name.as_str(), x.start, x.end))
        .chain(sounds.iter().map(|x| (x.item, x.name.as_str(), x.start, x.end)))
        .filter(|(_, _, a, b)| scope.holds(*a, *b))
        .collect();
    used.sort_by_key(|x| x.2);
    for (item, name, start, _) in used {
        if seen.contains(&item) {
            continue;
        }
        seen.push(item);
        match crate::provenance::get(&s.project, item) {
            None => unknown
                .push(issue(start, format!("“{name}” has no record of where it came from: the person confirms it is theirs or licensed (Credits and Rights)"))),
            Some(r) if r.status() == Status::Risk => forbidden.push(issue(start, format!("“{name}”: {}", r.summary()))),
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

/// The share of `[a, b)` that sound of `lane` covers (the sounds of one lane do not overlap).
fn covered(sounds: &[Sound], lane: crate::sound_list::Lane, a: i64, b: i64) -> f64 {
    if b <= a {
        return 1.0;
    }
    let got: i64 = sounds.iter().filter(|x| x.lane == lane).map(|x| (x.end.min(b) - x.start.max(a)).max(0)).sum();
    got as f64 / (b - a) as f64
}

fn sound_under_ideas(s: &Session, sounds: &[Sound], scope: &Scope) -> Value {
    use crate::sound_list::Lane;
    let title = "Sound under the ideas that want it";
    let wanting: Vec<crate::soundplan::Place> = crate::soundplan::places(s)
        .into_iter()
        .filter(|p| p.beat.link.is_some() && scope.holds(p.start, p.start + p.length))
        .filter(|p| !p.beat.sound.ambience.trim().is_empty() || p.beat.sound.music.is_some())
        .collect();
    let voice = !crate::narration_timings::spoken_spans(s).is_empty();
    if wanting.is_empty() {
        let here = sounds.iter().filter(|x| scope.holds(x.start, x.end)).count();
        return if here == 0 && voice {
            check(
                "soundUnderIdeas",
                "should",
                "note",
                title,
                "nothing sounds but the voice: no music, ambience or effects in the lanes (if silence is meant, say so; otherwise a bed or a score)".into(),
                Vec::new(),
            )
        } else if here == 0 {
            check("soundUnderIdeas", "should", "na", title, "no sound in the lanes and nothing speaks".into(), Vec::new())
        } else {
            check(
                "soundUnderIdeas",
                "should",
                "pass",
                title,
                format!("{} in the lanes; the plan names no bed or music for an idea that has none under it", plural(here, "sound", "sounds")),
                Vec::new(),
            )
        };
    }
    let mut issues = Vec::new();
    for p in &wanting {
        let (a, b) = (p.start, p.start + p.length);
        if !p.beat.sound.ambience.trim().is_empty() && covered(sounds, Lane::Ambience, a, b) < 0.8 {
            issues.push(issue(
                a,
                format!(
                    "“{}” plans a bed ({}) but little or nothing sounds under it from {} to {}",
                    p.beat.name,
                    p.beat.sound.ambience.trim(),
                    clock(a),
                    clock(b)
                ),
            ));
        }
        if let Some(m) = &p.beat.sound.music {
            let (ma, mb) = (a + m.enters.0, m.leaves.map_or(b, |l| a + l.0));
            if covered(sounds, Lane::Music, ma, mb.max(ma)) < 0.8 {
                issues
                    .push(issue(ma, format!("“{}” plans music ({}) but little or none plays from {} to {}", p.beat.name, m.mood.trim(), clock(ma), clock(mb))));
            }
        }
    }
    let summary = if issues.is_empty() {
        format!("every one of the {} that plan a bed or music has it", plural(wanting.len(), "idea", "ideas"))
    } else {
        format!("{} planned sound that is missing", plural(issues.len(), "piece of", "pieces of"))
    };
    check("soundUnderIdeas", "should", status_of(&issues, "note"), title, summary, issues)
}

fn music_ducked(s: &Session, sounds: &[Sound], scope: &Scope) -> Value {
    use crate::sound_list::Lane;
    let title = "Music and ambience lowered under the voice";
    let spoken = crate::narration_timings::spoken_spans(s);
    let bed: Vec<&Sound> = sounds.iter().filter(|x| matches!(x.lane, Lane::Music | Lane::Ambience) && scope.holds(x.start, x.end)).collect();
    if spoken.is_empty() || bed.is_empty() {
        let why = if spoken.is_empty() { "nothing speaks" } else { "there is no music or ambience in the lanes" };
        return check("musicDucked", "should", "na", title, format!("{why}, so there is nothing to lower"), Vec::new());
    }
    let mut issues = Vec::new();
    for b in bed {
        let under: Vec<&(filmcraft_time::Tick, filmcraft_time::Tick, String)> = spoken.iter().filter(|(a, z, _)| a.0 < b.end && z.0 > b.start).collect();
        if let (false, Some(first)) = (b.ducked, under.first()) {
            let at = first.0.0.max(b.start);
            issues.push(issue(at, format!("“{}” plays under the voice from {} but its level is not lowered there; sound.autoMix ducks it", b.name, clock(at))));
        }
    }
    let summary = if issues.is_empty() {
        "every music or ambience clip that plays under the voice dips for it".to_string()
    } else {
        format!("{} under the voice at full level", plural(issues.len(), "clip plays", "clips play"))
    };
    check("musicDucked", "should", status_of(&issues, "note"), title, summary, issues)
}

fn mix_loudness(s: &Session, p: &Value, scope: &Scope) -> Result<Value> {
    let title = "Mix loudness near the target";
    let target = match p.get("targetLufs").filter(|v| !v.is_null()) {
        None => crate::sound_mix::DIALOGUE_LUFS,
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite() && (-60.0..=0.0).contains(x))
            .ok_or_else(|| bad("edit.checks", "`targetLufs` must be a loudness from -60 to 0"))?,
    };
    if p.get("measure").and_then(Value::as_bool) != Some(true) {
        return Ok(check(
            "mixLoudness",
            "should",
            "na",
            title,
            "not measured; ask with measure: true (it plays the whole mix through the mixer, which takes a moment)".into(),
            Vec::new(),
        ));
    }
    let (from, to) = (filmcraft_time::Tick(scope.from), filmcraft_time::Tick(scope.to));
    Ok(match crate::sound_mix::mix_lufs(s, from, to) {
        None => check("mixLoudness", "should", "na", title, "the mix is silent here, so it has no loudness".into(), Vec::new()),
        Some(l) if (l - target).abs() <= 2.0 => {
            check("mixLoudness", "should", "pass", title, format!("the mix measures {l:.1} LUFS; the aim is {target:.0} within 2"), Vec::new())
        }
        Some(l) => check(
            "mixLoudness",
            "should",
            "note",
            title,
            format!(
                "the mix measures {l:.1} LUFS; the aim is {target:.0} within 2 ({})",
                if l < target {
                    "too quiet: sound.autoMix, or the sequence's master level"
                } else {
                    "too loud: sound.autoMix, or lower the sequence's master level"
                }
            ),
            Vec::new(),
        ),
    })
}

fn text_readable(texts: &[Text], scope: &Scope) -> Value {
    let title = "On-screen text is up long enough to read";
    let mine: Vec<&Text> = texts.iter().filter(|x| scope.holds(x.start, x.end)).collect();
    if mine.is_empty() {
        return check("textReadable", "should", "na", title, "no titles or on-screen text".into(), Vec::new());
    }
    let issues: Vec<Value> = mine
        .iter()
        .filter_map(|x| {
            let (up, need) = (secs(x.end - x.start), crate::text_list::reading_seconds(x.words));
            (up + 0.05 < need).then(|| {
                issue(
                    x.start,
                    format!(
                        "“{}” is up for {up:.1} s at {}; {} word{} need about {need:.1} s",
                        x.shown,
                        clock(x.start),
                        x.words,
                        if x.words == 1 { "" } else { "s" }
                    ),
                )
            })
        })
        .collect();
    let summary = if issues.is_empty() {
        format!("all {} up for a second and a quarter of a second a word or more", plural(mine.len(), "piece of text is", "pieces of text are"))
    } else {
        format!("{} up too briefly to read", plural(issues.len(), "piece of text is", "pieces of text are"))
    };
    check("textReadable", "should", status_of(&issues, "note"), title, summary, issues)
}

/// A title near the start counts as an opening title.
const OPENING_SECONDS: f64 = 15.0;

fn opening_title(texts: &[Text], scope: &Scope) -> Value {
    let title = "An opening title";
    if scope.from > 0 {
        return check("openingTitle", "should", "na", title, "only the whole edit has an opening".into(), Vec::new());
    }
    let limit = (OPENING_SECONDS * TICKS_PER_SECOND as f64) as i64;
    match texts.iter().find(|x| x.start < limit) {
        Some(x) => check("openingTitle", "should", "pass", title, format!("“{}” opens it at {}", x.shown, clock(x.start)), Vec::new()),
        None => check(
            "openingTitle",
            "should",
            "note",
            title,
            format!("no title or text in the first {OPENING_SECONDS:.0} seconds (right for a cold open; otherwise the film's name belongs here)"),
            Vec::new(),
        ),
    }
}

fn chapter_titles(s: &Session, texts: &[Text], scope: &Scope) -> Value {
    let title = "A chapter title where each section begins";
    // each section's start: its first idea that is placed on the timeline
    let mut starts: Vec<(String, i64)> = Vec::new();
    for p in crate::soundplan::places(s).into_iter().filter(|p| p.beat.link.is_some()) {
        match starts.iter_mut().find(|(n, _)| *n == p.section) {
            Some(x) => x.1 = x.1.min(p.start),
            None => starts.push((p.section.clone(), p.start)),
        }
    }
    starts.sort_by_key(|x| x.1);
    if starts.len() < 2 {
        return check(
            "chapterTitles",
            "should",
            "na",
            title,
            "the plan has fewer than two sections placed on the timeline, so there are no chapters".into(),
            Vec::new(),
        );
    }
    let (before, after) = ((1.0 * TICKS_PER_SECOND as f64) as i64, (3.0 * TICKS_PER_SECOND as f64) as i64);
    // the first section opens the film: its title is the film's
    let issues: Vec<Value> = starts
        .iter()
        .skip(1)
        .filter(|(_, at)| scope.holds(*at, *at + 1))
        .filter(|(_, at)| !texts.iter().any(|x| x.start >= at - before && x.start <= at + after))
        .map(|(name, at)| issue(*at, format!("the section “{name}” begins at {} with no title (a chapter card, if this film has them)", clock(*at))))
        .collect();
    let summary = if issues.is_empty() {
        format!("each of the {} after the first has text where it begins", plural(starts.len() - 1, "section", "sections"))
    } else {
        format!("{} with no title where it begins", plural(issues.len(), "section", "sections"))
    };
    check("chapterTitles", "should", status_of(&issues, "note"), title, summary, issues)
}

/// A cut may carry a transition for this many seconds of picture at most, on average: one a minute and a half is
/// plenty; more than eight a minute is a slideshow.
const MAX_PER_MINUTE: f64 = 8.0;

fn transition_restraint(s: &Session, scope: &Scope) -> Value {
    let title = "Transitions are rare";
    let pics = crate::polish::pictures(s);
    let all: Vec<crate::polish::Cut> = crate::polish::cuts(s, &pics).into_iter().filter(|c| scope.holds(c.into.start.0, c.into.start.0 + 1)).collect();
    if all.is_empty() {
        return check("transitionRestraint", "should", "na", title, "no cuts between shots here".into(), Vec::new());
    }
    let used = all.iter().filter(|c| c.has_transition).count();
    let pol = crate::polish::policy(s);
    // a small edit may have one: 20% of five cuts
    let allowed = ((all.len() as f64 * pol.share / 100.0).ceil() as usize).max(1);
    let minutes = secs(scope.to.min(all.iter().map(|c| c.into.end.0).max().unwrap_or(0)) - scope.from) / 60.0;
    let rate = if minutes > 0.0 { used as f64 / minutes } else { 0.0 };
    let mut issues = Vec::new();
    if used > allowed {
        issues.push(issue(
            scope.from,
            format!(
                "{used} of the {} cuts carry a transition ({:.0}%): the plan's pack allows about {:.0}%; the rest are better hard",
                all.len(),
                used as f64 * 100.0 / all.len() as f64,
                pol.share
            ),
        ));
    }
    if rate > MAX_PER_MINUTE && minutes >= 0.5 {
        issues.push(issue(scope.from, format!("{rate:.1} transitions a minute: more than {MAX_PER_MINUTE:.0} a minute reads as a slideshow")));
    }
    let summary = if issues.is_empty() {
        format!("{} of {} carry a transition (up to {} may, by the pack's {:.0}%)", plural(used, "cut", "cuts"), all.len(), allowed, pol.share)
    } else {
        format!("too many transitions: {used} of {} cuts", all.len())
    };
    check("transitionRestraint", "should", status_of(&issues, "note"), title, summary, issues)
}

fn transition_fit(s: &Session, scope: &Scope) -> Value {
    let title = "Transitions fit their shots and match each other";
    let pics = crate::polish::pictures(s);
    let length_of = |id: filmcraft_project::ClipId| pics.iter().flatten().find(|p| p.id == id).map(|p| p.end.0 - p.start.0);
    let frame = s.sequence_rate().frame_duration().0;
    let trs: Vec<crate::polish::Tr> = crate::polish::transitions(s).into_iter().filter(|t| scope.holds(t.start.0, t.start.0 + t.duration.0)).collect();
    if trs.is_empty() {
        return check("transitionFit", "should", "na", title, "no transitions here".into(), Vec::new());
    }
    let mut issues = Vec::new();
    for t in &trs {
        for (side, id) in [("before", t.from), ("after", t.to)] {
            if let Some(len) = id.and_then(length_of)
                && t.duration.0 > len / 2 + frame
            {
                issues.push(issue(
                    t.start.0,
                    format!("the {:.1} s transition at {} is longer than half of the {:.1} s shot {side} it", secs(t.duration.0), clock(t.start.0), secs(len)),
                ));
            }
        }
    }
    // the dissolves between two shots are all one length
    let dissolves: Vec<&crate::polish::Tr> = trs.iter().filter(|t| t.effect == "cross_dissolve" && t.from.is_some() && t.to.is_some()).collect();
    if let Some(first) = dissolves.first() {
        for d in dissolves.iter().skip(1).filter(|d| (d.duration.0 - first.duration.0).abs() > frame) {
            issues.push(issue(
                d.start.0,
                format!(
                    "the dissolve at {} runs {:.1} s; the first runs {:.1} s: pick one length",
                    clock(d.start.0),
                    secs(d.duration.0),
                    secs(first.duration.0)
                ),
            ));
        }
    }
    let summary = if issues.is_empty() {
        format!("{} fit their shots, and the dissolves are all one length", plural(trs.len(), "transition", "transitions"))
    } else {
        format!("{} that do not fit or match", plural(issues.len(), "transition", "transitions"))
    };
    check("transitionFit", "should", status_of(&issues, "note"), title, summary, issues)
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
    let lanes = sounds(s);
    let words_on_screen = texts(s);
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
        rights(s, &all, &lanes, &scope),
        dead_air(s, &scope),
        pace(s, &all, &scope),
        variety(&all, &scope),
        stills_move(&all, &scope),
        sound_under_ideas(s, &lanes, &scope),
        music_ducked(s, &lanes, &scope),
        mix_loudness(s, p, &scope)?,
        text_readable(&words_on_screen, &scope),
        opening_title(&words_on_screen, &scope),
        chapter_titles(s, &words_on_screen, &scope),
        transition_restraint(s, &scope),
        transition_fit(s, &scope),
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
        params: r#"{"from":time?,"to":time?,"beat":id?,"measure":bool?,"targetLufs":lufs?}"#,
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
        "Check the cut against what \"done\" means. Must pass (blocks done): every moment has picture, no flash frames, no cut inside a spoken word, no shot spanning two ideas, shot lengths in the Format Pack's range, every clip's rights known and allowing the use, no unplanned dead air. Should (measured only): pace against the plan, footage not reused too soon, stills with a camera move, sound under the ideas that want it, music and ambience lowered under the voice, and with `measure: true` the loudness of the whole mix near -16 LUFS (it plays the mix through the mixer, so ask once the sound is placed and mixed), titles up long enough to read, an opening title, a chapter title where each section after the first begins, few transitions (within the pack's share of the cuts), none longer than half a shot, dissolves all one length. Taste is the person's. Run it after each idea you cut and before you say you are done; narrow it with `beat` (one idea of the plan) or `from` and `to`. Never claim done while a must-pass check fails. A clip with no recorded origin is for the person to confirm, not a reason to stop.",
        json!({
            "beat": {"type": "integer", "description": "Check only this idea of the plan (its beat id, placed on the timeline)."},
            "from": {"type": "string", "description": "Check only from this timeline time (seconds, \"1:25\", or a timecode)."},
            "to": {"type": "string", "description": "Check only up to this timeline time."},
            "measure": {"type": "boolean", "description": "Also measure the loudness of the mix (takes a moment). Default false."},
            "targetLufs": {"type": "number", "description": "The loudness the mix should be near, -60 to 0 (default -16)."},
        }),
        edit_checks,
    )]
}
