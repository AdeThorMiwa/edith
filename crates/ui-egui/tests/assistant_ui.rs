//! Headless UI tests of the Assistant panel (fork roadmap F03): opening it, the not-connected hint,
//! a question that streams in with a tool card and a cost, Stop, choosing the default model, and the
//! context chips. The model is scripted, so nothing here touches the network or the keychain.

#![allow(dead_code)]

use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use edith_ai::scripted::ScriptedModel;
use edith_ai::{Cancel, Capabilities, ChatModel, LlmError, Message, ModelInfo, Part, Request, Response, Role, StopReason, StreamEvent, Usage};
use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

/// The in-memory secret store is process-wide: one test at a time.
static LOCK: Mutex<()> = Mutex::new(());

fn guard() -> MutexGuard<'static, ()> {
    let g = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    // Never the real keychain.
    let _ = edith_net::secrets::install(edith_net::secrets::in_memory().unwrap());
    g
}

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    snapshots: Option<std::path::PathBuf>,
}

impl Driver {
    fn new() -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        Self::with(session)
    }

    fn with(session: Session) -> Self {
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        // `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` also renders offscreen with wgpu and writes `assistant-*.png`.
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

    fn elements(&mut self, prefix: &str) -> Vec<Value> {
        self.ok("ui.elements", json!({"prefix": prefix})).as_array().cloned().unwrap_or_default()
    }

    fn has(&mut self, id: &str) -> bool {
        self.elements(id).iter().any(|e| e["id"] == json!(id))
    }

    fn label(&mut self, id: &str) -> String {
        self.elements(id).iter().find(|e| e["id"] == json!(id)).and_then(|e| e["label"].as_str().map(str::to_string)).unwrap_or_default()
    }

    /// The labels of all conversation entries and tool cards, in order of their ids.
    fn entries(&mut self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .elements("assistant.")
            .into_iter()
            .filter(|e| e["id"].as_str().is_some_and(|i| i.starts_with("assistant.entry.") || i.starts_with("assistant.tool.")))
            .map(|e| (e["id"].as_str().unwrap_or("").to_string(), e["label"].as_str().unwrap_or("").to_string()))
            .collect();
        v.sort_by_key(|(id, _)| id.rsplit('.').next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0));
        v
    }

    fn app(&mut self) -> &mut FilmcraftApp {
        self.harness.state_mut()
    }

    fn open_assistant(&mut self) {
        self.ok("ui.menu.invoke", json!({"id": "window.panel.Assistant"}));
        self.frames(4);
    }

    /// Steps frames until `done` is true, or fails after ten seconds.
    fn wait(&mut self, what: &str, mut done: impl FnMut(&mut Driver) -> bool) {
        let started = Instant::now();
        loop {
            if done(self) {
                return;
            }
            assert!(started.elapsed() < Duration::from_secs(10), "timed out waiting for: {what}");
            std::thread::sleep(Duration::from_millis(10));
            self.frames(2);
        }
    }

    fn snapshot(&mut self, name: &str) {
        let Some(dir) = self.snapshots.clone() else { return };
        self.frames(2);
        match self.harness.render() {
            Ok(img) => {
                std::fs::create_dir_all(&dir).unwrap();
                let path = dir.join(format!("{name}.png"));
                img.save(&path).unwrap();
                eprintln!("snapshot: {}", path.display());
            }
            Err(e) => eprintln!("snapshot {name} skipped: {e}"),
        }
    }

    fn type_into_input(&mut self, text: &str) {
        self.click("assistant.input");
        self.ok("ui.type", json!({"text": text}));
        self.frames(2);
    }
}

fn text_reply(t: &str) -> (Vec<StreamEvent>, Response) {
    (
        vec![StreamEvent::Text(t.into())],
        Response {
            message: Message::assistant_text(t),
            stop_reason: StopReason::EndTurn,
            usage: Usage { input_tokens: 2_000, output_tokens: 150, ..Default::default() },
            model: "claude-haiku-5-5".into(),
        },
    )
}

fn tool_reply(id: &str, name: &str, input: Value) -> (Vec<StreamEvent>, Response) {
    (
        vec![],
        Response {
            message: Message { role: Role::Assistant, parts: vec![Part::ToolUse { id: id.into(), name: name.into(), input, input_error: None }] },
            stop_reason: StopReason::ToolUse,
            usage: Usage { input_tokens: 1_500, output_tokens: 40, ..Default::default() },
            model: "claude-haiku-5-5".into(),
        },
    )
}

fn scripted(replies: Vec<(Vec<StreamEvent>, Response)>) -> Arc<ScriptedModel> {
    let m = Arc::new(ScriptedModel::new());
    for (e, r) in replies {
        m.push_reply(e, r);
    }
    m
}

struct WaitsForStop;
impl ChatModel for WaitsForStop {
    fn provider(&self) -> &str {
        "anthropic"
    }
    fn models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(vec![])
    }
    fn stream(&self, _r: &Request, cancel: &Cancel, sink: &mut dyn FnMut(StreamEvent)) -> Result<Response, LlmError> {
        sink(StreamEvent::Text("Let me think about that".into()));
        while !cancel.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(LlmError::Cancelled)
    }
}

#[test]
fn the_panel_opens_from_the_window_menu_and_points_to_connections_when_there_is_no_key() {
    let _g = guard();
    let mut d = Driver::new();
    let menu = d.ok("ui.menu.list", json!({}));
    let window: Vec<String> =
        menu.as_array().unwrap().iter().filter(|m| m["path"] == json!(["Window"])).filter_map(|m| m["label"].as_str().map(str::to_string)).collect();
    assert!(window.iter().any(|w| w == "Assistant"), "{window:?}");
    d.open_assistant();
    for id in [
        "assistant.model",
        "assistant.thread",
        "assistant.new",
        "assistant.input",
        "assistant.send",
        "assistant.chip.selection",
        "assistant.chip.playhead",
        "assistant.connect",
    ] {
        assert!(d.has(id), "{id} missing: {:?}", d.elements("assistant.").iter().map(|e| e["id"].clone()).collect::<Vec<_>>());
    }
    assert!(!d.has("assistant.starter.0"), "no starter prompts until it can answer");
    d.snapshot("assistant-connect");
    d.click("assistant.connect");
    assert!(d.has("panel.tab.Connections"), "Open Connections opens the Connections panel");
}

#[test]
fn a_question_streams_in_with_a_tool_card_an_answer_and_a_cost() {
    let _g = guard();
    let mut d = Driver::new();
    d.app()
        .session
        .assistant
        .use_model(scripted(vec![tool_reply("t1", "sequence_view", json!({"to": 6})), text_reply("Your first shot is **Ocean_Sunset.mp4**.")]));
    d.open_assistant();
    assert!(d.has("assistant.starter.0") && d.has("assistant.starter.2"), "starter prompts appear once it can answer");
    d.type_into_input("what is my first shot?");
    d.click("assistant.send");
    d.wait("the answer", |d| d.entries().iter().any(|(_, l)| l.contains("Ocean_Sunset.mp4") && l.contains("first shot")));
    let entries = d.entries();
    assert_eq!(entries[0].1, "what is my first shot?", "{entries:?}");
    let tool = entries.iter().find(|(id, _)| id.starts_with("assistant.tool.")).expect("a tool card");
    assert!(tool.1.starts_with("sequence_view:") && tool.1.contains("Main Edit"), "{tool:?}");
    d.wait("the cost", |d| d.label("assistant.cost").contains("tokens"));
    assert!(d.label("assistant.cost").contains("about $") || d.label("assistant.cost").contains("tokens"), "{}", d.label("assistant.cost"));
    assert!(d.has("assistant.send") && !d.has("assistant.stop"), "back to Send once it has answered");
    d.snapshot("assistant-answer");
    // the engine agrees with what the panel shows
    let thread = d.exec("assistant.threads", json!({}))["threads"][0]["id"].clone();
    assert_eq!(d.exec("assistant.thread", json!({"thread": thread}))["finish"]["kind"], "done");
}

#[test]
fn enter_sends_and_the_starter_prompts_ask_for_you() {
    let _g = guard();
    let mut d = Driver::new();
    d.app().session.assistant.use_model(scripted(vec![text_reply("one"), text_reply("two")]));
    d.open_assistant();
    d.type_into_input("hello");
    d.ok("ui.key", json!({"key": "Enter"}));
    d.wait("the first answer", |d| d.entries().iter().any(|(_, l)| l == "one"));
    assert_eq!(d.entries()[0].1, "hello");
    // a starter prompt works on a fresh thread
    d.click("assistant.new");
    d.click("assistant.starter.0");
    d.wait("the second answer", |d| d.entries().iter().any(|(_, l)| l == "two"));
    assert_eq!(d.entries()[0].1, "Summarize my edit");
}

#[test]
fn stop_is_offered_while_it_works_and_ends_the_turn() {
    let _g = guard();
    let mut d = Driver::new();
    d.app().session.assistant.use_model(Arc::new(WaitsForStop));
    d.open_assistant();
    d.type_into_input("hello");
    d.click("assistant.send");
    d.wait("Stop to appear", |d| d.has("assistant.stop"));
    assert!(!d.has("assistant.send"));
    d.click("assistant.stop");
    d.wait("the turn to end", |d| d.has("assistant.send") && d.entries().iter().any(|(_, l)| l == "Stopped."));
    assert!(d.entries().iter().any(|(_, l)| l.contains("Let me think")), "what it had said is kept");
}

#[test]
fn the_default_model_is_chosen_in_the_panel_and_saved_as_a_setting() {
    let _g = guard();
    let mut d = Driver::new();
    let caps = Capabilities {
        tools: true,
        parallel_tools: true,
        images: true,
        thinking: true,
        effort: true,
        caching: true,
        context_tokens: 1_000_000,
        max_output_tokens: 128_000,
    };
    let info = |id: &str, name: &str| ModelInfo { id: id.into(), display_name: name.into(), capabilities: caps.clone(), price: None };
    d.app().session.assistant.use_model(Arc::new(
        ScriptedModel::new().with_models(vec![info("claude-haiku-5-5", "Claude Haiku 5.5"), info("claude-sonnet-5-5", "Claude Sonnet 5.5")]),
    ));
    d.open_assistant();
    assert_eq!(d.exec("assistant.settings", json!({}))["settings"]["model"], "claude-haiku-5-5");
    d.wait("the model list", |d| {
        d.click("assistant.model");
        d.has("assistant.model.claude-sonnet-5-5")
    });
    d.click("assistant.model.claude-sonnet-5-5");
    d.wait("the setting to change", |d| d.exec("assistant.settings", json!({}))["settings"]["model"] == "claude-sonnet-5-5");
    assert!(d.label("assistant.model").contains("Sonnet"), "{}", d.label("assistant.model"));
}

#[test]
fn the_chips_decide_what_is_sent_along_with_the_question() {
    let _g = guard();
    let mut d = Driver::new();
    let model = scripted(vec![text_reply("a"), text_reply("b")]);
    d.app().session.assistant.use_model(model.clone());
    let clip = d.app().session.active_sequence().unwrap().video_tracks[0].items[0].id;
    d.app().session.state.selection = vec![clip];
    d.open_assistant();
    d.type_into_input("first");
    d.click("assistant.send");
    d.wait("the first answer", |d| d.entries().iter().any(|(_, l)| l == "a"));
    let first = model.requests()[0].messages[0].text();
    assert!(first.contains("[selection]") && first.contains(&format!("#{}", clip.0)) && first.contains("[playhead]"), "{first}");
    // with both chips off nothing but the question goes
    d.click("assistant.chip.selection");
    d.click("assistant.chip.playhead");
    assert_eq!(d.label("assistant.chip.selection"), "Selection off");
    d.type_into_input("second");
    d.click("assistant.send");
    d.wait("the second answer", |d| d.entries().iter().any(|(_, l)| l == "b"));
    let last = model.requests().pop().unwrap().messages.last().unwrap().text();
    assert!(last.contains("second") && !last.contains("[selection]") && !last.contains("[playhead]"), "{last}");
}

#[test]
fn typing_a_slash_offers_the_commands_and_choosing_one_asks_for_it() {
    let _g = guard();
    let mut d = Driver::new();
    let model = scripted(vec![text_reply("The pacing is steady.")]);
    d.app().session.assistant.use_model(model.clone());
    d.open_assistant();
    d.type_into_input("/pa");
    d.wait("the command to be offered", |d| d.has("assistant.slash.pacing"));
    assert!(!d.has("assistant.slash.glossary"), "only commands that match what was typed");
    d.type_into_input("c");
    d.click("assistant.slash.pacing");
    d.ok("ui.key", json!({"key": "Enter"}));
    d.wait("the answer", |d| d.entries().iter().any(|(_, l)| l.contains("steady")));
    assert_eq!(d.entries()[0].1, "/pacing", "the conversation shows what was typed");
    assert!(model.requests()[0].messages[0].text().contains("Review the pacing of the whole edit"));
}

fn data_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("filmcraft-assistant-ui-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn a_conversation_from_an_earlier_run_is_there_when_the_panel_opens() {
    let _g = guard();
    let dir = data_dir("restore");
    // an earlier run of the app: one conversation, saved when it ended
    let mut earlier = Session::default();
    earlier.execute("file.openDemoProject", json!({})).unwrap();
    earlier.assistant.set_dir(&dir);
    earlier.assistant.use_model(scripted(vec![tool_reply("t1", "project_overview", json!({})), text_reply("It is **26 seconds** long.")]));
    earlier.execute("assistant.send", json!({"text": "how long is my edit?", "wait": true})).unwrap();
    // this run
    let mut session = Session::default();
    session.execute("file.openDemoProject", json!({})).unwrap();
    session.assistant.set_dir(&dir);
    let mut d = Driver::with(session);
    d.open_assistant();
    d.wait("the old conversation", |d| d.entries().iter().any(|(_, l)| l.contains("26 seconds")));
    let entries = d.entries();
    assert_eq!(entries[0].1, "how long is my edit?", "{entries:?}");
    assert!(entries.iter().any(|(id, _)| id.starts_with("assistant.tool.")), "its tool card is restored too");
    assert!(d.label("assistant.thread").contains("how long"), "{}", d.label("assistant.thread"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_needs_two_clicks_and_removes_the_conversation_and_its_file() {
    let _g = guard();
    let dir = data_dir("delete");
    let mut session = Session::default();
    session.execute("file.openDemoProject", json!({})).unwrap();
    session.assistant.set_dir(&dir);
    session.assistant.use_model(scripted(vec![text_reply("done")]));
    let id = session.execute("assistant.send", json!({"text": "hello", "wait": true})).unwrap()["thread"].as_u64().unwrap();
    let file = dir.join("edith-threads").join(format!("{id}.json"));
    assert!(file.exists());
    let mut d = Driver::with(session);
    d.open_assistant();
    d.wait("the thread", |d| d.has("assistant.delete"));
    d.click("assistant.delete");
    assert_eq!(d.label("assistant.delete"), "Click again to delete");
    assert!(file.exists() && !d.exec("assistant.threads", json!({}))["threads"].as_array().unwrap().is_empty(), "one click only arms it");
    d.click("assistant.delete");
    assert!(!file.exists());
    assert!(d.exec("assistant.threads", json!({}))["threads"].as_array().unwrap().is_empty());
    assert!(!d.has("assistant.delete"), "with no thread there is nothing to delete");
    let _ = std::fs::remove_dir_all(&dir);
}

fn vision_model(images: bool, replies: Vec<(Vec<StreamEvent>, Response)>) -> Arc<ScriptedModel> {
    let caps = Capabilities {
        tools: true,
        parallel_tools: true,
        images,
        thinking: true,
        effort: true,
        caching: true,
        context_tokens: 1_000_000,
        max_output_tokens: 128_000,
    };
    let m = ScriptedModel::new().with_models(vec![ModelInfo {
        id: "claude-haiku-5-5".into(),
        display_name: "Claude Haiku 5.5".into(),
        capabilities: caps,
        price: None,
    }]);
    for (e, r) in replies {
        m.push_reply(e, r);
    }
    Arc::new(m)
}

#[test]
fn the_transcript_markers_and_frame_chips_send_what_they_name_and_the_message_says_so() {
    let _g = guard();
    let mut d = Driver::new();
    let model = vision_model(true, vec![text_reply("I can see a sunset.")]);
    d.app().session.assistant.use_model(model.clone());
    d.open_assistant();
    d.wait("the model list", |d| d.exec("assistant.models", json!({}))["models"].as_array().is_some_and(|m| !m.is_empty()));
    assert_eq!(d.label("assistant.chip.frame"), "Frame off", "the frame chip starts off: pictures cost more than text");
    for chip in ["transcript", "markers", "frame"] {
        d.click(&format!("assistant.chip.{chip}"));
    }
    assert_eq!(d.label("assistant.chip.frame"), "Frame on");
    d.type_into_input("what is on screen?");
    d.click("assistant.send");
    d.wait("the answer", |d| d.entries().iter().any(|(_, l)| l.contains("sunset")));
    let first = &model.requests()[0].messages[0];
    assert!(first.parts.iter().any(|p| matches!(p, Part::Image { media_type, .. } if media_type == "image/jpeg")), "the frame went with the question");
    assert!(first.text().contains("[markers]") && first.text().contains("[transcript]"), "{}", first.text());
    let entries = d.entries();
    let sent = entries.iter().find(|(_, l)| l.starts_with("Sent along:")).expect("a line saying what was sent");
    for label in ["playhead", "transcript", "markers", "frame"] {
        assert!(sent.1.contains(label), "{label} missing from {:?}", sent.1);
    }
    d.snapshot("assistant-frame");
}

#[test]
fn a_model_that_cannot_see_has_the_frame_chip_disabled() {
    let _g = guard();
    let mut d = Driver::new();
    d.app().session.assistant.use_model(vision_model(false, vec![]));
    d.open_assistant();
    d.wait("the model list", |d| d.exec("assistant.models", json!({}))["models"].as_array().is_some_and(|m| !m.is_empty()));
    d.frames(6);
    let chip = d.elements("assistant.chip.frame").into_iter().find(|e| e["id"] == "assistant.chip.frame").expect("the chip is there");
    assert_eq!(chip["label"], "Frame off");
    assert_eq!(chip["enabled"], json!(false), "{chip}");
}
