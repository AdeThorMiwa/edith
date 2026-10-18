//! Workflows in the engine (fork roadmap F16.4): the library of workflow files, where the assistant is in one, and
//! the commands and tools around them. The files and their shape are `edith_agent::workflow`.
//!
//! **The library** is the workflows that ship with Edith plus the person's own: `.md` files in a `workflows` folder
//! beside the preferences (a file with the name of a shipped one replaces it). A file that does not parse is skipped
//! and named, with why; the others still load.
//!
//! **Where it is** is saved in the project (`Project::ext["edith.workflow"]`: the workflow, the stage it is at, its
//! status and a note), so it is saved and recovered with the project and, when the assistant is working in a
//! version, it is part of that version (a discarded version takes its progress with it, and a saved version
//! brings it back). The panel shows it; the assistant is told where it stood at the start of each message in Edit
//! mode, so a workflow can be picked up after the app was closed.
//!
//! | command | does |
//! |---|---|
//! | `workflow.list` | the workflows (name, what each is for, its stages, whether it is yours) and the files that were skipped |
//! | `workflow.state` | which workflow the project is in, at which stage, with what status and note |
//! | `workflow.progress {workflow?, stage, status?, note?}` | record the stage (a number or its name) and its status (`working`, `waiting`, `done`, `blocked`) |
//! | `workflow.clear` | forget which workflow the project is in |
//!
//! Tools: `workflow_list` (names and descriptions only, so the conversation stays small), `workflow_load {name}`
//! (the whole text: only now does it enter the conversation) and `workflow_progress` (the same as the command,
//! Edit mode only, inside the version the assistant has open).

use std::path::PathBuf;

use edith_ai::ToolOutput;
use edith_ai::workflow::{self, MAX_BYTES, Workflow};
use serde_json::{Value, json};

use crate::assistant_tools::{ToolDef, def};
use crate::commands::{CommandSpec, always, bad, str_p};
use crate::{Result, Session};

pub const KEY: &str = "edith.workflow";
const DIR: &str = "workflows";
/// Files read from the person's folder.
const MAX_FILES: usize = 50;
const MAX_NOTE: usize = 300;

/// A workflow and whether it is the person's own.
pub struct Listed {
    pub workflow: Workflow,
    pub yours: bool,
}

fn workflows_dir(s: &Session) -> Option<PathBuf> {
    s.prefs_path.as_ref().and_then(|p| p.parent()).map(|d| d.join(DIR))
}

/// The shipped workflows and the person's own, and the files that were skipped (with why).
pub fn library(s: &Session) -> (Vec<Listed>, Vec<String>) {
    let mut out: Vec<Listed> = workflow::builtin().into_iter().map(|workflow| Listed { workflow, yours: false }).collect();
    let mut problems = Vec::new();
    let Some(dir) = workflows_dir(s) else { return (out, problems) };
    let Ok(rd) = std::fs::read_dir(&dir) else { return (out, problems) };
    let mut files: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("md"))).collect();
    files.sort();
    for path in files.into_iter().take(MAX_FILES) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let text = match std::fs::metadata(&path) {
            Ok(m) if m.len() as usize <= MAX_BYTES => std::fs::read_to_string(&path).map_err(|e| e.to_string()),
            Ok(_) => Err(format!("the file is longer than {} KB", MAX_BYTES / 1024)),
            Err(e) => Err(e.to_string()),
        };
        match text.and_then(|t| workflow::parse(&t)) {
            Ok(w) => {
                out.retain(|l| l.workflow.name != w.name);
                out.push(Listed { workflow: w, yours: true });
            }
            Err(why) => problems.push(format!("{name}: {why}")),
        }
    }
    (out, problems)
}

fn find(s: &Session, name: &str) -> Option<Listed> {
    let n = name.trim().to_lowercase();
    library(s).0.into_iter().find(|l| l.workflow.name == n)
}

// ---------------------------------------------------------------------------- where the project is

/// Where the project is in a workflow.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub name: String,
    pub stage: usize,
    pub status: String,
    pub note: String,
    pub done: Vec<usize>,
}

pub const STATUSES: [&str; 4] = ["working", "waiting", "done", "blocked"];

pub fn state(p: &filmcraft_project::Project) -> Option<State> {
    let v = p.ext.get(KEY)?;
    let name = v["name"].as_str()?.to_string();
    let stage = usize::try_from(v["stage"].as_u64()?).ok()?;
    let status = v["status"].as_str().filter(|s| STATUSES.contains(s)).unwrap_or("working").to_string();
    let note: String = v["note"].as_str().unwrap_or("").chars().filter(|c| !c.is_control()).take(MAX_NOTE).collect();
    let done = v["done"]
        .as_array()
        .map(|a| a.iter().filter_map(|n| n.as_u64().and_then(|n| usize::try_from(n).ok())).take(workflow::MAX_STAGES).collect())
        .unwrap_or_default();
    Some(State { name, stage, status, note, done })
}

fn state_json(s: &Session) -> Value {
    let Some(st) = state(&s.project) else { return json!({"active": false}) };
    let w = find(s, &st.name).map(|l| l.workflow);
    json!({
        "active": true, "name": st.name, "known": w.is_some(), "stage": st.stage,
        "stageName": w.as_ref().and_then(|w| w.stage(&st.stage.to_string()).map(|x| x.name.clone())),
        "stages": w.as_ref().map(|w| w.stages.len()), "status": st.status, "note": st.note, "done": st.done,
    })
}

/// A line telling the assistant where the project stands in a workflow, or nothing.
pub(crate) fn context_line(s: &Session) -> Option<String> {
    let st = state(&s.project)?;
    let w = find(s, &st.name)?.workflow;
    let stage = w.stage(&st.stage.to_string())?;
    Some(format!(
        "Workflow in progress: “{}”, at stage {} of {} (“{}”), {}{}{}. Load it again with workflow_load if you need its text.",
        w.name,
        stage.number,
        w.stages.len(),
        stage.name,
        st.status,
        if st.done.is_empty() { String::new() } else { format!("; stages done: {}", st.done.iter().map(usize::to_string).collect::<Vec<_>>().join(", ")) },
        if st.note.is_empty() { String::new() } else { format!("; your note then: {}", st.note) }
    ))
}

// ---------------------------------------------------------------------------- commands

fn listed_json(l: &Listed) -> Value {
    json!({
        "name": l.workflow.name, "description": l.workflow.description, "yours": l.yours,
        "stages": l.workflow.stages.iter().map(|x| json!({"number": x.number, "name": x.name})).collect::<Vec<_>>(),
    })
}

fn list_cmd(s: &mut Session, _p: &Value) -> Result<Value> {
    let (all, problems) = library(s);
    Ok(
        json!({"workflows": all.iter().map(listed_json).collect::<Vec<_>>(), "problems": problems, "folder": workflows_dir(s).map(|d| d.to_string_lossy().into_owned())}),
    )
}

fn state_cmd(s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(state_json(s))
}

fn progress_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "workflow.progress";
    let name = match str_p(p, "workflow") {
        Some(n) => n.trim().to_lowercase(),
        None => state(&s.project)
            .map(|st| st.name)
            .ok_or_else(|| bad(cmd, "`workflow` is required when the project is not in one yet (workflow_list names them)"))?,
    };
    let w = find(s, &name).ok_or_else(|| bad(cmd, format!("there is no workflow “{name}” (workflow_list names them)")))?.workflow;
    let key = match p.get("stage") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(t)) => t.clone(),
        _ => return Err(bad(cmd, "`stage` is required: the stage's number or its name")),
    };
    let stage = w
        .stage(&key)
        .ok_or_else(|| {
            bad(
                cmd,
                format!(
                    "“{name}” has no stage “{key}”; its stages are {}",
                    w.stages.iter().map(|x| format!("{} {}", x.number, x.name)).collect::<Vec<_>>().join(", ")
                ),
            )
        })?
        .clone();
    let status = match str_p(p, "status") {
        None => "working",
        Some(x) if STATUSES.contains(&x) => x,
        Some(_) => return Err(bad(cmd, "`status` must be working, waiting, done or blocked")),
    };
    let note: String = match p.get("note") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(t)) => t.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" "),
        Some(_) => return Err(bad(cmd, "`note` must be text")),
    };
    if note.chars().count() > MAX_NOTE {
        return Err(bad(cmd, format!("`note` is longer than {MAX_NOTE} characters; keep it to a sentence")));
    }
    let before = state(&s.project);
    let mut done: Vec<usize> = before.as_ref().filter(|b| b.name == name).map(|b| b.done.clone()).unwrap_or_default();
    if status == "done" && !done.contains(&stage.number) {
        done.push(stage.number);
        done.sort_unstable();
    }
    let label = format!("Workflow: {} ({})", stage.name, status);
    let value = json!({"name": name, "stage": stage.number, "status": status, "note": note, "done": done});
    s.edit(&label, |pr, _| {
        pr.ext.insert(KEY.to_string(), value);
        Ok(())
    })?;
    Ok(state_json(s))
}

fn clear_cmd(s: &mut Session, _p: &Value) -> Result<Value> {
    if state(&s.project).is_none() {
        return Ok(json!({"active": false}));
    }
    s.edit("Clear Workflow", |pr, _| {
        pr.ext.remove(KEY);
        Ok(())
    })?;
    Ok(json!({"active": false}))
}

pub fn commands() -> Vec<CommandSpec> {
    let spec = |id: &'static str, label: &'static str, params: &'static str, run: fn(&mut Session, &Value) -> Result<Value>, journal: bool| CommandSpec {
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
        spec("workflow.list", "Workflows", "{}", list_cmd, false),
        spec("workflow.state", "Workflow State", "{}", state_cmd, false),
        spec(
            "workflow.progress",
            "Record Workflow Progress",
            r#"{"workflow":str?,"stage":number|str,"status":"working|waiting|done|blocked"?,"note":str?}"#,
            progress_cmd,
            true,
        ),
        spec("workflow.clear", "Clear Workflow", "{}", clear_cmd, true),
    ]
}

// ---------------------------------------------------------------------------- tools

fn workflow_list(s: &Session, _p: &Value) -> ToolOutput {
    let (all, problems) = library(s);
    let mut out = String::from("Workflows (guidance for a bigger job; load one with workflow_load to read it):\n");
    for l in &all {
        out.push_str(&format!(
            "- {}{} — {} ({} stages: {})\n",
            l.workflow.name,
            if l.yours { " [the person's own]" } else { "" },
            l.workflow.description,
            l.workflow.stages.len(),
            l.workflow.stages.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    if let Some(line) = context_line(s) {
        out.push_str(&format!("\n{line}\n"));
    }
    if !problems.is_empty() {
        out.push_str(&format!("\nFiles the person wrote that could not be read (tell them if it matters): {}\n", problems.join("; ")));
    }
    ToolOutput::ok(out)
}

fn workflow_load(s: &Session, p: &Value) -> ToolOutput {
    let Some(name) = p.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty() && n.len() <= 80) else {
        return ToolOutput::error("`name` is required: a workflow from workflow_list");
    };
    let Some(l) = find(s, name) else {
        let names: Vec<String> = library(s).0.into_iter().map(|l| l.workflow.name).collect();
        return ToolOutput::error(format!("there is no workflow “{name}”; the workflows are: {}", names.join(", ")));
    };
    let w = &l.workflow;
    let mut out = format!(
        "Workflow “{}”: {}\nStages: {}.\nFollow them in order, tell the person which workflow and stage you are in, and record where you are with workflow_progress (Edit mode) so it survives closing the app.\n",
        w.name,
        w.description,
        w.stages.iter().map(|x| format!("{} {}", x.number, x.name)).collect::<Vec<_>>().join(", ")
    );
    if let Some(line) = context_line(s).filter(|_| state(&s.project).is_some_and(|st| st.name == w.name)) {
        out.push_str(&format!("{line}\n"));
    }
    out.push_str("\n---\n");
    out.push_str(&w.body);
    ToolOutput::ok(out)
}

/// `workflow_progress`, answered on the session thread in Edit mode: it changes the version the assistant has open.
pub(crate) fn progress_tool(s: &mut Session, input: &Value) -> ToolOutput {
    if !s.versions.is_open() {
        return ToolOutput::error("No version is open. Call begin_version first: the workflow's progress is kept in the version, with your work.");
    }
    let params = if input.is_object() { input.clone() } else { json!({}) };
    let run = s.in_version(|s| {
        let previous = std::mem::replace(&mut s.versions.actor, crate::version::Actor::Assistant);
        let r = s.execute("workflow.progress", params);
        s.versions.actor = previous;
        r
    });
    match run {
        Ok(Ok(v)) => ToolOutput::ok(format!(
            "Recorded: “{}”, stage {} of {} (“{}”), {}.",
            v["name"].as_str().unwrap_or(""),
            v["stage"],
            v["stages"],
            v["stageName"].as_str().unwrap_or(""),
            v["status"].as_str().unwrap_or("")
        )),
        Ok(Err(e)) => ToolOutput::error(format!("{e}")),
        Err(e) => ToolOutput::error(e.to_string()),
    }
}

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut progress = def(
        "workflow_progress",
        "Record which workflow you are following and which stage you are at, so the person sees it and it survives closing the app. Call it when you start a stage (status working), when you stop to wait for the person (waiting), when a stage is finished (done) and if you are stuck (blocked). Edit mode only, inside the version you have open.",
        json!({
            "workflow": {"type": "string", "description": "The workflow's name (needed the first time)."},
            "stage": {"type": "string", "description": "The stage's number or its name."},
            "status": {"type": "string", "enum": ["working", "waiting", "done", "blocked"], "description": "Default working."},
            "note": {"type": "string", "description": "One sentence for whoever picks this up: what was decided, what is next."},
        }),
        |_, _| ToolOutput::error("workflow_progress works in Edit mode only"),
    );
    progress.read_only = false;
    vec![
        def(
            "workflow_list",
            "The workflows you can follow for a bigger job (a rough cut from a narration, say): each one's name, what it is for, and its stages. Descriptions only: load one with workflow_load to read it. Use one when the person's request fits it, rather than inventing a procedure.",
            json!({}),
            workflow_list,
        ),
        def(
            "workflow_load",
            "Read a workflow in full: its stages, when to ask the person, and what done means. Load it when you start the job it is for, then follow its stages in order.",
            json!({"name": {"type": "string", "description": "A workflow's name from workflow_list."}}),
            workflow_load,
        ),
        progress,
    ]
}
