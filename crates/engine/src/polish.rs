//! `edit.polish` (fork roadmap F31): the last light pass on a picture edit, with restraint.
//!
//! A cut is the default. Most good edits are almost all hard cuts, and the transitions they do have mean something:
//! time passes, a section ends, a film begins and ends. So this does not decorate; it applies the Format Pack's
//! policy to the plan's structure, and says in plain words what it did and what it left alone:
//!
//! - **Inside an idea**, hard cuts, always.
//! - **Between two ideas of low intensity** (both at or under `dissolve_max_intensity`, default 5), a **dissolve**
//!   of `dissolve_seconds` (default 0.8).
//! - **Where one section ends and the next begins**, a **dip to black** of `dip_seconds` (default 1), unless the
//!   pack sets `dip_at_section_ends` to 0.
//! - **The start and the end of the film** fade in from black and out to black (`fades`, default on), 1 s.
//! - **Stills** held for two seconds or more with no camera move get a slow push or drift, turn and turn about.
//! - **At most `transition_share_percent` of the cuts** (default 20) carry a transition; when more would qualify the
//!   dips come first, then the quietest dissolves, and the rest stay hard (and are listed).
//!
//! It never replaces a transition that is already there (so asked again it changes nothing), skips a cut whose shots
//! have no spare picture on either side for the transition to use or are too short for it, and shortens a transition
//! to half the shorter shot when that is still long enough to see. Where the plan's ideas are not placed on the
//! timeline, only the fades and the stills are done: there is no way to know where an idea changes.
//!
//! One undo step. `dryRun: true` says what it would do and changes nothing.
//!
//! `edit.polish {from?, to?, fades?, moves?, dryRun?}`; the numbers come from the plan's pack (`[targets]`:
//! `dissolve_seconds`, `dip_seconds`, `dip_at_section_ends`, `dissolve_max_intensity`, `transition_share_percent`).

use filmcraft_project::{ClipId, ItemKind, Sequence, TrackItem};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use crate::assistant_tools::parse_time;
use crate::commands::{CommandSpec, bad, bool_p, has_seq};
use crate::{EngineError, Result, Session};

const CMD: &str = "edit.polish";
const DISSOLVE_SECONDS: f64 = 0.8;
const DIP_SECONDS: f64 = 1.0;
const FADE_SECONDS: f64 = 1.0;
const SHARE_PERCENT: f64 = 20.0;
/// An idea is "low" in intensity at or under this (0 to 10).
const LOW_INTENSITY: f64 = 5.0;
/// A transition shorter than this is not worth making (seconds).
const MIN_SECONDS: f64 = 0.25;
/// A still is given a move when it is held this long (seconds).
const STILL_MOVE_SECONDS: f64 = 2.0;
/// The picture left to use on either side of a still is without limit.
const PLENTY: Tick = Tick(1_000_000 * TICKS_PER_SECOND);
const MOVES: [&str; 4] = ["pushIn", "driftLeft", "pullOut", "driftRight"];

fn secs(t: Tick) -> f64 {
    t.0 as f64 / TICKS_PER_SECOND as f64
}

fn clock(t: Tick) -> String {
    let s = secs(t.max(Tick::ZERO));
    format!("{}:{:04.1}", (s / 60.0).floor() as u64, s % 60.0)
}

/// What the plan's pack says about transitions.
pub(crate) struct Policy {
    pub dissolve: f64,
    pub dip: f64,
    pub dip_at_section_ends: bool,
    pub low: f64,
    pub share: f64,
}

pub(crate) fn policy(s: &Session) -> Policy {
    let plan = crate::planner::get(&s.project);
    let get = |key: &str| plan.as_ref().and_then(|p| crate::planner::pack_target(s, p, key));
    let seconds = |key: &str, default: f64| get(key).filter(|v| (MIN_SECONDS..=5.0).contains(v)).unwrap_or(default);
    Policy {
        dissolve: seconds("dissolve_seconds", DISSOLVE_SECONDS),
        dip: seconds("dip_seconds", DIP_SECONDS),
        dip_at_section_ends: get("dip_at_section_ends").is_none_or(|v| v != 0.0),
        low: get("dissolve_max_intensity").filter(|v| (0.0..=10.0).contains(v)).unwrap_or(LOW_INTENSITY),
        share: get("transition_share_percent").filter(|v| (0.0..=100.0).contains(v)).unwrap_or(SHARE_PERCENT),
    }
}

/// A picture clip of the edit, as polish sees it.
#[derive(Clone)]
pub(crate) struct Pic {
    pub id: ClipId,
    pub name: String,
    pub start: Tick,
    pub end: Tick,
    pub still: bool,
    pub moves: bool,
    /// Picture to spare before the first frame used and after the last, in the clip's own time.
    pub before: Tick,
    pub after: Tick,
    pub speed: f64,
}

/// The picture left on either side of what a clip shows.
fn handles(s: &Session, it: &TrackItem) -> (Tick, Tick) {
    let Some(pi) = s.project.item(it.item) else { return (Tick::ZERO, Tick::ZERO) };
    if matches!(&pi.kind, ItemKind::Media(m) if matches!(m.info.kind, filmcraft_media::MediaKind::Still)) {
        return (PLENTY, PLENTY);
    }
    let (lo, hi) = match &pi.kind {
        ItemKind::Subclip { range, .. } => (range.start, range.end()),
        _ => (Tick::ZERO, crate::commands::full_duration(s, pi)),
    };
    let used = Tick((it.duration.0 as f64 * it.speed.abs()).round() as i64);
    ((it.source_in - lo).max(Tick::ZERO), (hi - (it.source_in + used)).max(Tick::ZERO))
}

/// The picture clips of the edit, track by track, in time. Titles and other graphics are not shots.
pub(crate) fn pictures(s: &Session) -> Vec<Vec<Pic>> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    q.video_tracks
        .iter()
        .filter(|t| t.enabled && !t.name.trim().eq_ignore_ascii_case("titles"))
        .map(|t| {
            t.items
                .iter()
                .filter(|it| it.enabled && it.graphic.is_none() && it.duration.0 > 0)
                .filter(|it| s.project.item(it.item).is_some_and(|pi| matches!(pi.kind, ItemKind::Media(_) | ItemKind::Subclip { .. } | ItemKind::Sequence(_))))
                .map(|it| {
                    let (before, after) = handles(s, it);
                    let still = s
                        .project
                        .item(it.item)
                        .is_some_and(|pi| matches!(&pi.kind, ItemKind::Media(m) if matches!(m.info.kind, filmcraft_media::MediaKind::Still)));
                    Pic {
                        id: it.id,
                        name: it.name.clone(),
                        start: it.start,
                        end: it.end(),
                        still,
                        moves: it.effects.iter().any(|e| e.params.iter().any(|(_, p)| !p.keyframes.is_empty())),
                        before,
                        after,
                        speed: it.speed.abs().max(0.01),
                    }
                })
                .collect()
        })
        .filter(|v: &Vec<Pic>| !v.is_empty())
        .collect()
}

/// A cut between two pictures that touch.
#[derive(Clone)]
pub(crate) struct Cut {
    pub out: Pic,
    pub into: Pic,
    /// A transition is already on it.
    pub has_transition: bool,
}

pub(crate) fn cuts(s: &Session, pics: &[Vec<Pic>]) -> Vec<Cut> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    let tol = s.sequence_rate().frame_duration().0 / 2;
    let video: Vec<&filmcraft_project::Track> = q.video_tracks.iter().filter(|t| t.enabled && !t.name.trim().eq_ignore_ascii_case("titles")).collect();
    let mut out = Vec::new();
    for row in pics {
        let transitions = row.first().and_then(|p| video.iter().find(|t| t.items.iter().any(|i| i.id == p.id))).map(|t| &t.transitions);
        for w in row.windows(2) {
            if (w[0].end - w[1].start).0.abs() <= tol {
                let has = transitions.is_some_and(|ts| ts.iter().any(|t| t.from == Some(w[0].id) && t.to == Some(w[1].id)));
                out.push(Cut { out: w[0].clone(), into: w[1].clone(), has_transition: has });
            }
        }
    }
    out
}

/// A transition between pictures, or at the edge of one (a fade).
pub(crate) struct Tr {
    pub from: Option<ClipId>,
    pub to: Option<ClipId>,
    pub effect: String,
    pub start: Tick,
    pub duration: Tick,
}

/// Every transition on the picture tracks (the Titles track has its own fades and is not counted).
pub(crate) fn transitions(s: &Session) -> Vec<Tr> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    q.video_tracks
        .iter()
        .filter(|t| t.enabled && !t.name.trim().eq_ignore_ascii_case("titles"))
        .flat_map(|t| t.transitions.iter())
        .map(|t| Tr { from: t.from, to: t.to, effect: t.effect.effect.clone(), start: t.start, duration: t.duration })
        .collect()
}

/// What a cut is, from the plan.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Within,
    Idea { quiet: f64 },
    Section,
}

/// How each cut sits in the plan: inside an idea, between two ideas, or between two sections.
fn classify(s: &Session, cut: &Cut, low: f64) -> Kind {
    let tol = 2 * s.sequence_rate().frame_duration().0;
    let mut places: Vec<crate::soundplan::Place> = crate::soundplan::places(s).into_iter().filter(|p| p.beat.link.is_some()).collect();
    places.sort_by_key(|p| p.start);
    for pair in places.windows(2) {
        if (pair[1].start - cut.into.start.0).abs() <= tol {
            if pair[0].section != pair[1].section {
                return Kind::Section;
            }
            let (a, b) = (f64::from(pair[0].beat.intensity) / 10.0, f64::from(pair[1].beat.intensity) / 10.0);
            return if a <= low && b <= low { Kind::Idea { quiet: a.max(b) } } else { Kind::Within };
        }
    }
    Kind::Within
}

/// Whether the plan has ideas placed on the timeline.
fn plan_placed(s: &Session) -> bool {
    crate::soundplan::places(s).iter().filter(|p| p.beat.link.is_some()).count() >= 2
}

fn frames_for(s: &Session, seconds: f64) -> i64 {
    let rate = s.sequence_rate();
    (rate.snap_nearest(Tick::from_seconds_f64(seconds)).0 / rate.frame_duration().0.max(1)).max(1)
}

/// How long a transition of `seconds` between `a` and `b` may be: shortened to half the shorter shot, `None` if that
/// is too short to see, or if either has too little picture to spare.
fn fit(seconds: f64, a: &Pic, b: &Pic) -> std::result::Result<f64, String> {
    let shorter = secs(a.end - a.start).min(secs(b.end - b.start));
    let d = seconds.min(shorter / 2.0);
    if d < MIN_SECONDS {
        return Err(format!("a shot is only {shorter:.1} s, too short for one"));
    }
    // a transition centred on the cut takes half its length from the end of one and the start of the next
    let half = Tick::from_seconds_f64(d / 2.0);
    let need = |t: Tick, speed: f64| Tick((t.0 as f64 * speed).round() as i64);
    if a.after < need(half, a.speed) || b.before < need(half, b.speed) {
        return Err("a shot has no spare picture beyond its cut for it to use".into());
    }
    Ok(d)
}

fn edge_free(s: &Session, p: &Pic, edge_in: bool) -> bool {
    let Some(q) = s.active_sequence() else { return false };
    q.video_tracks.iter().any(|t| {
        t.items.iter().any(|i| i.id == p.id)
            && !t.transitions.iter().any(|x| if edge_in { x.to == Some(p.id) && x.from.is_none() } else { x.from == Some(p.id) && x.to.is_none() })
    })
}

#[derive(Default)]
struct Plan {
    dissolves: Vec<(Cut, f64, f64)>,
    dips: Vec<(Cut, f64)>,
    fade_in: Option<(Pic, f64)>,
    fade_out: Option<(Pic, f64)>,
    moves: Vec<(ClipId, &'static str, String)>,
    kept_hard: Vec<Value>,
    skipped: Vec<Value>,
    hard: usize,
}

fn make_plan(s: &Session, p: &Value, q: &Sequence) -> Result<Plan> {
    let pol = policy(s);
    let pics = pictures(s);
    let all = cuts(s, &pics);
    let frame = s.sequence_rate().frame_duration();
    let time = |key: &str| -> Result<Option<Tick>> {
        match p.get(key).filter(|v| !v.is_null()) {
            None => Ok(None),
            Some(v) => parse_time(v, q, key).map(Some).map_err(|e| bad(CMD, e)),
        }
    };
    let (from, to) = (time("from")?.unwrap_or(Tick::ZERO), time("to")?.unwrap_or(Tick(i64::MAX)));
    if to <= from {
        return Err(bad(CMD, "`to` must be after `from`"));
    }
    let in_scope = |t: Tick| t >= from && t <= to;
    let mut plan = Plan::default();
    let scoped: Vec<&Cut> = all.iter().filter(|c| in_scope(c.into.start)).collect();
    let placed = plan_placed(s);
    // candidates, dips first, then the quietest dissolves
    let mut cands: Vec<(u8, f64, &Cut, Kind)> = Vec::new();
    for c in &scoped {
        match classify(s, c, pol.low) {
            Kind::Section if pol.dip_at_section_ends => cands.push((0, 0.0, c, Kind::Section)),
            Kind::Section => {}
            k @ Kind::Idea { quiet } => cands.push((1, quiet, c, k)),
            Kind::Within => {}
        }
    }
    cands.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.into.start.cmp(&b.2.into.start)));
    let existing = all.iter().filter(|c| c.has_transition).count();
    let allowed = (all.len() as f64 * pol.share / 100.0).ceil() as usize;
    let mut used = existing;
    for (_, _, c, kind) in cands {
        if c.has_transition {
            continue;
        }
        if !placed {
            break;
        }
        if used >= allowed {
            plan.kept_hard.push(json!({"at": c.into.start.0, "seconds": secs(c.into.start), "why": format!("at {} the share of cuts with a transition is already at its limit ({:.0}%), so this one stays hard", clock(c.into.start), pol.share)}));
            continue;
        }
        let want = if kind == Kind::Section { pol.dip } else { pol.dissolve };
        match fit(want, &c.out, &c.into) {
            Ok(d) => {
                used += 1;
                match kind {
                    Kind::Section => plan.dips.push(((*c).clone(), d)),
                    Kind::Idea { quiet } => plan.dissolves.push(((*c).clone(), d, quiet)),
                    Kind::Within => {}
                }
            }
            Err(why) => {
                plan.skipped.push(json!({"at": c.into.start.0, "seconds": secs(c.into.start), "why": format!("the cut at {}: {why}", clock(c.into.start))}))
            }
        }
    }
    plan.hard = all.len().saturating_sub(used);
    // the start and the end of the film
    if bool_p(p, "fades").unwrap_or(true) {
        let first = pics.iter().flatten().min_by_key(|x| x.start);
        let last = pics.iter().flatten().max_by_key(|x| x.end);
        if let Some(f) = first.filter(|f| in_scope(f.start) && f.start <= Tick(frame.0 * 2) && edge_free(s, f, true)) {
            plan.fade_in = Some((f.clone(), FADE_SECONDS.min(secs(f.end - f.start) / 2.0)));
        }
        if let Some(l) = last.filter(|l| to == Tick(i64::MAX) || in_scope(l.end)).filter(|l| edge_free(s, l, false)) {
            plan.fade_out = Some((l.clone(), FADE_SECONDS.min(secs(l.end - l.start) / 2.0)));
        }
        if plan.fade_in.as_ref().is_some_and(|(_, d)| *d < MIN_SECONDS) {
            plan.fade_in = None;
        }
        if plan.fade_out.as_ref().is_some_and(|(_, d)| *d < MIN_SECONDS) {
            plan.fade_out = None;
        }
    }
    // stills held with no move
    if bool_p(p, "moves").unwrap_or(true) {
        let held = pics.iter().flatten().filter(|x| x.still && !x.moves && in_scope(x.start) && secs(x.end - x.start) >= STILL_MOVE_SECONDS);
        for (turn, pic) in held.enumerate() {
            plan.moves.push((pic.id, MOVES[turn % MOVES.len()], pic.name.clone()));
        }
    }
    Ok(plan)
}

fn says(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn polish(s: &mut Session, p: &Value) -> Result<Value> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?.clone();
    let dry = bool_p(p, "dryRun").unwrap_or(false);
    let plan = make_plan(s, p, &q)?;
    if !dry {
        let n = plan.dissolves.len() + plan.dips.len() + usize::from(plan.fade_in.is_some()) + usize::from(plan.fade_out.is_some()) + plan.moves.len();
        if n > 0 {
            s.edit_group("Polish the Edit", |s| {
                let apply = |s: &mut Session, clip: ClipId, edge: &str, seconds: f64, effect: &str, what: &str| -> Result<()> {
                    let frames = frames_for(s, seconds);
                    s.execute("sequence.applyVideoTransition", json!({"clip": clip.0, "edge": edge, "frames": frames, "effect": effect}))
                        .map(|_| ())
                        .map_err(|e| bad(CMD, format!("{what}: {e}")))
                };
                for (c, d, _) in &plan.dissolves {
                    apply(s, c.into.id, "in", *d, "cross_dissolve", &format!("the dissolve at {}", clock(c.into.start)))?;
                }
                for (c, d) in &plan.dips {
                    apply(s, c.into.id, "in", *d, "dip_to_black", &format!("the dip at {}", clock(c.into.start)))?;
                }
                if let Some((f, d)) = &plan.fade_in {
                    apply(s, f.id, "in", *d, "dip_to_black", "the fade in")?;
                }
                if let Some((l, d)) = &plan.fade_out {
                    apply(s, l.id, "out", *d, "dip_to_black", "the fade out")?;
                }
                for (clip, mv, name) in &plan.moves {
                    s.execute("motion.cameraMove", json!({"clip": clip.0, "move": mv})).map_err(|e| bad(CMD, format!("the move on “{name}”: {e}")))?;
                }
                Ok(())
            })?;
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if !plan.dissolves.is_empty() {
        parts.push(format!("{} (where one quiet idea gives way to the next)", says(plan.dissolves.len(), "dissolve", "dissolves")));
    }
    if !plan.dips.is_empty() {
        parts.push(format!("{} (where a section ends)", says(plan.dips.len(), "dip to black", "dips to black")));
    }
    match (plan.fade_in.is_some(), plan.fade_out.is_some()) {
        (true, true) => parts.push("a fade in and a fade out".into()),
        (true, false) => parts.push("a fade in".into()),
        (false, true) => parts.push("a fade out".into()),
        _ => {}
    }
    if !plan.moves.is_empty() {
        parts.push(format!("a slow move on {}", says(plan.moves.len(), "still", "stills")));
    }
    let did = if parts.is_empty() { "Nothing needed adding".to_string() } else { format!("{} {}", if dry { "Would add" } else { "Added" }, parts.join(", ")) };
    let mut summary = format!("{did}. {} stay hard cuts.", says(plan.hard, "cut", "cuts"));
    if !plan_placed(s) {
        summary.push_str(
            " The plan's ideas are not placed on the timeline, so only the fades and the stills were done: there is no way to tell where an idea changes.",
        );
    }
    if !plan.kept_hard.is_empty() {
        summary.push_str(&format!(" {} kept hard to stay within the share of cuts that may carry a transition.", says(plan.kept_hard.len(), "more", "more")));
    }
    if !plan.skipped.is_empty() {
        summary.push_str(&format!(" {} skipped (no spare picture, or too short).", says(plan.skipped.len(), "cut", "cuts")));
    }
    Ok(json!({
        "summary": summary, "dryRun": dry,
        "dissolves": plan.dissolves.len(), "dips": plan.dips.len(), "fadeIn": plan.fade_in.is_some(), "fadeOut": plan.fade_out.is_some(),
        "stillsMoved": plan.moves.len(), "hardCuts": plan.hard, "keptHard": plan.kept_hard, "skipped": plan.skipped,
    }))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "edit.polish",
        label: "Polish the Edit",
        menu: &[],
        shortcut: None,
        params: r#"{"from":time?,"to":time?,"fades":bool=true,"moves":bool=true,"dryRun":bool=false}"#,
        enabled: has_seq,
        run: polish,
        journal: true,
    }]
}
