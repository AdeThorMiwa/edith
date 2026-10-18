//! The assistant's way to change the edit (fork roadmap F15): **Edit mode**, and the tools that work
//! inside a version (F14).
//!
//! In Ask and Coach the assistant has no tool that changes anything. In Edit mode it must open a version
//! first, then works inside it over as many rounds as it takes, while the person watches the timeline and
//! may edit the same version. It can never commit or discard: only the person does.
//!
//! | tool | does |
//! |---|---|
//! | `begin_version {title}` | open a version of the edit (or say that one is already open: the person's or its own); nothing it does reaches the committed edit until the person commits |
//! | `edit {command, params}` | run any editor command inside the version, recorded as the assistant's |
//! | `undo_steps {steps?}`, `redo_step` | take back its own latest steps, and put one back; never the person's steps |
//! | `version_status` | the version: its title, steps with who made each, and what it changes in plain words (read-only) |
//! | `command_list {query?}` | the commands it may run, with their parameters (read-only) |
//!
//! Safety is layered. The agent offers the edit tools only in Edit mode and the engine checks the mode
//! again when one runs. A command must be on the allowlist (the families that edit the project; anything
//! not listed is refused), must not name a risky action, and its parameters must not name a file or an
//! address: whatever reaches outside the edit (files, exports, downloads, settings, the network, the
//! global libraries) is never run by the assistant, and a version could not take it back. Exports, file
//! writes and downloads always ask the person directly.

use edith_ai::ToolOutput;
use serde_json::{Value, json};

use crate::Session;
use crate::assistant_tools::{ToolDef, def};
use crate::version::Actor;

/// The families of commands that edit the project and nothing else.
const ALLOWED: [&str; 14] = [
    "timeline.",
    "clip.",
    "edit.",
    "sequence.",
    "effects.",
    "markers.",
    "trim.",
    "planner.",
    "sound.",
    "lumetri.",
    "essentialSound.",
    "graphics.",
    "captions.",
    "mixer.",
];
/// More families, kept apart because each needs a word of care.
const ALLOWED_MORE: [&str; 3] = ["masks.", "multicam.", "project."];
/// Commands inside an allowed family that are still not the assistant's to run.
const NEVER: [&str; 5] = [
    // its own undo steps have a tool that never reaches the person's
    "edit.undo",
    "edit.redo",
    // they write a file
    "sound.sheet",
    "planner.export",
    "planner.import",
];
/// A word in a command's id that makes it one the assistant does not run.
const RISKY_WORDS: [&str; 13] =
    ["export", "save", "import", "download", "record", "capture", "render", "reveal", "relink", "consolidate", "archive", "backup", "install"];
/// A parameter that names a file or an address.
const FILE_PARAMS: [&str; 9] = ["path", "paths", "file", "files", "url", "uri", "folder", "dir", "directory"];
/// The longest answer of a command given back to the model.
const MAX_RESULT_CHARS: usize = 1200;
/// Commands listed at once.
const MAX_LISTED: usize = 40;
/// The most steps taken back in one call.
const MAX_UNDO: u64 = 20;

/// Whether the assistant may run command `id`, and why not when it may not.
pub(crate) fn may_run(id: &str) -> Result<(), String> {
    let lower = id.to_ascii_lowercase();
    if NEVER.contains(&id) {
        return Err(format!("`{id}` is not one the assistant may run: it reaches outside the edit, or it has a tool of its own."));
    }
    if id == "look.match" {
        return Ok(());
    }
    if id.starts_with("look.") {
        return Err(format!("`{id}` is not one the assistant may run: the look library is shared by every project, so a discard could not take it back."));
    }
    if !ALLOWED.iter().chain(ALLOWED_MORE.iter()).any(|p| id.starts_with(p)) {
        return Err(format!(
            "`{id}` is not one the assistant may run. It may run commands that edit the project (timeline, clips, effects, markers, trimming, the plan, sound, colour, graphics, captions, the mixer). Files, exports, downloads, settings and the network are the person's to do."
        ));
    }
    if let Some(w) = RISKY_WORDS.iter().find(|w| lower.contains(*w)) {
        return Err(format!("`{id}` is not one the assistant may run: it looks like it would {w} something outside the edit."));
    }
    Ok(())
}

/// Refuses parameters that name a file or an address, at any depth.
fn check_params(params: &Value, depth: usize) -> Result<(), String> {
    if depth > 6 {
        return Ok(());
    }
    match params {
        Value::Object(m) => {
            for (k, v) in m {
                if FILE_PARAMS.contains(&k.as_str()) {
                    return Err(format!("`{k}` names a file or an address: the assistant does not read or write files or reach the network from an edit."));
                }
                check_params(v, depth + 1)?;
            }
            Ok(())
        }
        Value::Array(a) => a.iter().take(1000).try_for_each(|v| check_params(v, depth + 1)),
        _ => Ok(()),
    }
}

fn stub(_: &Session, _: &Value) -> ToolOutput {
    ToolOutput::error("this tool is answered by the session")
}

/// A tool that changes the project (answered in [`run_edit`], on the session thread).
fn edit_tool(name: &str, description: &str, properties: Value) -> ToolDef {
    let mut d = def(name, description, properties, stub);
    d.read_only = false;
    d
}

fn result_text(v: &Value) -> String {
    if v.is_null() {
        return String::new();
    }
    let t = v.to_string();
    let cut: String = t.chars().take(MAX_RESULT_CHARS).collect();
    if cut.len() < t.len() { format!("{cut}…") } else { cut }
}

fn status_text(s: &Session) -> String {
    let Some(info) = s.version_info() else {
        return "No version is open. Call begin_version to open one before changing anything.".to_string();
    };
    let mut out = format!(
        "A version “{}” is open (started by {}){}. Everything you change goes into it; the person can edit it too, look at the original, and commit or discard it. You cannot commit.\n",
        info.title,
        info.started_by.id(),
        if info.peeking { "; the person is looking at the original right now" } else { "" }
    );
    if info.steps.is_empty() {
        out.push_str("It has no steps yet.\n");
    } else {
        out.push_str(&format!("{} step{} so far, oldest first:\n", info.steps.len(), if info.steps.len() == 1 { "" } else { "s" }));
        let skip = info.steps.len().saturating_sub(30);
        if skip > 0 {
            out.push_str(&format!("(the first {skip} are not listed)\n"));
        }
        for (i, st) in info.steps.iter().enumerate().skip(skip) {
            out.push_str(&format!("{}. {} (by {})\n", i + 1, st.label, st.by.id()));
        }
    }
    match s.version_changes() {
        Some(d) if !d.is_empty() => {
            out.push_str("What the version changes against the original:\n");
            for l in d.lines.iter().take(20) {
                out.push_str(&format!("- {l}\n"));
            }
        }
        _ => out.push_str("It changes nothing against the original yet.\n"),
    }
    out
}

/// What Edit mode is told with each question: where the version stands.
pub(crate) fn context(s: &Session) -> String {
    status_text(s)
}

fn version_status(s: &Session, _p: &Value) -> ToolOutput {
    ToolOutput::ok(status_text(s))
}

/// Beats shown by `planner_get`, and how much of a beat's narration.
const MAX_BEATS_SHOWN: usize = 80;
const NARRATION_CHARS: usize = 140;

fn clock_text(ticks: i64) -> String {
    let n = (ticks.max(0) / filmcraft_time::TICKS_PER_SECOND).max(0) as u64;
    if n >= 3600 { format!("{}:{:02}:{:02}", n / 3600, (n / 60) % 60, n % 60) } else { format!("{}:{:02}", n / 60, n % 60) }
}

fn planner_get(s: &Session, _p: &Value) -> ToolOutput {
    let Some(plan) = crate::planner::get(&s.project) else {
        return ToolOutput::ok("There is no plan yet. The person makes one in the Planner (a Format Pack and a length).");
    };
    let mut out = format!(
        "Plan “{}”{}: target {}, planned {} over {} beat{}.\n",
        plan.title,
        plan.pack.as_deref().map_or(String::new(), |p| format!(" (from the {p} pack)")),
        clock_text(plan.target.0),
        clock_text(plan.planned().0),
        plan.beat_count(),
        if plan.beat_count() == 1 { "" } else { "s" }
    );
    let mut shown = 0usize;
    for sec in &plan.sections {
        out.push_str(&format!("\n{}:\n", sec.name));
        for b in &sec.beats {
            if shown >= MAX_BEATS_SHOWN {
                break;
            }
            shown += 1;
            let words = b.narration.split_whitespace().count();
            let first: String = b.narration.chars().take(NARRATION_CHARS).collect();
            out.push_str(&format!(
                "- beat {} “{}”, {} long, intensity {:.1}, {}{}{}\n",
                b.id,
                b.name,
                clock_text(b.target.0),
                f64::from(b.intensity) / 10.0,
                b.status.id(),
                b.link.map_or(String::new(), |l| format!(", on the timeline at {}", clock_text(l.start.0))),
                if b.purpose.trim().is_empty() { String::new() } else { format!("; purpose: {}", b.purpose.trim().replace('\n', " ")) }
            ));
            if words > 0 {
                out.push_str(&format!(
                    "    narration ({words} words): {}{}\n",
                    first.replace('\n', " "),
                    if first.chars().count() < b.narration.chars().count() { "…" } else { "" }
                ));
            }
            if !b.shots.is_empty() {
                out.push_str(&format!("    {} shot{}\n", b.shots.len(), if b.shots.len() == 1 { "" } else { "s" }));
            }
        }
    }
    if plan.beat_count() > MAX_BEATS_SHOWN {
        out.push_str(&format!("\n({} more beats are not listed)\n", plan.beat_count() - MAX_BEATS_SHOWN));
    }
    ToolOutput::ok(out)
}

fn command_list(_s: &Session, p: &Value) -> ToolOutput {
    let query = p.get("query").and_then(Value::as_str).unwrap_or("").trim().to_ascii_lowercase();
    let all = crate::commands::command_specs();
    let mine: Vec<&crate::commands::CommandSpec> = all.iter().filter(|c| may_run(c.id).is_ok()).collect();
    if query.is_empty() {
        let mut families: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for c in &mine {
            *families.entry(c.id.split('.').next().unwrap_or("")).or_default() += 1;
        }
        let list: Vec<String> = families.iter().map(|(f, n)| format!("{f} ({n})")).collect();
        return ToolOutput::ok(format!(
            "You may run {} commands, in these families: {}. Give a query (a word from a command's id or name, such as \"marker\" or \"trim\") to list them with their parameters.",
            mine.len(),
            list.join(", ")
        ));
    }
    let hits: Vec<&&crate::commands::CommandSpec> =
        mine.iter().filter(|c| c.id.to_ascii_lowercase().contains(&query) || c.label.to_ascii_lowercase().contains(&query)).collect();
    if hits.is_empty() {
        return ToolOutput::ok(format!("No command you may run matches “{query}”. Try a shorter word."));
    }
    let mut out = String::new();
    for c in hits.iter().take(MAX_LISTED) {
        out.push_str(&format!("- {} — {}{}: {}\n", c.id, c.label, if c.journal { "" } else { " (only reads)" }, c.params));
    }
    if hits.len() > MAX_LISTED {
        out.push_str(&format!("({} more match: use a narrower query)\n", hits.len() - MAX_LISTED));
    }
    ToolOutput::ok(out)
}

pub(crate) fn tools() -> Vec<ToolDef> {
    let none = json!({});
    vec![
        def(
            "version_status",
            "Whether a version is open and what is in it: its title, who started it, its steps with who made each (you or the person), and what it changes against the original edit in plain words. Call it first in Edit mode and again before you change something the person may have touched.",
            none,
            version_status,
        ),
        def(
            "planner_get",
            "The plan of the video: its sections and beats with their ids, lengths, intensity, status, purpose and the start of the narration, and where each beat is on the timeline once laid out. Use the beat ids with planner commands and with sound_plan.",
            json!({}),
            planner_get,
        ),
        def(
            "command_list",
            "The editor commands you may run with the edit tool, with their parameters. With no query it names the families; give a word from a command's id or name (\"marker\", \"trim\", \"effects\") to list matching commands.",
            json!({"query": {"type": "string", "description": "A word from a command id or name."}}),
            command_list,
        ),
        edit_tool(
            "begin_version",
            "Open a version of the person's edit to work in. Nothing you do reaches their committed edit until they commit the version, and they can look at the original, edit alongside you, discard it, or commit it; you cannot. Call it before any change. If a version is already open, it says so and you work in that one.",
            json!({"title": {"type": "string", "description": "A short name for what this version is for, e.g. \"Tighten act two\"."}}),
        ),
        edit_tool(
            "edit",
            "Run one editor command inside the open version (list them with command_list). It is recorded as yours. The person may be editing too, so read the edit again (version_status, sequence_view) before changing something they may have touched. Make small steps and say what you did. Files, exports, downloads and settings are not available to you.",
            json!({
                "command": {"type": "string", "description": "A command id from command_list, such as markers.add."},
                "params": {"type": "object", "description": "The command's parameters, as command_list shows them."},
            }),
        ),
        edit_tool(
            "undo_steps",
            "Take back your own latest steps in the version, newest first. It stops at a step the person made: those are theirs to undo.",
            json!({"steps": {"type": "integer", "description": "How many of your latest steps to take back, 1 to 20. Default 1."}}),
        ),
        edit_tool("redo_step", "Put back the step you last took back.", json!({})),
    ]
}

// ---------------------------------------------------------------------------- running

/// Answers the tools that change the edit, on the session thread; `None` for any other tool.
pub(crate) fn run_edit(s: &mut Session, name: &str, input: &Value) -> Option<ToolOutput> {
    if !matches!(name, "begin_version" | "edit" | "undo_steps" | "redo_step") {
        return None;
    }
    // the agent offers these only in Edit mode; the engine asks again
    if crate::assistant::profile_of(&s.assistant.settings.mode) != edith_ai::Profile::Propose {
        return Some(ToolOutput::error("this assistant is not in Edit mode, so it may not change the edit"));
    }
    let input = if input.is_object() { input.clone() } else { json!({}) };
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match name {
        "begin_version" => begin(s, &input),
        "edit" => edit(s, &input),
        "undo_steps" => undo(s, &input),
        _ => redo(s),
    }));
    Some(out.unwrap_or_else(|_| ToolOutput::error("the edit tool failed internally; nothing was changed")))
}

fn begin(s: &mut Session, p: &Value) -> ToolOutput {
    if let Some(t) = s.versions.title().map(str::to_string) {
        return ToolOutput::ok(format!("A version “{t}” is already open, so you work in that one. {}", status_text(s)));
    }
    let title = p.get("title").and_then(Value::as_str).unwrap_or("");
    match s.version_create(title, Actor::Assistant) {
        Ok(_) => ToolOutput::ok(format!(
            "Opened the version “{}”. Everything you change now goes into it; the person sees a banner above the editor and can look at the original, edit alongside you, and commit or discard it. You cannot commit.",
            s.versions.title().unwrap_or("")
        )),
        Err(e) => ToolOutput::error(e.to_string()),
    }
}

fn edit(s: &mut Session, p: &Value) -> ToolOutput {
    if !s.versions.is_open() {
        return ToolOutput::error("No version is open. Call begin_version first: every change goes into a version the person can look at, commit or discard.");
    }
    let Some(command) = p.get("command").and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty() && c.len() <= 100) else {
        return ToolOutput::error("`command` is required: a command id from command_list");
    };
    if let Err(e) = may_run(command) {
        return ToolOutput::error(e);
    }
    let params = match p.get("params") {
        None | Some(Value::Null) => json!({}),
        Some(v) if v.is_object() => v.clone(),
        Some(_) => return ToolOutput::error("`params` must be an object"),
    };
    if let Err(e) = check_params(&params, 0) {
        return ToolOutput::error(e);
    }
    let before = s.version_info().map_or(0, |i| i.steps.len());
    let run = s.in_version(|s| {
        let previous = std::mem::replace(&mut s.versions.actor, Actor::Assistant);
        let r = s.execute(command, params);
        s.versions.actor = previous;
        r
    });
    match run {
        Ok(Ok(v)) => {
            let after = s.version_info().map_or(0, |i| i.steps.len());
            let r = result_text(&v);
            ToolOutput::ok(format!(
                "Done: {command}{}. The version has {after} step{} ({} new).",
                if r.is_empty() { String::new() } else { format!(" → {r}") },
                if after == 1 { "" } else { "s" },
                after.saturating_sub(before)
            ))
        }
        Ok(Err(e)) => ToolOutput::error(format!("{command} failed: {e}. Nothing was changed by it.")),
        Err(e) => ToolOutput::error(e.to_string()),
    }
}

fn undo(s: &mut Session, p: &Value) -> ToolOutput {
    let Some(info) = s.version_info() else { return ToolOutput::error("No version is open.") };
    let want = match p.get("steps").filter(|v| !v.is_null()) {
        None => 1,
        Some(v) => match v.as_u64().filter(|n| (1..=MAX_UNDO).contains(n)) {
            Some(n) => n,
            None => return ToolOutput::error("`steps` must be a whole number from 1 to 20"),
        },
    };
    let live = info.steps.len().saturating_sub(info.locked);
    let mut undone: Vec<String> = Vec::new();
    let mut stopped = None;
    for k in 0..want as usize {
        let Some(info) = s.version_info() else { break };
        let live_now = info.steps.len().saturating_sub(info.locked);
        if live_now == 0 || k >= live {
            stopped = Some("there is nothing further to take back");
            break;
        }
        match info.steps.last() {
            Some(last) if last.by == Actor::Assistant => {
                let label = last.label.clone();
                match s.in_version(|s| s.undo()) {
                    Ok(Some(_)) => undone.push(label),
                    _ => {
                        stopped = Some("it could not be taken back");
                        break;
                    }
                }
            }
            _ => {
                stopped = Some("the latest step is the person's, and theirs to undo");
                break;
            }
        }
    }
    let mut out = if undone.is_empty() { "Nothing was taken back".to_string() } else { format!("Took back {}: {}", undone.len(), undone.join("; ")) };
    if let Some(why) = stopped {
        out.push_str(&format!(" ({why})"));
    }
    out.push('.');
    if undone.is_empty() { ToolOutput::error(out) } else { ToolOutput::ok(out) }
}

fn redo(s: &mut Session) -> ToolOutput {
    if !s.versions.is_open() {
        return ToolOutput::error("No version is open.");
    }
    match s.in_version(|s| s.redo()) {
        Ok(Some(label)) => ToolOutput::ok(format!("Put back: {label}.")),
        Ok(None) => ToolOutput::error("There is nothing to put back."),
        Err(e) => ToolOutput::error(e.to_string()),
    }
}
