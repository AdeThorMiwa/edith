//! Building the provider for a service (feature `net`). The key is read from the keychain on every
//! request by the provider itself; nothing here holds it.

use edith_llm::ChatModel;
use edith_net::http::Client;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AiError {
    #[error("`{0}` is not a service Edith can talk to yet")]
    UnknownService(String),
    #[error("{0}")]
    NotAvailable(String),
}

/// The provider for the service id in the settings (`anthropic` today).
pub fn model_for(service: &str, client: Arc<Client>) -> Result<Arc<dyn ChatModel>, AiError> {
    match service {
        #[cfg(feature = "keychain")]
        "anthropic" => Ok(Arc::new(edith_llm::anthropic::Anthropic::from_keychain(client))),
        #[cfg(not(feature = "keychain"))]
        "anthropic" => {
            let _ = client;
            Err(AiError::NotAvailable("this build cannot read provider keys from the keychain".to_string()))
        }
        other => Err(AiError::UnknownService(other.chars().take(40).collect())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edith_net::NetState;
    use edith_net::http::ClientConfig;

    #[test]
    fn known_and_unknown_services() {
        let client = Arc::new(Client::new(ClientConfig::default(), Arc::new(NetState::new())));
        let r = model_for("anthropic", client.clone());
        if cfg!(feature = "keychain") {
            assert_eq!(r.map(|m| m.provider().to_string()).unwrap_or_default(), "anthropic");
        } else {
            assert!(matches!(r, Err(AiError::NotAvailable(_))));
        }
        assert!(matches!(model_for("martian-ai", client), Err(AiError::UnknownService(s)) if s == "martian-ai"));
    }
}
