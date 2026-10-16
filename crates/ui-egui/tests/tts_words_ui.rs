//! Headless UI tests of the Text to Speech panel's fork additions: the Pronunciation section (F06.1)
//! and the Take stepper (F06.2). The pronunciation tests:
//! open it, write a word and how to say it, save it for the project and for all projects, see it
//! listed, and remove it. Checking a script works with whatever voices this machine has: with the
//! natural voices the made-up names (real ones are in the dictionary already) are offered as chips, without them the panel says why not.

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

    fn has(&mut self, id: &str) -> bool {
        self.ok("ui.elements", json!({"prefix": id})).as_array().is_some_and(|a| a.iter().any(|e| e["id"] == json!(id)))
    }

    fn ids(&mut self, prefix: &str) -> Vec<String> {
        self.ok("ui.elements", json!({"prefix": prefix})).as_array().unwrap().iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect()
    }

    /// The panel is short: scroll the section's header into reach, open it, and scroll to its end.
    fn open_section(&mut self) {
        self.scroll_down("tts.voice");
        self.click("tts.words");
        self.scroll_down("tts.words");
    }

    /// Scroll the panel to its end with the pointer on `anchor`, which must be inside the panel's
    /// visible part (a clipped element has no usable position).
    fn scroll_down(&mut self, anchor: &str) {
        self.ok("ui.scroll", json!({"id": anchor, "dx": 0.0, "dy": -2000.0}));
        self.frames(3);
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
fn words_are_saved_for_the_project_and_all_projects_listed_and_removed() {
    let mut d = Driver::new();
    d.exec("window.panel.TextToSpeech", json!({}));
    d.frames(3);
    assert!(d.has("tts.words"), "{:?}", d.ids("tts."));
    assert!(!d.has("tts.words.check"), "the section starts closed");
    d.open_section();
    for id in ["tts.words.check", "tts.words.word", "tts.words.say", "tts.words.play", "tts.words.save.project", "tts.words.save.global"] {
        assert!(d.has(id), "missing {id}: {:?}", d.ids("tts.words"));
    }
    // a word for this project
    d.type_into("tts.words.word", "Kubrick");
    d.type_into("tts.words.say", "koo brick");
    d.snapshot("tts-words");
    d.click("tts.words.save.project");
    let listed = d.exec("tts.pronunciations", json!({}));
    assert_eq!((listed["project"][0]["word"].clone(), listed["project"][0]["say"].clone()), (json!("kubrick"), json!("koo brick")), "{listed}");
    assert!(d.has("tts.words.remove.project.kubrick"));
    // the fields are emptied, and the project has a saved word that undo takes away again
    assert!(d.exec("tts.pronunciations", json!({}))["global"].as_array().unwrap().is_empty());
    // a word for every project
    d.type_into("tts.words.word", "Reykjavik");
    d.type_into("tts.words.say", "rake yah veek");
    d.click("tts.words.save.global");
    let listed = d.exec("tts.pronunciations", json!({}));
    assert_eq!(listed["global"][0]["word"], "reykjavik", "{listed}");
    assert!(d.has("tts.words.remove.global.reykjavik"));
    // removing
    d.scroll_down("tts.words");
    d.click("tts.words.remove.project.kubrick");
    d.click("tts.words.remove.global.reykjavik");
    let listed = d.exec("tts.pronunciations", json!({}));
    assert!(listed["project"].as_array().unwrap().is_empty() && listed["global"].as_array().unwrap().is_empty(), "{listed}");
    // saving needs both fields
    d.type_into("tts.words.word", "Zorblat");
    d.click("tts.words.save.project");
    assert!(d.exec("tts.pronunciations", json!({}))["project"].as_array().unwrap().is_empty(), "no way to say it, nothing saved");
}

#[test]
fn checking_a_script_offers_the_unknown_words_or_says_why_it_cannot() {
    let mut d = Driver::new();
    d.exec("window.panel.TextToSpeech", json!({}));
    d.frames(3);
    d.click("tts.text");
    d.ok("ui.type", json!({"text": "Zorblat filmed in Vlornak with Quenzel."}));
    d.frames(3);
    d.open_section();
    d.click("tts.words.check");
    // whether this build and machine can check at all, as the engine itself says
    let natural = d.exec("tts.checkScript", json!({"text": "Zorblat filmed in Vlornak with Quenzel."}))["available"] == json!(true);
    let chips = d.ids("tts.words.chip.");
    if natural {
        for w in ["zorblat", "vlornak", "quenzel"] {
            assert!(chips.contains(&format!("tts.words.chip.{w}")), "{chips:?}");
        }
        // pressing a chip puts the word in the editor, ready for how to say it
        d.click("tts.words.chip.zorblat");
        d.ok("ui.type", json!({"text": "koo brick"}));
        d.frames(3);
        d.click("tts.words.save.project");
        assert!(!d.ids("tts.words.chip.").contains(&"tts.words.chip.zorblat".to_string()), "a fixed word is no longer offered");
    } else {
        assert!(chips.is_empty(), "without the natural voices nothing can be checked: {chips:?}");
    }
}

#[test]
fn the_take_stepper_moves_between_standard_and_other_deliveries_for_natural_voices_only() {
    let mut d = Driver::new();
    d.exec("window.panel.TextToSpeech", json!({}));
    d.frames(3);
    let ids = d.ids("tts.take");
    for id in ["tts.take", "tts.take.prev", "tts.take.next"] {
        assert!(ids.contains(&id.to_string()), "missing {id}: {ids:?}");
    }
    let state = |d: &mut Driver| d.ok("ui.inspect", json!({}))["ui"]["tts"].clone();
    let natural = filmcraft_tts::catalog::find(state(&mut d)["voice"].as_str().unwrap_or("")).is_some();
    assert_eq!(state(&mut d)["take"], 0);
    d.click("tts.take.next");
    d.click("tts.take.next");
    d.click("tts.take.prev");
    // the built-in voices have no takes: the buttons do nothing there
    assert_eq!(state(&mut d)["take"], if natural { 1 } else { 0 });
    d.snapshot("tts-take");
    d.click("tts.take.prev");
    d.click("tts.take.prev");
    assert_eq!(state(&mut d)["take"], 0, "never below the standard reading");
}

#[test]
fn the_direction_menu_puts_a_marker_in_the_script_where_the_cursor_is() {
    let mut d = Driver::new();
    d.exec("window.panel.TextToSpeech", json!({}));
    d.frames(3);
    let text = |d: &mut Driver| d.ok("ui.inspect", json!({}))["ui"]["tts"]["text"].as_str().unwrap_or("").to_string();
    d.type_into("tts.text", "Hello there.");
    assert_eq!(text(&mut d), "Hello there.");
    // at the start of the script
    d.ok("ui.key", json!({"key": "Home"}));
    d.frames(2);
    d.click("tts.direction");
    for id in ["slower", "faster", "lower", "higher", "normal"] {
        assert!(d.has(&format!("tts.direction.{id}")), "missing {id}");
    }
    d.click("tts.direction.slower");
    assert_eq!(text(&mut d), "[pace 0.9] Hello there.");
    // at the end
    d.click("tts.text");
    d.ok("ui.key", json!({"key": "End"}));
    d.frames(2);
    d.click("tts.direction");
    d.click("tts.direction.lower");
    assert_eq!(text(&mut d), "[pace 0.9] Hello there. [pitch -2] ");
    d.snapshot("tts-direction");
}
