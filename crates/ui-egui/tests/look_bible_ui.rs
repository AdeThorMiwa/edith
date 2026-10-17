//! Headless UI test of the Look Bible (fork roadmap F12.7c): make a look, put pictures on it from a
//! file and from the playhead, caption and measure them, compare with the frame, match a clip to a
//! picture, and remove with a second click.

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

fn png(dir: &std::path::Path, name: &str, rgb: [u8; 3]) -> String {
    let path = dir.join(name);
    image::RgbaImage::from_fn(320, 180, |_, _| image::Rgba([rgb[0], rgb[1], rgb[2], 255])).save(&path).unwrap();
    path.to_string_lossy().into_owned()
}

fn wait_for(d: &mut Driver, what: &str, mut done: impl FnMut(&mut Driver) -> bool) {
    let t0 = std::time::Instant::now();
    while !done(d) {
        assert!(t0.elapsed().as_secs() < 60, "{what}: never happened");
        d.frames(2);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn error_text(d: &mut Driver) -> String {
    let e = d.ok("ui.elements", json!({"prefix": "looks.error"}));
    e.as_array()
        .and_then(|a| a.iter().find(|x| x["id"] == json!("looks.error")).cloned())
        .map(|x| x["label"].as_str().unwrap_or("").to_string())
        .unwrap_or_default()
}

fn open_looks(d: &mut Driver) {
    d.ok("ui.panel.show", json!({"panel": "LookBible"}));
    d.exec("window.floatPanel", json!({"panel": "LookBible", "rect": [40.0, 40.0, 900.0, 920.0]}));
    d.frames(6);
}

fn looks(d: &mut Driver) -> Vec<Value> {
    d.exec("look.list", json!({}))["looks"].as_array().cloned().unwrap_or_default()
}

fn enabled(d: &mut Driver, id: &str) -> bool {
    let e = d.ok("ui.elements", json!({"prefix": id}));
    e.as_array().and_then(|a| a.iter().find(|x| x["id"] == json!(id)).cloned()).is_some_and(|x| x["enabled"] != json!(false))
}

#[test]
fn an_empty_library_says_so_and_a_look_is_made_by_name() {
    let mut d = Driver::new();
    open_looks(&mut d);
    assert!(d.has("looks.empty"));
    assert!(!enabled(&mut d, "looks.new.add"), "a look needs a name");
    d.snapshot("looks-empty");
    d.type_into("looks.new.name", "Cold harbour");
    assert!(enabled(&mut d, "looks.new.add"));
    d.click("looks.new.add");
    let l = looks(&mut d);
    assert_eq!(l.len(), 1);
    assert_eq!(l[0]["name"], "Cold harbour");
    let id = l[0]["id"].as_str().unwrap().to_string();
    d.frames(4);
    assert!(!d.has("looks.empty"));
    assert!(d.has(&format!("looks.pick.{id}")));
    for f in [
        "looks.name",
        "looks.notes",
        "looks.aspect",
        "looks.grain",
        "looks.lut",
        "looks.path",
        "looks.rights",
        "looks.addFile",
        "looks.addFrame",
        "looks.remove",
    ] {
        assert!(d.has(f), "missing {f}");
    }
    assert!(!enabled(&mut d, "looks.addFile"), "a file needs a path and a note of why it may be used");
    assert!(!enabled(&mut d, "looks.match"), "nothing to match to yet");
    // the written wishes reach the look when you leave the field
    d.type_into("looks.aspect", "2.39:1");
    d.type_into("looks.grain", "heavy 35 mm");
    d.type_into("looks.notes", "grey light");
    d.click("looks.lut");
    let got = d.exec("look.get", json!({"id": id}));
    assert_eq!((got["aspect"].as_str(), got["grain"].as_str(), got["notes"].as_str()), (Some("2.39:1"), Some("heavy 35 mm"), Some("grey light")), "{got}");
}

#[test]
fn pictures_are_added_measured_captioned_compared_matched_and_removed() {
    let dir = std::env::temp_dir().join(format!("looks-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut d = Driver::new();
    open_looks(&mut d);
    d.type_into("looks.new.name", "Teal and orange");
    d.click("looks.new.add");
    d.frames(3);
    let look = looks(&mut d)[0]["id"].as_str().unwrap().to_string();
    // a picture from a file
    let teal = png(&dir, "teal.png", [20, 90, 110]);
    d.type_into("looks.path", &teal);
    d.type_into("looks.rights", "made for this test");
    d.type_into("looks.caption", "the harbour");
    assert!(enabled(&mut d, "looks.addFile"));
    d.click("looks.addFile");
    let pics = |d: &mut Driver| d.exec("look.get", json!({"id": look}))["pictures"].as_array().cloned().unwrap_or_default();
    // the file is read in the background: the form holds until the picture arrives
    wait_for(&mut d, "the picture", |d| pics(d).len() == 1);
    d.frames(4);
    assert!(!d.has("looks.progress"), "the job is over");
    assert!(!enabled(&mut d, "looks.addFile"), "the form was cleared, so there is nothing to add now");
    let first = pics(&mut d)[0]["id"].as_str().unwrap().to_string();
    assert_eq!(pics(&mut d)[0]["caption"], "the harbour");
    assert_eq!(pics(&mut d)[0]["rights"], "made for this test");
    d.frames(6);
    assert!(d.has(&format!("looks.pic.{first}")));
    assert!(d.has("looks.numbers") && d.has("looks.swatch.0"), "what the picture measures");
    d.snapshot("looks-one-picture");
    // a second picture: the frame at the playhead
    d.click("looks.addFrame");
    let all = pics(&mut d);
    assert_eq!(all.len(), 2);
    assert_eq!(all[1]["source"], "Program frame");
    let second = all[1]["id"].as_str().unwrap().to_string();
    d.frames(6);
    // caption edit reaches the library
    d.type_into(&format!("looks.pic.{second}.caption"), "my frame");
    d.click("looks.name");
    assert_eq!(pics(&mut d)[1]["caption"], "my frame");
    // a file that is not a picture says why and keeps nothing
    let junk = dir.join("junk.png");
    std::fs::write(&junk, "not a picture").unwrap();
    d.type_into("looks.path", &junk.to_string_lossy());
    d.type_into("looks.rights", "x");
    d.click("looks.addFile");
    wait_for(&mut d, "the complaint", |d| d.has("looks.error"));
    assert!(error_text(&mut d).contains("junk.png"), "{}", error_text(&mut d));
    assert_eq!(pics(&mut d).len(), 2);
    d.type_into("looks.path", &teal);
    d.type_into("looks.seconds", "soon");
    d.click("looks.addFile");
    assert!(error_text(&mut d).contains("seconds"), "a moment that is not a number: {}", error_text(&mut d));
    assert_eq!(pics(&mut d).len(), 2);
    // compare with the frame at the playhead
    d.click("looks.compare");
    d.frames(4);
    let rows = d.ok("ui.elements", json!({"prefix": "looks.row."}));
    assert!(rows.as_array().is_some_and(|a| a.len() >= 4), "{rows}");
    d.snapshot("looks-compare");
    // match: pick the teal picture, with a clip selected
    let clip = {
        let s = &d.harness.state().session;
        let q = s.active_sequence().unwrap();
        q.video_tracks[0].item_at(s.playhead()).unwrap().id
    };
    d.harness.state_mut().session.state.selection = vec![clip];
    d.frames(2);
    assert!(enabled(&mut d, "looks.match"), "the picture just added is picked");
    d.click(&format!("looks.pic.{first}"));
    assert!(enabled(&mut d, "looks.match"));
    d.click("looks.match");
    let wheels_moved = {
        let q = d.harness.state().session.active_sequence().unwrap();
        let e = q.find_item(clip).unwrap().1.effects.iter().find(|e| e.effect == "lumetri").cloned().unwrap();
        ["wheel_shadows", "wheel_midtones", "wheel_highlights"]
            .iter()
            .any(|w| e.param(w).and_then(|p| p.value.as_vec2()).is_some_and(|v| v.x.abs() + v.y.abs() > 1e-3))
    };
    assert!(wheels_moved, "the clip was graded towards the picture");
    assert!(d.ok("ui.inspect", json!({}))["ui"]["status"].as_str().unwrap_or("").contains("Matched"));
    d.exec("edit.undo", json!({}));
    // remove a picture, then the look, each with a second click
    d.click(&format!("looks.pic.{second}.remove"));
    assert_eq!(pics(&mut d).len(), 2, "the first click only asks");
    d.click(&format!("looks.pic.{second}.remove"));
    assert_eq!(pics(&mut d).len(), 1);
    d.click("looks.remove");
    assert_eq!(looks(&mut d).len(), 1);
    d.click("looks.remove");
    assert!(looks(&mut d).is_empty());
    d.frames(3);
    assert!(d.has("looks.empty"));
    // the only error logged is the bad file added on purpose (as a failed job); nothing is polled that can fail
    let events = d.exec("events.list", json!({}));
    assert_eq!(events["counts"]["error"], json!(1), "{events}");
    let failed: Vec<&Value> = events["entries"].as_array().unwrap().iter().filter(|e| e["level"] == "error").collect();
    assert!(failed[0]["message"].as_str().unwrap_or("").contains("junk.png"), "{events}");
    let _ = std::fs::remove_dir_all(&dir);
}
