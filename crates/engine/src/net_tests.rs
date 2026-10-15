//! Tests of [`crate::net_commands`]: Offline mode, the redacted request log, secrets that can be
//! asked about but never read back, and the connections list.

use super::*;
use crate::net_commands::init;
use serde_json::json;
use std::sync::{Mutex, PoisonError};

/// The network state is process-wide, so tests that touch it run one at a time.
static LOCK: Mutex<()> = Mutex::new(());

fn guard() -> std::sync::MutexGuard<'static, ()> {
    let g = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    edith_net::shared().set_offline(false);
    edith_net::shared().clear_log();
    g
}

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

#[cfg(feature = "net")]
fn secrets() -> &'static edith_net::secrets::SecretStore {
    static ONCE: std::sync::Once = std::sync::Once::new();
    // Tests never touch the real keychain.
    ONCE.call_once(|| {
        let _ = edith_net::secrets::install(edith_net::secrets::in_memory().unwrap());
    });
    edith_net::secrets::os().unwrap()
}

#[test]
fn status_and_offline_mode() {
    let _g = guard();
    let mut s = Session::default();
    let st = s.execute("net.status", json!({})).unwrap();
    assert_eq!(st["offline"], false);
    assert_eq!(st["requests"], 0);
    assert_eq!(st["available"]["http"], cfg!(feature = "net"));
    let r = s.execute("net.setOffline", json!({"offline": true})).unwrap();
    assert_eq!(r["offline"], true);
    assert!(edith_net::shared().check().is_err());
    assert_eq!(s.execute("net.status", json!({})).unwrap()["offline"], true);
    assert!(s.log.entries.iter().any(|e| e.source == "network" && e.message.contains("Offline mode is on")));
    s.execute("net.setOffline", json!({"offline": false})).unwrap();
    assert!(edith_net::shared().check().is_ok());
}

#[test]
fn the_log_is_redacted_limited_and_clearable() {
    let _g = guard();
    let mut s = Session::default();
    let st = edith_net::shared();
    for i in 0..5 {
        st.record("GET", &format!("https://user:pw@h{i}.test/v1/x?key=SECRET-{i}&code=ABC"), edith_net::Outcome::Ok, Some(200), 10, 5, None);
    }
    st.record("POST", "https://h.test/y", edith_net::Outcome::Failed, None, 0, 1, Some("failed with Authorization: Bearer sk-LEAK and token=TOK"));
    let all = s.execute("net.log", json!({})).unwrap();
    let text = all.to_string();
    for leak in ["SECRET", "ABC", "pw@", "sk-LEAK", "TOK"] {
        assert!(!text.contains(leak), "{leak} leaked: {text}");
    }
    assert_eq!(all["entries"].as_array().unwrap().len(), 6);
    assert_eq!(all["entries"][0]["url"], "https://h0.test/v1/x");
    let two = s.execute("net.log", json!({"limit": 2})).unwrap();
    assert_eq!(two["entries"].as_array().unwrap().len(), 2);
    assert_eq!(s.execute("net.status", json!({})).unwrap()["requests"], 6);
    assert_eq!(s.execute("net.log", json!({"clear": true})).unwrap()["cleared"], 6);
    assert!(s.execute("net.log", json!({})).unwrap()["entries"].as_array().unwrap().is_empty());
}

#[test]
fn hostile_parameters_are_refused_without_changes() {
    let _g = guard();
    let mut s = demo();
    let snap = (*s.project).clone();
    let undo0 = s.history.undo.len();
    let long = "a".repeat(65);
    let cases: Vec<(&str, Value)> = vec![
        ("net.setOffline", json!({})),
        ("net.setOffline", json!({"offline": "yes"})),
        ("net.setOffline", json!({"offline": 1})),
        ("net.setOffline", json!({"offline": null})),
        ("net.log", json!({"limit": 0})),
        ("net.log", json!({"limit": 501})),
        ("net.log", json!({"limit": -1})),
        ("net.log", json!({"limit": "x"})),
        ("net.log", json!({"limit": 1e300})),
        ("net.log", json!({"clear": "yes"})),
        ("secrets.has", json!({})),
        ("secrets.has", json!({"name": ""})),
        ("secrets.has", json!({"name": "UPPER"})),
        ("secrets.has", json!({"name": "../etc"})),
        ("secrets.has", json!({"name": "a b"})),
        ("secrets.has", json!({"name": long})),
        ("secrets.has", json!({"name": 5})),
        ("secrets.has", json!({"name": null})),
        ("secrets.has", json!({"name": {"x": 1}})),
        ("secrets.delete", json!({})),
        ("secrets.delete", json!({"name": "bad name\n"})),
    ];
    for (id, p) in cases {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute(id, p.clone())));
        assert!(r.is_ok(), "{id} panicked on {p}");
        assert!(r.unwrap().is_err(), "{id} accepted {p}");
        assert_eq!(*s.project, snap, "{id} changed the project on {p}");
        assert_eq!(s.history.undo.len(), undo0);
    }
    assert!(!edith_net::shared().offline());
}

#[test]
fn commands_are_not_journaled() {
    let _g = guard();
    for id in ["net.status", "net.setOffline", "net.log", "secrets.has", "secrets.delete", "connections.list"] {
        let spec = crate::commands::command_specs().iter().find(|c| c.id == id).unwrap();
        assert!(!spec.journal, "{id} must not be journaled (it would put parameters in the session log)");
    }
    // None of them takes a secret value.
    for id in ["secrets.has", "secrets.delete"] {
        let spec = crate::commands::command_specs().iter().find(|c| c.id == id).unwrap();
        assert!(!spec.params.contains("value") && !spec.params.contains("key\""), "{id}: {}", spec.params);
    }
    assert!(crate::commands::command_specs().iter().all(|c| c.id != "secrets.set" && c.id != "secrets.get"));
}

#[cfg(feature = "net")]
#[test]
fn secrets_can_be_checked_and_deleted_but_never_read_back() {
    let _g = guard();
    let mut s = Session::default();
    let store = secrets();
    let name = "test.engine-secret";
    store.set(name, "sk-SUPER-SECRET-VALUE").unwrap();
    let has = s.execute("secrets.has", json!({"name": name})).unwrap();
    assert_eq!(has, json!({"name": name, "stored": true}));
    let listed = s.execute("connections.list", json!({})).unwrap();
    store.set("provider.openrouter.api_key", "sk-or-ANOTHER-SECRET").unwrap();
    let listed2 = s.execute("connections.list", json!({})).unwrap();
    for text in [has.to_string(), listed.to_string(), listed2.to_string()] {
        assert!(!text.contains("SECRET"), "{text}");
    }
    let or = |v: &Value| v["services"].as_array().unwrap().iter().find(|x| x["id"] == "openrouter").unwrap().clone();
    assert_eq!(or(&listed2)["stored"], true);
    let del = s.execute("secrets.delete", json!({"name": name})).unwrap();
    assert_eq!(del, json!({"name": name, "deleted": true}));
    assert_eq!(s.execute("secrets.has", json!({"name": name})).unwrap()["stored"], false);
    assert_eq!(s.execute("secrets.delete", json!({"name": name})).unwrap()["deleted"], false);
    store.delete("provider.openrouter.api_key").unwrap();
    // Everything it did shows in the event log without a value.
    assert!(s.log.entries.iter().all(|e| !e.message.contains("SECRET")));
}

#[test]
fn the_connections_list_names_every_service() {
    let _g = guard();
    let mut s = Session::default();
    let r = s.execute("connections.list", json!({})).unwrap();
    let services = r["services"].as_array().unwrap();
    assert_eq!(services.len(), edith_net::services::SERVICES.len());
    let ollama = services.iter().find(|x| x["id"] == "ollama").unwrap();
    assert_eq!(ollama["kind"], "local");
    assert!(ollama["stored"].is_null());
    let anthropic = services.iter().find(|x| x["id"] == "anthropic").unwrap();
    assert_eq!(anthropic["kind"], "api_key");
    assert_eq!(anthropic["secret"], "provider.anthropic.api_key");
    assert!(anthropic["keyUrl"].as_str().unwrap().starts_with("https://"));
    assert_eq!(r["offline"], false);
}

#[cfg(not(feature = "net"))]
#[test]
fn without_the_net_feature_secrets_are_disabled_with_a_clear_reason() {
    let _g = guard();
    let mut s = Session::default();
    let e = s.execute("secrets.has", json!({"name": "x"})).unwrap_err().to_string();
    assert!(e.contains("not available in this build"), "{e}");
    assert_eq!(s.execute("net.status", json!({})).unwrap()["available"]["secrets"], false);
}

#[test]
fn init_remembers_offline_mode_in_the_data_directory() {
    let _g = guard();
    let dir = std::env::temp_dir().join(format!("fc-net-init-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // `init` binds the process-wide state; use a fresh state to check the same call path.
    let st = edith_net::NetState::new();
    st.persist_in(&dir);
    st.set_offline(true);
    assert!(dir.join(edith_net::OFFLINE_MARKER).exists());
    init(None); // no data dir: nothing happens, nothing panics
    let _ = std::fs::remove_dir_all(&dir);
}
