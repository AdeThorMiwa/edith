//! Media sources for Edith (fork roadmap F05): ask free stock sites for video, pictures and sound
//! through one request shape, get one candidate shape back with the site's licence read into what it
//! allows, and (feature `net`) fetch a chosen file.
//!
//! - [`types`]: [`Request`], [`Candidate`], [`Rights`], [`Page`], [`SourceError`].
//! - [`query`]: turns what a person typed into the words a site searches for.
//! - [`pexels`], [`pixabay`], [`freesound`]: each site's address building and answer reading. Plain
//!   code, tested against recorded answers; every field read from an answer is treated as hostile
//!   (cut to length, addresses must be `http(s)`).
//! - `client` (feature `net`): the HTTPS side through `edith-net`, so Offline mode, the request log
//!   and per-site rate limits apply.
//!
//! The licence a candidate carries is what the **site says**. It is recorded as that and no more.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub mod freesound;
pub mod pexels;
pub mod pixabay;
pub mod query;
pub mod types;
mod util;

pub use types::{Candidate, Download, Kind, Orientation, Page, Request, Rights, SourceError};
pub use util::apply_request;
