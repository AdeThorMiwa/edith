//! Headless UI test of the Storyboard (fork roadmap F09.4): a tile per shot under its beat, with the
//! picture or the written intent, and the shot's type, camera move, length and intent editable.

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
        self.ok("ui.key", json!({"key": "End"}));
        for _ in 0..80 {
            self.ok("ui.key", json!({"key": "Backspace"}));
        }
        self.ok("ui.type", json!({"text": text}));
        self.frames(3);
    }

    fn has(&mut self, id: &str) -> bool {
        self.ok("ui.elements", json!({"prefix": id})).as_array().is_some_and(|a| a.iter().any(|e| e["id"] == json!(id)))
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

fn open_storyboard(d: &mut Driver) {
    d.ok("ui.panel.show", json!({"panel": "Storyboard"}));
    d.exec("window.floatPanel", json!({"panel": "Storyboard", "rect": [40.0, 60.0, 1500.0, 860.0]}));
    d.frames(6);
}

fn shot(d: &mut Driver, id: u64) -> Value {
    let plan = d.exec("planner.get", json!({}));
    plan["plan"]["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["beats"].as_array().unwrap().iter())
        .flat_map(|b| b["shots"].as_array().unwrap().iter())
        .find(|x| x["id"] == json!(id))
        .cloned()
        .unwrap_or(Value::Null)
}

fn first_beat(d: &mut Driver) -> u64 {
    d.exec("planner.get", json!({}))["plan"]["sections"][0]["beats"][0]["id"].as_u64().unwrap()
}

#[test]
fn without_a_plan_the_storyboard_says_how_to_make_one() {
    let mut d = Driver::new();
    open_storyboard(&mut d);
    assert!(d.has("story.empty"));
    d.exec("planner.create", json!({"pack": "mystery", "minutes": 1}));
    d.frames(3);
    assert!(!d.has("story.empty"));
    d.exec("planner.clear", json!({}));
    d.frames(3);
    assert!(d.has("story.empty"));
}

#[test]
fn a_tile_per_shot_with_type_move_length_and_intent_editable_and_shots_added_and_deleted() {
    let mut d = Driver::new();
    d.exec("planner.create", json!({"pack": "mystery", "minutes": 1}));
    let beat = first_beat(&mut d);
    let a = d.exec("planner.addShot", json!({"beat": beat, "intent": "A bell in the fog", "shotType": "wide"}))["shot"].as_u64().unwrap();
    let b = d.exec("planner.addShot", json!({"beat": beat, "intent": "The empty shore", "cameraMove": "push in", "seconds": 4}))["shot"].as_u64().unwrap();
    open_storyboard(&mut d);
    d.exec("events.clear", json!({}));
    for s in [a, b] {
        for f in ["", ".type", ".move", ".length", ".intent", ".delete"] {
            assert!(d.has(&format!("story.shot.{s}{f}")), "missing story.shot.{s}{f}");
        }
    }
    assert!(!d.has(&format!("story.shot.{a}.clear")), "no picture to take off");
    assert!(d.has(&format!("story.addShot.{beat}")));
    d.snapshot("storyboard");
    // edit the kind, the move, the length and the words; each reaches the plan
    d.type_into(&format!("story.shot.{a}.type"), "close-up");
    d.type_into(&format!("story.shot.{a}.move"), "still");
    d.type_into(&format!("story.shot.{a}.length"), "3.5");
    d.type_into(&format!("story.shot.{a}.intent"), "A bell, then nothing");
    d.click(&format!("story.shot.{b}.length"));
    let x = shot(&mut d, a);
    assert_eq!(
        (x["shotType"].as_str(), x["cameraMove"].as_str(), x["intent"].as_str()),
        (Some("close-up"), Some("still"), Some("A bell, then nothing")),
        "{x}"
    );
    assert_eq!(x["target"], json!(254_016_000_000i64 * 7 / 2));
    assert_eq!(shot(&mut d, b)["target"], json!(254_016_000_000i64 * 4), "the other shot is untouched");
    // a length that is not a number is refused and says so, the shot keeps its length
    d.type_into(&format!("story.shot.{a}.length"), "soon");
    d.click(&format!("story.shot.{b}.length"));
    assert_eq!(shot(&mut d, a)["target"], json!(254_016_000_000i64 * 7 / 2));
    assert!(d.ok("ui.inspect", json!({}))["ui"]["status"].as_str().unwrap_or("").contains("seconds"));
    // add a shot, then delete it
    d.click(&format!("story.addShot.{beat}"));
    let count = |d: &mut Driver| d.exec("planner.lines", json!({}))["beats"][0]["shots"].as_array().unwrap().len();
    assert_eq!(count(&mut d), 3);
    let new = d.exec("planner.lines", json!({}))["beats"][0]["shots"][2]["id"].as_u64().unwrap();
    d.click(&format!("story.shot.{new}.delete"));
    assert_eq!(count(&mut d), 2);
    d.snapshot("storyboard-edited");
    let events = d.exec("events.list", json!({}));
    assert_eq!(events["counts"]["error"], json!(0), "{events}");
}

#[test]
fn the_selected_project_clip_becomes_the_picture_and_no_picture_takes_it_off() {
    let mut d = Driver::new();
    d.exec("planner.create", json!({"pack": "mystery", "minutes": 1}));
    let beat = first_beat(&mut d);
    let s = d.exec("planner.addShot", json!({"beat": beat, "intent": "The shore"}))["shot"].as_u64().unwrap();
    open_storyboard(&mut d);
    let item = d
        .harness
        .state()
        .session
        .project
        .items
        .values()
        .find(|i| matches!(&i.kind, filmcraft_project::ItemKind::Media(m) if m.info.video.is_some()))
        .map(|i| i.id.0)
        .expect("a video clip in the demo project");
    d.exec("project.select", json!({"items": [item]}));
    d.click(&format!("story.shot.{s}"));
    assert_eq!(shot(&mut d, s)["source"], json!(item));
    d.frames(6);
    d.snapshot("storyboard-picture");
    assert!(d.has(&format!("story.shot.{s}.clear")));
    d.click(&format!("story.shot.{s}.clear"));
    assert_eq!(shot(&mut d, s)["source"], Value::Null);
    // with nothing selected a click does nothing
    d.exec("project.select", json!({"items": []}));
    d.click(&format!("story.shot.{s}"));
    assert_eq!(shot(&mut d, s)["source"], Value::Null);
}
