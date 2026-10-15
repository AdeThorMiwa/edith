//! Building the provider for a service (feature `net`). The key is read from the keychain on every
//! request by the provider itself; nothing here holds it.

use crate::endpoints::{self, Endpoint};
use edith_llm::ChatModel;
use edith_llm::openai::OpenAiCompatible;
use edith_net::http::{Client, ClientConfig};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AiError {
    #[error("`{0}` is not a service Edith can talk to yet")]
    UnknownService(String),
    #[error("{0}")]
    NotAvailable(String),
}

/// The network client settings for `service`: the defaults, plus plain `http` to this computer for
/// a local model server (never for anything else).
pub fn client_config_for(service: &str) -> ClientConfig {
    ClientConfig { allow_loopback_http: endpoints::is_local(service), ..ClientConfig::default() }
}

#[cfg(feature = "keychain")]
fn openai_style(e: &'static Endpoint, client: Arc<Client>) -> Result<Arc<dyn ChatModel>, AiError> {
    let key = e.needs_key.then(|| OpenAiCompatible::key_from_keychain(e.service));
    let mut m = OpenAiCompatible::new(client, e.flavor.clone(), e.base_url, key);
    if e.attribution {
        // The app's name and address, so the provider can tell who is calling.
        m = m.with_header("HTTP-Referer", edith_net::brand::HOME_URL).with_header("X-Title", edith_net::brand::NAME);
    }
    Ok(Arc::new(m))
}

#[cfg(not(feature = "keychain"))]
fn openai_style(e: &'static Endpoint, client: Arc<Client>) -> Result<Arc<dyn ChatModel>, AiError> {
    if e.needs_key {
        let _ = client;
        return Err(AiError::NotAvailable("this build cannot read provider keys from the keychain".to_string()));
    }
    let mut m = OpenAiCompatible::new(client, e.flavor.clone(), e.base_url, None);
    if e.attribution {
        m = m.with_header("HTTP-Referer", edith_net::brand::HOME_URL).with_header("X-Title", edith_net::brand::NAME);
    }
    Ok(Arc::new(m))
}

/// The provider for the service id in the settings. Use a client made from
/// [`client_config_for`] so local servers can be reached.
pub fn model_for(service: &str, client: Arc<Client>) -> Result<Arc<dyn ChatModel>, AiError> {
    if service == "anthropic" {
        #[cfg(feature = "keychain")]
        return Ok(Arc::new(edith_llm::anthropic::Anthropic::from_keychain(client)));
        #[cfg(not(feature = "keychain"))]
        {
            let _ = client;
            return Err(AiError::NotAvailable("this build cannot read provider keys from the keychain".to_string()));
        }
    }
    match endpoints::endpoint(service) {
        Some(e) => openai_style(e, client),
        None => Err(AiError::UnknownService(service.chars().take(40).collect())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edith_net::NetState;

    fn client(service: &str) -> Arc<Client> {
        Arc::new(Client::new(client_config_for(service), Arc::new(NetState::new())))
    }

    #[test]
    fn known_and_unknown_services() {
        for (service, needs_key) in [("anthropic", true), ("openrouter", true), ("openai", true), ("ollama", false), ("lmstudio", false)] {
            let r = model_for(service, client(service));
            if cfg!(feature = "keychain") || !needs_key {
                assert_eq!(r.map(|m| m.provider().to_string()).unwrap_or_default(), service);
            } else {
                assert!(matches!(r, Err(AiError::NotAvailable(_))), "{service}");
            }
        }
        assert!(matches!(model_for("martian-ai", client("x")), Err(AiError::UnknownService(s)) if s == "martian-ai"));
    }

    #[test]
    fn only_local_servers_may_use_plain_http() {
        assert!(client_config_for("ollama").allow_loopback_http && client_config_for("lmstudio").allow_loopback_http);
        for s in ["openrouter", "openai", "anthropic", "nope"] {
            assert!(!client_config_for(s).allow_loopback_http, "{s}");
        }
    }
}
