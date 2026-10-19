//! The assistant: threads, the worker that runs the Edith agent, and the commands around them.
//!
//! **Threading.** A turn runs on a worker thread, because a model call takes seconds. The project,
//! though, belongs to the session thread, so the worker's tool calls travel over a channel and are
//! answered by [`Session::pump_assistant`] on the session thread, between frames (the UI calls it
//! every frame; every `assistant.*` command calls it too, so a headless client that polls
//! `assistant.thread` serves the tools as it goes). `assistant.send {wait: true}` pumps until the
//! turn is done.
//!
//! | command | does |
//! |---|---|
//! | `assistant.status` | whether the assistant can run, whether a key is stored, the settings, the threads |
//! | `assistant.settings` | read or change the settings (service, default model, effort, thinking summary, step limit, cost cap) |
//! | `assistant.models` | the provider's models with capability badges and prices (fetched in the background) |
//! | `assistant.newThread` / `assistant.threads` / `assistant.deleteThread` | manage threads |
//! | `assistant.send` | add your message to a thread and run the agent (`wait: true` blocks until it is done) |
//! | `assistant.thread` | a thread's state and the events since an index (the stream the panel draws) |
//! | `assistant.cancel` | Stop the running turn |
//! | `assistant.renameThread` | give a thread a name of your own (the model then no longer renames it) |
//! | `assistant.branch` | a new thread with the conversation up to one of your questions, asking again (retry) or asking something else (edit); the old thread is untouched |
//! | `assistant.trace` | a thread's run as JSON lines (replayable, for evaluation and bug reports) |
//! | `assistant.commands` | the slash commands (`/pacing`, `/glossary term`, …); `assistant.send` expands them |
//!
//! The assistant is read-only: its tools only look at the project (`assistant_tools`). Nothing here
//! stores or returns a provider key; the provider reads it from the keychain per request.

pub mod eval;

/// A fixture session, for tests that check the suite against what the tools say.
#[cfg(test)]
pub(crate) fn eval_fixture_for_tests(name: &str) -> Session {
    eval::fixture_for_tests(name)
}

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};

use edith_ai::{
    AgentEvent, AiSettings, Assistant, Cancel, ChatModel, ContextBlock, Effort, Environment, Finish, ModelInfo, Profile, Thread, ToolOutput, ToolSpec, Trace,
    TraceMeta, Usage, badges, resolve_model, usage_summary,
};

use crate::assistant_store::{self, SavedThread};
use crate::assistant_tools;
use crate::commands::{CommandSpec, always, bad, bool_p, str_p, u64_p};
use crate::{EngineError, Job, Result, Session};

/// Events kept per thread (the oldest are dropped; indexes keep counting).
const MAX_EVENTS: usize = 20_000;
/// Events returned by one `assistant.thread` call.
const EVENTS_PER_CALL: usize = 500;
const MAX_THREADS: usize = 50;
pub(crate) const MAX_TEXT: usize = 20_000;
/// How long the worker waits for the session thread to answer a tool call.
const TOOL_WAIT: Duration = Duration::from_secs(60);

pub(crate) struct ToolRequest {
    /// The thread whose turn made the call (the question cards belong to it).
    pub(crate) thread: u64,
    pub(crate) name: String,
    pub(crate) input: Value,
    pub(crate) reply: SyncSender<ToolOutput>,
}

/// The agent's view of the editor: tools whose calls are answered on the session thread.
struct ChannelEnvironment {
    thread: u64,
    specs: Vec<(ToolSpec, bool)>,
    tx: Mutex<Sender<ToolRequest>>,
}

impl Environment for ChannelEnvironment {
    fn tools(&self) -> Vec<(ToolSpec, bool)> {
        self.specs.clone()
    }

    fn run(&self, name: &str, input: &Value, cancel: &Cancel) -> ToolOutput {
        let (reply, answer) = sync_channel(1);
        let sent = self.tx.lock().unwrap_or_else(PoisonError::into_inner).send(ToolRequest {
            thread: self.thread,
            name: name.to_string(),
            input: input.clone(),
            reply,
        });
        if sent.is_err() {
            return ToolOutput::error("the editor is not available");
        }
        let started = web_time::Instant::now();
        loop {
            match answer.recv_timeout(Duration::from_millis(25)) {
                Ok(out) => return out,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return ToolOutput::error("the editor did not answer"),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if cancel.load(Ordering::SeqCst) {
                        return ToolOutput::error("stopped");
                    }
                    if started.elapsed() > TOOL_WAIT {
                        return ToolOutput::error("the editor did not answer in time");
                    }
                }
            }
        }
    }
}

pub(crate) struct ThreadState {
    thread: Thread,
    pub(crate) events: VecDeque<AgentEvent>,
    /// Events dropped from the front, so indexes stay absolute.
    dropped: u64,
    pub(crate) running: bool,
    finish: Option<Finish>,
    trace: Trace,
    usage: Usage,
    profile: Profile,
    /// The project this conversation belongs to; `None` for one made before conversations belonged to projects.
    pub(crate) project: Option<String>,
}

pub(crate) struct ThreadShared {
    pub(crate) state: Mutex<ThreadState>,
    cancel: Cancel,
    title: Mutex<String>,
    /// The title is still the first question; the model may name the thread after its first answer.
    title_auto: std::sync::atomic::AtomicBool,
}

impl ThreadShared {
    fn new(state: ThreadState, title: &str, auto: bool) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(state),
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            title: Mutex::new(title.chars().take(80).collect()),
            title_auto: std::sync::atomic::AtomicBool::new(auto),
        })
    }

    fn title(&self) -> String {
        lock(&self.title).clone()
    }
}

pub(crate) struct ThreadSlot {
    pub(crate) id: u64,
    pub(crate) shared: Arc<ThreadShared>,
    progress: Option<Arc<filmcraft_export::Progress>>,
}

#[derive(Default)]
struct ModelCache {
    models: Vec<ModelInfo>,
    updating: bool,
    fetched: bool,
    error: Option<String>,
    /// Bumped when the provider changes, so a list still being fetched for the old one is dropped.
    generation: u64,
}

/// Everything the assistant keeps on a session. Not part of the project and not undoable.
pub struct AssistantHub {
    pub(crate) threads: Vec<ThreadSlot>,
    next_id: u64,
    tool_tx: Sender<ToolRequest>,
    tool_rx: Arc<Mutex<Receiver<ToolRequest>>>,
    pub settings: AiSettings,
    dir: Option<PathBuf>,
    /// A note to show about the settings file (it was damaged), once.
    pub settings_note: Option<String>,
    /// A note about saved conversations that could not be restored.
    pub restore_note: Option<String>,
    model_override: Option<Arc<dyn ChatModel>>,
    /// `analysis_run` calls waiting for their job (fork; roadmap F11.2b).
    pub(crate) pending_tools: Vec<crate::analysis_tools::PendingTool>,
    /// The person's choices on the open question cards, not yet sent, and the `media_search` calls waiting for
    /// their results (fork; roadmap F16.1).
    pub(crate) cards: crate::assistant_cards::Cards,
    /// Titles are normally not asked of a plugged-in model (tests script exact replies); this allows it.
    titles_with_override: bool,
    /// A prompt pack to use instead of the built-in one (the evaluation suite compares packs).
    pack_override: Option<edith_ai::PromptPack>,
    models: Arc<Mutex<ModelCache>>,
}

/// A conversation read back from its file, ready to show and to continue.
fn slot_from_saved(rec: SavedThread) -> ThreadSlot {
    let events: VecDeque<AgentEvent> = rec.events.into();
    let mut trace = Trace::new(rec.meta);
    trace.events = events.iter().cloned().collect();
    trace.replies = rec.replies;
    let state = ThreadState {
        thread: rec.thread,
        events,
        dropped: rec.dropped,
        running: false,
        finish: rec.finish,
        trace,
        usage: rec.usage,
        profile: Profile::ReadOnly,
        project: rec.project.filter(|k| valid_key(k)),
    };
    // A restored thread keeps the title it had: it is not renamed again.
    ThreadSlot { id: rec.id, shared: ThreadShared::new(state, &rec.title, false), progress: None }
}

fn saved_from_state(id: u64, title: &str, st: &ThreadState) -> SavedThread {
    SavedThread {
        version: 1,
        id,
        title: title.to_string(),
        thread: st.thread.without_images(),
        events: st.events.iter().cloned().collect(),
        dropped: st.dropped,
        finish: st.finish.clone(),
        usage: st.usage,
        meta: st.trace.meta.clone(),
        replies: st.trace.replies.clone(),
        project: st.project.clone(),
    }
}

// ---------------------------------------------------------------------------------------------
// which project a conversation belongs to (fork roadmap F16.6)
// ---------------------------------------------------------------------------------------------

/// Where a project keeps its identity (`Project::ext`). It is saved in the project file, so the conversations
/// follow the project when the file is moved or renamed, and a copy of the file carries on the same ones.
pub(crate) const PROJECT_KEY: &str = "edith.project";

fn valid_key(k: &str) -> bool {
    (8..=64).contains(&k.len()) && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The identity of a project, if it has one yet (it gets one the first time the assistant is used in it).
pub(crate) fn project_key(p: &filmcraft_project::Project) -> Option<String> {
    p.ext.get(PROJECT_KEY)?.get("id")?.as_str().filter(|k| valid_key(k)).map(str::to_string)
}

fn new_key() -> String {
    use std::hash::{BuildHasher, Hasher};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_i64(crate::autosave::unix_now());
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u32(std::process::id());
    let a = h.finish();
    h.write_u64(a);
    format!("{a:016x}{:016x}", h.finish())
}

impl Session {
    /// The identity of the open project, if the assistant has been used in it: what the Assistant panel watches
    /// to start empty in a new project and to find a project's own conversations again.
    pub fn assistant_scope(&self) -> Option<String> {
        project_key(&self.project)
    }

    /// The identity of the open project, giving it one (and marking it changed, so it is saved) if it has none.
    pub(crate) fn ensure_project_key(&mut self) -> String {
        if let Some(k) = project_key(&self.project) {
            return k;
        }
        let key = new_key();
        Arc::make_mut(&mut self.project).ext.insert(PROJECT_KEY.to_string(), json!({"id": key}));
        self.bump();
        key
    }

    /// After undo, redo or a revert, the project it came from still has its identity: an older snapshot
    /// from before the assistant was first used must not take it away.
    pub(crate) fn carry_project_key(&mut self, from: &filmcraft_project::Project) {
        if project_key(&self.project).is_none()
            && let Some(k) = project_key(from)
        {
            Arc::make_mut(&mut self.project).ext.insert(PROJECT_KEY.to_string(), json!({"id": k}));
        }
    }

    /// Whether the conversation belongs to the open project.
    pub(crate) fn thread_in_scope(&self, t: &ThreadSlot) -> bool {
        project_key(&self.project).is_some_and(|k| lock(&t.shared.state).project.as_deref() == Some(k.as_str()))
    }

    /// The conversations of the open project, oldest first.
    pub(crate) fn scoped_threads(&self) -> impl Iterator<Item = &ThreadSlot> {
        self.assistant.threads.iter().filter(|t| self.thread_in_scope(t))
    }
}

impl Default for AssistantHub {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            threads: Vec::new(),
            next_id: 1,
            tool_tx: tx,
            tool_rx: Arc::new(Mutex::new(rx)),
            settings: AiSettings::default(),
            dir: None,
            settings_note: None,
            restore_note: None,
            model_override: None,
            pending_tools: Vec::new(),
            cards: Default::default(),
            titles_with_override: false,
            pack_override: None,
            models: Arc::new(Mutex::new(ModelCache::default())),
        }
    }
}

impl AssistantHub {
    /// How many `analysis_run` calls are waiting for their job (tests).
    #[cfg(test)]
    pub(crate) fn pending_tools_len(&self) -> usize {
        self.pending_tools.len()
    }

    /// Remembers the data directory and loads the saved settings from it. Hosts call this once.
    pub fn set_dir(&mut self, dir: &Path) {
        let (settings, note) = AiSettings::load(dir);
        self.settings = settings;
        self.settings_note = note;
        self.dir = Some(dir.to_path_buf());
        let (saved, restore_note) = assistant_store::load_all(dir, MAX_THREADS);
        self.next_id = saved.iter().map(|r| r.id).max().map_or(self.next_id.max(1), |m| m + 1);
        self.threads = saved.into_iter().map(slot_from_saved).collect();
        self.restore_note = restore_note;
    }

    /// Uses `model` instead of the provider named in the settings. For tests and tools.
    pub fn use_model(&mut self, model: Arc<dyn ChatModel>) {
        self.model_override = Some(model);
    }

    /// Lets a plugged-in model also be asked for thread titles. For tests.
    pub fn allow_titles_with_test_model(&mut self) {
        self.titles_with_override = true;
    }

    /// Uses `pack` instead of the built-in prompt pack. For the evaluation suite and tools.
    pub fn use_prompt_pack(&mut self, pack: edith_ai::PromptPack) {
        self.pack_override = Some(pack);
    }

    /// The prompt pack in use.
    pub fn pack(&self) -> edith_ai::PromptPack {
        self.pack_override.clone().unwrap_or_else(edith_ai::PromptPack::builtin)
    }

    /// Sets a thread's running flag and dropped-event count directly. For tests.
    #[cfg(test)]
    pub(crate) fn force_state_for_tests(&self, id: u64, running: bool, dropped: u64) {
        if let Some(t) = self.slot(id) {
            let mut st = lock(&t.shared.state);
            st.running = running;
            st.dropped = dropped;
        }
    }

    pub(crate) fn slot(&self, id: u64) -> Option<&ThreadSlot> {
        self.threads.iter().find(|t| t.id == id)
    }
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn push_event(shared: &ThreadShared, ev: AgentEvent) {
    let mut st = lock(&shared.state);
    if let AgentEvent::Usage { total, .. } = &ev {
        st.usage = *total;
    }
    if st.events.len() >= MAX_EVENTS {
        st.events.pop_front();
        st.dropped += 1;
    }
    st.trace.record(&ev);
    st.events.push_back(ev);
}

fn finish_json(f: &Finish) -> Value {
    serde_json::to_value(f).unwrap_or(Value::Null)
}

impl Session {
    /// Answers the tool calls waiting from assistant workers, on this (the session) thread, and
    /// passes a job's Cancel on to its thread. `wait` is how long to wait for the first call.
    pub fn serve_assistant(&mut self, wait: Duration) {
        for t in &self.assistant.threads {
            if t.progress.as_ref().is_some_and(|p| p.cancel.load(Ordering::Relaxed)) {
                t.shared.cancel.store(true, Ordering::SeqCst);
            }
        }
        crate::analysis_tools::finish_pending(self);
        crate::assistant_cards::finish_searches(self);
        crate::assistant_narration::pump(self);
        crate::assistant_look::pump(self);
        let rx = self.assistant.tool_rx.clone();
        let mut first = true;
        loop {
            let req = {
                let guard = lock(&rx);
                if first && !wait.is_zero() { guard.recv_timeout(wait).ok() } else { guard.try_recv().ok() }
            };
            first = false;
            let Some(req) = req else { break };
            if req.name == "analysis_run" {
                crate::analysis_tools::start(self, req);
                continue;
            }
            // the question cards and the media search need the thread and the session's hub
            let req = match crate::assistant_cards::serve(self, req) {
                None => continue,
                Some(req) => req,
            };
            if let Some(out) = crate::assistant_edit::run_edit(self, &req.name, &req.input) {
                let _ = req.reply.send(out);
                continue;
            }
            let out = self.run_assistant_tool(&req.name, &req.input);
            let _ = req.reply.send(out);
        }
    }

    /// [`Session::serve_assistant`] without waiting. The UI calls this every frame.
    pub fn pump_assistant(&mut self) {
        self.serve_assistant(Duration::ZERO);
    }

    /// Runs one tool. The engine checks the read-only flag again here, whatever the agent decided.
    pub(crate) fn run_assistant_tool(&self, name: &str, input: &Value) -> ToolOutput {
        let tools = assistant_tools::tools();
        let Some(def) = tools.iter().find(|d| d.spec.name == name) else { return ToolOutput::error(format!("unknown tool {name}")) };
        if !def.read_only {
            return ToolOutput::error("this assistant may not change the project");
        }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (def.run)(self, input))).unwrap_or_else(|_| ToolOutput::error("the tool failed internally"))
    }

    fn assistant_provider(&mut self) -> Result<Arc<dyn ChatModel>> {
        if let Some(m) = &self.assistant.model_override {
            return Ok(m.clone());
        }
        #[cfg(feature = "net")]
        {
            let service = &self.assistant.settings.service;
            let client = Arc::new(edith_net::http::Client::new(edith_ai::client_config_for(service), edith_net::shared().clone()));
            edith_ai::model_for(service, client).map_err(|e| EngineError::Other(e.to_string()))
        }
        #[cfg(not(feature = "net"))]
        Err(EngineError::Other("the assistant is not available in this build (built without the `net` feature)".into()))
    }

    fn model_info(&self) -> Option<ModelInfo> {
        let cache = lock(&self.assistant.models);
        cache.models.iter().find(|m| m.id == self.assistant.settings.model).cloned()
    }
}

// ---------------------------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------------------------

pub(crate) fn thread_arg(s: &Session, p: &Value, cmd: &str) -> Result<u64> {
    let id = p.get("thread").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "`thread` must be a thread id (a whole number)"))?;
    let slot = s.assistant.slot(id).ok_or_else(|| bad(cmd, format!("there is no thread {id}")))?;
    if !s.thread_in_scope(slot) {
        return Err(bad(cmd, "that conversation belongs to another project (assistant.adoptThread brings one from before projects into this one)"));
    }
    Ok(slot.id)
}

fn thread_summary(t: &ThreadSlot, settings: &AiSettings, price: Option<&edith_ai::Price>) -> Value {
    let st = lock(&t.shared.state);
    json!({"id": t.id, "title": t.shared.title(), "running": st.running, "finish": st.finish.as_ref().map(finish_json), "events": st.dropped + st.events.len() as u64, "usage": st.usage, "usageText": usage_summary(&st.usage, price), "model": settings.model})
}

fn status(s: &mut Session, _p: &Value) -> Result<Value> {
    s.pump_assistant();
    let info = s.model_info();
    let price = info.as_ref().and_then(|i| i.price.clone());
    let available = s.assistant.model_override.is_some() || cfg!(feature = "net");
    let threads: Vec<Value> = s.scoped_threads().map(|t| thread_summary(t, &s.assistant.settings, price.as_ref())).collect();
    let older = older_summaries(s, price.as_ref());
    let service = s.assistant.settings.service.clone();
    let local = edith_ai::endpoints::is_local(&service);
    let connected = if local { None } else { crate::net_commands::connected(&service) };
    // Every provider the assistant can use, with whether it is ready (a key is stored, or it runs
    // on this computer). Nothing is hidden because it is not connected yet.
    let providers: Vec<Value> = edith_ai::endpoints::CHAT_SERVICES
        .iter()
        .map(|id| {
            let label = edith_net::services::find(id).map_or(*id, |sv| sv.label);
            let local = edith_ai::endpoints::is_local(id);
            let connected = if local { None } else { crate::net_commands::connected(id) };
            json!({"id": id, "label": label, "local": local, "connected": connected, "ready": local || connected == Some(true)})
        })
        .collect();
    Ok(json!({
        "available": available,
        "connected": connected,
        "providers": providers,
        // The person must pick a model before asking (none is chosen for them on this provider).
        "needsModel": s.assistant.settings.model.is_empty(),
        // Ready to answer: a provider is plugged in (tests, tools), a key is stored for the
        // service, or it is a model server on this computer.
        "ready": available && (s.assistant.model_override.is_some() || local || connected == Some(true)),
        "settings": settings_json(&s.assistant.settings),
        "settingsNote": s.assistant.settings_note,
        "restoreNote": s.assistant.restore_note,
        "model": info.as_ref().map(model_json),
        "threads": threads,
        // conversations from before they belonged to projects: adopt one to move it into the open project
        "older": older,
        // which project the threads above belong to (null until the assistant is first used in it)
        "scope": {"project": project_key(&s.project)},
        // How the assistant can work (the Mode picker): the same tools, a different way of teaching.
        "modes": [{"id": "ask", "label": "Ask"}, {"id": "coach", "label": "Coach"}, {"id": "edit", "label": "Edit"}],
    }))
}

fn settings_json(st: &AiSettings) -> Value {
    json!({"service": st.service, "model": st.model, "effort": st.effort, "showThinking": st.show_thinking, "autoTitle": st.auto_title, "chips": st.chips, "maxSteps": st.max_steps, "maxCostUsd": st.max_cost_usd, "maxOutputTokens": st.max_output_tokens, "mode": st.mode})
}

fn model_json(m: &ModelInfo) -> Value {
    json!({"id": m.id, "name": m.display_name, "badges": badges(m), "capabilities": m.capabilities, "price": m.price})
}

fn effort_from(v: &Value, cmd: &str) -> Result<Option<Effort>> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) => match s.as_str() {
            "low" => Ok(Some(Effort::Low)),
            "medium" => Ok(Some(Effort::Medium)),
            "high" => Ok(Some(Effort::High)),
            "xhigh" => Ok(Some(Effort::XHigh)),
            "max" => Ok(Some(Effort::Max)),
            other => Err(bad(cmd, format!("effort must be low, medium, high, xhigh, max or null, not {:?}", other.chars().take(20).collect::<String>()))),
        },
        _ => Err(bad(cmd, "effort must be a string or null")),
    }
}

fn settings(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "assistant.settings";
    let mut next = s.assistant.settings.clone();
    let word = |v: &Value, what: &str| -> Result<String> {
        let t = v
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty() && t.len() <= 100 && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-')));
        t.map(str::to_string).ok_or_else(|| bad(cmd, format!("`{what}` must be 1 to 100 letters, digits or . _ : / -")))
    };
    let mut service_changed = false;
    if let Some(v) = p.get("service") {
        let service = word(v, "service")?;
        if !edith_ai::endpoints::is_chat_service(&service) {
            return Err(bad(cmd, format!("`service` must be one of {}", edith_ai::endpoints::CHAT_SERVICES.join(", "))));
        }
        service_changed = service != next.service;
        next.switch_service(&service);
    }
    if let Some(v) = p.get("model") {
        next.model = word(v, "model")?;
    }
    if let Some(v) = p.get("effort") {
        next.effort = effort_from(v, cmd)?;
    }
    if let Some(v) = p.get("mode") {
        next.mode = match v.as_str() {
            Some(m @ ("ask" | "coach" | "edit")) => m.to_string(),
            _ => return Err(bad(cmd, "`mode` must be \"ask\", \"coach\" or \"edit\"")),
        };
    }
    if let Some(v) = p.get("showThinking") {
        next.show_thinking = v.as_bool().ok_or_else(|| bad(cmd, "`showThinking` must be true or false"))?;
    }
    if let Some(v) = p.get("autoTitle") {
        next.auto_title = v.as_bool().ok_or_else(|| bad(cmd, "`autoTitle` must be true or false"))?;
    }
    if let Some(v) = p.get("chips") {
        let o = v.as_object().ok_or_else(|| bad(cmd, "`chips` must be an object such as {\"frame\": false}"))?;
        for (k, on) in o {
            let on = on.as_bool().ok_or_else(|| bad(cmd, format!("`chips.{}` must be true or false", k.chars().take(20).collect::<String>())))?;
            match k.as_str() {
                "selection" => next.chips.selection = on,
                "playhead" => next.chips.playhead = on,
                "transcript" => next.chips.transcript = on,
                "markers" => next.chips.markers = on,
                "frame" => next.chips.frame = on,
                other => {
                    return Err(bad(
                        cmd,
                        format!("there is no chip `{}`; use selection, playhead, transcript, markers or frame", other.chars().take(20).collect::<String>()),
                    ));
                }
            }
        }
    }
    if let Some(v) = p.get("maxSteps") {
        next.max_steps =
            v.as_u64().filter(|n| (1..=40).contains(n)).map(|n| n as u32).ok_or_else(|| bad(cmd, "`maxSteps` must be a whole number from 1 to 40"))?;
    }
    if let Some(v) = p.get("maxOutputTokens") {
        next.max_output_tokens = v
            .as_u64()
            .filter(|n| (256..=64_000).contains(n))
            .map(|n| n as u32)
            .ok_or_else(|| bad(cmd, "`maxOutputTokens` must be a whole number from 256 to 64000"))?;
    }
    if let Some(v) = p.get("maxCostUsd") {
        next.max_cost_usd = match v {
            Value::Null => None,
            v => Some(
                v.as_f64()
                    .filter(|c| c.is_finite() && *c > 0.0 && *c <= 10_000.0)
                    .ok_or_else(|| bad(cmd, "`maxCostUsd` must be a number above 0, or null for no limit"))?,
            ),
        };
    }
    s.assistant.settings = next.sanitized();
    if service_changed {
        // The old provider's models mean nothing to the new one.
        let mut c = lock(&s.assistant.models);
        c.generation = c.generation.wrapping_add(1);
        c.models.clear();
        c.updating = false;
        c.fetched = false;
        c.error = None;
    }
    let save_error = s.assistant.dir.as_deref().and_then(|d| s.assistant.settings.save(d).err());
    Ok(json!({"settings": settings_json(&s.assistant.settings), "saved": s.assistant.dir.is_some() && save_error.is_none(), "saveError": save_error}))
}

/// An error from a provider as one plain sentence. A model server on this computer that does not
/// answer is most likely just not started.
pub(crate) fn friendly(e: &edith_ai::LlmError, local: Option<&'static str>) -> String {
    use edith_net::NetError;
    match (e, local) {
        (edith_ai::LlmError::Net(NetError::Transport(_) | NetError::Timeout), Some(label)) => {
            format!("Could not reach {label}. Is it running? Start it, then refresh the model list.")
        }
        _ => e.to_string(),
    }
}

fn start_model_fetch(s: &mut Session) {
    {
        let mut c = lock(&s.assistant.models);
        if c.updating {
            return;
        }
        c.updating = true;
    }
    let cache = s.assistant.models.clone();
    let generation = lock(&cache).generation;
    let local_label = edith_ai::endpoints::is_local(&s.assistant.settings.service)
        .then(|| edith_net::services::find(&s.assistant.settings.service).map_or("The model server", |sv| sv.label));
    match s.assistant_provider() {
        Err(e) => {
            let mut c = lock(&cache);
            c.updating = false;
            c.fetched = true;
            c.error = Some(e.to_string());
        }
        Ok(model) => {
            let spawned = std::thread::Builder::new().name("assistant-models".into()).spawn({
                let cache = cache.clone();
                move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| model.models()))
                        .unwrap_or_else(|_| Err(edith_ai::LlmError::Unavailable("the model list failed internally".into())));
                    let mut c = lock(&cache);
                    if c.generation != generation {
                        return;
                    }
                    c.updating = false;
                    c.fetched = true;
                    match result {
                        Ok(list) => {
                            c.models = list;
                            c.error = None;
                        }
                        Err(e) => c.error = Some(friendly(&e, local_label)),
                    }
                }
            });
            if spawned.is_err() {
                let mut c = lock(&cache);
                c.updating = false;
                c.error = Some("could not start the model list request".into());
            }
        }
    }
}

fn models(s: &mut Session, p: &Value) -> Result<Value> {
    let refresh = match p.get("refresh") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad("assistant.models", "`refresh` must be true or false")),
    };
    let (fetched, updating) = {
        let c = lock(&s.assistant.models);
        (c.fetched, c.updating)
    };
    if refresh || (!fetched && !updating) {
        start_model_fetch(s);
    }
    let c = lock(&s.assistant.models);
    let service = &s.assistant.settings.service;
    let chosen =
        resolve_model(&c.models, &s.assistant.settings.model, edith_ai::endpoints::usual_models(service), !edith_ai::endpoints::must_choose_model(service))
            .map(|m| m.id.clone());
    Ok(
        json!({"models": c.models.iter().map(model_json).collect::<Vec<_>>(), "updating": c.updating, "error": c.error, "selected": s.assistant.settings.model, "resolved": chosen}),
    )
}

/// The profile for a `mode` setting.
pub(crate) fn profile_of(mode: &str) -> Profile {
    match mode {
        "coach" => Profile::Coach,
        "edit" => Profile::Propose,
        _ => Profile::ReadOnly,
    }
}

fn new_slot(s: &mut Session, title: &str) -> Result<u64> {
    if s.assistant.threads.len() >= MAX_THREADS {
        return Err(EngineError::Other(format!("there are already {MAX_THREADS} threads; delete one first")));
    }
    let id = s.assistant.next_id;
    s.assistant.next_id += 1;
    let meta = TraceMeta {
        prompt_version: s.assistant.pack().version,
        model: s.assistant.settings.model.clone(),
        profile: profile_of(&s.assistant.settings.mode),
        started_unix: crate::autosave::unix_now().max(0) as u64,
    };
    let state = ThreadState {
        thread: Thread::new(),
        events: VecDeque::new(),
        dropped: 0,
        running: false,
        finish: None,
        trace: Trace::new(meta),
        usage: Usage::default(),
        profile: profile_of(&s.assistant.settings.mode),
        project: Some(s.ensure_project_key()),
    };
    s.assistant.threads.push(ThreadSlot { id, shared: ThreadShared::new(state, title, true), progress: None });
    Ok(id)
}

fn new_thread(s: &mut Session, p: &Value) -> Result<Value> {
    let title = match p.get("title") {
        None | Some(Value::Null) => "New thread".to_string(),
        Some(v) => v
            .as_str()
            .filter(|t| !t.trim().is_empty() && t.len() <= 200)
            .ok_or_else(|| bad("assistant.newThread", "`title` must be text up to 200 characters"))?
            .trim()
            .to_string(),
    };
    Ok(json!({"thread": new_slot(s, &title)?}))
}

fn threads(s: &mut Session, _p: &Value) -> Result<Value> {
    s.pump_assistant();
    let price = s.model_info().and_then(|i| i.price);
    Ok(json!({
        "threads": s.scoped_threads().map(|t| thread_summary(t, &s.assistant.settings, price.as_ref())).collect::<Vec<_>>(),
        "older": older_summaries(s, price.as_ref()),
        "scope": {"project": project_key(&s.project)},
    }))
}

/// Conversations made before they belonged to projects, newest last.
fn older_summaries(s: &Session, price: Option<&edith_ai::Price>) -> Vec<Value> {
    s.assistant.threads.iter().filter(|t| lock(&t.shared.state).project.is_none()).map(|t| thread_summary(t, &s.assistant.settings, price)).collect()
}

/// Moves a conversation from before projects into the open project.
fn adopt_thread(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "assistant.adoptThread";
    let id = p.get("thread").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "`thread` must be a thread id (a whole number)"))?;
    let slot = s.assistant.slot(id).ok_or_else(|| bad(cmd, format!("there is no thread {id}")))?;
    if lock(&slot.shared.state).project.is_some() {
        return Err(bad(cmd, "that conversation already belongs to a project"));
    }
    if lock(&slot.shared.state).running {
        return Err(bad(cmd, "the conversation is running; stop it first"));
    }
    let key = s.ensure_project_key();
    let Some(slot) = s.assistant.slot(id) else { return Err(bad(cmd, "no such thread")) };
    lock(&slot.shared.state).project = Some(key.clone());
    let mut saved = true;
    if let Some(dir) = &s.assistant.dir {
        saved = assistant_store::save(dir, &saved_from_state(id, &slot.shared.title(), &lock(&slot.shared.state))).is_ok();
    }
    Ok(json!({"thread": id, "project": key, "saved": saved}))
}

fn rename_thread(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "assistant.renameThread";
    let id = thread_arg(s, p, cmd)?;
    let raw = str_p(p, "title").ok_or_else(|| bad(cmd, "`title` is required"))?;
    let title = raw.trim();
    if title.is_empty() || title.chars().count() > 80 || title.chars().any(char::is_control) {
        return Err(bad(cmd, "`title` must be 1 to 80 characters on one line"));
    }
    let slot = s.assistant.slot(id).ok_or_else(|| bad(cmd, "no such thread"))?;
    *lock(&slot.shared.title) = title.to_string();
    // The person chose this name: the model must not replace it.
    slot.shared.title_auto.store(false, Ordering::SeqCst);
    let mut saved = true;
    if let Some(dir) = &s.assistant.dir {
        let st = lock(&slot.shared.state);
        // A thread that is mid-turn is saved when the turn ends, with this title.
        if !st.running {
            saved = assistant_store::save(dir, &saved_from_state(id, title, &st)).is_ok();
        }
    }
    Ok(json!({"thread": id, "title": title, "saved": saved}))
}

/// The positions of the person's own messages (those with words or a picture; a message holding only
/// tool answers is not a turn).
fn turn_starts(messages: &[edith_ai::Message]) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == edith_ai::Role::User && m.parts.iter().any(|p| matches!(p, edith_ai::Part::Text { .. } | edith_ai::Part::Image { .. })))
        .map(|(i, _)| i)
        .collect()
}

/// `assistant.branch`: a new thread holding a conversation up to the start of one of the person's
/// turns, then asking again (the same question, or `text`). The old thread is left as it was.
fn branch(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "assistant.branch";
    s.pump_assistant();
    let id = thread_arg(s, p, cmd)?;
    let turn = p
        .get("turn")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| bad(cmd, "`turn` must be a whole number (0 is the first question)"))?;
    let (messages, events, replies, original, meta, model_title) = {
        let slot = s.assistant.slot(id).ok_or_else(|| bad(cmd, "no such thread"))?;
        let st = lock(&slot.shared.state);
        if st.running {
            return Err(bad(cmd, "this thread is still answering; wait for it or stop it"));
        }
        if st.dropped > 0 {
            return Err(bad(cmd, "this conversation is too long to branch: its earliest messages are no longer kept"));
        }
        let starts = turn_starts(&st.thread.messages);
        let cut = *starts.get(turn).ok_or_else(|| bad(cmd, format!("there is no turn {turn}; this thread has {}", starts.len())))?;
        let shown: Vec<usize> = st.events.iter().enumerate().filter(|(_, e)| matches!(e, AgentEvent::UserMessage { .. })).map(|(i, _)| i).collect();
        let at = *shown.get(turn).ok_or_else(|| bad(cmd, "that turn's record is missing"))?;
        let original = match st.events.get(at) {
            Some(AgentEvent::UserMessage { text, .. }) => text.clone(),
            _ => String::new(),
        };
        let kept: Vec<edith_ai::Message> = st.thread.messages.iter().take(cut).cloned().collect();
        let replies_kept = kept.iter().filter(|m| m.role == edith_ai::Role::Assistant).count();
        (
            kept,
            st.events.iter().take(at).cloned().collect::<Vec<_>>(),
            st.trace.replies.iter().take(replies_kept).cloned().collect::<Vec<_>>(),
            original,
            st.trace.meta.clone(),
            slot.shared.title(),
        )
    };
    let text = match p.get("text").filter(|v| !v.is_null()) {
        Some(v) => v.as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad(cmd, "`text` must be the question to ask"))?.to_string(),
        None if !original.trim().is_empty() => original,
        None => return Err(bad(cmd, "`text` is required: the original question is not known")),
    };
    let new_id = new_slot(s, &model_title)?;
    if let Some(slot) = s.assistant.slot(new_id) {
        let mut st = lock(&slot.shared.state);
        let mut usage = Usage::default();
        replies.iter().for_each(|r| usage.add(&r.usage));
        st.thread = Thread { messages, usage };
        st.usage = usage;
        let mut trace = Trace::new(meta);
        trace.events = events.clone();
        trace.replies = replies;
        st.trace = trace;
        st.events = events.into();
        // the title was already chosen for this conversation
        slot.shared.title_auto.store(false, Ordering::SeqCst);
    }
    let mut params = json!({"text": text, "thread": new_id});
    for key in ["context", "wait"] {
        if let Some(v) = p.get(key) {
            params[key] = v.clone();
        }
    }
    match send(s, &params) {
        Ok(mut v) => {
            v["branchedFrom"] = json!(id);
            Ok(v)
        }
        Err(e) => {
            // nothing was asked, so no thread is left behind
            s.assistant.threads.retain(|t| t.id != new_id);
            Err(e)
        }
    }
}

fn delete_thread(s: &mut Session, p: &Value) -> Result<Value> {
    let id = thread_arg(s, p, "assistant.deleteThread")?;
    if s.assistant.slot(id).is_some_and(|t| lock(&t.shared.state).running) {
        return Err(bad("assistant.deleteThread", "the thread is running; stop it first"));
    }
    s.assistant.threads.retain(|t| t.id != id);
    if let Some(dir) = &s.assistant.dir {
        assistant_store::remove(dir, id);
    }
    Ok(json!({"deleted": id}))
}

/// The context sent along with a question. Each block is `{label, text}`, or `{kind}` for what the
/// engine knows how to gather itself: `frame` (a picture of the Program monitor at the playhead,
/// only for a model that can see), `transcript` (what is said near the playhead) and `markers`.
fn context_blocks(s: &Session, p: &Value, cmd: &str, can_see: bool) -> Result<Vec<ContextBlock>> {
    let Some(v) = p.get("context").filter(|v| !v.is_null()) else { return Ok(Vec::new()) };
    let arr = v.as_array().ok_or_else(|| bad(cmd, "`context` must be a list of {label, text} or {kind}"))?;
    if arr.len() > 10 {
        return Err(bad(cmd, "at most 10 context blocks"));
    }
    let attached = arr.iter().filter(|b| b.get("kind").and_then(Value::as_str) == Some("item")).count();
    if attached > crate::project_items::MAX_ATTACHED {
        return Err(bad(cmd, format!("at most {} project files can be attached to one message", crate::project_items::MAX_ATTACHED)));
    }
    arr.iter()
        .map(|b| {
            if let Some(kind) = b.get("kind") {
                return match kind.as_str() {
                    Some("frame") if !can_see => {
                        Err(bad(cmd, "the chosen model cannot look at pictures; pick a model with the `images` badge, or turn the Frame chip off"))
                    }
                    Some("frame") => assistant_tools::frame_block(s).map_err(|e| EngineError::Other(format!("could not attach the frame: {e}"))),
                    Some("transcript") => Ok(assistant_tools::transcript_block(s)),
                    Some("markers") => Ok(assistant_tools::markers_block(s)),
                    // a project file the person attached (fork roadmap F16.1c)
                    Some("item") => b
                        .get("item")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| bad(cmd, "an attached file needs `item`: the number of a media item in the project"))
                        .and_then(|id| crate::project_items::item_block(s, id).map_err(|e| bad(cmd, e))),
                    _ => Err(bad(cmd, "a context `kind` must be frame, transcript, markers or item")),
                };
            }
            let label = b.get("label").and_then(Value::as_str).filter(|l| !l.is_empty() && l.chars().count() <= 40);
            let text = b.get("text").and_then(Value::as_str).filter(|t| t.len() <= 100_000);
            match (label, text) {
                (Some(l), Some(t)) => Ok(ContextBlock::text(l, t)),
                _ => Err(bad(cmd, "each context block needs a `label` (up to 40 characters) and `text` (up to 100000 bytes), or a `kind`")),
            }
        })
        .collect()
}

/// A thread title as one short plain line: the first line, without quotes, marks or control characters.
pub(crate) fn clean_title(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let t: String = line.chars().filter(|c| !c.is_control()).collect();
    let t = t.trim_matches(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '“' | '”' | '‘' | '’' | '.' | '*' | '#'));
    let t: String = t.chars().take(60).collect();
    let t = t.trim().to_string();
    (!t.is_empty()).then_some(t)
}

#[cfg(test)]
pub(crate) fn clean_title_for_tests(raw: &str) -> Option<String> {
    clean_title(raw)
}

/// Asks the model to name a conversation from its first question and answer: one small request, no
/// tools. Any failure is just no title.
fn ask_for_title(model: &dyn ChatModel, model_id: &str, question: &str, answer: &str, cancel: &Cancel) -> Option<String> {
    let cut = |t: &str| -> String { t.chars().take(600).collect() };
    let mut req = edith_ai::Request::new(model_id);
    req.system = vec![edith_ai::SystemPart {
        text: "Name this conversation in at most six words. Reply with the title only: no quotes, no full stop.".into(),
        cache: false,
    }];
    req.messages = vec![edith_ai::Message::user_text(format!("Question:\n{}\n\nAnswer:\n{}", cut(question), cut(answer)))];
    req.max_output_tokens = 32;
    let reply = model.stream(&req, cancel, &mut |_| {}).ok()?;
    clean_title(&reply.message.text())
}

fn send(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "assistant.send";
    s.pump_assistant();
    let text = str_p(p, "text").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad(cmd, "`text` is required"))?;
    if text.chars().count() > MAX_TEXT {
        return Err(bad(cmd, format!("`text` is longer than {MAX_TEXT} characters")));
    }
    let wait = match p.get("wait") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(cmd, "`wait` must be true or false")),
    };
    if s.assistant.model_override.is_none() && s.assistant.settings.model.is_empty() {
        return Err(bad(cmd, "no model is chosen yet; pick one from the model list (`assistant.models`, then `assistant.settings` with `model`)"));
    }
    // A model that cannot see pictures is neither offered the frame tool nor sent a frame.
    let can_see = s.model_info().is_none_or(|i| i.capabilities.images);
    let mut ctx = context_blocks(s, p, cmd, can_see)?;
    // A slash command is expanded now, before any thread exists, so a mistyped one leaves nothing behind.
    let ask = match s.assistant.pack().expand(text) {
        Ok(Some(expanded)) => expanded,
        Ok(None) => text.to_string(),
        Err(e) => return Err(bad(cmd, e)),
    };
    let id = match p.get("thread").filter(|v| !v.is_null()) {
        Some(_) => thread_arg(s, p, cmd)?,
        None => new_slot(s, &text.chars().take(60).collect::<String>())?,
    };
    let shared = s.assistant.slot(id).map(|t| t.shared.clone()).ok_or_else(|| bad(cmd, "no such thread"))?;
    if lock(&shared.state).running {
        return Err(bad(cmd, "this thread is still answering; wait for it or stop it"));
    }
    let model = s.assistant_provider()?;
    let info = s.model_info();
    let settings = s.assistant.settings.clone();
    // the mode is read now, so it can change in the middle of a thread
    let profile = profile_of(&settings.mode);
    lock(&shared.state).profile = profile;
    if profile == Profile::Coach
        && let Some(text) = crate::analysis_tools::coach_context(s)
    {
        // what the coach needs to know before it looks: the edit's headline numbers and what the rules found
        ctx.insert(0, ContextBlock::text("coach", text));
    }
    if profile == Profile::Propose {
        // where the version stands, so it knows whether to open one and what the person has done in it
        ctx.insert(0, ContextBlock::text("edit", crate::assistant_edit::context(s)));
    }
    let defs = assistant_tools::tools();
    let env = Arc::new(ChannelEnvironment {
        thread: id,
        specs: defs
            .iter()
            .filter(|d| can_see || !matches!(d.spec.name.as_str(), "frame_view" | "look_see" | "media_look"))
            .map(|d| (d.spec.clone(), d.read_only))
            .collect(),
        tx: Mutex::new(s.assistant.tool_tx.clone()),
    });
    let mut assistant = Assistant::new(model, env, &settings, info.as_ref(), profile);
    assistant.pack = s.assistant.pack();
    let assistant = Arc::new(assistant);

    let job =
        Job { id: s.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1, label: "Assistant".into(), progress: Default::default(), result: Default::default() };
    {
        let mut st = lock(&shared.state);
        st.running = true;
        st.finish = None;
        shared.cancel.store(false, Ordering::SeqCst);
    }
    let (worker_shared, progress, result, user_text, shown_text) = (shared.clone(), job.progress.clone(), job.result.clone(), ask, text.to_string());
    let shown_text_for_title = shown_text.clone();
    let save_dir = s.assistant.dir.clone();
    let title_model = (settings.auto_title && (s.assistant.model_override.is_none() || s.assistant.titles_with_override))
        .then(|| (assistant.model.clone(), settings.model.clone()));
    let run = crate::export_tools::guard_job(job.progress.clone(), job.result.clone(), move || {
        // However this body ends (even in a panic), the thread stops being "running".
        struct Done(Arc<ThreadShared>);
        impl Drop for Done {
            fn drop(&mut self) {
                let mut st = lock(&self.0.state);
                if st.running {
                    st.running = false;
                    st.finish.get_or_insert(Finish::Failed { message: "internal error; the assistant was stopped".into(), retryable: false });
                }
            }
        }
        let _done = Done(worker_shared.clone());
        let started = web_time::Instant::now();
        // Work on a copy of the conversation: if this turn dies, the thread is untouched. While
        // it runs, the shared trace collects only this turn's events; they are joined to the
        // earlier ones at the end.
        let (mut thread, earlier) = {
            let mut st = lock(&worker_shared.state);
            let fresh = Trace::new(st.trace.meta.clone());
            (st.thread.clone(), std::mem::replace(&mut st.trace, fresh))
        };
        let report = assistant.run_turn_as(&mut thread, &ctx, &user_text, &shown_text, &worker_shared.cancel, &mut |ev| push_event(&worker_shared, ev));
        {
            let mut st = lock(&worker_shared.state);
            let this_turn = std::mem::replace(&mut st.trace, earlier);
            st.trace.events.extend(this_turn.events);
            st.trace.finish(&report);
            st.thread = thread;
            st.finish = Some(report.finish.clone());
            // `running` stays true until the title and the saved file are done, so a caller that
            // waits for the turn (`wait: true`) finds the conversation saved and named.
        }
        // After the first answer, the model names the thread (once; a rename by the person stops it).
        if let Some((model, model_id)) = &title_model
            && matches!(report.finish, Finish::Done)
            && worker_shared.title_auto.load(Ordering::SeqCst)
        {
            let (turns, answer) = {
                let st = lock(&worker_shared.state);
                let turns = st
                    .thread
                    .messages
                    .iter()
                    .filter(|m| m.role == edith_ai::Role::User && m.parts.iter().any(|p| matches!(p, edith_ai::Part::Text { .. })))
                    .count();
                (turns, st.thread.messages.iter().rev().find(|m| m.role == edith_ai::Role::Assistant).map(|m| m.text()).unwrap_or_default())
            };
            if turns == 1
                && let Some(t) = ask_for_title(&**model, model_id, &shown_text_for_title, &answer, &worker_shared.cancel)
                && worker_shared.title_auto.swap(false, Ordering::SeqCst)
            {
                *lock(&worker_shared.title) = t;
            }
        }
        // The conversation is saved when a turn ends, never in the middle of one.
        if let Some(dir) = &save_dir {
            let rec = saved_from_state(id, &worker_shared.title(), &lock(&worker_shared.state));
            if let Err(why) = assistant_store::save(dir, &rec) {
                push_event(&worker_shared, AgentEvent::Notice { text: format!("This conversation could not be saved: {why}.") });
            }
        }
        lock(&worker_shared.state).running = false;
        *lock(&progress.status) = format!("{:?}", report.finish);
        if matches!(report.finish, Finish::Failed { .. }) {
            *lock(&progress.error) = Some("the assistant could not finish".into());
        }
        progress.finished.store(true, Ordering::Relaxed);
        *lock(&result) = Some(Ok(filmcraft_export::Report {
            path: String::new(),
            frames: 0,
            seconds: started.elapsed().as_secs_f64(),
            bytes: 0,
            render_fps: 0.0,
            extra_files: Vec::new(),
        }));
    });
    let progress_for_slot = job.progress.clone();
    s.jobs.push(job);
    if let Some(slot) = s.assistant.threads.iter_mut().find(|t| t.id == id) {
        slot.progress = Some(progress_for_slot);
    }
    if let Err(e) = std::thread::Builder::new().name("assistant".into()).spawn(run) {
        lock(&shared.state).running = false;
        return Err(EngineError::Other(format!("could not start the assistant: {e}")));
    }
    if !wait {
        return Ok(json!({"thread": id, "running": true}));
    }
    loop {
        s.serve_assistant(Duration::from_millis(25));
        if !lock(&shared.state).running {
            break;
        }
    }
    let st = lock(&shared.state);
    let last_text = st.thread.messages.iter().rev().find(|m| m.role == edith_ai::Role::Assistant).map(|m| m.text()).unwrap_or_default();
    Ok(
        json!({"thread": id, "running": false, "finish": st.finish.as_ref().map(finish_json), "text": last_text, "usage": st.usage, "usageText": usage_summary(&st.usage, info.as_ref().and_then(|i| i.price.as_ref()))}),
    )
}

fn thread_view(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "assistant.thread";
    s.pump_assistant();
    let id = thread_arg(s, p, cmd)?;
    let since = match p.get("since") {
        None | Some(Value::Null) => 0,
        Some(v) => v.as_u64().ok_or_else(|| bad(cmd, "`since` must be a whole number"))?,
    };
    let price = s.model_info().and_then(|i| i.price);
    let slot = s.assistant.slot(id).ok_or_else(|| bad(cmd, "no such thread"))?;
    let st = lock(&slot.shared.state);
    let start = since.max(st.dropped);
    let skip = usize::try_from(start - st.dropped).unwrap_or(usize::MAX);
    let events: Vec<&AgentEvent> = st.events.iter().skip(skip).take(EVENTS_PER_CALL).collect();
    let next = start + events.len() as u64;
    Ok(json!({
        "thread": id, "title": slot.shared.title(), "running": st.running, "finish": st.finish.as_ref().map(finish_json),
        "first": st.dropped, "next": next, "events": events,
        "usage": st.usage, "usageText": usage_summary(&st.usage, price.as_ref()),
    }))
}

fn cancel(s: &mut Session, p: &Value) -> Result<Value> {
    let id = thread_arg(s, p, "assistant.cancel")?;
    let slot = s.assistant.slot(id).ok_or_else(|| bad("assistant.cancel", "no such thread"))?;
    let running = lock(&slot.shared.state).running;
    if running {
        slot.shared.cancel.store(true, Ordering::SeqCst);
    }
    Ok(json!({"thread": id, "stopping": running}))
}

fn trace(s: &mut Session, p: &Value) -> Result<Value> {
    let id = thread_arg(s, p, "assistant.trace")?;
    let slot = s.assistant.slot(id).ok_or_else(|| bad("assistant.trace", "no such thread"))?;
    let st = lock(&slot.shared.state);
    Ok(json!({"thread": id, "jsonl": st.trace.to_jsonl()}))
}

fn slash_commands(s: &mut Session, _p: &Value) -> Result<Value> {
    let pack = s.assistant.pack();
    Ok(
        json!({"commands": pack.commands.iter().map(|c| json!({"name": c.name, "description": c.description, "takesText": c.template.contains("{args}")})).collect::<Vec<_>>(), "promptVersion": pack.version}),
    )
}

fn spec(id: &'static str, label: &'static str, params: &'static str, run: fn(&mut Session, &Value) -> Result<Value>) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false }
}

pub fn commands() -> Vec<CommandSpec> {
    let _ = (bool_p, u64_p); // (parameter helpers used by siblings; kept imported for the next steps)
    vec![
        spec("assistant.status", "Assistant Status", "{}", status),
        spec(
            "assistant.settings",
            "Assistant Settings",
            r#"{"service":str?,"model":str?,"effort":"low|medium|high|xhigh|max"|null?,"showThinking":bool?,"autoTitle":bool?,"chips":{"selection":bool?,"playhead":bool?,"transcript":bool?,"markers":bool?,"frame":bool?}?,"maxSteps":1..40?,"maxCostUsd":f64|null?,"maxOutputTokens":256..64000?}"#,
            settings,
        ),
        spec("assistant.models", "Assistant Models", r#"{"refresh":bool?}"#, models),
        spec("assistant.newThread", "New Assistant Thread", r#"{"title":str?}"#, new_thread),
        spec("assistant.threads", "Assistant Threads", "{}", threads),
        spec("assistant.adoptThread", "Move an Older Conversation Here", r#"{"thread":id}"#, adopt_thread),
        spec("assistant.renameThread", "Rename Assistant Thread", r#"{"thread":id,"title":str}"#, rename_thread),
        spec("assistant.branch", "Branch Assistant Thread", r#"{"thread":id,"turn":u64,"text":str?,"context":[...]?,"wait":bool?}"#, branch),
        spec("assistant.deleteThread", "Delete Assistant Thread", r#"{"thread":id}"#, delete_thread),
        spec("assistant.send", "Ask the Assistant", r#"{"text":str,"thread":id?,"context":[{"label":str,"text":str}]?,"wait":bool?}"#, send),
        spec("assistant.thread", "Assistant Thread", r#"{"thread":id,"since":u64?}"#, thread_view),
        spec("assistant.cancel", "Stop the Assistant", r#"{"thread":id}"#, cancel),
        spec("assistant.trace", "Assistant Trace", r#"{"thread":id}"#, trace),
        spec("assistant.commands", "Assistant Slash Commands", "{}", slash_commands),
    ]
    .into_iter()
    .chain(eval::commands())
    .chain(crate::assistant_cards::commands())
    .collect()
}
