//! Headless UI test of the References panel (fork roadmap F12.6): add a film by path and a note of
//! rights, watch it measured, see its card, use it as the coach's target, compare, read what its
//! numbers suggest, rename it and remove it with a second click.

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

/// A mono 16-bit WAV of a sine, so the measuring is quick.
fn wav(path: &std::path::Path, seconds: f64) {
    let rate = 16_000u32;
    let n = (f64::from(rate) * seconds) as usize;
    let mut data = Vec::with_capacity(n * 2);
    for i in 0..n {
        let v = (0.2 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / f64::from(rate)).sin() * 32767.0) as i16;
        data.extend(v.to_le_bytes());
    }
    let mut out = Vec::new();
    out.extend(b"RIFF");
    out.extend((36 + data.len() as u32).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(rate.to_le_bytes());
    out.extend((rate * 2).to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(data);
    std::fs::write(path, out).unwrap();
}

fn open_references(d: &mut Driver) {
    d.ok("ui.panel.show", json!({"panel": "References"}));
    d.exec("window.floatPanel", json!({"panel": "References", "rect": [40.0, 40.0, 900.0, 900.0]}));
    d.frames(6);
}

fn wait_for(d: &mut Driver, what: &str, mut done: impl FnMut(&mut Driver) -> bool) {
    let t0 = std::time::Instant::now();
    while !done(d) {
        assert!(t0.elapsed().as_secs() < 60, "{what}: never happened");
        d.frames(2);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn references(d: &mut Driver) -> Vec<Value> {
    d.exec("reference.list", json!({}))["references"].as_array().cloned().unwrap_or_default()
}

#[test]
fn an_empty_library_says_so_and_adding_needs_a_path_and_a_note_of_rights() {
    let mut d = Driver::new();
    open_references(&mut d);
    assert!(d.has("references.empty"));
    for id in ["references.path", "references.browse", "references.name", "references.rights", "references.add", "references.import"] {
        assert!(d.has(id), "missing {id}");
    }
    d.snapshot("references-empty");
    let disabled = |d: &mut Driver| {
        let e = d.ok("ui.elements", json!({"prefix": "references.add"}));
        e.as_array().and_then(|a| a.iter().find(|x| x["id"] == json!("references.add")).cloned()).map(|x| x["enabled"] == json!(false)).unwrap_or(false)
    };
    assert!(disabled(&mut d), "nothing typed yet");
    d.type_into("references.path", "/somewhere/film.mp4");
    assert!(disabled(&mut d), "a path alone is not enough: the note of rights is required");
    d.type_into("references.rights", "public domain");
    assert!(!disabled(&mut d));
}

#[test]
fn a_film_is_measured_shown_as_a_card_adopted_compared_renamed_and_removed() {
    let dir = std::env::temp_dir().join(format!("references-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tone.wav");
    wav(&path, 30.0);
    let mut d = Driver::new();
    open_references(&mut d);
    d.type_into("references.path", &path.to_string_lossy());
    d.type_into("references.rights", "my own recording");
    d.type_into("references.name", "A slow tone");
    d.click("references.add");
    wait_for(&mut d, "the film to be measured", |d| !references(d).is_empty());
    d.frames(4);
    let r = references(&mut d);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["name"], json!("A slow tone"));
    assert_eq!(r[0]["rights"], json!("my own recording"));
    let id = r[0]["id"].as_str().unwrap().to_string();
    assert!(d.has(&format!("references.card.{id}")));
    assert!(!d.has("references.empty"));
    assert!(!d.has("references.progress"), "the job is over");
    d.snapshot("references-card");
    // use as the coach's target, and stop
    assert!(!d.has(&format!("references.adopted.{id}")));
    d.click(&format!("references.adopt.{id}"));
    assert_eq!(d.exec("reference.adopted", json!({}))["adopted"]["id"], json!(id));
    d.frames(3);
    assert!(d.has(&format!("references.adopted.{id}")));
    d.click(&format!("references.adopt.{id}"));
    assert!(d.exec("reference.adopted", json!({}))["adopted"].is_null());
    // what its numbers suggest
    d.click(&format!("references.details.{id}"));
    assert!(d.has(&format!("references.reading.{id}.0")), "at least one reading");
    d.snapshot("references-readings");
    d.click(&format!("references.details.{id}"));
    // your edit against it
    d.click(&format!("references.compare.{id}"));
    d.frames(4);
    let rows = d.ok("ui.elements", json!({"prefix": "references.row."}));
    assert!(rows.as_array().is_some_and(|a| !a.is_empty()), "{rows}");
    d.snapshot("references-compare");
    // rename: the field takes the new name on Enter
    d.click(&format!("references.rename.{id}"));
    d.frames(2);
    assert!(d.has("references.rename.field"));
    d.ok("ui.key", json!({"key": "End"}));
    for _ in 0..30 {
        d.ok("ui.key", json!({"key": "Backspace"}));
    }
    d.ok("ui.type", json!({"text": "Quiet room"}));
    d.ok("ui.key", json!({"key": "Enter"}));
    d.frames(4);
    assert_eq!(references(&mut d)[0]["name"], json!("Quiet room"));
    // remove takes two clicks
    d.click(&format!("references.remove.{id}"));
    assert_eq!(references(&mut d).len(), 1, "the first click only asks");
    d.click(&format!("references.remove.{id}"));
    assert!(references(&mut d).is_empty());
    d.frames(3);
    assert!(d.has("references.empty"));
    let events = d.exec("events.list", json!({}));
    assert_eq!(events["counts"]["error"], json!(0), "{events}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_film_that_cannot_be_measured_says_why_and_the_panel_carries_on() {
    let dir = std::env::temp_dir().join(format!("references-ui-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("notes.mov");
    std::fs::write(&path, "this is not a film").unwrap();
    let mut d = Driver::new();
    open_references(&mut d);
    d.type_into("references.path", &path.to_string_lossy());
    d.type_into("references.rights", "none really");
    d.click("references.add");
    wait_for(&mut d, "the complaint", |d| d.has("references.error"));
    assert!(references(&mut d).is_empty());
    assert!(d.has("references.add"), "the form is still there to try again");
    let _ = std::fs::remove_dir_all(&dir);
}
