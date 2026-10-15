//! The HTTPS agent. The TLS provider is chosen in this one function, so swapping it (the RustCrypto
//! provider is still alpha) touches nothing else.

use std::sync::Arc;
use std::time::Duration;

/// An agent with pure-Rust TLS, OS certificate verification and no proxy from the environment.
///
/// Never use `ureq::get` or `Agent::new_with_defaults`: with this feature set their default
/// certificate store and crypto provider are absent and the first handshake panics.
pub fn agent(user_agent: &str, connect: Duration, global: Duration, https_only: bool, max_redirects: u32) -> ureq::Agent {
    let provider = Arc::new(rustls_rustcrypto::provider());
    ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::Rustls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .unversioned_rustls_crypto_provider(provider)
                .build(),
        )
        .user_agent(user_agent)
        .timeout_connect(Some(connect))
        .timeout_global(Some(global))
        .https_only(https_only)
        .max_redirects(max_redirects)
        .max_response_header_size(32 * 1024)
        .http_status_as_error(false)
        .proxy(None)
        .build()
        .into()
}
