//! Looking for a model server on this computer (Ollama, LM Studio; roadmap F03.3e).
//!
//! `connections.probe {service}` asks the server for its model list once, with a short timeout, and
//! remembers the answer for `connections.list` (`probe`). It runs only when asked (the Check button,
//! or a command), never in the background, and like every request it goes through the Offline
//! switch and the request log. Nothing is stored and no key is involved.

use std::sync::{Mutex, PoisonError};

use serde_json::{Value, json};

use crate::commands::{bad, bool_p, str_p};
use crate::{EngineError, Result, Session};

/// What the last check found.
#[derive(Clone, Debug, PartialEq)]
pub enum Probe {
    Checking,
    Up {
        models: usize,
    },
    /// Not reachable, or it answered with something unusable; the message is plain text safe to show.
    Down(String),
}

impl Probe {
    pub fn to_json(&self) -> Value {
        match self {
            Probe::Checking => json!({"state": "checking"}),
            Probe::Up { models } => json!({"state": "up", "models": models}),
            Probe::Down(message) => json!({"state": "down", "message": message}),
        }
    }
}

static PROBES: Mutex<Vec<(String, Probe)>> = Mutex::new(Vec::new());

#[cfg(feature = "net")]
fn set(service: &str, probe: Probe) {
    let mut all = PROBES.lock().unwrap_or_else(PoisonError::into_inner);
    all.retain(|(s, _)| s != service);
    all.push((service.to_string(), probe));
    super::connect::bump();
}

/// What the last check of this service found, if one was made.
pub fn state(service: &str) -> Option<Probe> {
    PROBES.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(s, _)| s == service).map(|(_, p)| p.clone())
}

pub fn probe(_s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "connections.probe";
    let id = str_p(p, "service").ok_or_else(|| bad(cmd, "`service` is required"))?;
    let wait = match p.get("wait") {
        None | Some(Value::Null) => false,
        _ => bool_p(p, "wait").ok_or_else(|| bad(cmd, "`wait` must be true or false"))?,
    };
    let service = edith_net::services::find(id).filter(|sv| sv.kind == edith_net::services::Kind::Local);
    let Some(service) = service else {
        return Err(bad(cmd, "`service` must be a model server that runs on this computer (ollama or lmstudio)"));
    };
    start(service.id, service.label)?;
    if wait {
        let started = web_time::Instant::now();
        while state(service.id) == Some(Probe::Checking) && started.elapsed() < std::time::Duration::from_secs(8) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    Ok(json!({"service": service.id, "probe": state(service.id).map(|p| p.to_json())}))
}

/// Tests point a check at a mock server on this computer instead of the usual port.
#[cfg(feature = "net")]
static TEST_BASE: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// For tests and tools: check `base_url` (a mock, e.g. `http://127.0.0.1:PORT/v1`) instead of the
/// usual address of `service`; `None` restores it.
#[cfg(feature = "net")]
#[doc(hidden)]
pub fn use_test_base(service: &str, base_url: Option<&str>) {
    let mut all = TEST_BASE.lock().unwrap_or_else(PoisonError::into_inner);
    all.retain(|(s, _)| s != service);
    all.extend(base_url.map(|u| (service.to_string(), u.to_string())));
}

#[cfg(feature = "net")]
fn start(service: &'static str, label: &'static str) -> Result<()> {
    use edith_ai::llm::{ChatModel, LlmError, openai::OpenAiCompatible};
    use edith_net::NetError;
    use edith_net::http::{Client, ClientConfig};
    use std::time::Duration;

    if edith_net::shared().offline() {
        return Err(EngineError::Other("Offline mode is on; turn it off to look for a server.".to_string()));
    }
    let Some(endpoint) = edith_ai::endpoints::endpoint(service) else {
        return Err(EngineError::Other(format!("{label} is not set up as a model server")));
    };
    if state(service) == Some(Probe::Checking) {
        return Ok(());
    }
    let base_url = TEST_BASE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|(s, _)| s == service)
        .map_or_else(|| endpoint.base_url.to_string(), |(_, u)| u.clone());
    set(service, Probe::Checking);
    let spawned = std::thread::Builder::new().name("probe".into()).spawn(move || {
        let result = std::panic::catch_unwind(|| {
            let cfg = ClientConfig {
                allow_loopback_http: true,
                connect_timeout: Duration::from_secs(1),
                timeout: Duration::from_secs(3),
                retries: 0,
                min_host_interval: Duration::ZERO,
                ..ClientConfig::default()
            };
            let client = std::sync::Arc::new(Client::new(cfg, edith_net::shared().clone()));
            OpenAiCompatible::new(client, endpoint.flavor.clone(), &base_url, None).models()
        });
        let found = match result {
            Ok(Ok(list)) => Probe::Up { models: list.len() },
            Ok(Err(LlmError::Net(NetError::Transport(_) | NetError::Timeout))) => Probe::Down(format!("{label} is not running on this computer.")),
            Ok(Err(LlmError::Net(NetError::Offline))) => Probe::Down("Offline mode is on.".to_string()),
            Ok(Err(e)) => Probe::Down(e.to_string()),
            Err(_) => Probe::Down("the check failed internally".to_string()),
        };
        set(service, found);
    });
    if spawned.is_err() {
        set(service, Probe::Down("could not start the check".to_string()));
        return Err(EngineError::Other("could not start the check".to_string()));
    }
    Ok(())
}

#[cfg(not(feature = "net"))]
fn start(_service: &'static str, _label: &'static str) -> Result<()> {
    Err(EngineError::Other("looking for a server is not available in this build (built without the `net` feature)".to_string()))
}
