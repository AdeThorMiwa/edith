//! Connect and Disconnect for services that sign in through their own page (roadmap F03.3).
//!
//! | command | does |
//! |---|---|
//! | `connections.connect {service}` | starts the sign-in and returns `{job, url}`; the person approves in the browser, the key is stored in the keychain, and the job ends. Cancel with `jobs.cancel` |
//! | `connections.disconnect {service}` | stops a sign-in that is waiting, and removes the stored key |
//!
//! The key is never in a parameter, a result, a job record or the log: the worker thread trades the
//! code for it and hands it straight to the keychain. `connections.list` shows where a sign-in is
//! (`connecting`). Only the service id, the sign-in address (which carries no secret) and a short
//! plain message are kept in the open.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use serde_json::{Value, json};

use crate::commands::{bad, str_p};
use crate::panels::Level;
use crate::{EngineError, Result, Session};

/// Where one service's sign-in is.
#[derive(Clone, Debug, PartialEq)]
pub enum Attempt {
    /// Waiting for the person in the browser.
    Waiting { job: u64, url: String },
    /// It ended without a key; the message is plain text safe to show.
    Failed(String),
}

impl Attempt {
    pub fn to_json(&self) -> Value {
        match self {
            Attempt::Waiting { job, url } => json!({"state": "waiting", "job": job, "url": url}),
            Attempt::Failed(message) => json!({"state": "failed", "message": message}),
        }
    }
}

// Sign-ins belong to the process (like the keychain and the Offline switch), not to a project.
static ATTEMPTS: Mutex<Vec<(String, Attempt)>> = Mutex::new(Vec::new());
static CHANGES: AtomicU64 = AtomicU64::new(0);

fn set(service: &str, attempt: Option<Attempt>) {
    let mut all = ATTEMPTS.lock().unwrap_or_else(PoisonError::into_inner);
    all.retain(|(s, _)| s != service);
    all.extend(attempt.map(|a| (service.to_string(), a)));
    CHANGES.fetch_add(1, Ordering::SeqCst);
}

/// Tells panels that something changed (a check finished).
#[cfg(feature = "net")]
pub(super) fn bump() {
    CHANGES.fetch_add(1, Ordering::SeqCst);
}

/// Where this service's sign-in is, if one has started and not been cleared.
pub fn attempt(service: &str) -> Option<Attempt> {
    ATTEMPTS.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(s, _)| s == service).map(|(_, a)| a.clone())
}

/// A number that changes whenever a sign-in starts, ends or is cleared, so a panel knows when to
/// look again (and to ask the keychain afresh).
pub fn changes() -> u64 {
    CHANGES.load(Ordering::SeqCst)
}

#[cfg(feature = "net")]
mod live {
    use super::*;
    use crate::Job;
    use crate::net_commands::keychain;
    use edith_ai::connect::OpenRouter;
    use edith_net::NetError;
    use edith_net::http::{Client, ClientConfig};
    use std::time::Duration;

    /// Tests point the sign-in at a mock provider on this computer (`None` restores the real one).
    static TEST_PROVIDER: Mutex<Option<OpenRouter>> = Mutex::new(None);

    /// For tests and tools: use `provider` (a mock on a loopback address) instead of the real
    /// OpenRouter. Plain `http` to this computer is allowed while it is set.
    #[doc(hidden)]
    pub fn use_test_provider(provider: Option<OpenRouter>) {
        *TEST_PROVIDER.lock().unwrap_or_else(PoisonError::into_inner) = provider;
    }

    /// How long a sign-in may take: the provider's code lasts ten minutes.
    const SIGN_IN_TIME: Duration = Duration::from_secs(600);

    pub fn connect(s: &mut Session, p: &Value) -> Result<Value> {
        let cmd = "connections.connect";
        let id = str_p(p, "service").ok_or_else(|| bad(cmd, "`service` is required"))?;
        let service = edith_net::services::find(id).ok_or_else(|| bad(cmd, format!("`{}` is not a known service", id.chars().take(40).collect::<String>())))?;
        let (true, Some(name)) = (service.kind == edith_net::services::Kind::SignIn, service.secret) else {
            return Err(bad(cmd, format!("{} does not sign in through a page; paste a key in Connections instead", service.label)));
        };
        if edith_net::shared().offline() {
            return Err(EngineError::Other("Offline mode is on; turn it off to sign in.".to_string()));
        }
        if let Some(Attempt::Waiting { job, url }) = attempt(service.id) {
            return Ok(json!({"service": service.id, "job": job, "url": url, "resumed": true}));
        }
        let (provider, mock) = {
            let t = TEST_PROVIDER.lock().unwrap_or_else(PoisonError::into_inner);
            match t.as_ref() {
                Some(m) => (OpenRouter { auth_url: m.auth_url.clone(), key_url: m.key_url.clone() }, true),
                None => (OpenRouter::default(), false),
            }
        };
        let flow = provider.start().map_err(|e| EngineError::Other(e.to_string()))?;
        let url = flow.authorize_url.clone();

        let job = Job {
            id: s.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1,
            label: format!("Connect {}", service.label),
            progress: Default::default(),
            result: Default::default(),
        };
        let (progress, result, job_id, label) = (job.progress.clone(), job.result.clone(), job.id, service.label);
        let service_id = service.id;
        let run = crate::export_tools::guard_job(job.progress.clone(), job.result.clone(), move || {
            *progress.status.lock().unwrap_or_else(PoisonError::into_inner) = "Waiting for the browser".to_string();
            let outcome: std::result::Result<(), NetError> = (|| {
                let (code, pkce) = flow.wait_until(SIGN_IN_TIME, &|| progress.cancel.load(Ordering::Relaxed))?;
                *progress.status.lock().unwrap_or_else(PoisonError::into_inner) = "Getting your key".to_string();
                let client = Client::new(ClientConfig { allow_loopback_http: mock, ..ClientConfig::default() }, edith_net::shared().clone());
                let key = provider.exchange(&client, &code, &pkce)?;
                keychain::set(name, &key).map_err(NetError::Secret)?;
                keychain::forget(name);
                Ok(())
            })();
            match outcome {
                Ok(()) => {
                    set(service_id, None);
                    *progress.status.lock().unwrap_or_else(PoisonError::into_inner) = format!("Connected to {label}");
                    progress.finished.store(true, Ordering::Relaxed);
                    *result.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some(Ok(filmcraft_export::Report { path: String::new(), frames: 0, seconds: 0.0, bytes: 0, render_fps: 0.0, extra_files: Vec::new() }));
                }
                Err(e) => {
                    let message = if e == NetError::Cancelled { "Sign-in stopped.".to_string() } else { e.to_string() };
                    // A stopped sign-in leaves nothing behind; a failed one says why until the next try.
                    set(service_id, (e != NetError::Cancelled).then(|| Attempt::Failed(message.clone())));
                    *progress.error.lock().unwrap_or_else(PoisonError::into_inner) = Some(message.clone());
                    progress.finished.store(true, Ordering::Relaxed);
                    *result.lock().unwrap_or_else(PoisonError::into_inner) = Some(Err(message));
                }
            }
        });
        set(service.id, Some(Attempt::Waiting { job: job_id, url: url.clone() }));
        s.jobs.push(job);
        if let Err(e) = std::thread::Builder::new().name("sign-in".into()).spawn(run) {
            set(service.id, Some(Attempt::Failed("could not start the sign-in".to_string())));
            return Err(EngineError::Other(format!("could not start the sign-in: {e}")));
        }
        s.log.push(Level::Info, "network", format!("Signing in to {}: waiting for the browser.", service.label));
        Ok(json!({"service": service.id, "job": job_id, "url": url, "resumed": false}))
    }

    pub fn stop_waiting(s: &Session, service: &str) {
        if let Some(Attempt::Waiting { job, .. }) = attempt(service)
            && let Some(j) = s.jobs.iter().find(|j| j.id == job)
        {
            j.progress.cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(feature = "net")]
pub use live::use_test_provider;

#[cfg(feature = "net")]
pub fn connect(s: &mut Session, p: &Value) -> Result<Value> {
    live::connect(s, p)
}

#[cfg(not(feature = "net"))]
pub fn connect(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(EngineError::Other("signing in is not available in this build (built without the `net` feature)".to_string()))
}

pub fn disconnect(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "connections.disconnect";
    let id = str_p(p, "service").ok_or_else(|| bad(cmd, "`service` is required"))?;
    let service = edith_net::services::find(id).ok_or_else(|| bad(cmd, format!("`{}` is not a known service", id.chars().take(40).collect::<String>())))?;
    let Some(name) = service.secret else {
        return Err(bad(cmd, format!("{} keeps no key on this computer", service.label)));
    };
    #[cfg(feature = "net")]
    live::stop_waiting(s, service.id);
    let deleted = super::keychain::delete(name).map_err(EngineError::Other)?;
    super::keychain::forget(name);
    set(service.id, None);
    if deleted {
        s.log.push(Level::Info, "network", format!("Disconnected {}: its key was removed from this computer.", service.label));
    }
    Ok(json!({"service": service.id, "disconnected": deleted}))
}
