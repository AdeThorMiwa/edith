//! The Anthropic Messages API (`POST /v1/messages`, `GET /v1/models`).
//!
//! Everything except the HTTPS client is plain code: [`build_body`] makes the request,
//! [`StreamAssembler`] turns the server-sent events into a [`Response`](crate::Response), and
//! [`parse_models`] reads the model list. The client ([`Anthropic`], feature `net`) only moves bytes.

mod models;
mod stream;
mod wire;

#[cfg(feature = "net")]
mod client;

pub use models::{PRICES_AS_OF, parse_models, price_for};
pub use stream::StreamAssembler;
pub use wire::build_body;

#[cfg(test)]
pub(crate) use stream::fixtures as stream_fixtures;

#[cfg(feature = "net")]
pub use client::{Anthropic, KeySource};

/// Tags this provider's opaque parts.
pub const PROVIDER: &str = "anthropic";

/// The API version header value.
pub const API_VERSION: &str = "2023-06-01";
