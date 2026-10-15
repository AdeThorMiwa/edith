//! A record of a run, as JSON lines, that replays with no network. This is the foundation of the
//! evaluation suite: an agent change can be measured against recorded model replies, and a bug
//! report can carry the run that showed it.

use crate::agent::{AgentEvent, TurnReport};
use crate::environment::Profile;
use edith_llm::Response;
use edith_llm::scripted::ScriptedModel;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceMeta {
    pub prompt_version: String,
    pub model: String,
    pub profile: Profile,
    /// Seconds since the Unix epoch when the run began (0 when not known).
    pub started_unix: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Trace {
    pub meta: TraceMeta,
    pub events: Vec<AgentEvent>,
    /// The model's replies, in order.
    pub replies: Vec<Response>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "line", rename_all = "snake_case")]
enum Line {
    Meta(TraceMeta),
    Event(AgentEvent),
    Reply(Response),
}

impl Trace {
    pub fn new(meta: TraceMeta) -> Self {
        Self { meta, events: Vec::new(), replies: Vec::new() }
    }

    /// Notes one event (call it from the turn's sink).
    pub fn record(&mut self, event: &AgentEvent) {
        self.events.push(event.clone());
    }

    /// Keeps the model's replies from a finished turn.
    pub fn finish(&mut self, report: &TurnReport) {
        self.replies.extend(report.replies.iter().cloned());
    }

    pub fn to_jsonl(&self) -> String {
        let mut out = String::new();
        let mut push = |l: &Line| {
            if let Ok(s) = serde_json::to_string(l) {
                out.push_str(&s);
                out.push('\n');
            }
        };
        push(&Line::Meta(self.meta.clone()));
        self.events.iter().for_each(|e| push(&Line::Event(e.clone())));
        self.replies.iter().for_each(|r| push(&Line::Reply(r.clone())));
        out
    }

    /// Reads a trace back. A bad line is an error naming its number (never its content).
    pub fn from_jsonl(text: &str) -> Result<Trace, String> {
        let mut meta = None;
        let (mut events, mut replies) = (Vec::new(), Vec::new());
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Line>(line).map_err(|_| format!("line {} is not a valid trace line", n + 1))? {
                Line::Meta(m) => meta = Some(m),
                Line::Event(e) => events.push(e),
                Line::Reply(r) => replies.push(r),
            }
        }
        Ok(Trace { meta: meta.ok_or("the trace has no meta line")?, events, replies })
    }

    /// A model that answers with the recorded replies, in order. Running the same turn against it
    /// reproduces the recorded thread exactly.
    pub fn replay_model(&self) -> ScriptedModel {
        let m = ScriptedModel::new();
        for r in &self.replies {
            m.push_reply(Vec::new(), r.clone());
        }
        m
    }
}
