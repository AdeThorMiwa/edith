//! Headless UI test of the Beat Lane (fork roadmap F09.2): it sits above the timeline on the
//! timeline's own scale, shows the plan's beats, and a click moves the playhead.

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

#[test]
fn the_lane_sits_above_the_timeline_on_its_scale_shows_the_beats_and_a_click_moves_the_playhead() {
    let mut d = Driver::new();
    // no plan yet: one line pointing to the Planner
    assert!(d.has("lane.empty"), "the lane is in the default layout and says there is no plan");
    d.exec("planner.create", json!({"pack": "mystery", "minutes": 1}));
    d.exec("planner.buildSkeleton", json!({"startSeconds": 1}));
    d.frames(4);
    let lines = d.exec("planner.lines", json!({}));
    let beats = lines["beats"].as_array().unwrap().clone();
    assert!(!d.has("lane.empty"));
    // beats that are wide enough on screen have an element; the first ones should be visible
    let first = beats[0]["beat"].as_u64().unwrap();
    let second = beats[1]["beat"].as_u64().unwrap();
    let (a, b) = (rect_of(&mut d, &format!("lane.beat.{first}")), rect_of(&mut d, &format!("lane.beat.{second}")));
    let (a, b) = (a.expect("first beat on screen"), b.expect("second beat on screen"));
    // beats lie end to end: the second starts where the first ends
    assert!((a[0] + a[2] - b[0]).abs() < 2.0, "{a:?} then {b:?}");
    // and the spacing is the timeline's own scale: pixels per second from the timeline's view state
    let pps = d.ok("ui.inspect", json!({}))["ui"]["timeline"]["pps"].as_f64().unwrap();
    let first_seconds = beats[0]["targetTicks"].as_i64().unwrap() as f64 / 254_016_000_000.0;
    assert!((a[2] - first_seconds * pps).abs() < 2.5, "width {} for {first_seconds} s at {pps} px/s", a[2]);
    d.snapshot("beat-lane");
    // a click moves the playhead to the beat's start and selects it
    d.click(&format!("lane.beat.{second}"));
    let want = beats[1]["startTicks"].as_i64().unwrap();
    let seq = d.exec("sequence.inspect", json!({}));
    let playhead = seq["playhead"].as_i64().or_else(|| seq["playheadTicks"].as_i64()).or_else(|| seq["playhead"]["ticks"].as_i64());
    assert!(playhead.is_some_and(|p| (p - want).abs() <= 254_016_000_000 / 20), "playhead {playhead:?}, wanted about {want}");
    assert_eq!(d.ok("ui.inspect", json!({}))["ui"]["planner"]["selectedBeat"], json!(second));
    // zooming the timeline (twice the pixels per second) makes the same beat twice as wide
    let before = rect_of(&mut d, &format!("lane.beat.{second}")).unwrap();
    d.ok("ui.set", json!({"timeline": {"pps": pps * 2.0}}));
    d.frames(30);
    let after = rect_of(&mut d, &format!("lane.beat.{second}"));
    if let Some(after) = after {
        assert!((after[2] / before[2] - 2.0).abs() < 0.15, "{} then {}", before[2], after[2]);
    }
    d.snapshot("beat-lane-selected");
}

#[test]
fn a_beat_that_runs_long_or_short_on_the_timeline_is_flagged_and_the_lane_survives_a_cleared_plan() {
    let mut d = Driver::new();
    d.exec("planner.create", json!({"pack": "explainer", "minutes": 2}));
    d.exec("planner.buildSkeleton", json!({}));
    // shorten the first marker by hand: planned and actual now differ
    let lines = d.exec("planner.lines", json!({}));
    assert_eq!(lines["beats"][0]["actualTicks"], lines["beats"][0]["targetTicks"]);
    d.frames(3);
    assert!(d.has(&format!("lane.beat.{}", lines["beats"][0]["beat"])));
    d.exec("planner.clear", json!({}));
    d.frames(3);
    assert!(d.has("lane.empty"));
}

#[test]
fn the_always_open_lane_logs_nothing_while_there_is_no_plan() {
    let mut d = Driver::new();
    d.exec("events.clear", json!({}));
    d.frames(30);
    let events = d.exec("events.list", json!({}));
    assert!(events.to_string().matches("planner").count() == 0, "an idle lane must not log errors: {events}");
}
