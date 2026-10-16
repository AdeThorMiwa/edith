//! Headless UI test of the Beat Board (fork roadmap F09.3): the plan as cards by section; dragging a
//! card reorders the plan only, and a button rebuilds the markers to match.

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

fn rect_of(d: &mut Driver, id: &str) -> Option<Vec<f64>> {
    d.ok("ui.elements", json!({"prefix": id}))
        .as_array()
        .and_then(|a| a.iter().find(|e| e["id"] == json!(id)).cloned())
        .and_then(|e| e["rect"].as_array().map(|r| r.iter().filter_map(Value::as_f64).collect()))
}

/// The board, floated large so every column is on screen.
fn open_board(d: &mut Driver) {
    d.ok("ui.panel.show", json!({"panel": "BeatBoard"}));
    d.exec("window.floatPanel", json!({"panel": "BeatBoard", "rect": [40.0, 60.0, 1500.0, 860.0]}));
    d.frames(6);
}

/// (section id, [beat ids]) in plan order.
fn order(d: &mut Driver) -> Vec<(u64, Vec<u64>)> {
    let plan = d.exec("planner.get", json!({}));
    plan["plan"]["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["id"].as_u64().unwrap(), s["beats"].as_array().unwrap().iter().map(|b| b["id"].as_u64().unwrap()).collect()))
        .collect()
}

fn markers_in_time_order(d: &mut Driver) -> Vec<String> {
    let seq = d.exec("sequence.inspect", json!({}));
    let mut m: Vec<(i64, String)> = seq["markers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| (m["start"].as_i64().or_else(|| m["time"].as_i64()).unwrap_or(0), m["name"].as_str().unwrap_or("").to_string()))
        .collect();
    m.sort();
    // the plan's own markers; the demo project has others
    m.into_iter().map(|(_, n)| n).filter(|n| n.starts_with("Beat: ")).collect()
}

#[test]
fn without_a_plan_the_board_says_how_to_make_one() {
    let mut d = Driver::new();
    open_board(&mut d);
    assert!(d.has("board.empty"));
    d.exec("planner.create", json!({"pack": "mystery", "minutes": 1}));
    d.frames(3);
    assert!(!d.has("board.empty"));
    d.exec("planner.clear", json!({}));
    d.frames(3);
    assert!(d.has("board.empty"));
}

#[test]
fn dragging_a_card_reorders_the_plan_only_and_rebuild_moves_the_markers_to_match() {
    let mut d = Driver::new();
    d.exec("planner.create", json!({"pack": "mystery", "minutes": 1}));
    d.exec("planner.buildSkeleton", json!({"startSeconds": 1}));
    open_board(&mut d);
    d.exec("events.clear", json!({}));
    let before = order(&mut d);
    let at = before.iter().position(|(_, b)| b.len() >= 3).expect("a section with three beats");
    let first_beats = before[at].1.clone();
    for (sid, beats) in &before {
        assert!(d.has(&format!("board.section.{sid}")) && d.has(&format!("board.addBeat.{sid}")));
        for b in beats {
            assert!(d.has(&format!("board.beat.{b}")), "card for beat {b}");
        }
    }
    assert!(d.has("board.hint"));
    assert!(!d.has("board.rebuild"), "nothing is out of step yet");
    d.snapshot("beat-board");
    let sequence_before = d.exec("sequence.inspect", json!({}));
    let markers_before = markers_in_time_order(&mut d);

    // drag the third beat onto the first card: it goes before it
    let (a, c) = (first_beats[0], first_beats[2]);
    d.ok("ui.drag", json!({"from": {"id": format!("board.beat.{c}")}, "to": {"id": format!("board.beat.{a}")}}));
    d.frames(6);
    let after = order(&mut d);
    assert_eq!(after[at].1[..3], [c, a, first_beats[1]], "{after:?}");
    assert_eq!(after[..at], before[..at], "other sections untouched");
    assert_eq!(after[at + 1..], before[at + 1..], "other sections untouched");
    d.snapshot("beat-board-reordered");

    // the clips and markers did not move, and the board says so
    assert_eq!(d.exec("sequence.inspect", json!({})), sequence_before, "dragging changes the plan only");
    assert!(d.has("board.rebuild"), "the markers are now out of step with the plan");
    d.click("board.rebuild");
    d.frames(4);
    assert!(!d.has("board.rebuild"), "rebuilt");
    let markers_after = markers_in_time_order(&mut d);
    assert_ne!(markers_after, markers_before, "the markers follow the new order");
    let names: Vec<String> = d.exec("planner.get", json!({}))["plan"]["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["beats"].as_array().unwrap().iter().map(|b| b["name"].as_str().unwrap().to_string()).collect::<Vec<_>>())
        .collect();
    assert!(names.iter().zip(&markers_after).all(|(n, m)| m == &format!("Beat: {n}")), "{names:?} vs {markers_after:?}");

    // undo the rebuild, then the move: the plan is back as it was
    d.exec("edit.undo", json!({}));
    d.exec("edit.undo", json!({}));
    assert_eq!(order(&mut d), before);
    let events = d.exec("events.list", json!({}));
    assert_eq!(events["counts"]["error"], json!(0), "{events}");
}

#[test]
fn a_card_dropped_in_another_section_goes_there_and_a_drop_in_place_changes_nothing() {
    let mut d = Driver::new();
    d.exec("planner.create", json!({"pack": "mystery", "minutes": 1}));
    open_board(&mut d);
    let before = order(&mut d);
    let (s0, b0) = before[0].clone();
    let (s1, b1) = before[1].clone();
    // onto the other column's "Add beat" button: the end of that column
    let moved = b0[0];
    d.ok("ui.drag", json!({"from": {"id": format!("board.beat.{moved}")}, "to": {"id": format!("board.addBeat.{s1}")}}));
    d.frames(6);
    let after = order(&mut d);
    assert_eq!(after[0], (s0, b0[1..].to_vec()));
    assert_eq!(after[1].1.last(), Some(&moved), "{after:?}");
    assert_eq!(after[1].1.len(), b1.len() + 1);
    // a little drag that ends on the same card is not an edit
    let history = d.exec("history.list", json!({})).to_string();
    d.ok("ui.drag", json!({"from": {"id": format!("board.beat.{moved}")}, "to": {"id": format!("board.beat.{moved}")}}));
    d.frames(4);
    assert_eq!(order(&mut d), after);
    assert_eq!(d.exec("history.list", json!({})).to_string(), history, "no undo step for a drop in place");
}

#[test]
fn a_section_can_be_renamed_and_a_beat_added_from_the_board() {
    let mut d = Driver::new();
    d.exec("planner.create", json!({"pack": "mystery", "minutes": 1}));
    open_board(&mut d);
    let before = order(&mut d);
    let (sid, beats) = before[0].clone();
    d.type_into(&format!("board.section.{sid}"), "Act Zero");
    d.click(&format!("board.beat.{}", beats[0]));
    let plan = d.exec("planner.get", json!({}));
    assert_eq!(plan["plan"]["sections"][0]["name"], json!("Act Zero"), "{plan}");
    assert_eq!(d.ok("ui.inspect", json!({}))["ui"]["planner"]["selectedBeat"], json!(beats[0]), "a click selects the beat");
    d.click(&format!("board.addBeat.{sid}"));
    let after = order(&mut d);
    assert_eq!(after[0].1.len(), beats.len() + 1);
    assert_eq!(after[0].1[..beats.len()], beats[..]);
}
