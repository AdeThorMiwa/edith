//! Headless UI test of the rights badges in the Project panel (fork roadmap F04): a media item with
//! a provenance record shows a badge that says what it means, one without shows none, and undo
//! takes the badge away.

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
        let v = self.ok("engine.execute", json!({"command": command, "params": params}));
        self.frames(3);
        v
    }

    /// The rights badges on screen: (automation id, label).
    fn badges(&mut self) -> Vec<(String, String)> {
        self.ok("ui.elements", json!({"prefix": ""}))
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e["id"].as_str().is_some_and(|i| i.ends_with(".rights")))
            .map(|e| (e["id"].as_str().unwrap_or("").to_string(), e["label"].as_str().unwrap_or("").to_string()))
            .collect()
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
fn a_badge_shows_for_an_item_with_a_record_and_says_what_it_means() {
    let mut d = Driver::new();
    assert!(d.badges().is_empty(), "no records, no badges");
    let ids: Vec<u64> = d.exec("provenance.list", json!({}))["items"].as_array().unwrap().iter().map(|i| i["item"].as_u64().unwrap()).collect();
    // the demo keeps its clips in bins; bring four to the top level so the panel shows them
    d.exec("project.moveToBin", json!({"items": &ids[..4], "bin": null}));
    d.exec("provenance.set", json!({"item": ids[0], "tag": "own"}));
    d.exec(
        "provenance.set",
        json!({"item": ids[1], "source": "wikimedia", "author": "A. Person", "license": {"name": "CC BY 4.0", "commercial": true, "attributionRequired": true}}),
    );
    d.exec("provenance.set", json!({"item": ids[2], "source": "web", "license": {"name": "CC BY-NC", "commercial": false}}));
    d.exec("provenance.set", json!({"item": ids[3], "tag": "own", "synthetic": {"model": "some-model"}}));
    let badges = d.badges();
    assert_eq!(badges.len(), 4, "{badges:?}");
    let label_of = |id: u64| badges.iter().find(|(i, _)| i.ends_with(&format!(".item.{id}.rights"))).map(|(_, l)| l.clone()).unwrap_or_default();
    assert!(label_of(ids[0]).starts_with("Free to use") && label_of(ids[0]).contains("Your own work"), "{}", label_of(ids[0]));
    assert!(label_of(ids[1]).starts_with("Credit needed") && label_of(ids[1]).contains("by A. Person"), "{}", label_of(ids[1]));
    assert!(label_of(ids[2]).starts_with("Check the terms") && label_of(ids[2]).contains("NOT for commercial use"), "{}", label_of(ids[2]));
    assert!(label_of(ids[3]).starts_with("AI-generated"), "{}", label_of(ids[3]));
    d.snapshot("rights-badges");
    // undo takes the last one away, clearing takes another
    d.exec("edit.undo", json!({}));
    assert_eq!(d.badges().len(), 3);
    d.exec("provenance.set", json!({"item": ids[0], "clear": true}));
    assert_eq!(d.badges().len(), 2);
}

impl Driver {
    fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({"id": id}));
        self.frames(3);
    }

    fn label(&mut self, id: &str) -> String {
        self.ok("ui.elements", json!({"prefix": id}))
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .find(|e| e["id"] == json!(id))
            .and_then(|e| e["label"].as_str().map(str::to_string))
            .unwrap_or_default()
    }

    fn has(&mut self, id: &str) -> bool {
        self.ok("ui.elements", json!({"prefix": id})).as_array().is_some_and(|a| a.iter().any(|e| e["id"] == json!(id)))
    }
}

#[test]
fn the_panel_shows_the_policy_the_check_and_the_credits_and_follows_changes() {
    let mut d = Driver::new();
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Rights"}));
    d.frames(4);
    assert!(d.has("rights.policy.commercial") && d.has("rights.summary"));
    assert!(d.label("rights.summary").contains("to check"), "no records: every clip is a warning: {}", d.label("rights.summary"));
    let ids: Vec<u64> = d.exec("provenance.list", json!({}))["items"].as_array().unwrap().iter().map(|i| i["item"].as_u64().unwrap()).collect();
    // a non-commercial clip is an error under the default (commercial) policy; the panel notices the change
    d.exec("provenance.set", json!({"item": ids[0], "source": "web", "license": {"name": "CC BY-NC", "commercial": false}}));
    d.exec(
        "provenance.set",
        json!({"item": ids[1], "source": "pexels", "author": "Jane Doe", "license": {"name": "CC BY 4.0", "commercial": true, "attributionRequired": true}}),
    );
    d.frames(3);
    assert!(d.label("rights.summary").starts_with("1 to fix"), "{}", d.label("rights.summary"));
    assert!(d.label("rights.finding.0").starts_with("error:"), "errors come first: {}", d.label("rights.finding.0"));
    // turning "commercial" off from the panel changes the project's policy and clears the error
    assert!(d.label("rights.policy.commercial").ends_with("on"));
    d.click("rights.policy.commercial");
    assert_eq!(d.exec("provenance.policy", json!({}))["policy"]["commercial"], false);
    assert!(d.label("rights.summary").starts_with("0 to fix"), "{}", d.label("rights.summary"));
    d.snapshot("rights-panel");
    // the credits block lists the one clip that asks for a credit, in each format
    d.ok("ui.scroll", json!({"id": "rights.summary", "dy": -900.0}));
    d.frames(3);
    assert!(d.has("rights.credits") && d.label("rights.credits").contains("by Jane Doe"), "{}", d.label("rights.credits"));
    d.click("rights.format.youtube");
    assert!(d.label("rights.credits").starts_with("Credits:"), "{}", d.label("rights.credits"));
    d.click("rights.format.markdown");
    assert!(d.label("rights.credits").starts_with("- "), "{}", d.label("rights.credits"));
    d.click("rights.copy");
    // undo of the policy is followed too
    d.exec("edit.undo", json!({}));
    d.exec("edit.undo", json!({}));
    d.exec("edit.undo", json!({}));
    d.frames(3);
    assert!(d.label("rights.summary").contains("to check"), "{}", d.label("rights.summary"));
}
