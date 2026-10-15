//! What the agent may do in the editor: call tools. The engine implements [`Environment`] over
//! the session; tests use [`FakeEnvironment`].

use edith_llm::{Cancel, ToolSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a tool answered: words, and optionally pictures for a model that can see.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    pub images: Vec<edith_llm::Image>,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false, images: Vec::new() }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true, images: Vec::new() }
    }

    /// Words and a picture.
    pub fn with_image(content: impl Into<String>, image: edith_llm::Image) -> Self {
        Self { content: content.into(), is_error: false, images: vec![image] }
    }
}

/// What a thread may do. The gate is checked here, in the agent, and again by the engine, so a bug
/// in one place cannot grant edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    /// Look and explain. No tool that changes the project is offered or run.
    ReadOnly,
    /// May also propose edits (they change nothing until the person accepts).
    Propose,
}

impl Profile {
    pub fn allows(self, tool_is_read_only: bool) -> bool {
        tool_is_read_only || self == Profile::Propose
    }

    /// One line for the system prompt, so the model knows what it may do.
    pub fn note(self) -> &'static str {
        match self {
            Profile::ReadOnly => "This thread is read-only: you can look at the project but you cannot change it.",
            Profile::Propose => "You may propose edits with the propose tools. Nothing changes until the person accepts a proposal.",
        }
    }
}

/// The tools the agent can call.
pub trait Environment: Send + Sync {
    /// Every tool, with whether it leaves the project unchanged.
    fn tools(&self) -> Vec<(ToolSpec, bool)>;

    /// Runs a tool. May block (the engine hands the call to the session thread and waits); should
    /// return promptly once `cancel` is set. The agent has already checked the profile, but a
    /// real environment checks again.
    fn run(&self, name: &str, input: &Value, cancel: &Cancel) -> ToolOutput;
}

type Handler = Box<dyn Fn(&Value) -> ToolOutput + Send + Sync>;

/// A scripted editor: tools with canned handlers, and a log of what was called.
#[derive(Default)]
pub struct FakeEnvironment {
    tools: Vec<(ToolSpec, bool, Handler)>,
    calls: std::sync::Mutex<Vec<(String, Value)>>,
}

impl FakeEnvironment {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tool(mut self, name: &str, read_only: bool, handler: impl Fn(&Value) -> ToolOutput + Send + Sync + 'static) -> Self {
        let spec = ToolSpec {
            name: name.to_string(),
            description: format!("Test tool {name}."),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
        };
        self.tools.push((spec, read_only, Box::new(handler)));
        self
    }

    /// Every call made so far, in order.
    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

impl Environment for FakeEnvironment {
    fn tools(&self) -> Vec<(ToolSpec, bool)> {
        self.tools.iter().map(|(s, ro, _)| (s.clone(), *ro)).collect()
    }

    fn run(&self, name: &str, input: &Value, _cancel: &Cancel) -> ToolOutput {
        self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((name.to_string(), input.clone()));
        match self.tools.iter().find(|(s, ..)| s.name == name) {
            Some((_, _, h)) => h(input),
            None => ToolOutput::error(format!("unknown tool {name}")),
        }
    }
}
