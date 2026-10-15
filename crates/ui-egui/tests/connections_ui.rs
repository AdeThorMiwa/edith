//! Headless UI tests of the Connections panel and the status-bar Offline switch (fork roadmap F02).
//! A key typed into the panel must reach the keychain (here an in-memory store) and nothing else:
//! not `ui.inspect`, not any automation label, not the event log.

#![allow(dead_code)]

use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, MutexGuard, PoisonError};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

/// The network state and the secret store are process-wide: one test at a time.
static LOCK: Mutex<()> = Mutex::new(());

fn guard() -> MutexGuard<'static, ()> {
    let g = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    // Never the real keychain.
    let _ = filmcraft_net::secrets::install(filmcraft_net::secrets::in_memory().unwrap());
    filmcraft_net::shared().set_offline(false);
    filmcraft_net::shared().clear_log();
    g
}

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    snapshots: Option<std::path::PathBuf>,
}

impl Driver {
    fn new() -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        // `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` also renders offscreen with wgpu and writes `connections-*.png`.
        let snapshots = std::env::var_os("FILMCRAFT_UI_SNAPSHOT_DIR").map(std::path::PathBuf::from);
        let mut b = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000);
        if snapshots.is_some() {
            b = b.wgpu();
        }
        let harness = b.build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, snapshots };
        d.frames(4);
        d
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                assert_eq!(v["ok"], json!(true), "{method} {params} failed: {v}");
                return v["result"].clone();
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({"command": command, "params": params}))
    }

    fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({"id": id}));
        self.frames(3);
    }

    fn elements(&mut self, prefix: &str) -> Vec<Value> {
        self.ok("ui.elements", json!({"prefix": prefix})).as_array().cloned().unwrap_or_default()
    }

    fn has(&mut self, id: &str) -> bool {
        self.elements(id).iter().any(|e| e["id"] == json!(id))
    }

    fn label(&mut self, id: &str) -> String {
        self.elements(id).iter().find(|e| e["id"] == json!(id)).and_then(|e| e["label"].as_str().map(str::to_string)).unwrap_or_default()
    }

    fn app(&mut self) -> &mut FilmcraftApp {
        self.harness.state_mut()
    }

    /// Offscreen render of the whole window.
    fn snapshot(&mut self, name: &str) {
        let Some(dir) = self.snapshots.clone() else { return };
        self.frames(2);
        match self.harness.render() {
            Ok(img) => {
                std::fs::create_dir_all(&dir).unwrap();
                let path = dir.join(format!("{name}.png"));
                img.save(&path).unwrap();
                eprintln!("snapshot: {}", path.display());
            }
            Err(e) => eprintln!("snapshot {name} skipped: {e}"),
        }
    }

    fn open_connections(&mut self) {
        self.ok("ui.menu.invoke", json!({"id": "window.panel.Connections"}));
        self.frames(4);
    }
}

#[test]
fn connections_opens_from_the_window_menu() {
    let _g = guard();
    let mut d = Driver::new();
    let menu = d.ok("ui.menu.list", json!({}));
    let window: Vec<String> =
        menu.as_array().unwrap().iter().filter(|m| m["path"] == json!(["Window"])).filter_map(|m| m["label"].as_str().map(str::to_string)).collect();
    assert!(window.iter().any(|w| w == "Connections"), "{window:?}");
    d.open_connections();
    for id in [
        "connections.offline",
        "connections.log",
        "connections.log.clear",
        "connections.openrouter.key",
        "connections.anthropic.key",
        "connections.gemini.getKey",
    ] {
        assert!(d.has(id), "{id} missing: {:?}", d.elements("connections").iter().map(|e| e["id"].clone()).collect::<Vec<_>>());
    }
    // a local service has nothing to store
    assert!(!d.has("connections.ollama.key"));
}

#[test]
fn the_offline_switch_works_from_the_panel_and_the_status_bar() {
    let _g = guard();
    let mut d = Driver::new();
    d.open_connections();
    assert_eq!(d.label("status.offline"), "Offline mode off");
    d.click("connections.offline");
    assert!(filmcraft_net::shared().offline());
    assert_eq!(d.label("status.offline"), "Offline mode on");
    assert_eq!(d.exec("net.status", json!({}))["offline"], true);
    d.click("status.offline");
    assert!(!filmcraft_net::shared().offline());
    assert_eq!(d.label("status.offline"), "Offline mode off");
}

#[test]
fn a_typed_key_goes_to_the_keychain_and_nowhere_else() {
    let _g = guard();
    const SECRET: &str = "sk-or-TYPED-SECRET-123";
    let name = "provider.openrouter.api_key";
    let mut d = Driver::new();
    d.open_connections();
    let store = filmcraft_net::secrets::os().unwrap();
    let _ = store.delete(name);
    d.click("connections.openrouter.key");
    d.ok("ui.type", json!({"text": SECRET}));
    d.frames(3);
    // typed, not saved yet: not in the keychain, and visible nowhere an agent can read
    assert!(!store.has(name).unwrap());
    let inspect = d.ok("ui.inspect", json!({})).to_string();
    assert!(!inspect.contains("TYPED-SECRET"), "ui.inspect leaked the key");
    let labels = d.ok("ui.elements", json!({})).to_string();
    assert!(!labels.contains("TYPED-SECRET"), "an automation label leaked the key");
    d.click("connections.openrouter.save");
    assert_eq!(store.get(name).unwrap().as_deref().map(String::as_str), Some(SECRET));
    assert_eq!(d.exec("secrets.has", json!({"name": name}))["stored"], true);
    // the field is empty again, "Remove" appeared, and the key is in no output
    assert!(d.has("connections.openrouter.remove"));
    for (what, text) in [
        ("ui.inspect", d.ok("ui.inspect", json!({})).to_string()),
        ("ui.elements", d.ok("ui.elements", json!({})).to_string()),
        ("events", d.exec("events.list", json!({})).to_string()),
        ("connections.list", d.exec("connections.list", json!({})).to_string()),
        ("status", d.app().ui.status.clone()),
    ] {
        assert!(!text.contains("TYPED-SECRET"), "{what} leaked the key");
    }
    let rows = d.exec("connections.list", json!({}));
    let or = rows["services"].as_array().unwrap().iter().find(|s| s["id"] == "openrouter").unwrap().clone();
    assert_eq!(or["stored"], true);
    d.click("connections.openrouter.remove");
    assert!(!store.has(name).unwrap());
    assert!(!d.has("connections.openrouter.remove"));
}

#[test]
fn the_log_shows_redacted_requests_and_can_be_cleared() {
    let _g = guard();
    let mut d = Driver::new();
    filmcraft_net::shared().record("GET", "https://api.example.test/v1/x?key=URL-SECRET", filmcraft_net::Outcome::Ok, Some(200), 10, 5, None);
    d.open_connections();
    assert_eq!(d.label("connections.log"), "1 requests");
    assert!(!d.ok("ui.elements", json!({})).to_string().contains("URL-SECRET"));
    // The log is below the service list: scroll the panel down to it, as a user would.
    let top = d.elements("connections.offline").first().map(|e| e["rect"].clone()).unwrap();
    let (x, y) = (top[0].as_f64().unwrap() + 40.0, top[1].as_f64().unwrap() + 10.0);
    d.ok("ui.scroll", json!({"x": x, "y": y, "dx": 0.0, "dy": -5000.0}));
    d.frames(3);
    d.click("connections.log.clear");
    assert_eq!(d.label("connections.log"), "0 requests");
    assert!(filmcraft_net::shared().entries(None).is_empty());
}

/// Looks at the panel (set `FILMCRAFT_UI_SNAPSHOT_DIR` to keep the pictures): a connected service,
/// a key being typed (masked), Offline on, and a few requests in the log.
#[test]
fn the_panel_looks_right() {
    let _g = guard();
    let mut d = Driver::new();
    let store = filmcraft_net::secrets::os().unwrap();
    store.set("provider.gemini.api_key", "AIza-picture-key").unwrap();
    let st = filmcraft_net::shared();
    st.record("GET", "https://openrouter.ai/api/v1/models", filmcraft_net::Outcome::Ok, Some(200), 52_000, 310, None);
    st.record("POST", "https://api.anthropic.com/v1/messages?key=x", filmcraft_net::Outcome::Failed, Some(401), 120, 95, None);
    st.record("GET", "https://api.pexels.com/videos/search", filmcraft_net::Outcome::Cached, Some(200), 8_000, 4, None);
    d.open_connections();
    d.click("connections.offline");
    d.click("connections.openrouter.key");
    d.ok("ui.type", json!({"text": "sk-or-masked"}));
    d.frames(3);
    d.snapshot("connections-top");
    let top = d.elements("connections.offline").first().map(|e| e["rect"].clone()).unwrap();
    d.ok("ui.scroll", json!({"x": top[0].as_f64().unwrap() + 40.0, "y": top[1].as_f64().unwrap() + 10.0, "dx": 0.0, "dy": -5000.0}));
    d.frames(3);
    d.snapshot("connections-bottom");
    store.delete("provider.gemini.api_key").unwrap();
}
