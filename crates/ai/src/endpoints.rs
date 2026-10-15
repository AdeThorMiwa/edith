//! Where each chat provider lives and how it differs. Pure data, so the engine, the tests and the
//! panels can ask about a provider without building one.

use edith_llm::openai::Flavor;

/// A provider that speaks the OpenAI-style chat API.
pub struct Endpoint {
    /// The service id (`openrouter`), the same as in `edith_net::services`.
    pub service: &'static str,
    pub base_url: &'static str,
    pub flavor: Flavor,
    /// A key is sent (and must be stored); local servers need none.
    pub needs_key: bool,
    /// Runs on this computer, so plain `http` to the loopback address is allowed.
    pub local: bool,
    /// Tell the server which app is calling (OpenRouter shows it in its app rankings).
    pub attribution: bool,
}

pub const ENDPOINTS: &[Endpoint] = &[
    Endpoint { service: "openrouter", base_url: "https://openrouter.ai/api/v1", flavor: Flavor::OPENROUTER, needs_key: true, local: false, attribution: true },
    Endpoint { service: "openai", base_url: "https://api.openai.com/v1", flavor: Flavor::OPENAI, needs_key: true, local: false, attribution: false },
    Endpoint { service: "ollama", base_url: "http://127.0.0.1:11434/v1", flavor: Flavor::OLLAMA, needs_key: false, local: true, attribution: false },
    Endpoint { service: "lmstudio", base_url: "http://127.0.0.1:1234/v1", flavor: Flavor::LMSTUDIO, needs_key: false, local: true, attribution: false },
];

/// The OpenAI-style endpoint for a service id.
pub fn endpoint(service: &str) -> Option<&'static Endpoint> {
    ENDPOINTS.iter().find(|e| e.service == service)
}

/// Every service the assistant can talk to, in the order the picker lists them.
pub const CHAT_SERVICES: &[&str] = &["anthropic", "openrouter", "openai", "ollama", "lmstudio"];

pub fn is_chat_service(service: &str) -> bool {
    CHAT_SERVICES.contains(&service)
}

/// Runs on this computer.
pub fn is_local(service: &str) -> bool {
    endpoint(service).is_some_and(|e| e.local)
}

/// The model a new setup starts with, or `""` when the person should choose: Anthropic has a few
/// models and a cheap sensible one, while the others list hundreds (OpenRouter) or whatever is
/// installed (local), and picking one for them could mean paying for something they did not choose.
pub fn default_model(service: &str) -> &'static str {
    if service == "anthropic" { "claude-haiku-5-5" } else { "" }
}

/// Models to prefer, cheap to capable, when the chosen one is not offered.
pub fn usual_models(service: &str) -> &'static [&'static str] {
    if service == "anthropic" { &["claude-haiku-5-5", "claude-sonnet-5-5", "claude-opus-5-5"] } else { &[] }
}

/// The person must pick a model; none is chosen for them.
pub fn must_choose_model(service: &str) -> bool {
    service != "anthropic"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_endpoint_is_a_known_service_with_a_matching_flavor() {
        for e in ENDPOINTS {
            let s = edith_net::services::find(e.service).unwrap_or_else(|| panic!("{} is not in the services list", e.service));
            assert_eq!(s.secret.is_some(), e.needs_key, "{}", e.service);
            assert_eq!(e.flavor.provider, e.service);
            assert!(CHAT_SERVICES.contains(&e.service));
            assert!(if e.local { e.base_url.starts_with("http://127.0.0.1") } else { e.base_url.starts_with("https://") }, "{}", e.base_url);
            assert!(!e.base_url.ends_with('/'));
        }
        assert!(CHAT_SERVICES.iter().all(|s| edith_net::services::find(s).is_some()));
    }

    #[test]
    fn only_openrouter_is_told_the_app_name_and_only_local_servers_are_local() {
        let named: Vec<_> = ENDPOINTS.iter().filter(|e| e.attribution).map(|e| e.service).collect();
        assert_eq!(named, ["openrouter"]);
        assert!(is_local("ollama") && is_local("lmstudio") && !is_local("openrouter") && !is_local("anthropic"));
    }

    #[test]
    fn nobody_picks_a_model_for_the_person_except_the_cheap_claude_default() {
        assert_eq!(default_model("anthropic"), "claude-haiku-5-5");
        assert!(!must_choose_model("anthropic") && must_choose_model("openrouter") && must_choose_model("ollama"));
        assert_eq!(default_model("openrouter"), "");
        assert!(usual_models("openrouter").is_empty() && usual_models("anthropic").len() == 3);
    }
}
