//! Headless UI test of the Find Media panel (fork roadmap F05.3), against a local server that plays
//! the stock sites: type what you need, see results with their licences, shortlist one, add one to
//! the project. The keychain is the in-memory store and no real site is reached.

#![allow(dead_code)]

use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use edith_net::NetState;
use edith_net::http::{Client, ClientConfig};
use edith_net::testserver::{Server, response, serve};
use edith_sources::Sources;
use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};
use zeroize::Zeroizing;

static LOCK: Mutex<()> = Mutex::new(());

fn guard() -> MutexGuard<'static, ()> {
    let g = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = edith_net::secrets::install(edith_net::secrets::in_memory().unwrap());
    edith_net::shared().set_offline(false);
    g
}

fn png() -> Vec<u8> {
    let mut out = Vec::new();
    let img = image::RgbaImage::from_pixel(16, 9, image::Rgba([60, 120, 200, 255]));
    image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

struct Rig {
    sites: Server,
    files: Server,
}

fn rig() -> (Rig, Sources) {
    let image = png();
    let files = serve(move |_, _| response(200, &[("content-type", "image/png")], &image));
    let furl = files.url.clone();
    let sites = serve(move |_, req| {
        let json = |b: String| response(200, &[("content-type", "application/json")], b.as_bytes());
        if req.starts_with("GET /v1/search") {
            json(format!(
                r#"{{"total_results":2,"photos":[
                {{"id":1,"width":800,"height":600,"url":"https://www.pexels.com/photo/1/","photographer":"Sam","alt":"A pier at dawn","src":{{"large2x":"{furl}/big1.jpg","medium":"{furl}/t1.png"}}}},
                {{"id":2,"width":800,"height":600,"url":"https://www.pexels.com/photo/2/","photographer":"Lee","alt":"Fog on the water","src":{{"large2x":"{furl}/big2.jpg","medium":"{furl}/t2.png"}}}}]}}"#
            ))
        } else if req.starts_with("GET /apiv2/search") {
            json(format!(
                r#"{{"count":1,"results":[{{"id":77,"name":"Fog horn","username":"sound man","license":"https://creativecommons.org/licenses/by-nc/4.0/","url":"https://freesound.org/s/77/","duration":8,"previews":{{"preview-hq-mp3":"{furl}/horn.mp3"}}}}]}}"#
            ))
        } else {
            response(404, &[], b"")
        }
    });
    let client = Arc::new(Client::new(
        ClientConfig { allow_loopback_http: true, min_host_interval: Duration::ZERO, timeout: Duration::from_secs(5), ..ClientConfig::default() },
        Arc::new(NetState::new()),
    ));
    let sources = Sources::new(client, Arc::new(|_| Ok(Zeroizing::new("KEY-0123456789".to_string())))).with_origin(&sites.url);
    (Rig { sites, files }, sources)
}

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    snapshots: Option<std::path::PathBuf>,
}

impl Driver {
    fn new(sources: Sources) -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        session.hub.use_sources(Arc::new(sources));
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
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

    fn element(&mut self, id: &str) -> Option<Value> {
        self.ok("ui.elements", json!({"prefix": id})).as_array().and_then(|a| a.iter().find(|e| e["id"] == json!(id)).cloned())
    }

    fn has(&mut self, id: &str) -> bool {
        self.element(id).is_some()
    }

    fn label(&mut self, id: &str) -> String {
        self.element(id).and_then(|e| e["label"].as_str().map(str::to_string)).unwrap_or_default()
    }

    fn wait(&mut self, what: &str, mut done: impl FnMut(&mut Driver) -> bool) {
        let started = Instant::now();
        while !done(self) {
            assert!(started.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
            self.frames(3);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn snapshot(&mut self, name: &str) {
        let Some(dir) = self.snapshots.clone() else { return };
        self.frames(2);
        if let Ok(img) = self.harness.render() {
            std::fs::create_dir_all(&dir).unwrap();
            img.save(dir.join(format!("{name}.png"))).unwrap();
        }
    }
}

#[test]
fn search_shows_results_with_licences_shortlist_keeps_one_and_add_brings_it_into_the_project() {
    let _g = guard();
    let (rig, sources) = rig();
    let mut d = Driver::new(sources);
    d.ok("ui.menu.invoke", json!({"id": "window.panel.FindMedia"}));
    d.frames(4);
    assert!(d.has("find.query") && d.has("find.search") && d.has("find.kind.video"));
    assert_eq!(rig.sites.hits(), 0, "nothing is looked for until you ask");
    // photos and sound, then a search
    d.click("find.kind.image");
    d.click("find.kind.audio");
    d.click("find.kind.video");
    d.click("find.query");
    d.ok("ui.type", json!({"text": "a pier at dawn"}));
    d.click("find.search");
    d.wait("the results", |d| d.has("find.card.pexels:photo-1") && d.has("find.card.freesound:77"));
    d.wait("the pictures", |d| d.has("find.star.pexels:photo-2"));
    d.snapshot("find-media-results");
    assert!(
        d.label("find.card.pexels:photo-1").contains("Pexels License") && d.label("find.card.pexels:photo-1").contains("no credit needed"),
        "{}",
        d.label("find.card.pexels:photo-1")
    );
    assert!(d.label("find.card.freesound:77").contains("NOT for commercial use"), "{}", d.label("find.card.freesound:77"));
    assert!(d.label("find.source.pexels").contains("2 found"), "{}", d.label("find.source.pexels"));
    // shortlist: the star keeps it, and the Shortlist view shows only that
    d.click("find.star.pexels:photo-2");
    d.wait("the shortlist", |d| d.exec("hub.shortlist", json!({}))["shortlist"].as_array().is_some_and(|l| l.len() == 1));
    d.wait("the count", |d| d.label("find.shortlist").contains("1 chosen"));
    d.click("find.shortlist");
    d.wait("only the starred one", |d| d.has("find.card.pexels:photo-2") && !d.has("find.card.pexels:photo-1"));
    d.click("find.shortlist");
    // add one to the project: it downloads, is imported into Sources > Pexels, and carries its record
    let dir = std::env::temp_dir().join(format!("edith-find-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    d.exec("file.projectSettings.scratchDisks", json!({"captured": dir.to_string_lossy()}));
    d.wait("the card again", |d| d.has("find.get.pexels:photo-1"));
    d.click("find.get.pexels:photo-1");
    d.wait("the download to finish", |d| {
        d.exec("hub.results", json!({}))["candidates"].as_array().unwrap().iter().any(|c| c["id"] == "pexels:photo-1" && c["acquired"]["state"] == "done")
    });
    let item = d.exec("hub.results", json!({}))["candidates"].as_array().unwrap().iter().find(|c| c["id"] == "pexels:photo-1").unwrap()["acquired"]["item"]
        .as_u64()
        .unwrap();
    let rec = d.exec("provenance.get", json!({"item": item}));
    assert_eq!((rec["status"].clone(), rec["record"]["author"].clone()), (json!("clear"), json!("Sam")), "{rec}");
    assert!(d.label("find.get.pexels:photo-1").starts_with("Add"), "the button is still the one that was pressed");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(rig.files.hits() >= 1);
}

#[test]
fn without_a_search_the_panel_explains_what_to_do_and_a_missing_key_points_to_connections() {
    let _g = guard();
    let (_rig, sources) = rig();
    let mut d = Driver::new(sources);
    d.ok("ui.menu.invoke", json!({"id": "window.panel.FindMedia"}));
    d.frames(4);
    // no keys are stored in this process, so the panel offers the way to add them
    assert!(d.has("find.connections"), "{:?}", d.ok("ui.elements", json!({"prefix": "find."})));
    d.click("find.connections");
    assert!(d.ok("ui.inspect", json!({})).to_string().contains("Connections"));
}
