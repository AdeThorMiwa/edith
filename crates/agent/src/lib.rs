//! The Edith agent.
//!
//! This crate is Edith's craft: the loop that talks to a model and runs tools, the prompt packs,
//! how context is assembled, the budgets, the safety profile, and the records of every run. It
//! talks to a model only through [`edith_llm::ChatModel`] and to the editor only through
//! [`Environment`], so it can be tested with a scripted model and a fake editor, and improved
//! without touching any provider. See `docs/upcoming-features.md` section 4.8.
//!
//! - [`Agent::run_turn`]: one user message in, a finished turn out, with every step reported as an
//!   [`AgentEvent`].
//! - [`Environment`]: the tools the agent may call. The engine implements it.
//! - [`PromptPack`]: the versioned prompt text.
//! - [`Trace`]: a JSON-lines record of a run that replays with no network ([`Trace::replay_model`]).
//!
//! Rules the loop keeps, each with a test: the conversation only grows (thinking blocks stay as the
//! model sent them); every tool call gets an answer, even when a turn ends early, so the thread is
//! always valid to send again; all answers to one reply go in one message; tool results are data,
//! never instructions; a read-only thread cannot run an editing tool.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

mod agent;
mod context;
mod environment;
mod prompt;
mod trace;

pub use agent::{Agent, AgentConfig, AgentEvent, Finish, Thread, TurnReport};
pub use context::{ContextBlock, clip_text, user_message};
pub use environment::{Environment, Profile, ToolOutput};
pub use prompt::PromptPack;
pub use trace::{Trace, TraceMeta};

#[cfg(test)]
mod tests;

/// A fake editor for tests and for the evaluation suite.
pub mod testing {
    pub use crate::environment::FakeEnvironment;
}
