//! Versions (fork roadmap F14): the mechanism that keeps the person in charge of their edit.
//!
//! A **version** is a branch of the edit that you or the assistant start. While one is open the
//! session works on it: the timeline, the Program monitor and the History panel all show the version,
//! every edit from either of you goes into it, and Undo and Redo work inside it. **Main** (the
//! committed edit) is parked, untouched, until the version is committed or discarded, so nothing can
//! change it in the meantime and there is never anything to merge.
//!
//! | command | does |
//! |---|---|
//! | `version.create {title?, by?}` | start a version of the edit as it is now and check out to it; one at a time |
//! | `version.list` | whether one is open, its title, who started it, whether you are looking at the original, and its steps (each with who made it) |
//! | `version.peek {on?}` | look at the original (main) read-only, and back; edits are refused while you look |
//! | `version.commit` | put the whole version into main as **one undo step** (`Version: <title>`), then work on main again |
//! | `version.discard` | throw the version away and work on main again, exactly as it was |
//! | `version.rename {title}` | rename the open version |
//! | `version.rewind {steps}` | undo the last steps of the version (never further back than where it started) |
//! | `version.run {command, params, by?}` | run any command in the version, even while you are looking at the original, recording who made it (the assistant's way in) |
//!
//! What cannot happen while a version is open: opening, creating, saving or reverting a project, and
//! exporting (the things that reach outside the edit and cannot be taken back with a discard). Saving
//! is refused rather than quietly saving main, so the open version is never lost behind your back.
//! The recovery journal still snapshots what is on screen, so a crash keeps the work in progress.
//! Everything in a version is the ordinary edit machinery: snapshots of the project, the same
//! commands, the same undo.

use std::collections::HashMap;
use std::sync::Arc;

use filmcraft_project::Project;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, str_p, u64_p};
use crate::{EditorState, EngineError, History, Result, Session};

/// The most steps a version keeps for undo.
pub const MAX_STEPS: usize = 100;
const MAX_TITLE: usize = 80;
const DEFAULT_TITLE: &str = "New version";

/// Who made a step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Actor {
    #[default]
    You,
    Assistant,
}

impl Actor {
    pub fn id(self) -> &'static str {
        match self {
            Actor::You => "you",
            Actor::Assistant => "assistant",
        }
    }

    fn parse(s: &str) -> Option<Actor> {
        match s.trim().to_ascii_lowercase().as_str() {
            "you" | "user" | "person" => Some(Actor::You),
            "assistant" | "ai" => Some(Actor::Assistant),
            _ => None,
        }
    }
}

/// One side of the swap: the project, the editor state and the undo history that go together.
struct Saved {
    project: Arc<Project>,
    state: EditorState,
    history: History,
}

struct Open {
    id: u64,
    title: String,
    started_by: Actor,
    started_unix: i64,
    /// Main, parked while the version is open. Its project is where the version started.
    main: Saved,
    /// Whether main had no unsaved changes when the version started (so a discard leaves it clean).
    main_was_clean: bool,
    /// While you look at the original: the version, parked, and main is what the session shows.
    peeking: Option<Saved>,
    /// Who made each step, by the step's snapshot (stable across undo and redo).
    who: HashMap<usize, Actor>,
}

/// What the session keeps about versions.
#[derive(Default)]
pub struct Versions {
    open: Option<Open>,
    next_id: u64,
    /// Who the next edit is from: you, except while the assistant's `version.run` is running.
    pub(crate) actor: Actor,
}

impl Versions {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn is_peeking(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.peeking.is_some())
    }

    pub fn title(&self) -> Option<&str> {
        self.open.as_ref().map(|o| o.title.as_str())
    }
}

/// Commands that reach outside the edit and cannot be taken back by discarding a version.
const DENIED: [&str; 17] = [
    "file.newProject",
    "file.open",
    "file.openDemoProject",
    "file.recover",
    "file.revert",
    "file.save",
    "file.saveAs",
    "file.saveCopy",
    "file.autoSaveNow",
    "file.projectManager",
    "file.exportEdl",
    "file.exportFcp7Xml",
    "file.exportFcpxml",
    "file.exportOtio",
    "export.quick",
    "export.queue.add",
    "export.queue.start",
];

fn clean_title(t: &str) -> String {
    let t: String = t.chars().filter(|c| !c.is_control()).take(MAX_TITLE).collect();
    let t = t.trim();
    if t.is_empty() { DEFAULT_TITLE.to_string() } else { t.to_string() }
}

fn fresh_history() -> History {
    History { limit: MAX_STEPS, ..Default::default() }
}

impl Session {
    /// Why `id` may not run right now because of an open version (`None` when it may).
    pub(crate) fn version_refuses(&self, id: &str) -> Option<String> {
        let open = self.versions.open.as_ref()?;
        if id.starts_with("version.") {
            return None;
        }
        if DENIED.contains(&id) {
            return Some(format!(
                "“{}” is open as a version. Commit it or discard it first: opening, saving and exporting would reach outside the version and could not be taken back.",
                open.title
            ));
        }
        None
    }

    /// The reason an edit is refused while you are looking at the original, if you are.
    pub(crate) fn version_peek_refusal(&self) -> Option<EngineError> {
        self.versions.is_peeking().then(|| EngineError::Other("You are looking at the original. Switch back to the version to edit.".to_string()))
    }

    /// Notes who made every step the history gained since the last look.
    pub(crate) fn version_note_steps(&mut self) {
        let actor = self.versions.actor;
        let Some(open) = self.versions.open.as_mut() else { return };
        for (_, snapshot) in &self.history.undo {
            open.who.entry(Arc::as_ptr(snapshot) as usize).or_insert(actor);
        }
    }

    /// Start a version of the edit as it is, and work on it.
    pub fn version_create(&mut self, title: &str, by: Actor) -> Result<u64> {
        if let Some(o) = &self.versions.open {
            return Err(EngineError::Other(format!("“{}” is already open as a version. Commit it or discard it first: one at a time.", o.title)));
        }
        self.versions.next_id += 1;
        let id = self.versions.next_id;
        let main = Saved { project: self.project.clone(), state: self.state.clone(), history: std::mem::replace(&mut self.history, fresh_history()) };
        self.versions.open = Some(Open {
            id,
            title: clean_title(title),
            started_by: by,
            started_unix: crate::autosave::unix_now(),
            main,
            main_was_clean: !self.is_dirty(),
            peeking: None,
            who: HashMap::new(),
        });
        self.bump_view();
        Ok(id)
    }

    /// Show main (read-only) or come back to the version.
    pub fn version_peek(&mut self, on: bool) -> Result<bool> {
        let open = self.versions.open.as_mut().ok_or_else(|| EngineError::Other("no version is open".into()))?;
        match (on, open.peeking.is_some()) {
            (true, false) => {
                let parked = Saved {
                    project: std::mem::replace(&mut self.project, open.main.project.clone()),
                    state: self.state.clone(),
                    history: std::mem::replace(&mut self.history, fresh_history()),
                };
                open.peeking = Some(parked);
            }
            (false, true) => {
                if let Some(parked) = open.peeking.take() {
                    self.project = parked.project;
                    self.state = parked.state;
                    self.history = parked.history;
                }
            }
            _ => return Ok(open.peeking.is_some()),
        }
        let peeking = open.peeking.is_some();
        self.fix_state();
        self.bump_view();
        Ok(peeking)
    }

    /// Put the whole version into main as one undo step.
    pub fn version_commit(&mut self) -> Result<Value> {
        if self.versions.is_peeking() {
            self.version_peek(false)?;
        }
        let open = self.versions.open.take().ok_or_else(|| EngineError::Other("no version is open".into()))?;
        let steps = self.history.undo.len();
        let by_you = open.who.values().filter(|a| **a == Actor::You).count();
        let by_assistant = open.who.values().filter(|a| **a == Actor::Assistant).count();
        let changed = !Arc::ptr_eq(&self.project, &open.main.project);
        self.history = open.main.history;
        if changed {
            self.history.undo.push((format!("Version: {}", open.title), open.main.project));
            if self.history.undo.len() > self.history.limit {
                self.history.undo.remove(0);
            }
            self.history.redo.clear();
            self.history.merge_key = None;
            self.drop_stale_gap();
            self.bump();
        } else {
            // nothing was changed (or everything was undone): main is as it was
            self.state = open.main.state;
            self.fix_state();
            self.bump_view();
        }
        Ok(json!({"committed": changed, "title": open.title, "steps": steps, "byYou": by_you, "byAssistant": by_assistant}))
    }

    /// Throw the version away: main is back exactly as it was.
    pub fn version_discard(&mut self) -> Result<()> {
        let open = self.versions.open.take().ok_or_else(|| EngineError::Other("no version is open".into()))?;
        self.project = open.main.project;
        self.state = open.main.state;
        self.history = open.main.history;
        self.fix_state();
        if open.main_was_clean {
            self.bump_view();
            self.saved_revision = self.revision;
        } else {
            self.bump();
        }
        Ok(())
    }

    /// Run `f` on the version even while looking at the original, then look at the original again.
    pub(crate) fn in_version<R>(&mut self, f: impl FnOnce(&mut Session) -> R) -> Result<R> {
        if !self.versions.is_open() {
            return Err(EngineError::Other("no version is open: start one with version.create".into()));
        }
        let was_peeking = self.versions.is_peeking();
        if was_peeking {
            self.version_peek(false)?;
        }
        let r = f(self);
        if was_peeking {
            self.version_peek(true)?;
        }
        Ok(r)
    }
}

// ---------------------------------------------------------------------------- commands

fn actor_param(p: &Value, cmd: &str, default: Actor) -> Result<Actor> {
    match p.get("by").filter(|v| !v.is_null()) {
        None => Ok(default),
        Some(v) => v.as_str().and_then(Actor::parse).ok_or_else(|| bad(cmd, "`by` must be \"you\" or \"assistant\"")),
    }
}

fn describe(s: &Session) -> Value {
    let Some(open) = s.versions.open.as_ref() else { return json!({"open": false}) };
    // while looking at the original the version's own history is parked
    let history = open.peeking.as_ref().map_or(&s.history, |p| &p.history);
    let steps: Vec<Value> = history
        .undo
        .iter()
        .map(|(label, snapshot)| json!({"label": label, "by": open.who.get(&(Arc::as_ptr(snapshot) as usize)).copied().unwrap_or_default().id()}))
        .collect();
    json!({
        "open": true, "id": open.id, "title": open.title, "startedBy": open.started_by.id(), "startedUnix": open.started_unix,
        "peeking": open.peeking.is_some(), "steps": steps, "stepCount": history.undo.len(),
        "canUndo": history.can_undo(), "canRedo": history.can_redo(),
    })
}

fn create_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "version.create";
    let by = actor_param(p, cmd, Actor::You)?;
    let title = match p.get("title").filter(|v| !v.is_null()) {
        None => DEFAULT_TITLE.to_string(),
        Some(v) => v.as_str().map(str::to_string).ok_or_else(|| bad(cmd, "`title` must be text"))?,
    };
    s.version_create(&title, by)?;
    Ok(describe(s))
}

fn list_command(s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(describe(s))
}

fn peek_command(s: &mut Session, p: &Value) -> Result<Value> {
    let want = match bool_p(p, "on") {
        Some(on) => on,
        None => !s.versions.is_peeking(),
    };
    s.version_peek(want)?;
    Ok(describe(s))
}

fn commit_command(s: &mut Session, _p: &Value) -> Result<Value> {
    s.version_commit()
}

fn discard_command(s: &mut Session, _p: &Value) -> Result<Value> {
    s.version_discard()?;
    Ok(json!({"open": false}))
}

fn rename_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "version.rename";
    let title = str_p(p, "title").map(clean_title).ok_or_else(|| bad(cmd, "`title` is required"))?;
    let open = s.versions.open.as_mut().ok_or_else(|| bad(cmd, "no version is open"))?;
    open.title = title;
    s.bump_view();
    Ok(describe(s))
}

fn rewind_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "version.rewind";
    let n = u64_p(p, "steps").filter(|n| *n >= 1).ok_or_else(|| bad(cmd, "`steps` must be a whole number, 1 or more"))?;
    if !s.versions.is_open() {
        return Err(bad(cmd, "no version is open"));
    }
    let mut undone = 0u64;
    s.in_version(|s| {
        while undone < n && s.undo().is_some() {
            undone += 1;
        }
    })?;
    Ok(json!({"undone": undone, "version": describe(s)}))
}

fn run_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "version.run";
    let by = actor_param(p, cmd, Actor::Assistant)?;
    let inner =
        str_p(p, "command").filter(|c| !c.is_empty() && c.len() <= 100).ok_or_else(|| bad(cmd, "`command` is required: the id of a command"))?.to_string();
    if inner.starts_with("version.") {
        return Err(bad(cmd, "a version command cannot be run inside a version"));
    }
    let params = p.get("params").cloned().unwrap_or_else(|| json!({}));
    if !params.is_object() {
        return Err(bad(cmd, "`params` must be an object"));
    }
    let previous = s.versions.actor;
    let r = s.in_version(|s| {
        s.versions.actor = by;
        s.execute(&inner, params)
    });
    s.versions.actor = previous;
    r?
}

pub fn commands() -> Vec<CommandSpec> {
    let spec = |id, label, params, run: fn(&mut Session, &Value) -> Result<Value>, journal| CommandSpec {
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
        spec("version.create", "New Version", r#"{"title":str?,"by":"you|assistant"?}"#, create_command, false),
        spec("version.list", "Version", "{}", list_command, false),
        spec("version.peek", "See Original", r#"{"on":bool?}"#, peek_command, false),
        spec("version.commit", "Commit Version", "{}", commit_command, false),
        spec("version.discard", "Discard Version", "{}", discard_command, false),
        spec("version.rename", "Rename Version", r#"{"title":str}"#, rename_command, false),
        spec("version.rewind", "Rewind Version", r#"{"steps":n}"#, rewind_command, false),
        spec("version.run", "Run in Version", r#"{"command":str,"params":object?,"by":"you|assistant"?}"#, run_command, false),
    ]
}
