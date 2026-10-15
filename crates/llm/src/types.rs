//! The neutral types every provider translates to and from.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

/// One piece of a message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text {
        text: String,
    },
    /// The model asks to run a tool. `input` is always a JSON object (`{}` when the model's input
    /// could not be parsed, in which case `input_error` says why and the tool must not run).
    ToolUse {
        id: String,
        name: String,
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_error: Option<String>,
    },
    /// The answer to a [`Part::ToolUse`], sent back in a user message. All answers to one reply
    /// go in the **same** message.
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
    /// A provider-specific block that must be sent back unchanged. Never read by the agent.
    Opaque {
        provider: String,
        value: Value,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Self { role: Role::User, parts: vec![Part::Text { text: text.into() }] }
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self { role: Role::Assistant, parts: vec![Part::Text { text: text.into() }] }
    }

    /// A user message holding the answers to a reply's tool calls.
    pub fn tool_results(results: Vec<Part>) -> Self {
        Self { role: Role::User, parts: results }
    }

    /// The text parts joined together.
    pub fn text(&self) -> String {
        self.parts.iter().filter_map(|p| if let Part::Text { text } = p { Some(text.as_str()) } else { None }).collect::<Vec<_>>().join("")
    }

    /// The tool calls in this message, in order.
    pub fn tool_uses(&self) -> impl Iterator<Item = &Part> {
        self.parts.iter().filter(|p| matches!(p, Part::ToolUse { .. }))
    }
}

/// A tool the model may call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema of the input object.
    pub input_schema: Value,
}

/// A piece of the system prompt. Parts marked `cache` end the cacheable prefix (tools, then the
/// system prompt, up to and including the last marked part).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SystemPart {
    pub text: String,
    pub cache: bool,
}

/// How hard the model should think and work, where the provider supports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::XHigh => "xhigh",
            Effort::Max => "max",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub model: String,
    pub system: Vec<SystemPart>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub effort: Option<Effort>,
    /// Ask for a readable summary of the model's reasoning, where supported.
    pub thinking_summary: bool,
    pub max_output_tokens: u32,
}

impl Request {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            system: Vec::new(),
            messages: Vec::new(),
            tools: Vec::new(),
            effort: None,
            thinking_summary: false,
            max_output_tokens: 8192,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    /// The reply was cut off: a tool call in it may be incomplete and must not run.
    MaxTokens,
    StopSequence,
    /// A server-side tool hit its limit; send the reply back to continue.
    PauseTurn,
    /// The provider declined to answer. A tool call in the reply must not run.
    Refusal,
    Other(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self.cache_read_tokens.saturating_add(other.cache_read_tokens);
        self.cache_write_tokens = self.cache_write_tokens.saturating_add(other.cache_write_tokens);
    }
}

/// What the agent is told as a reply arrives.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    Text(String),
    /// A piece of the model's reasoning summary.
    Thinking(String),
    ToolUseStarted {
        id: String,
        name: String,
    },
    ToolUseFinished {
        id: String,
    },
    /// Usage so far (cumulative for this reply).
    Usage(Usage),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub message: Message,
    pub stop_reason: StopReason,
    pub usage: Usage,
    /// The model that actually answered.
    pub model: String,
}

/// What a model can do. Shown as badges; nothing is hidden because a model lacks a capability.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub tools: bool,
    pub parallel_tools: bool,
    pub images: bool,
    pub thinking: bool,
    pub effort: bool,
    pub caching: bool,
    pub context_tokens: u64,
    pub max_output_tokens: u64,
}

/// Prices in US dollars per million tokens, as an estimate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: Option<f64>,
    /// Where the numbers come from and when (shown with the estimate).
    pub source: String,
}

impl Price {
    /// Estimated cost of `usage` in US dollars. Cache writes are counted at the input price.
    pub fn estimate(&self, usage: &Usage) -> f64 {
        let m = 1_000_000.0;
        let read_price = self.cache_read_per_mtok.unwrap_or(self.input_per_mtok);
        (usage.input_tokens as f64 * self.input_per_mtok
            + usage.cache_write_tokens as f64 * self.input_per_mtok
            + usage.cache_read_tokens as f64 * read_price
            + usage.output_tokens as f64 * self.output_per_mtok)
            / m
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub capabilities: Capabilities,
    pub price: Option<Price>,
}
