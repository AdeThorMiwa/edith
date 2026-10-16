//! Headless UI test of the A/V Script panel (fork roadmap F09.1): the plan's narration a sentence
//! at a time beside its shots, edits that reach the plan (and so every other view), a click on a
//! time that moves the playhead, and sending a beat's narration to Text to Speech.

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

fn narration(d: &mut Driver, beat: u64) -> String {
    let plan = d.exec("planner.get", json!({}));
    plan["plan"]["sections"].as_array().unwrap().iter().flat_map(|s| s["beats"].as_array().unwrap()).find(|b| b["id"] == beat).unwrap()["narration"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn edit_a_sentence_add_a_visual_jump_the_timeline_and_send_to_narration() {
    let mut d = Driver::new();
    // a plan with a beat that has narration and a shot for its second sentence
    d.exec("planner.create", json!({"pack": "mystery", "minutes": 10}));
    let cold = d.exec("planner.get", json!({}))["plan"]["sections"][0]["beats"][0]["id"].as_u64().unwrap();
    d.exec("planner.updateBeat", json!({"beat": cold, "narration": "It rang once. Nobody came to the shore."}));
    let shot = d.exec("planner.addShot", json!({"beat": cold, "intent": "The bell in the fog", "line": 1}))["shot"].as_u64().unwrap();
    d.ok("ui.menu.invoke", json!({"id": "window.panel.AvScript"}));
    d.frames(4);
    let id = |f: &str| format!("script.line.{cold}.{f}");
    for f in ["0.text", "0.time", "0.delete", "1.text", "1.time"] {
        assert!(d.has(&id(f)), "missing {}", id(f));
    }
    for e in [
        format!("script.beat.{cold}"),
        format!("script.send.{cold}"),
        format!("script.addLine.{cold}"),
        format!("script.shot.{shot}.intent"),
        format!("script.shot.{shot}.picture"),
        format!("script.addShot.{cold}.0"),
    ] {
        assert!(d.has(&e), "missing {e}");
    }
    d.snapshot("av-script");
    // edit the first sentence, leave the box: the plan has the new narration, joined
    d.type_into(&id("0.text"), "The bell rang once.");
    d.click(&id("1.time"));
    assert_eq!(narration(&mut d, cold), "The bell rang once. Nobody came to the shore.", "leaving the box made one edit");
    // the shot's words are editable and reach the plan
    d.type_into(&format!("script.shot.{shot}.intent"), "A bell, then the empty shore");
    d.click(&id("1.time"));
    let lines = d.exec("planner.lines", json!({}));
    assert_eq!(lines["beats"][0]["shots"][0]["intent"], "A bell, then the empty shore");
    // clicking a time moves the playhead to that sentence's start, and selects it
    let want = lines["beats"][0]["lines"][1]["startTicks"].as_i64().unwrap();
    let seq = d.exec("sequence.inspect", json!({}));
    let playhead = seq["playhead"].as_i64().or_else(|| seq["playheadTicks"].as_i64()).or_else(|| seq["playhead"]["ticks"].as_i64());
    // the playhead sits on a whole frame, so it is within one frame (at most 1/20 s) of the exact start
    let near = playhead.is_some_and(|p| (p - want).abs() <= 254_016_000_000 / 20);
    assert!(near, "playhead {playhead:?}, wanted about {want}");
    let view = d.ok("ui.inspect", json!({}))["ui"]["planner"].clone();
    assert_eq!((view["selectedBeat"].clone(), view["selectedLine"].clone()), (json!(cold), json!(1)));
    // add a visual and a sentence, then delete the new sentence again
    d.click(&format!("script.addShot.{cold}.0"));
    assert_eq!(d.exec("planner.lines", json!({}))["beats"][0]["lines"][0]["shots"].as_array().unwrap().len(), 1);
    d.click(&format!("script.addLine.{cold}"));
    assert_eq!(d.exec("planner.lines", json!({}))["beats"][0]["lines"].as_array().unwrap().len(), 3);
    d.click(&id("2.delete"));
    assert_eq!(d.exec("planner.lines", json!({}))["beats"][0]["lines"].as_array().unwrap().len(), 2);
    // every edit above is undoable
    let undo = d.exec("planner.get", json!({}));
    d.exec("edit.undo", json!({}));
    assert_ne!(d.exec("planner.get", json!({})), undo);
    d.snapshot("av-script-edited");
    // Narrate puts the beat's narration in Text to Speech (and brings that panel forward)
    d.click(&format!("script.send.{cold}"));
    let tts = d.ok("ui.inspect", json!({}))["ui"]["tts"]["text"].as_str().unwrap_or("").to_string();
    assert_eq!(tts, narration(&mut d, cold));
    assert!(tts.starts_with("The bell rang once."), "{tts}");
}

#[test]
fn without_a_plan_the_panel_says_how_to_make_one() {
    let mut d = Driver::new();
    d.ok("ui.menu.invoke", json!({"id": "window.panel.AvScript"}));
    d.frames(4);
    let found = d.ok("ui.elements", json!({"prefix": "script."}));
    assert!(found.as_array().unwrap().is_empty(), "nothing to edit before a plan exists");
    d.exec("planner.create", json!({"pack": "explainer", "minutes": 6}));
    d.frames(3);
    let found = d.ok("ui.elements", json!({"prefix": "script.beat."}));
    assert!(!found.as_array().unwrap().is_empty());
}
