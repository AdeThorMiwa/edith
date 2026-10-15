//! Model providers for the Edith agent, behind one narrow interface.
//!
//! This crate knows how to talk to a language model and nothing else: it has no idea what a video
//! editor is. The agent (`edith-agent`) sees only the types here, so a provider can be replaced
//! without touching it, and the agent can be tested with [`scripted::ScriptedModel`] and no network.
//!
//! - [`types`]: [`Message`](types::Message) made of [`Part`](types::Part)s, tool specs, the request,
//!   stream events, usage, stop reasons, and what a model can do ([`Capabilities`](types::Capabilities)).
//! - [`ChatModel`]: the trait every provider implements.
//! - [`anthropic`]: the Anthropic Messages API. The wire format, the stream assembler and the model
//!   list parser are pure code; the HTTPS client is behind feature `net`.
//! - [`scripted`]: a model that replays recorded streams.
//!
//! **Opaque parts.** Some vendors need blocks echoed back unchanged (Anthropic's thinking blocks).
//! They travel as [`Part::Opaque`](types::Part::Opaque), tagged with the provider, are replayed in
//! order, are never read by the agent, and are dropped when the next request goes to a different
//! provider. That keeps a conversation append-only and survivable across a model switch.
//!
//! Design notes and the research behind them: `docs/upcoming-features.md` section 4.8.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub mod anthropic;
mod error;
pub mod scripted;
pub mod types;

pub use error::LlmError;
pub use types::*;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Set to `true` to stop a running request (the Stop button).
pub type Cancel = Arc<AtomicBool>;

/// A chat model provider.
pub trait ChatModel: Send + Sync {
    /// The provider's id, used to tag opaque parts (`"anthropic"`).
    fn provider(&self) -> &str;

    /// The models this provider offers right now, with what each can do.
    fn models(&self) -> Result<Vec<ModelInfo>, LlmError>;

    /// Runs one request, calling `sink` for each piece as it arrives, and returns the whole reply.
    /// The returned [`Response::message`] is what to append to the conversation, unchanged.
    fn stream(&self, request: &Request, cancel: &Cancel, sink: &mut dyn FnMut(StreamEvent)) -> Result<Response, LlmError>;
}
