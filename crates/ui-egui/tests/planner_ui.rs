//! Headless UI test of the Planner panel (fork roadmap F08.4): pick a Format Pack and a length,
//! create the plan, edit a beat, build the skeleton on the timeline, and see the plan survive.

#![allow(dead_code)]

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

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

    fn type_into(&mut self, id: &str, text: &str) {
        self.click(id);
        self.ok("ui.type", json!({"text": text}));
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
fn create_a_mystery_plan_edit_a_beat_and_build_the_skeleton() {
    let mut d = Driver::new();
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Planner"}));
    d.frames(4);
    for id in ["planner.pack", "planner.minutes", "planner.clues", "planner.create"] {
        assert!(d.has(id), "missing {id}");
    }
    assert!(!d.has("planner.skeleton"), "no plan yet");
    d.snapshot("planner-create");
    d.click("planner.create");
    let got = d.exec("planner.get", json!({}));
    assert_eq!(got["exists"], true, "{got}");
    assert_eq!(got["plan"]["pack"], "mystery");
    assert_eq!(got["summary"]["beats"], 12);
    assert_eq!(got["summary"]["plannedTicks"], got["summary"]["targetTicks"], "the beats add up to the length");
    assert_eq!(got["summary"]["targetTicks"], 10 * 60 * 254_016_000_000i64, "ten minutes by default");
    // the panel now shows the plan
    assert!(d.has("planner.skeleton") && d.has("planner.title") && d.has("planner.length"));
    assert!(d.label("planner.total").contains("12 beats"), "{}", d.label("planner.total"));
    let cold = got["plan"]["sections"][0]["beats"][0]["id"].as_u64().unwrap();
    let id = |f: &str| format!("planner.beat.{cold}.{f}");
    for f in ["name", "length", "intensity", "status", "purpose", "narration"] {
        assert!(d.has(&id(f)), "missing {}", id(f));
    }
    // type narration, leave the box: one edit is made
    let undo = d.exec("history.list", json!({})).to_string().len();
    d.type_into(&id("narration"), "It rang once, and nobody came.");
    d.click("planner.title");
    let after = d.exec("planner.get", json!({}));
    assert_eq!(after["plan"]["sections"][0]["beats"][0]["narration"], "It rang once, and nobody came.");
    assert_ne!(d.exec("history.list", json!({})).to_string().len(), undo);
    // change the length of the beat: the plan no longer adds up, and the panel says so, with a fix
    d.click(&id("length"));
    d.ok("ui.key", json!({"key": "End"}));
    for _ in 0..10 {
        d.ok("ui.key", json!({"key": "Backspace"}));
    }
    d.ok("ui.type", json!({"text": "0:45"}));
    d.click("planner.title");
    let changed = d.exec("planner.get", json!({}));
    assert_eq!(changed["plan"]["sections"][0]["beats"][0]["target"], 45 * 254_016_000_000i64);
    assert_ne!(changed["summary"]["plannedTicks"], changed["summary"]["targetTicks"]);
    assert!(d.has("planner.fit"), "a way to fit the beats to the length");
    d.click("planner.fit");
    let fitted = d.exec("planner.get", json!({}));
    assert_eq!(fitted["summary"]["plannedTicks"], fitted["summary"]["targetTicks"]);
    // build the skeleton: markers appear on the timeline, one per beat
    let before = d.exec("sequence.inspect", json!({}))["markers"].as_array().map_or(0, Vec::len);
    d.click("planner.skeleton");
    let markers = d.exec("sequence.inspect", json!({}))["markers"].as_array().map_or(0, Vec::len);
    assert_eq!(markers, before + 12, "a marker for every beat");
    assert!(d.ok("ui.inspect", json!({}))["ui"]["status"].as_str().unwrap_or("").contains("12 markers"));
    d.snapshot("planner-plan");
    // start over
    d.click("planner.clear");
    assert_eq!(d.exec("planner.get", json!({}))["exists"], false);
    assert!(d.has("planner.create"));
}
