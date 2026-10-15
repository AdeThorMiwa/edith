//! The one crate the rest of Edith imports for AI.
//!
//! It re-exports everything from [`edith_llm`] (providers) and [`edith_agent`] (the agent), and adds
//! what ties them to the app:
//!
//! - [`AiSettings`]: the user's choices (service, default model, effort, budgets), kept in a
//!   fork-owned file in the data directory, never in upstream's preferences.
//! - [`endpoints`]: where each chat provider lives, and which one needs a key or runs locally.
//! - [`models`]: capability badges and price text for the picker, and choosing the model to use.
//! - [`Assistant`]: a model, the editor's tools and the settings, ready to run turns.
//! - [`usage_summary`]: the thread's cost meter text.
//! - [`model_for`] (feature `net`): the provider for a service, with its key from the keychain.
//!
//! Nothing here is vendor-specific beyond choosing which provider to build; the agent and the
//! interface stay clean. Design: `docs/upcoming-features.md` section 4.8.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

mod assistant;
pub mod endpoints;
pub mod models;
mod settings;

pub use assistant::{Assistant, agent_config, usage_summary};
pub use models::{badges, resolve_model};
pub use settings::{AiSettings, SETTINGS_FILE};

pub use edith_agent as agent;
pub use edith_agent::*;
pub use edith_llm as llm;
pub use edith_llm::*;
pub use edith_net::brand;

#[cfg(feature = "net")]
mod provider;
#[cfg(feature = "net")]
pub use provider::{AiError, client_config_for, model_for};
