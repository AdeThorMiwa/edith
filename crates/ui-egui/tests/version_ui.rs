//! Headless UI test of the version banner (fork roadmap F14.3): it appears when a version is open,
//! the version can be renamed, looked at against the original, listed, rewound, committed as one
//! undo step, or discarded (asking twice).

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

fn notes(d: &mut Driver) -> String {
    d.harness.state().session.project.notes.clone()
}

fn undo_len(d: &mut Driver) -> usize {
    d.harness.state().session.history.undo.len()
}

fn set_notes(d: &mut Driver, text: &str) {
    d.exec("project.setNotes", json!({"text": text}));
    d.frames(3);
}

fn label_of(d: &mut Driver, id: &str) -> String {
    let e = d.ok("ui.elements", json!({"prefix": id}));
    e.as_array().and_then(|a| a.iter().find(|x| x["id"] == json!(id)).cloned()).map(|x| x["label"].as_str().unwrap_or("").to_string()).unwrap_or_default()
}

fn enabled(d: &mut Driver, id: &str) -> bool {
    let e = d.ok("ui.elements", json!({"prefix": id}));
    e.as_array().and_then(|a| a.iter().find(|x| x["id"] == json!(id)).cloned()).is_some_and(|x| x["enabled"] != json!(false))
}

fn open_version(d: &mut Driver, title: &str) {
    d.exec("version.create", json!({"title": title}));
    d.frames(4);
}

#[test]
fn there_is_no_banner_until_a_version_is_open_and_it_goes_when_it_is_closed() {
    let mut d = Driver::new();
    assert!(!d.has("version.banner"));
    open_version(&mut d, "Tighten act two");
    assert!(
        d.has("version.banner")
            && d.has("version.title")
            && d.has("version.peek")
            && d.has("version.commit")
            && d.has("version.discard")
            && d.has("version.changes")
    );
    d.snapshot("version-banner-empty");
    d.exec("version.discard", json!({}));
    d.frames(4);
    assert!(!d.has("version.banner"));
    // the Edit menu commands follow what is open
    assert!(d.harness.state_mut().session.execute("version.peek", json!({})).is_err(), "nothing to look at");
    open_version(&mut d, "x");
    assert!(d.harness.state_mut().session.execute("version.create", json!({})).is_err(), "one at a time");
}

#[test]
fn the_version_is_renamed_looked_at_against_the_original_and_listed_in_words() {
    let mut d = Driver::new();
    set_notes(&mut d, "original notes");
    open_version(&mut d, "Notes pass");
    d.type_into("version.title", "Colour pass");
    d.click("version.peek");
    d.click("version.peek");
    assert_eq!(d.harness.state().session.versions.title(), Some("Colour pass"), "the rename reached the version");
    set_notes(&mut d, "changed in the version");
    d.click("version.changes");
    d.frames(4);
    assert!(label_of(&mut d, "version.line.0").contains("Changes the project notes"), "{}", label_of(&mut d, "version.line.0"));
    assert!(d.has("version.step.0"));
    d.snapshot("version-banner-changes");
    // see original: read-only, and it is what main was
    d.click("version.peek");
    assert!(d.harness.state().session.versions.is_peeking());
    assert_eq!(notes(&mut d), "original notes");
    assert!(!d.has("version.title"), "the title is replaced by what you are looking at");
    assert!(!enabled(&mut d, "version.commit"), "nothing is committed from the original's side");
    assert!(d.harness.state_mut().session.execute("project.setNotes", json!({"text": "sneaky"})).is_err());
    d.snapshot("version-banner-original");
    d.click("version.peek");
    assert_eq!(notes(&mut d), "changed in the version");
    assert!(d.has("version.title"));
}

#[test]
fn steps_can_be_rewound_and_a_commit_is_one_undo_step() {
    let mut d = Driver::new();
    set_notes(&mut d, "main");
    let before = undo_len(&mut d);
    open_version(&mut d, "Three notes");
    for t in ["one", "two", "three"] {
        set_notes(&mut d, t);
    }
    d.click("version.changes");
    d.frames(4);
    assert!(d.has("version.step.0.rewind") && d.has("version.step.1.rewind"));
    assert!(!d.has("version.step.2.rewind"), "the last step has nothing after it to take back");
    d.click("version.step.0.rewind");
    assert_eq!(notes(&mut d), "one", "the steps after the first were taken back");
    set_notes(&mut d, "final");
    d.click("version.commit");
    assert!(!d.has("version.banner"));
    assert_eq!(notes(&mut d), "final");
    assert_eq!(undo_len(&mut d), before + 1, "one undo step however many steps it took");
    assert!(d.ok("ui.inspect", json!({}))["ui"]["status"].as_str().unwrap_or("").contains("Committed"));
    d.exec("edit.undo", json!({}));
    assert_eq!(notes(&mut d), "main");
}

#[test]
fn a_discard_asks_twice_and_gives_the_edit_back_as_it_was() {
    let mut d = Driver::new();
    set_notes(&mut d, "main");
    open_version(&mut d, "Maybe");
    set_notes(&mut d, "regret");
    d.click("version.discard");
    assert!(d.harness.state().session.versions.is_open(), "the first click only asks");
    assert!(d.has("version.banner"));
    d.click("version.discard");
    assert!(!d.harness.state().session.versions.is_open());
    assert_eq!(notes(&mut d), "main");
    d.frames(3);
    assert!(!d.has("version.banner"));
    let events = d.exec("events.list", json!({}));
    assert_eq!(events["counts"]["error"], json!(0), "{events}");
}

#[test]
fn saving_is_refused_while_a_version_is_open_and_says_why() {
    let mut d = Driver::new();
    open_version(&mut d, "Open one");
    let r = d.harness.state_mut().session.execute("file.save", json!({}));
    let e = r.expect_err("saving while a version is open").to_string();
    assert!(e.contains("Commit it or discard it first"), "{e}");
}
