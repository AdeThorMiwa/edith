//! The services FilmCraft knows how to connect to, and the keychain slot each one uses. The list
//! feeds `connections.list` and Settings ▸ Connections. Later features (the assistant, the Source
//! Hub) add their entries here, so there is one place that says what can hold a credential.

use serde::Serialize;

/// How the user connects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Sign in through the provider's own page (OAuth with PKCE); FilmCraft keeps the key it returns.
    SignIn,
    /// Paste a key from the provider's account page.
    ApiKey,
    /// Runs on this computer; nothing to store.
    Local,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    /// The keychain name holding the key or token (`None` for local services).
    pub secret: Option<&'static str>,
    /// The provider's page where a key is created.
    pub key_url: Option<&'static str>,
    /// Has a free tier that works without paying.
    pub free: bool,
    pub note: &'static str,
}

pub const SERVICES: &[Service] = &[
    Service {
        id: "openrouter",
        label: "OpenRouter",
        kind: Kind::SignIn,
        secret: Some("provider.openrouter.api_key"),
        key_url: Some("https://openrouter.ai/keys"),
        free: true,
        note: "One sign-in reaches many models, including free ones.",
    },
    Service {
        id: "anthropic",
        label: "Anthropic (Claude API)",
        kind: Kind::ApiKey,
        secret: Some("provider.anthropic.api_key"),
        key_url: Some("https://console.anthropic.com/settings/keys"),
        free: false,
        note: "Pay as you go. A Claude subscription cannot be used here; it signs in through Claude Code.",
    },
    Service {
        id: "openai",
        label: "OpenAI",
        kind: Kind::ApiKey,
        secret: Some("provider.openai.api_key"),
        key_url: Some("https://platform.openai.com/api-keys"),
        free: false,
        note: "Pay as you go.",
    },
    Service {
        id: "gemini",
        label: "Google Gemini",
        kind: Kind::ApiKey,
        secret: Some("provider.gemini.api_key"),
        key_url: Some("https://aistudio.google.com/apikey"),
        free: true,
        note: "Has a free tier with daily limits.",
    },
    Service {
        id: "ollama",
        label: "Ollama (on this computer)",
        kind: Kind::Local,
        secret: None,
        key_url: None,
        free: true,
        note: "Runs models locally; nothing leaves this computer.",
    },
    Service {
        id: "pexels",
        label: "Pexels",
        kind: Kind::ApiKey,
        secret: Some("source.pexels.api_key"),
        key_url: Some("https://www.pexels.com/api/"),
        free: true,
        note: "Free stock video and photos.",
    },
    Service {
        id: "pixabay",
        label: "Pixabay",
        kind: Kind::ApiKey,
        secret: Some("source.pixabay.api_key"),
        key_url: Some("https://pixabay.com/api/docs/"),
        free: true,
        note: "Free stock video and photos.",
    },
];

pub fn find(id: &str) -> Option<&'static Service> {
    SERVICES.iter().find(|s| s.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_secret_names_are_valid() {
        for (i, s) in SERVICES.iter().enumerate() {
            assert!(SERVICES.iter().skip(i + 1).all(|o| o.id != s.id), "{}", s.id);
            assert_eq!(s.secret.is_none(), s.kind == Kind::Local, "{}", s.id);
            if let Some(name) = s.secret {
                assert!(name.len() <= 64 && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')), "{name}");
            }
            if let Some(u) = s.key_url {
                assert!(u.starts_with("https://"), "{u}");
            }
        }
        assert!(find("openrouter").is_some() && find("nope").is_none());
    }
}
