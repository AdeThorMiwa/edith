//! Rights for a whole edit (fork roadmap F04.2): the project's policy, the credits block, and the
//! audit that checks every clip on the timeline against that policy.
//!
//! | command | does |
//! |---|---|
//! | `provenance.policy {commercial?, attributionOk?, syntheticOk?}` | reads, or (undoably) changes, what this project allows: commercial use (it is monetized), material that needs a credit, and AI-generated material |
//! | `provenance.credits {sequence?, format?}` | the credit lines for what the sequence uses (`text`, `markdown` or `youtube`), ready to paste into a description, and which clips still lack a credit line |
//! | `provenance.audit {sequence?}` | every clip used, checked against the policy: errors (must fix), warnings (check) and notes (AI disclosure), each with where it is on the timeline |
//!
//! "Used" means on an enabled track of the sequence, enabled and (for audio) not muted, following
//! nested sequences. Nothing here is legal advice: it reports what the records say, and says so
//! when there is no record.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use filmcraft_project::{ItemId, ItemKind, Project, Sequence, TrackKind};
use filmcraft_time::{Tick, format_timecode_frames};

use crate::commands::{CommandSpec, always, bad};
use crate::provenance::{self, Provenance};
use crate::{Result, Session};

/// The key in `Project::ext` that holds the policy.
pub const POLICY_KEY: &str = "edith.policy";
/// How deep nested sequences are followed.
const MAX_DEPTH: usize = 8;
/// The most uses listed for one finding.
const MAX_USES: usize = 50;

/// What this project allows. The defaults allow everything: a policy is something you turn on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Policy {
    /// The video is monetized or otherwise commercial: material not licensed for that is an error.
    pub commercial: bool,
    /// Material that needs a credit is acceptable (the credits block covers it).
    pub attribution_ok: bool,
    /// AI-generated material is acceptable.
    pub synthetic_ok: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self { commercial: true, attribution_ok: true, synthetic_ok: true }
    }
}

pub fn policy(p: &Project) -> Policy {
    p.ext.get(POLICY_KEY).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

fn put_policy(p: &mut Project, pol: Policy) {
    if pol == Policy::default() {
        p.ext.remove(POLICY_KEY);
    } else {
        p.ext.insert(POLICY_KEY.to_string(), serde_json::to_value(pol).unwrap_or(Value::Null));
    }
}

/// One place a clip is used.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Use {
    /// The sequence the person is looking at (for a nested clip, the outer one).
    pub sequence: u64,
    pub clip: u64,
    pub timecode: String,
    pub seconds: f64,
}

/// Every media item used by `seq`, with where. Nested sequences are followed (their media is
/// reported at the place the nested sequence sits), up to a depth limit.
pub fn uses(p: &Project, seq: &Sequence, seq_id: u64) -> BTreeMap<ItemId, Vec<Use>> {
    fn walk(
        p: &Project,
        q: &Sequence,
        top: &Sequence,
        top_id: u64,
        at: Option<(u64, Tick)>,
        depth: usize,
        seen: &mut Vec<u64>,
        out: &mut BTreeMap<ItemId, Vec<Use>>,
    ) {
        for t in q.all_tracks() {
            if !t.enabled || (t.kind == TrackKind::Audio && t.muted) {
                continue;
            }
            for i in t.items.iter().filter(|i| i.enabled) {
                let (clip, start) = at.unwrap_or((i.id.0, i.start));
                match p.item(i.item).map(|x| &x.kind) {
                    Some(ItemKind::Sequence(inner)) if depth < MAX_DEPTH && !seen.contains(&i.item.0) => {
                        seen.push(i.item.0);
                        walk(p, inner, top, top_id, Some((clip, start)), depth + 1, seen, out);
                        seen.pop();
                    }
                    _ => {
                        let Some(owner) = provenance::owner(p, i.item) else { continue };
                        let list = out.entry(owner).or_default();
                        if list.len() < MAX_USES {
                            let rate = top.settings.frame_rate;
                            list.push(Use {
                                sequence: top_id,
                                clip,
                                timecode: format_timecode_frames(rate.frame_at(start), rate, top.settings.drop_frame),
                                seconds: start.seconds(),
                            });
                        }
                    }
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(p, seq, seq, seq_id, None, 0, &mut vec![seq_id], &mut out);
    out
}

/// The sequence to look at: `sequence` if given, else the active one.
fn target(s: &Session, p: &Value, cmd: &str) -> Result<(u64, Sequence)> {
    let id = match p.get("sequence").filter(|v| !v.is_null()) {
        Some(v) => ItemId(v.as_u64().ok_or_else(|| bad(cmd, "`sequence` must be a sequence id"))?),
        None => s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?,
    };
    s.project.sequence(id).map(|q| (id.0, q.clone())).ok_or_else(|| bad(cmd, format!("{} is not a sequence", id.0)))
}

#[derive(Serialize)]
struct Finding {
    severity: &'static str,
    code: &'static str,
    item: u64,
    name: String,
    message: String,
    uses: Vec<Use>,
}

pub(crate) fn rank(s: &str) -> u8 {
    match s {
        "error" => 0,
        "warning" => 1,
        _ => 2,
    }
}

/// The findings for what a sequence uses, against `pol`.
fn findings(p: &Project, used: &BTreeMap<ItemId, Vec<Use>>, pol: Policy) -> Vec<Finding> {
    let mut out = Vec::new();
    for (id, at) in used {
        let name = p.item(*id).map(|i| i.name.clone()).unwrap_or_default();
        let mut add = |severity, code, message: String| out.push(Finding { severity, code, item: id.0, name: name.clone(), message, uses: at.clone() });
        let Some(r) = provenance::get(p, *id) else {
            add("warning", "unknown_origin", format!("Where {name} came from is not recorded, so its rights are not known."));
            continue;
        };
        if r.synthetic.is_some() {
            if pol.synthetic_ok {
                add("note", "ai_disclosure", format!("{name} is AI-generated. If it looks real, YouTube asks you to say so when you upload."));
            } else {
                add("error", "ai_not_allowed", format!("{name} is AI-generated, and this project does not allow AI-generated material."));
            }
        }
        match r.license.commercial {
            Some(false) if pol.commercial => {
                add("error", "not_commercial", format!("{name} is licensed for non-commercial use only, and this project is commercial."))
            }
            None if pol.commercial && r.synthetic.is_none() => {
                add("warning", "terms_unknown", format!("The licence of {name} does not say whether commercial use is allowed."))
            }
            _ => {}
        }
        match r.license.attribution_required {
            Some(true) if !pol.attribution_ok => {
                add("error", "credit_not_allowed", format!("{name} needs a credit, and this project does not allow material that needs one."))
            }
            None if r.synthetic.is_none() && r.source != "own" && r.source != "licensed" => {
                add("warning", "credit_unknown", format!("The licence of {name} does not say whether a credit is required."))
            }
            _ => {}
        }
    }
    out.sort_by_key(|f| (rank(f.severity), f.item));
    out
}

/// The credit line for one record, or `None` when no credit is needed.
fn credit_line(name: &str, r: &Provenance) -> Option<String> {
    let needed = r.license.attribution_required == Some(true) || !r.license.credit_text.is_empty();
    if !needed {
        return None;
    }
    if !r.license.credit_text.is_empty() {
        return Some(r.license.credit_text.clone());
    }
    let mut parts = vec![if r.author.is_empty() { name.to_string() } else { format!("{name} by {}", r.author) }];
    if !r.license.name.is_empty() {
        parts.push(r.license.name.clone());
    }
    if !r.source_url.is_empty() {
        parts.push(r.source_url.clone());
    }
    Some(parts.join(" · "))
}

fn policy_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "provenance.policy";
    let mut next = policy(&s.project);
    for (key, slot) in [("commercial", &mut next.commercial), ("attributionOk", &mut next.attribution_ok), ("syntheticOk", &mut next.synthetic_ok)] {
        if let Some(v) = p.get(key).filter(|v| !v.is_null()) {
            *slot = v.as_bool().ok_or_else(|| bad(cmd, format!("`{key}` must be true or false")))?;
        }
    }
    if next != policy(&s.project) {
        s.edit("Set Rights Policy", |pr, _| {
            put_policy(pr, next);
            Ok(())
        })?;
    }
    Ok(json!({"policy": next}))
}

fn audit_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let (id, seq) = target(s, p, "provenance.audit")?;
    let pol = policy(&s.project);
    let used = uses(&s.project, &seq, id);
    let list = findings(&s.project, &used, pol);
    let count = |sev: &str| list.iter().filter(|f| f.severity == sev).count();
    Ok(json!({
        "sequence": id, "policy": pol, "clipsChecked": used.len(),
        "ok": count("error") == 0,
        "errors": count("error"), "warnings": count("warning"), "notes": count("note"),
        "findings": list,
    }))
}

fn credits_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "provenance.credits";
    let format = match p.get("format").filter(|v| !v.is_null()) {
        None => "text",
        Some(v) => {
            v.as_str().filter(|f| matches!(*f, "text" | "markdown" | "youtube")).ok_or_else(|| bad(cmd, "`format` must be text, markdown or youtube"))?
        }
    };
    let (id, seq) = target(s, p, cmd)?;
    let used = uses(&s.project, &seq, id);
    let mut lines: Vec<(f64, String)> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for (item, at) in &used {
        let name = s.project.item(*item).map(|i| i.name.clone()).unwrap_or_default();
        let Some(r) = provenance::get(&s.project, *item) else { continue };
        let first = at.iter().map(|u| u.seconds).fold(f64::INFINITY, f64::min);
        if let Some(line) = credit_line(&name, &r) {
            if !lines.iter().any(|(_, l)| *l == line) {
                lines.push((first, line));
            }
            if r.license.credit_text.is_empty() && r.author.is_empty() {
                missing.push(name);
            }
        }
    }
    lines.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let lines: Vec<String> = lines.into_iter().map(|(_, l)| l).collect();
    let text = match format {
        "markdown" => lines.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n"),
        "youtube" if lines.is_empty() => String::new(),
        "youtube" => format!("Credits:\n{}", lines.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n")),
        _ => lines.join("\n"),
    };
    let unrecorded = used.keys().filter(|i| provenance::get(&s.project, **i).is_none()).count();
    Ok(json!({"sequence": id, "format": format, "text": text, "lines": lines, "count": lines.len(), "missingCreditText": missing, "unrecorded": unrecorded}))
}

pub fn commands() -> Vec<CommandSpec> {
    let spec = |id, label, params, journal, run: fn(&mut Session, &Value) -> Result<Value>| CommandSpec {
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
        spec("provenance.policy", "Rights Policy", r#"{"commercial":bool?,"attributionOk":bool?,"syntheticOk":bool?}"#, true, policy_cmd),
        spec("provenance.credits", "Credits for the Edit", r#"{"sequence":id?,"format":"text|markdown|youtube"?}"#, false, credits_cmd),
        spec("provenance.audit", "Check the Edit's Rights", r#"{"sequence":id?}"#, false, audit_cmd),
    ]
}
