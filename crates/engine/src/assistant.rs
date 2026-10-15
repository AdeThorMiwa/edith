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
//! | `assistant.trace` | a thread's run as JSON lines (replayable, for evaluation and bug reports) |
//! | `assistant.commands` | the slash commands (`/pacing`, `/glossary term`, …); `assistant.send` expands them |
//!
//! The assistant is read-only: its tools only look at the project (`assistant_tools`). Nothing here
//! stores or returns a provider key; the provider reads it from the keychain per request.

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
    name: String,
    input: Value,
    reply: SyncSender<ToolOutput>,
}

/// The agent's view of the editor: tools whose calls are answered on the session thread.
struct ChannelEnvironment {
    specs: Vec<(ToolSpec, bool)>,
    tx: Mutex<Sender<ToolRequest>>,
}

impl Environment for ChannelEnvironment {
    fn tools(&self) -> Vec<(ToolSpec, bool)> {
        self.specs.clone()
    }

    fn run(&self, name: &str, input: &Value, cancel: &Cancel) -> ToolOutput {
        let (reply, answer) = sync_channel(1);
        let sent = self.tx.lock().unwrap_or_else(PoisonError::into_inner).send(ToolRequest { name: name.to_string(), input: input.clone(), reply });
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

struct ThreadState {
    thread: Thread,
    events: VecDeque<AgentEvent>,
    /// Events dropped from the front, so indexes stay absolute.
    dropped: u64,
    running: bool,
    finish: Option<Finish>,
    trace: Trace,
    usage: Usage,
    profile: Profile,
}

struct ThreadShared {
    state: Mutex<ThreadState>,
    cancel: Cancel,
}

pub(crate) struct ThreadSlot {
    pub(crate) id: u64,
    title: String,
    shared: Arc<ThreadShared>,
    progress: Option<Arc<filmcraft_export::Progress>>,
}

#[derive(Default)]
struct ModelCache {
    models: Vec<ModelInfo>,
    updating: bool,
    fetched: bool,
    error: Option<String>,
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
    model_override: Option<Arc<dyn ChatModel>>,
    models: Arc<Mutex<ModelCache>>,
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
            model_override: None,
            models: Arc::new(Mutex::new(ModelCache::default())),
        }
    }
}

impl AssistantHub {
    /// Remembers the data directory and loads the saved settings from it. Hosts call this once.
    pub fn set_dir(&mut self, dir: &Path) {
        let (settings, note) = AiSettings::load(dir);
        self.settings = settings;
        self.settings_note = note;
        self.dir = Some(dir.to_path_buf());
    }

    /// Uses `model` instead of the provider named in the settings. For tests and tools.
    pub fn use_model(&mut self, model: Arc<dyn ChatModel>) {
        self.model_override = Some(model);
    }

    fn slot(&self, id: u64) -> Option<&ThreadSlot> {
        self.threads.iter().find(|t| t.id == id)
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn push_event(shared: &ThreadShared, ev: AgentEvent) {
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
        let rx = self.assistant.tool_rx.clone();
        let mut first = true;
        loop {
            let req = {
                let guard = lock(&rx);
                if first && !wait.is_zero() { guard.recv_timeout(wait).ok() } else { guard.try_recv().ok() }
            };
            first = false;
            let Some(req) = req else { break };
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
            let client = Arc::new(edith_net::http::Client::new(edith_net::http::ClientConfig::default(), edith_net::shared().clone()));
            edith_ai::model_for(&self.assistant.settings.service, client).map_err(|e| EngineError::Other(e.to_string()))
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

fn thread_arg(s: &Session, p: &Value, cmd: &str) -> Result<u64> {
    let id = p.get("thread").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "`thread` must be a thread id (a whole number)"))?;
    s.assistant.slot(id).map(|t| t.id).ok_or_else(|| bad(cmd, format!("there is no thread {id}")))
}

fn thread_summary(t: &ThreadSlot, settings: &AiSettings, price: Option<&edith_ai::Price>) -> Value {
    let st = lock(&t.shared.state);
    json!({"id": t.id, "title": t.title, "running": st.running, "finish": st.finish.as_ref().map(finish_json), "events": st.dropped + st.events.len() as u64, "usage": st.usage, "usageText": usage_summary(&st.usage, price), "model": settings.model})
}

fn status(s: &mut Session, _p: &Value) -> Result<Value> {
    s.pump_assistant();
    let info = s.model_info();
    let price = info.as_ref().and_then(|i| i.price.clone());
    let available = s.assistant.model_override.is_some() || cfg!(feature = "net");
    let threads: Vec<Value> = s.assistant.threads.iter().map(|t| thread_summary(t, &s.assistant.settings, price.as_ref())).collect();
    let connected = crate::net_commands::connected(&s.assistant.settings.service);
    Ok(json!({
        "available": available,
        "connected": connected,
        // Ready to answer: a provider is plugged in (tests, tools) or a key is stored for the service.
        "ready": available && (s.assistant.model_override.is_some() || connected == Some(true)),
        "settings": settings_json(&s.assistant.settings),
        "settingsNote": s.assistant.settings_note,
        "model": info.as_ref().map(model_json),
        "threads": threads,
    }))
}

fn settings_json(st: &AiSettings) -> Value {
    json!({"service": st.service, "model": st.model, "effort": st.effort, "showThinking": st.show_thinking, "maxSteps": st.max_steps, "maxCostUsd": st.max_cost_usd, "maxOutputTokens": st.max_output_tokens})
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
    if let Some(v) = p.get("service") {
        next.service = word(v, "service")?;
    }
    if let Some(v) = p.get("model") {
        next.model = word(v, "model")?;
    }
    if let Some(v) = p.get("effort") {
        next.effort = effort_from(v, cmd)?;
    }
    if let Some(v) = p.get("showThinking") {
        next.show_thinking = v.as_bool().ok_or_else(|| bad(cmd, "`showThinking` must be true or false"))?;
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
    let save_error = s.assistant.dir.as_deref().and_then(|d| s.assistant.settings.save(d).err());
    Ok(json!({"settings": settings_json(&s.assistant.settings), "saved": s.assistant.dir.is_some() && save_error.is_none(), "saveError": save_error}))
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
                    c.updating = false;
                    c.fetched = true;
                    match result {
                        Ok(list) => {
                            c.models = list;
                            c.error = None;
                        }
                        Err(e) => c.error = Some(e.to_string()),
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
    let chosen = resolve_model(&c.models, &s.assistant.settings.model).map(|m| m.id.clone());
    Ok(
        json!({"models": c.models.iter().map(model_json).collect::<Vec<_>>(), "updating": c.updating, "error": c.error, "selected": s.assistant.settings.model, "resolved": chosen}),
    )
}

fn new_slot(s: &mut Session, title: &str) -> Result<u64> {
    if s.assistant.threads.len() >= MAX_THREADS {
        return Err(EngineError::Other(format!("there are already {MAX_THREADS} threads; delete one first")));
    }
    let id = s.assistant.next_id;
    s.assistant.next_id += 1;
    let meta = TraceMeta {
        prompt_version: edith_ai::PromptPack::builtin().version,
        model: s.assistant.settings.model.clone(),
        profile: Profile::ReadOnly,
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
        profile: Profile::ReadOnly,
    };
    s.assistant.threads.push(ThreadSlot {
        id,
        title: title.chars().take(80).collect(),
        shared: Arc::new(ThreadShared { state: Mutex::new(state), cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)) }),
        progress: None,
    });
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
    Ok(json!({"threads": s.assistant.threads.iter().map(|t| thread_summary(t, &s.assistant.settings, price.as_ref())).collect::<Vec<_>>()}))
}

fn delete_thread(s: &mut Session, p: &Value) -> Result<Value> {
    let id = thread_arg(s, p, "assistant.deleteThread")?;
    if s.assistant.slot(id).is_some_and(|t| lock(&t.shared.state).running) {
        return Err(bad("assistant.deleteThread", "the thread is running; stop it first"));
    }
    s.assistant.threads.retain(|t| t.id != id);
    Ok(json!({"deleted": id}))
}

fn context_blocks(p: &Value, cmd: &str) -> Result<Vec<ContextBlock>> {
    let Some(v) = p.get("context").filter(|v| !v.is_null()) else { return Ok(Vec::new()) };
    let arr = v.as_array().ok_or_else(|| bad(cmd, "`context` must be a list of {label, text}"))?;
    if arr.len() > 10 {
        return Err(bad(cmd, "at most 10 context blocks"));
    }
    arr.iter()
        .map(|b| {
            let label = b.get("label").and_then(Value::as_str).filter(|l| !l.is_empty() && l.chars().count() <= 40);
            let text = b.get("text").and_then(Value::as_str).filter(|t| t.len() <= 100_000);
            match (label, text) {
                (Some(l), Some(t)) => Ok(ContextBlock { label: l.to_string(), text: t.to_string() }),
                _ => Err(bad(cmd, "each context block needs a `label` (up to 40 characters) and `text` (up to 100000 bytes)")),
            }
        })
        .collect()
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
    let ctx = context_blocks(p, cmd)?;
    // A slash command is expanded now, before any thread exists, so a mistyped one leaves nothing behind.
    let ask = match edith_ai::PromptPack::builtin().expand(text) {
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
    let profile = lock(&shared.state).profile;
    let defs = assistant_tools::tools();
    let env = Arc::new(ChannelEnvironment { specs: defs.iter().map(|d| (d.spec.clone(), d.read_only)).collect(), tx: Mutex::new(s.assistant.tool_tx.clone()) });
    let assistant = Arc::new(Assistant::new(model, env, &settings, info.as_ref(), profile));

    let job =
        Job { id: s.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1, label: "Assistant".into(), progress: Default::default(), result: Default::default() };
    {
        let mut st = lock(&shared.state);
        st.running = true;
        st.finish = None;
        shared.cancel.store(false, Ordering::SeqCst);
    }
    let (worker_shared, progress, result, user_text, shown_text) = (shared.clone(), job.progress.clone(), job.result.clone(), ask, text.to_string());
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
            st.running = false;
        }
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
        "thread": id, "title": slot.title, "running": st.running, "finish": st.finish.as_ref().map(finish_json),
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

fn slash_commands(_s: &mut Session, _p: &Value) -> Result<Value> {
    let pack = edith_ai::PromptPack::builtin();
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
            r#"{"service":str?,"model":str?,"effort":"low|medium|high|xhigh|max"|null?,"showThinking":bool?,"maxSteps":1..40?,"maxCostUsd":f64|null?,"maxOutputTokens":256..64000?}"#,
            settings,
        ),
        spec("assistant.models", "Assistant Models", r#"{"refresh":bool?}"#, models),
        spec("assistant.newThread", "New Assistant Thread", r#"{"title":str?}"#, new_thread),
        spec("assistant.threads", "Assistant Threads", "{}", threads),
        spec("assistant.deleteThread", "Delete Assistant Thread", r#"{"thread":id}"#, delete_thread),
        spec("assistant.send", "Ask the Assistant", r#"{"text":str,"thread":id?,"context":[{"label":str,"text":str}]?,"wait":bool?}"#, send),
        spec("assistant.thread", "Assistant Thread", r#"{"thread":id,"since":u64?}"#, thread_view),
        spec("assistant.cancel", "Stop the Assistant", r#"{"thread":id}"#, cancel),
        spec("assistant.trace", "Assistant Trace", r#"{"thread":id}"#, trace),
        spec("assistant.commands", "Assistant Slash Commands", "{}", slash_commands),
    ]
}
