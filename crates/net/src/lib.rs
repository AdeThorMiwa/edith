//! The one place Edith talks to the internet and keeps credentials (roadmap F02).
//!
//! - [`NetState`]: the global Offline switch and the request log. Every networked feature goes
//!   through it, so "no silent network calls" holds in exactly one place. [`shared`] is the
//!   process-wide instance; tests build their own.
//! - [`redact`]: URLs and messages with query strings, userinfo and credentials removed. Nothing
//!   that reaches a log, a toast or a command result may skip it.
//! - `http` (feature): the HTTP client (timeouts, retries, per-host rate limits, an ETag cache,
//!   streaming reads).
//! - `secrets` (feature `keychain`): the OS keychain store. Values are never returned to commands.
//! - `oauth` (feature `oauth`): PKCE and the loopback redirect receiver.
//!
//! The network, keychain and OAuth code sits behind features that are off by default and never
//! enabled for wasm (`cargo xtask wasm` checks this crate without them).

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub mod brand;
#[cfg(feature = "http")]
mod cache;
#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "oauth")]
pub mod oauth;
pub mod redact;
#[cfg(feature = "keychain")]
pub mod secrets;
pub mod services;
mod state;
#[cfg(all(any(test, feature = "testserver"), feature = "http"))]
pub mod testserver;
#[cfg(feature = "http")]
mod tls;

pub use state::{LogEntry, NetState, OFFLINE_MARKER, Outcome, shared};

/// Everything that can go wrong in this crate. Messages are safe to show and log: they never
/// contain a secret, a query string or a request body.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum NetError {
    #[error("offline mode is on; turn it off to use the network")]
    Offline,
    #[error("invalid address: {0}")]
    InvalidUrl(String),
    #[error("blocked: {0}")]
    Blocked(String),
    #[error("timed out")]
    Timeout,
    #[error("the server answered {0}")]
    Status(u16),
    #[error("the response is larger than the allowed {0} bytes")]
    TooLarge(u64),
    #[error("network: {0}")]
    Transport(String),
    #[error("keychain: {0}")]
    Secret(String),
    #[error("sign-in: {0}")]
    OAuth(String),
    #[error("stopped")]
    Cancelled,
}
