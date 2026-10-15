//! The OpenAI-style "chat completions" API, spoken by OpenRouter, OpenAI, Ollama and LM Studio
//! (`POST {base}/chat/completions`, `GET {base}/models`).
//!
//! Like [`crate::anthropic`], everything except the HTTPS client is plain code: [`build_body`]
//! makes the request, [`StreamAssembler`] turns the server-sent chunks into a
//! [`Response`](crate::Response), and [`parse_models`] reads the model list. The servers differ in
//! small ways (the name of the token limit, how reasoning effort is asked for, which headers
//! identify the app); a [`Flavor`] holds exactly those differences.

#[cfg(feature = "net")]
mod client;
mod models;
mod stream;
mod wire;

#[cfg(feature = "net")]
pub use client::OpenAiCompatible;
#[cfg(test)]
pub(crate) use stream::fixtures as stream_fixtures;

pub use models::{LocalModel, parse_lmstudio_models, parse_models, parse_ollama_show};
pub use stream::StreamAssembler;
pub use wire::build_body;

/// How a server wants reasoning effort asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffortStyle {
    /// Not sent: the server has no such setting (or ignores it).
    None,
    /// `"reasoning": {"effort": "medium"}` (OpenRouter, Ollama).
    ReasoningObject,
    /// `"reasoning_effort": "medium"` (OpenAI).
    ReasoningEffort,
}

/// A server's own model list, richer than the OpenAI-style one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeList {
    None,
    /// Ollama: `POST /api/show` for each model says what it can do.
    Ollama,
    /// LM Studio: `GET /api/v0/models` says which are vision models and how long a context they take.
    LmStudio,
}

/// The small differences between servers that speak this API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flavor {
    /// The provider id (`"openrouter"`), which is also the service id.
    pub provider: &'static str,
    /// The request field that limits the reply (`max_tokens`, or `max_completion_tokens` for OpenAI).
    pub max_tokens_field: &'static str,
    pub effort: EffortStyle,
    pub native: NativeList,
}

impl Flavor {
    pub const OPENROUTER: Flavor =
        Flavor { provider: "openrouter", max_tokens_field: "max_tokens", effort: EffortStyle::ReasoningObject, native: NativeList::None };
    pub const OPENAI: Flavor =
        Flavor { provider: "openai", max_tokens_field: "max_completion_tokens", effort: EffortStyle::ReasoningEffort, native: NativeList::None };
    pub const OLLAMA: Flavor = Flavor { provider: "ollama", max_tokens_field: "max_tokens", effort: EffortStyle::None, native: NativeList::Ollama };
    pub const LMSTUDIO: Flavor = Flavor { provider: "lmstudio", max_tokens_field: "max_tokens", effort: EffortStyle::None, native: NativeList::LmStudio };

    /// The flavor for a service id, if it speaks this API.
    pub fn for_service(id: &str) -> Option<Flavor> {
        [Self::OPENROUTER, Self::OPENAI, Self::OLLAMA, Self::LMSTUDIO].into_iter().find(|f| f.provider == id)
    }
}
