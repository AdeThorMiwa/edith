//! Network, secrets and connections (roadmap F02). The mechanics live in `edith-net`; these
//! commands let the UI, the CLI, the control channel and MCP see and steer them.
//!
//! | command | does |
//! |---|---|
//! | `net.status` | Offline switch, what this build can do (`http`, `secrets`, `signIn`), request count, the latest request |
//! | `net.setOffline` | turn Offline mode on or off (remembered across restarts); while on, every request fails at once |
//! | `net.log` | the redacted request log (newest `limit`, default 100), or `clear: true` to empty it |
//! | `secrets.has` | whether a secret with this name is stored; **never returns the secret** |
//! | `secrets.delete` | remove a stored secret |
//! | `connections.list` | every known service with its kind, whether a key is stored, and where to get one |
//!
//! There is deliberately no command that stores or reads a secret: a key can only be typed into
//! Settings ▸ Connections (or arrive through a sign-in), so it never appears in a command's
//! parameters, the journal, the session log, `ui.inspect` or an agent's transcript. None of these
//! commands is journaled.

use std::path::Path;

use serde_json::{Value, json};

use edith_net as net;
pub use net::services::{Kind, Service};
pub use net::{LogEntry, Outcome};

use crate::commands::{CommandSpec, always, bad, bool_p, str_p};
use crate::panels::Level;
use crate::{EngineError, Result, Session};

/// Tells the network layer where the data directory is, so Offline mode is remembered. Call once
/// at start-up (the apps do), before any request.
pub fn init(data_dir: Option<&Path>) {
    if let Some(dir) = data_dir {
        net::shared().persist_in(dir);
    }
}

/// What this build can do, for `net.status`.
pub fn available() -> Value {
    json!({"http": cfg!(feature = "net"), "secrets": cfg!(feature = "net"), "signIn": cfg!(feature = "net")})
}

#[cfg(feature = "net")]
mod keychain {
    use super::*;

    pub fn valid(name: &str) -> bool {
        net::secrets::valid_name(name)
    }
    pub fn has(name: &str) -> std::result::Result<bool, String> {
        net::secrets::os().and_then(|s| s.has(name)).map_err(|e| e.to_string())
    }
    pub fn delete(name: &str) -> std::result::Result<bool, String> {
        net::secrets::os().and_then(|s| s.delete(name)).map_err(|e| e.to_string())
    }
    pub fn set(name: &str, value: &str) -> std::result::Result<(), String> {
        net::secrets::os().and_then(|s| s.set(name, value)).map_err(|e| e.to_string())
    }
}

#[cfg(not(feature = "net"))]
mod keychain {
    pub fn valid(name: &str) -> bool {
        !name.is_empty() && name.len() <= 64
    }
    pub fn has(_: &str) -> std::result::Result<bool, String> {
        Err("secrets are not available in this build".to_string())
    }
    pub fn delete(_: &str) -> std::result::Result<bool, String> {
        Err("secrets are not available in this build".to_string())
    }
    pub fn set(_: &str, _: &str) -> std::result::Result<(), String> {
        Err("secrets are not available in this build".to_string())
    }
}

fn can_use_secrets(_: &Session) -> std::result::Result<(), String> {
    if cfg!(feature = "net") { Ok(()) } else { Err("secrets are not available in this build (built without the `net` feature)".into()) }
}

fn status(_s: &mut Session, _p: &Value) -> Result<Value> {
    let st = net::shared();
    let all = st.entries(None);
    Ok(json!({
        "offline": st.offline(),
        "available": available(),
        "requests": all.len(),
        "lastRequest": all.last(),
    }))
}

fn set_offline(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "net.setOffline";
    let on = bool_p(p, "offline").ok_or_else(|| bad(cmd, "`offline` must be true or false"))?;
    let st = net::shared();
    if st.offline() != on {
        st.set_offline(on);
        s.log.push(Level::Info, "network", if on { "Offline mode is on: no network requests will be made." } else { "Offline mode is off." });
    }
    Ok(json!({"offline": st.offline()}))
}

fn log(_s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "net.log";
    let st = net::shared();
    match p.get("clear") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => {
            let n = st.entries(None).len();
            st.clear_log();
            return Ok(json!({"cleared": n}));
        }
        Some(_) => return Err(bad(cmd, "`clear` must be true or false")),
    }
    let limit = match p.get("limit") {
        None | Some(Value::Null) => 100,
        Some(v) => v.as_u64().filter(|n| (1..=500).contains(n)).ok_or_else(|| bad(cmd, "`limit` must be a whole number from 1 to 500"))?,
    };
    Ok(json!({"entries": st.entries(Some(usize::try_from(limit).unwrap_or(100)))}))
}

fn secret_name<'a>(p: &'a Value, cmd: &str) -> Result<&'a str> {
    let name = str_p(p, "name").ok_or_else(|| bad(cmd, "`name` is required"))?;
    if keychain::valid(name) { Ok(name) } else { Err(bad(cmd, "`name` must be 1 to 64 characters: lower-case letters, digits, `.`, `_` or `-`")) }
}

fn secrets_has(_s: &mut Session, p: &Value) -> Result<Value> {
    let name = secret_name(p, "secrets.has")?;
    let stored = keychain::has(name).map_err(EngineError::Other)?;
    Ok(json!({"name": name, "stored": stored}))
}

fn secrets_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let name = secret_name(p, "secrets.delete")?;
    let deleted = keychain::delete(name).map_err(EngineError::Other)?;
    if deleted {
        s.log.push(Level::Info, "network", format!("Removed the stored secret `{name}`."));
    }
    Ok(json!({"name": name, "deleted": deleted}))
}

/// A known service and whether a key is stored for it (`None`: local, or the keychain could not
/// be asked).
pub struct ServiceStatus {
    pub service: &'static Service,
    pub stored: Option<bool>,
}

/// Every known service with its stored state, plus the first keychain problem met. Asks the
/// keychain once per service, so the UI caches the answer instead of calling this every frame.
pub fn service_statuses() -> (Vec<ServiceStatus>, Option<String>) {
    let mut problem: Option<String> = None;
    let list = net::services::SERVICES
        .iter()
        .map(|service| {
            let stored = service.secret.and_then(|name| match keychain::has(name) {
                Ok(v) => Some(v),
                Err(e) => {
                    problem.get_or_insert(e);
                    None
                }
            });
            ServiceStatus { service, stored }
        })
        .collect();
    (list, problem)
}

/// Stores a secret typed into Settings. **Not a command**, on purpose: a key must never travel
/// through command parameters, the journal or an agent. Only the Connections panel calls this.
/// `name` must be a known service's secret slot.
pub fn save_secret(name: &str, value: &str) -> std::result::Result<(), String> {
    if !net::services::SERVICES.iter().any(|s| s.secret == Some(name)) {
        return Err("unknown connection".to_string());
    }
    keychain::set(name, value)
}

/// The newest `limit` request-log entries, oldest first.
pub fn recent_requests(limit: usize) -> Vec<LogEntry> {
    net::shared().entries(Some(limit))
}

/// Whether a key is stored for the service with this id (`None`: the service has no key slot, or
/// the keychain could not be asked). Does not read the key.
pub fn connected(service_id: &str) -> Option<bool> {
    let name = net::services::find(service_id)?.secret?;
    keychain::has(name).ok()
}

/// Whether this build can store keys (built with the `net` feature).
pub fn secrets_available() -> bool {
    cfg!(feature = "net")
}

pub fn is_offline() -> bool {
    net::shared().offline()
}

fn connections_list(_s: &mut Session, _p: &Value) -> Result<Value> {
    let (statuses, problem) = service_statuses();
    let services: Vec<Value> = statuses
        .iter()
        .map(|st| {
            let mut v = serde_json::to_value(st.service).unwrap_or(Value::Null);
            v["stored"] = json!(st.stored);
            v
        })
        .collect();
    Ok(json!({"offline": net::shared().offline(), "available": available(), "services": services, "problem": problem}))
}

fn spec(
    id: &'static str,
    label: &'static str,
    params: &'static str,
    enabled: fn(&Session) -> std::result::Result<(), String>,
    run: fn(&mut Session, &Value) -> Result<Value>,
) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled, run, journal: false }
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        spec("net.status", "Network Status", "{}", always, status),
        spec("net.setOffline", "Set Offline Mode", r#"{"offline":bool}"#, always, set_offline),
        spec("net.log", "Network Request Log", r#"{"limit":1..500?,"clear":bool?}"#, always, log),
        spec("secrets.has", "Is a Secret Stored", r#"{"name":str}"#, can_use_secrets, secrets_has),
        spec("secrets.delete", "Delete a Stored Secret", r#"{"name":str}"#, can_use_secrets, secrets_delete),
        spec("connections.list", "List Connections", "{}", always, connections_list),
    ]
}
