//! Headless UI test of the Sound Map and the Spotting Sheet (fork roadmap F13.4): lanes, findings and
//! a click that moves the playhead; and a beat's bed, music and cues edited in place, put on the
//! timeline as markers, and refused when a time is wrong.

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

fn wait_for(d: &mut Driver, what: &str, mut done: impl FnMut(&mut Driver) -> bool) {
    let t0 = std::time::Instant::now();
    while !done(d) {
        assert!(t0.elapsed().as_secs() < 90, "{what}: never happened");
        d.frames(2);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn open(d: &mut Driver, panel: &str) {
    d.ok("ui.panel.show", json!({"panel": panel}));
    d.exec("window.floatPanel", json!({"panel": panel, "rect": [40.0, 40.0, 1000.0, 940.0]}));
    d.frames(6);
}

fn playhead(d: &mut Driver) -> f64 {
    d.harness.state().session.playhead().0 as f64 / filmcraft_time::TICKS_PER_SECOND as f64
}

fn label_of(d: &mut Driver, id: &str) -> String {
    let e = d.ok("ui.elements", json!({"prefix": id}));
    e.as_array().and_then(|a| a.iter().find(|x| x["id"] == json!(id)).cloned()).map(|x| x["label"].as_str().unwrap_or("").to_string()).unwrap_or_default()
}

/// The demo project with every audio clip tagged as narration: no music, no ambience.
fn tagged_demo() -> Driver {
    let mut d = Driver::new();
    let ids: Vec<u64> = {
        let q = d.harness.state().session.active_sequence().unwrap();
        q.audio_tracks.iter().flat_map(|t| t.items.iter().map(|c| c.id.0)).collect()
    };
    assert!(!ids.is_empty(), "the demo has sound");
    d.exec("essentialSound.setType", json!({"clips": ids, "type": "dialogue"}));
    d
}

#[test]
fn the_sound_map_shows_the_lanes_names_the_missing_bed_and_a_click_moves_the_playhead() {
    let mut d = tagged_demo();
    open(&mut d, "SoundMap");
    for lane in ["narration", "music", "ambience", "effects"] {
        assert!(d.has(&format!("soundmap.lane.{lane}")), "missing the {lane} lane");
    }
    assert!(!d.has("soundmap.lane.other"), "everything is sorted");
    assert!(d.has("soundmap.measure"));
    // the gap is named, with a timecode that goes there
    let title = label_of(&mut d, "soundmap.row.0");
    assert!(title.to_lowercase().contains("ambience"), "{title}");
    assert!(d.has("soundmap.finding.0") && d.has("soundmap.seek.0"));
    d.snapshot("soundmap-unmeasured");
    d.harness.state_mut().session.set_playhead(filmcraft_time::Tick(12 * filmcraft_time::TICKS_PER_SECOND));
    d.frames(2);
    d.click("soundmap.seek.0");
    assert!(playhead(&mut d) < 0.5, "the finding is at the start: {}", playhead(&mut d));
    // a click in a lane goes to that place in the video (the lane's middle is about half way)
    d.click("soundmap.lane.narration");
    let dur = d.exec("sound.map", json!({}))["durationSeconds"].as_f64().unwrap();
    assert!((playhead(&mut d) - dur / 2.0).abs() < dur * 0.15, "{} of {dur}", playhead(&mut d));
    // measure: a job with progress, then the curves
    d.click("soundmap.measure");
    wait_for(&mut d, "the measurement", |d| d.exec("sound.map", json!({}))["measured"] == json!(true));
    d.frames(8);
    assert!(!d.has("soundmap.progress"), "the job is over");
    let m = d.exec("sound.map", json!({}));
    assert!(m["lanes"][0]["levels"].as_array().is_some_and(|a| !a.is_empty()));
    d.snapshot("soundmap-measured");
    let events = d.exec("events.list", json!({}));
    assert_eq!(events["counts"]["error"], json!(0), "{events}");
}

#[test]
fn clips_nobody_sorted_are_pointed_out_and_an_empty_timeline_says_so() {
    let mut d = Driver::new();
    open(&mut d, "SoundMap");
    assert!(d.has("soundmap.lane.other"), "the demo's sound is not sorted yet");
    assert!(d.has("soundmap.unsorted"));
    let mut e = Driver::new();
    e.exec("file.newSequence", json!({"name": "blank", "audio": 2, "video": 1}));
    open(&mut e, "SoundMap");
    assert!(e.has("soundmap.empty"));
    assert!(!e.has("soundmap.measure"));
}

fn mystery(d: &mut Driver) -> Vec<u64> {
    d.exec("planner.create", json!({"pack": "mystery", "minutes": 10}));
    d.exec("planner.get", json!({}))["plan"]["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["beats"].as_array().unwrap().iter().map(|b| b["id"].as_u64().unwrap()))
        .collect()
}

fn first_beat_sound(d: &mut Driver) -> Value {
    d.exec("planner.get", json!({}))["plan"]["sections"][0]["beats"][0]["sound"].clone()
}

#[test]
fn without_a_plan_the_spotting_sheet_says_how_to_make_one() {
    let mut d = Driver::new();
    open(&mut d, "SpottingSheet");
    assert!(d.has("spot.empty"));
    d.frames(6);
    assert_eq!(d.exec("events.list", json!({}))["counts"]["error"], json!(0), "the panel does not ask for what cannot be had");
    mystery(&mut d);
    d.frames(3);
    assert!(!d.has("spot.empty"));
}

#[test]
fn a_beats_bed_music_and_cues_are_edited_on_the_card_and_put_on_the_timeline() {
    let mut d = Driver::new();
    let beats = mystery(&mut d);
    let b = beats[0];
    open(&mut d, "SpottingSheet");
    assert!(label_of(&mut d, "spot.summary").starts_with("0 of "), "{}", label_of(&mut d, "spot.summary"));
    assert!(d.has(&format!("spot.beat.{b}")) && d.has(&format!("spot.beat.{b}.ambience")) && d.has(&format!("spot.beat.{b}.musicAdd")));
    d.snapshot("spotting-empty");
    // the bed
    d.type_into(&format!("spot.beat.{b}.ambience"), "wind over a shore");
    d.click(&format!("spot.beat.{b}.addEffect"));
    assert_eq!(first_beat_sound(&mut d)["ambience"], "wind over a shore");
    // music
    d.click(&format!("spot.beat.{b}.musicAdd"));
    d.frames(3);
    d.type_into(&format!("spot.beat.{b}.musicMood"), "uneasy strings");
    d.type_into(&format!("spot.beat.{b}.musicIn"), "0:02");
    d.click(&format!("spot.beat.{b}.musicEnd"));
    d.frames(3);
    assert!(d.has(&format!("spot.beat.{b}.musicOut")), "now it has a time to go out");
    let sound = first_beat_sound(&mut d);
    assert_eq!(sound["music"]["mood"], "uneasy strings");
    assert_eq!(sound["music"]["enters"], 2 * filmcraft_time::TICKS_PER_SECOND);
    assert!(sound["music"]["leaves"].is_i64() || sound["music"]["leaves"].is_u64());
    // the effect added at one second: give it a name and a time
    let cue = sound["cues"][0]["id"].as_u64().unwrap();
    d.type_into(&format!("spot.cue.{cue}.text"), "a bell, far off");
    d.type_into(&format!("spot.cue.{cue}.at"), "0:05");
    d.click(&format!("spot.beat.{b}.ambience"));
    let sound = first_beat_sound(&mut d);
    assert_eq!((sound["cues"][0]["text"].as_str(), sound["cues"][0]["at"].as_i64()), (Some("a bell, far off"), Some(5 * filmcraft_time::TICKS_PER_SECOND)));
    // a silence: switch the cue's kind
    d.click(&format!("spot.cue.{cue}.kind"));
    d.frames(3);
    assert!(d.has(&format!("spot.cue.{cue}.length")), "a silence has a length");
    assert_eq!(first_beat_sound(&mut d)["cues"][0]["kind"], "silence");
    d.type_into(&format!("spot.cue.{cue}.length"), "3");
    d.click(&format!("spot.beat.{b}.ambience"));
    assert_eq!(first_beat_sound(&mut d)["cues"][0]["length"], 3 * filmcraft_time::TICKS_PER_SECOND);
    assert!(label_of(&mut d, "spot.summary").starts_with("1 of "));
    d.snapshot("spotting-planned");
    // a time that is not a time is refused with words, and nothing changes
    d.type_into(&format!("spot.cue.{cue}.at"), "soon");
    d.click(&format!("spot.beat.{b}.ambience"));
    assert!(d.has("spot.error"), "the complaint");
    assert_eq!(first_beat_sound(&mut d)["cues"][0]["at"], 5 * filmcraft_time::TICKS_PER_SECOND);
    // the plan on the timeline as markers, and taken off again
    let marks = |d: &mut Driver| d.harness.state().session.active_sequence().unwrap().markers.iter().filter(|m| m.name.starts_with("Sound · ")).count();
    d.click("spot.place");
    assert_eq!(marks(&mut d), 3, "the bed, the music and the silence");
    d.click("spot.clear");
    assert_eq!(marks(&mut d), 0);
    // a click on the beat's header goes there
    d.harness.state_mut().session.set_playhead(filmcraft_time::Tick(9 * filmcraft_time::TICKS_PER_SECOND));
    d.click(&format!("spot.beat.{b}"));
    assert!(playhead(&mut d) < 0.5);
    // removing the cue
    d.click(&format!("spot.cue.{cue}.remove"));
    assert!(first_beat_sound(&mut d)["cues"].as_array().unwrap().is_empty());
    d.click(&format!("spot.beat.{b}.musicRemove"));
    assert!(first_beat_sound(&mut d)["music"].is_null());
    assert!(d.has("spot.export") && d.has("spot.copy"));
}
