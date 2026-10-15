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
    let _ = edith_net::secrets::install(edith_net::secrets::in_memory().unwrap());
    edith_net::shared().set_offline(false);
    edith_net::shared().clear_log();
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
    assert!(edith_net::shared().offline());
    assert_eq!(d.label("status.offline"), "Offline mode on");
    assert_eq!(d.exec("net.status", json!({}))["offline"], true);
    d.click("status.offline");
    assert!(!edith_net::shared().offline());
    assert_eq!(d.label("status.offline"), "Offline mode off");
}

#[test]
fn a_typed_key_goes_to_the_keychain_and_nowhere_else() {
    let _g = guard();
    const SECRET: &str = "sk-or-TYPED-SECRET-123";
    let name = "provider.openrouter.api_key";
    let mut d = Driver::new();
    d.open_connections();
    let store = edith_net::secrets::os().unwrap();
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
    // the field is empty again, "Disconnect" appeared (OpenRouter signs in through a page, so its
    // key is disconnected rather than removed), and the key is in no output
    assert!(d.has("connections.openrouter.disconnect") && !d.has("connections.openrouter.remove"));
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
    d.click("connections.openrouter.disconnect");
    assert!(!store.has(name).unwrap());
    assert!(!d.has("connections.openrouter.disconnect") && d.has("connections.openrouter.connect"));
    // a service whose key is only pasted still says Remove
    d.click("connections.anthropic.key");
    d.ok("ui.type", json!({"text": "anthropic-TYPED-SECRET-456"}));
    d.click("connections.anthropic.save");
    assert!(d.has("connections.anthropic.remove") && !d.has("connections.anthropic.disconnect"));
    d.click("connections.anthropic.remove");
    assert!(!store.has("provider.anthropic.api_key").unwrap());
}

#[test]
fn the_log_shows_redacted_requests_and_can_be_cleared() {
    let _g = guard();
    let mut d = Driver::new();
    edith_net::shared().record("GET", "https://api.example.test/v1/x?key=URL-SECRET", edith_net::Outcome::Ok, Some(200), 10, 5, None);
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
    assert!(edith_net::shared().entries(None).is_empty());
}

/// Looks at the panel (set `FILMCRAFT_UI_SNAPSHOT_DIR` to keep the pictures): a connected service,
/// a key being typed (masked), Offline on, and a few requests in the log.
#[test]
fn the_panel_looks_right() {
    let _g = guard();
    let mut d = Driver::new();
    let store = edith_net::secrets::os().unwrap();
    store.set("provider.gemini.api_key", "AIza-picture-key").unwrap();
    let st = edith_net::shared();
    st.record("GET", "https://openrouter.ai/api/v1/models", edith_net::Outcome::Ok, Some(200), 52_000, 310, None);
    st.record("POST", "https://api.anthropic.com/v1/messages?key=x", edith_net::Outcome::Failed, Some(401), 120, 95, None);
    st.record("GET", "https://api.pexels.com/videos/search", edith_net::Outcome::Cached, Some(200), 8_000, 4, None);
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

// ---- signing in (F03.3d) ------------------------------------------------------------------

mod sign_in {
    use super::*;
    use edith_ai::connect::OpenRouter;
    use edith_net::testserver::{response, serve};
    use filmcraft_engine::net_commands::connect::use_test_provider;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const KEY: &str = "sk-or-v1-UITEST0123456789abcdef0123456789";
    const SLOT: &str = "provider.openrouter.api_key";

    fn param(url: &str, name: &str) -> String {
        let q = url.split_once('?').map_or("", |x| x.1);
        let v = q.split('&').find_map(|p| p.split_once('=').filter(|(k, _)| *k == name).map(|x| x.1)).unwrap_or("");
        v.replace("%3A", ":").replace("%2F", "/")
    }

    fn browser(url: &str, query: String) {
        let port: u16 = param(url, "callback_url").rsplit(':').next().unwrap().split('/').next().unwrap().parse().unwrap();
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let _ = c.write_all(format!("GET /callback?{query} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes());
        let mut out = String::new();
        let _ = c.read_to_string(&mut out);
    }

    fn wait(d: &mut Driver, what: &str, mut done: impl FnMut(&mut Driver) -> bool) {
        let started = Instant::now();
        while !done(d) {
            assert!(started.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
            d.frames(2);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn connect_opens_the_browser_waits_stores_the_key_and_disconnect_removes_it() {
        let _g = guard();
        let store = edith_net::secrets::os().unwrap();
        let _ = store.delete(SLOT);
        let exchange = serve(|_, _| response(200, &[("Content-Type", "application/json")], format!(r#"{{"key":"{KEY}"}}"#).as_bytes()));
        use_test_provider(Some(OpenRouter { auth_url: "https://openrouter.ai/auth".into(), key_url: format!("{}/api/v1/auth/keys", exchange.url) }));
        let mut d = Driver::new();
        let opened: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = opened.clone();
        d.app().hooks.open_url = Some(Box::new(move |u| {
            sink.lock().unwrap().push(u.to_string());
            Ok(())
        }));
        d.open_connections();
        assert!(d.has("connections.openrouter.connect") && !d.has("connections.openrouter.disconnect"));
        assert!(!d.has("connections.anthropic.connect"), "only services that sign in through a page get the button");

        // Connect, then Cancel: nothing is left behind
        d.click("connections.openrouter.connect");
        wait(&mut d, "the browser to open", |_| !opened.lock().unwrap().is_empty());
        assert!(opened.lock().unwrap()[0].starts_with("https://openrouter.ai/auth?"));
        wait(&mut d, "the waiting state", |d| d.has("connections.openrouter.cancel"));
        assert!(!d.has("connections.openrouter.connect") && d.has("connections.openrouter.reopen"));
        d.click("connections.openrouter.cancel");
        wait(&mut d, "the button to come back", |d| d.has("connections.openrouter.connect"));
        assert!(!store.has(SLOT).unwrap());

        // Connect again and approve in the "browser"
        opened.lock().unwrap().clear();
        d.click("connections.openrouter.connect");
        wait(&mut d, "the browser to open", |_| !opened.lock().unwrap().is_empty());
        let url = opened.lock().unwrap()[0].clone();
        let state = param(&url, "state");
        let page = std::thread::spawn(move || browser(&url, format!("code=THE-CODE&state={state}")));
        wait(&mut d, "the panel to show Connected", |d| d.has("connections.openrouter.disconnect"));
        page.join().unwrap();
        assert_eq!(store.get(SLOT).unwrap().unwrap().as_str(), KEY);
        // the key is in no label, no inspected state and no log line
        let everything = format!(
            "{} {} {}",
            d.ok("ui.elements", json!({"prefix": ""})),
            d.ok("ui.inspect", json!({})),
            d.app().session.log.entries.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")
        );
        for secret in [KEY, "UITEST0123456789", "THE-CODE"] {
            assert!(!everything.contains(secret), "{secret} leaked");
        }
        assert!(d.app().session.log.entries.iter().any(|e| e.message.contains("Connected to OpenRouter")));

        d.click("connections.openrouter.disconnect");
        wait(&mut d, "the key to go", |d| d.has("connections.openrouter.connect"));
        assert!(!store.has(SLOT).unwrap());
        use_test_provider(None);
    }
}

// ---- looking for local model servers (F03.3e) ----------------------------------------------

#[test]
fn check_looks_for_a_local_server_once_and_reports_what_it_found() {
    use edith_net::testserver::{response, serve};
    use filmcraft_engine::net_commands::probe::use_test_base;
    let _g = guard();
    let server = serve(|_, _| response(200, &[("content-type", "application/json")], br#"{"data":[{"id":"llama3.2:3b"}]}"#));
    use_test_base("ollama", Some(&format!("{}/v1", server.url)));
    let mut d = Driver::new();
    d.open_connections();
    assert!(d.has("connections.ollama.check") && d.has("connections.lmstudio.check"));
    assert!(!d.has("connections.openrouter.check") && !d.has("connections.anthropic.check"), "only local servers are checked");
    assert_eq!(server.hits(), 0, "nothing is looked for until the person asks");
    // the local servers are at the bottom of the list: scroll the panel until they are on screen
    d.ok("ui.scroll", json!({"id": "connections.offline", "dy": -700.0}));
    d.frames(4);
    d.click("connections.ollama.check");
    let started = std::time::Instant::now();
    loop {
        d.frames(2);
        let rows = d.exec("connections.list", json!({}));
        let ollama = rows["services"].as_array().unwrap().iter().find(|s| s["id"] == "ollama").unwrap().clone();
        if ollama["probe"]["state"] == "up" {
            assert_eq!(ollama["probe"]["models"], 1);
            break;
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "no answer: {ollama}");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(d.has("connections.ollama.check"), "the button is back for another look");
    assert!(edith_net::shared().entries(None).iter().any(|e| e.url.contains("/v1/models")));
    use_test_base("ollama", None);
}
