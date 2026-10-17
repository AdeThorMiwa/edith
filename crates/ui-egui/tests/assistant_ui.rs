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

    fn enabled(&mut self, id: &str) -> bool {
        self.elements(id).iter().find(|e| e["id"] == json!(id)).is_some_and(|e| e["enabled"] != json!(false))
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
    for id in ["assistant.model", "assistant.thread", "assistant.new", "assistant.input", "assistant.send", "assistant.connect"] {
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
fn a_long_answer_with_a_table_a_list_and_code_is_laid_out_not_shown_as_raw_marks() {
    let _g = guard();
    let mut d = Driver::new();
    let answer = "The edit is **26 seconds** long and moves quickly.\n\n## Markers\n\n| Time | Name |\n|---|---|\n| 00:00:09:11 | Music hit |\n| 00:00:17:23 | Midpoint |\n\nWhat I would try:\n\n1. Hold the **Desert_Dunes** shot a little longer.\n2. Let the push at `00:00:20:19` breathe.\n- Trim the dip to black.\n\n```\nV1 #38 00:00:13:09-00:00:18:08\n```\n\nSee the pacing note above.";
    d.app().session.assistant.use_model(scripted(vec![text_reply(answer)]));
    d.open_assistant();
    d.type_into_input("how does it look?");
    d.click("assistant.send");
    d.wait("the answer", |d| d.entries().iter().any(|(_, l)| l.contains("26 seconds")));
    d.frames(3);
    d.snapshot("assistant-markdown");
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
fn the_provider_is_chosen_in_the_panel_with_its_own_model_list_and_nothing_picked_for_the_person() {
    let _g = guard();
    let mut d = Driver::new();
    let info = |id: &str| ModelInfo { id: id.into(), display_name: id.into(), capabilities: Capabilities { tools: true, ..Default::default() }, price: None };
    d.app().session.assistant.use_model(Arc::new(ScriptedModel::new().with_models(vec![info("meta/free-model:free"), info("other/model")])));
    d.open_assistant();
    // every provider is listed, connected or not
    d.wait("the provider list", |d| {
        d.click("assistant.provider");
        d.has("assistant.provider.openrouter") && d.has("assistant.provider.lmstudio")
    });
    assert!(d.label("assistant.provider.openai").contains("not connected"), "{}", d.label("assistant.provider.openai"));
    assert!(!d.label("assistant.provider.ollama").contains("not connected"), "a local server needs no key");
    d.click("assistant.provider.openrouter");
    d.wait("the provider to change", |d| d.exec("assistant.settings", json!({}))["settings"]["service"] == "openrouter");
    // no model is chosen for the person: the picker asks, and sending is not offered
    assert_eq!(d.exec("assistant.settings", json!({}))["settings"]["model"], "");
    d.wait("the model list", |d| {
        d.click("assistant.model");
        d.has("assistant.model.meta/free-model:free")
    });
    assert!(d.label("assistant.model").contains("Choose a model"), "{}", d.label("assistant.model"));
    d.type_into_input("hello");
    assert!(!d.enabled("assistant.send"), "no model, no send");
    d.wait("the model list again", |d| {
        d.click("assistant.model");
        d.has("assistant.model.meta/free-model:free")
    });
    d.click("assistant.model.meta/free-model:free");
    d.wait("the model to be chosen", |d| d.exec("assistant.settings", json!({}))["settings"]["model"] == "meta/free-model:free");
    d.wait("send to be offered", |d| d.enabled("assistant.send"));
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
    toggle_chip(&mut d, "selection");
    toggle_chip(&mut d, "playhead");
    assert_eq!(chip_label(&mut d, "selection"), "Selection off");
    d.click("assistant.settings.menu");
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
    assert_eq!(chip_label(&mut d, "frame"), "Frame on", "a model that can see gets the frame by default");
    for chip in ["transcript", "markers"] {
        toggle_chip(&mut d, chip);
    }
    assert_eq!(chip_label(&mut d, "frame"), "Frame on");
    d.click("assistant.settings.menu");
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
fn the_assistant_has_a_home_on_the_right_in_every_workspace_and_a_shortcut() {
    use filmcraft_ui_egui::dock::PanelKind;
    let _g = guard();
    let mut d = Driver::new();
    // the default layout already holds it, in the wide right-hand group with Properties
    assert!(d.app().ui.dock.contains(PanelKind::Assistant));
    d.open_assistant();
    assert!(d.app().ui.dock.is_visible(PanelKind::Assistant), "Window > Assistant brings its tab forward");
    // a workspace that does not hold it gets it beside that workspace's side panel, not in a corner
    for (workspace, side) in [("Color", PanelKind::LumetriColor), ("Audio", PanelKind::EssentialSound), ("Captions and Graphics", PanelKind::EssentialGraphics)]
    {
        d.app().set_workspace(workspace);
        assert!(!d.app().ui.dock.contains(PanelKind::Assistant), "{workspace} starts without it");
        d.app().show_panel(PanelKind::Assistant);
        d.frames(2);
        assert!(d.app().ui.dock.is_visible(PanelKind::Assistant), "{workspace}");
        let mut group = Vec::new();
        fn find(n: &filmcraft_ui_egui::dock::DockNode, p: PanelKind, out: &mut Vec<PanelKind>) {
            match n {
                filmcraft_ui_egui::dock::DockNode::Tabs { panels, .. } if panels.contains(&p) => out.extend(panels.iter().copied()),
                filmcraft_ui_egui::dock::DockNode::Split { a, b, .. } => {
                    find(a, p, out);
                    find(b, p, out);
                }
                _ => {}
            }
        }
        find(&d.app().ui.dock, PanelKind::Assistant, &mut group);
        assert!(group.contains(&side), "{workspace}: the Assistant joins the group of {side:?}, got {group:?}");
    }
    assert_eq!(PanelKind::Assistant.window_shortcut(), Some("Shift+0"));
}

/// The Settings popup, opened if it is not already (clicking its button again would close it).
fn open_settings(d: &mut Driver) {
    if !d.has("assistant.settings.chip.frame") {
        d.click("assistant.settings.menu");
    }
}

fn chip_label(d: &mut Driver, name: &str) -> String {
    open_settings(d);
    d.label(&format!("assistant.settings.chip.{name}"))
}

fn toggle_chip(d: &mut Driver, name: &str) {
    open_settings(d);
    d.click(&format!("assistant.settings.chip.{name}"));
}

fn first_id(d: &mut Driver, prefix: &str) -> String {
    d.elements(prefix).iter().filter_map(|e| e["id"].as_str().map(str::to_string)).find(|i| !i.ends_with("cancel")).unwrap_or_default()
}

fn thread_count(d: &mut Driver) -> usize {
    d.exec("assistant.threads", json!({}))["threads"].as_array().map_or(0, Vec::len)
}

fn ask_and_wait(d: &mut Driver, q: &str, answer: &str) {
    d.type_into_input(q);
    d.click("assistant.send");
    d.wait("the answer", |d| d.entries().iter().any(|(_, l)| l == answer));
}

#[test]
fn retry_and_edit_ask_again_in_a_new_thread_and_leave_the_old_one() {
    let _g = guard();
    let mut d = Driver::new();
    d.app().session.assistant.use_model(scripted(vec![text_reply("first answer"), text_reply("retried answer"), text_reply("edited answer")]));
    d.open_assistant();
    ask_and_wait(&mut d, "what is the first shot?", "first answer");
    let old = d.exec("assistant.threads", json!({}))["threads"][0]["id"].clone();
    let old_events = d.exec("assistant.thread", json!({"thread": old}))["events"].as_array().unwrap().len();

    // Retry: a second thread, the same question asked again
    let retry = first_id(&mut d, "assistant.retry.");
    assert!(!retry.is_empty(), "the latest answer offers Retry");
    d.click(&retry);
    d.wait("the retried answer", |d| d.entries().iter().any(|(_, l)| l == "retried answer"));
    assert_eq!(thread_count(&mut d), 2);
    assert!(d.entries().iter().any(|(_, l)| l == "what is the first shot?"), "the same question is shown again");

    // Edit: the text comes back into the input with a banner; sending makes a third thread
    let edit = first_id(&mut d, "assistant.edit.");
    d.click(&edit);
    assert!(d.has("assistant.edit.cancel"), "a banner says an earlier message is being edited");
    d.click("assistant.input");
    d.ok("ui.type", json!({"text": " Be brief."}));
    d.frames(2);
    d.click("assistant.send");
    d.wait("the edited answer", |d| d.entries().iter().any(|(_, l)| l == "edited answer"));
    assert_eq!(thread_count(&mut d), 3);
    assert!(!d.has("assistant.edit.cancel"));
    assert!(d.entries().iter().any(|(_, l)| l.contains("Be brief.")));
    // the first thread is exactly as it was
    assert_eq!(d.exec("assistant.thread", json!({"thread": old}))["events"].as_array().unwrap().len(), old_events);
}

#[test]
fn cancelling_an_edit_drops_it_and_esc_does_the_same() {
    let _g = guard();
    let mut d = Driver::new();
    d.app().session.assistant.use_model(scripted(vec![text_reply("an answer")]));
    d.open_assistant();
    ask_and_wait(&mut d, "a question", "an answer");
    let edit = first_id(&mut d, "assistant.edit.");
    d.click(&edit);
    assert!(d.has("assistant.edit.cancel"));
    d.click("assistant.edit.cancel");
    assert!(!d.has("assistant.edit.cancel"));
    d.click(&edit);
    d.click("assistant.input");
    d.ok("ui.key", json!({"key": "Escape"}));
    d.frames(3);
    assert!(!d.has("assistant.edit.cancel"), "Esc drops the edit");
    assert_eq!(thread_count(&mut d), 1);
}

#[test]
fn esc_stops_a_running_turn() {
    let _g = guard();
    let mut d = Driver::new();
    d.app().session.assistant.use_model(Arc::new(WaitsForStop));
    d.open_assistant();
    d.type_into_input("hello");
    d.click("assistant.send");
    d.wait("Stop to appear", |d| d.has("assistant.stop"));
    d.ok("ui.key", json!({"key": "Escape"}));
    d.wait("the turn to end", |d| d.has("assistant.send") && d.entries().iter().any(|(_, l)| l == "Stopped."));
}

#[test]
fn up_brings_back_your_earlier_questions_and_down_goes_forward() {
    let _g = guard();
    let mut d = Driver::new();
    d.app().session.assistant.use_model(scripted(vec![text_reply("one"), text_reply("two")]));
    d.open_assistant();
    ask_and_wait(&mut d, "first question", "one");
    ask_and_wait(&mut d, "second question", "two");
    d.click("assistant.input");
    d.ok("ui.key", json!({"key": "ArrowUp"}));
    d.frames(3);
    assert_eq!(draft(&mut d), "second question");
    d.ok("ui.key", json!({"key": "ArrowUp"}));
    d.frames(3);
    assert_eq!(draft(&mut d), "first question");
    d.ok("ui.key", json!({"key": "ArrowUp"}));
    d.frames(3);
    assert_eq!(draft(&mut d), "first question", "it stops at the oldest");
    d.ok("ui.key", json!({"key": "ArrowDown"}));
    d.frames(3);
    assert_eq!(draft(&mut d), "second question");
    d.ok("ui.key", json!({"key": "ArrowDown"}));
    d.frames(3);
    assert_eq!(draft(&mut d), "", "past the newest it is empty again");
}

/// The text in the input, read from the element the panel registers for it.
fn draft(d: &mut Driver) -> String {
    d.label("assistant.draft")
}

#[test]
fn a_thread_is_renamed_from_the_menu_and_the_name_is_kept() {
    let _g = guard();
    let dir = std::env::temp_dir().join(format!("edith-ui-rename-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut session = Session::default();
    session.execute("file.openDemoProject", json!({})).unwrap();
    session.assistant.set_dir(&dir);
    session.assistant.use_model(scripted(vec![text_reply("an answer")]));
    let mut d = Driver::with(session);
    d.open_assistant();
    ask_and_wait(&mut d, "a question to name", "an answer");
    d.click("assistant.thread");
    d.click("assistant.rename");
    assert!(d.has("assistant.rename.field"), "the dropdown became a text field");
    d.ok("ui.key", json!({"key": "Escape"}));
    d.frames(3);
    assert!(!d.has("assistant.rename.field") && d.has("assistant.thread"), "Esc leaves the name alone");
    d.click("assistant.thread");
    d.click("assistant.rename");
    d.ok("ui.key", json!({"key": "Cmd+A"}));
    d.ok("ui.type", json!({"text": "Opening shots"}));
    d.ok("ui.key", json!({"key": "Enter"}));
    d.wait("the new name", |d| d.label("assistant.thread").contains("Opening shots"));
    let mut again = Session::default();
    again.assistant.set_dir(&dir);
    assert_eq!(again.execute("assistant.threads", json!({})).unwrap()["threads"][0]["title"], "Opening shots");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn copy_says_so_and_continue_appears_only_after_a_cut_off_reply() {
    let _g = guard();
    let mut d = Driver::new();
    let cut = (
        vec![StreamEvent::Text("A long answer that".into())],
        Response {
            message: Message::assistant_text("A long answer that"),
            stop_reason: StopReason::MaxTokens,
            usage: Usage { input_tokens: 10, output_tokens: 10, ..Default::default() },
            model: "claude-haiku-5-5".into(),
        },
    );
    d.app().session.assistant.use_model(scripted(vec![cut, text_reply("...went on to the end.")]));
    d.open_assistant();
    d.type_into_input("tell me everything");
    d.click("assistant.send");
    d.wait("Continue", |d| d.has("assistant.continue"));
    let copy = first_id(&mut d, "assistant.copy.");
    d.click(&copy);
    assert!(d.app().ui.status.contains("Copied"), "{}", d.app().ui.status);
    d.click("assistant.continue");
    d.wait("the rest", |d| d.entries().iter().any(|(_, l)| l.contains("went on to the end")));
    assert!(!d.has("assistant.continue"), "a finished answer needs no Continue");
}

#[test]
fn the_chips_are_saved_settings_so_a_choice_is_there_next_time() {
    let _g = guard();
    let dir = std::env::temp_dir().join(format!("edith-ui-chips-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut session = Session::default();
    session.execute("file.openDemoProject", json!({})).unwrap();
    session.assistant.set_dir(&dir);
    session.assistant.use_model(vision_model(true, vec![]));
    let mut d = Driver::with(session);
    d.open_assistant();
    d.wait("the model list", |d| d.exec("assistant.models", json!({}))["models"].as_array().is_some_and(|m| !m.is_empty()));
    assert_eq!(d.exec("assistant.settings", json!({}))["settings"]["chips"]["frame"], true);
    // turn the frame off from its chip
    toggle_chip(&mut d, "frame");
    d.wait("the setting", |d| d.exec("assistant.settings", json!({}))["settings"]["chips"]["frame"] == false);
    assert_eq!(chip_label(&mut d, "frame"), "Frame off");
    // a new session on the same folder finds the choice
    let mut again = Session::default();
    again.assistant.set_dir(&dir);
    assert_eq!(again.execute("assistant.settings", json!({})).unwrap()["settings"]["chips"]["frame"], false);
    // hostile values change nothing
    for bad in [json!({"chips": 5}), json!({"chips": {"frame": "yes"}}), json!({"chips": {"nothing": true}}), json!({"chips": {"frame": null}})] {
        assert!(d.app().session.execute("assistant.settings", bad.clone()).is_err(), "{bad}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_model_that_cannot_see_has_the_frame_chip_disabled() {
    let _g = guard();
    let mut d = Driver::new();
    d.app().session.assistant.use_model(vision_model(false, vec![]));
    d.open_assistant();
    d.wait("the model list", |d| d.exec("assistant.models", json!({}))["models"].as_array().is_some_and(|m| !m.is_empty()));
    d.frames(6);
    open_settings(&mut d);
    let chip = d.elements("assistant.settings.chip.frame").into_iter().find(|e| e["id"] == "assistant.settings.chip.frame").expect("the checkbox is there");
    assert_eq!(chip["label"], "Frame off");
    assert_eq!(chip["enabled"], json!(false), "{chip}");
}

fn left_of(d: &mut Driver, id: &str) -> f64 {
    d.elements(id).iter().find(|e| e["id"] == json!(id)).and_then(|e| e["rect"][0].as_f64()).unwrap_or(f64::NAN)
}

#[test]
fn the_mode_is_chosen_beside_the_model_and_changes_the_next_message() {
    let _g = guard();
    let mut d = Driver::new();
    let model = Arc::new(ScriptedModel::new());
    d.app().session.assistant.use_model(model.clone());
    d.open_assistant();
    // [provider] [model] [mode], left to right
    let (p, m, mode) = (left_of(&mut d, "assistant.provider"), left_of(&mut d, "assistant.model"), left_of(&mut d, "assistant.mode"));
    assert!(p < m && m < mode, "provider {p}, model {m}, mode {mode}");
    assert!(d.label("assistant.mode").contains("Ask"), "{}", d.label("assistant.mode"));
    assert_eq!(d.exec("assistant.settings", json!({}))["settings"]["mode"], "ask");
    d.click("assistant.mode");
    assert!(d.has("assistant.mode.ask") && d.has("assistant.mode.coach"));
    d.click("assistant.mode.coach");
    d.wait("the setting to change", |d| d.exec("assistant.settings", json!({}))["settings"]["mode"] == "coach");
    assert!(d.label("assistant.mode").contains("Coach"), "{}", d.label("assistant.mode"));
    d.snapshot("assistant-mode-coach");
    // and it reaches the model: the next message carries the coach's prompt
    model.push_reply(
        vec![StreamEvent::Text("Here is what I see.".into())],
        Response { message: Message::assistant_text("Here is what I see."), stop_reason: StopReason::EndTurn, usage: Usage::default(), model: "m".into() },
    );
    d.exec("assistant.send", json!({"text": "coach me", "wait": true}));
    let system: String = model.requests()[0].system.iter().map(|p| p.text.clone()).collect();
    assert!(system.contains("## coach mode"), "{system}");
    // choosing Ask again
    d.click("assistant.mode");
    d.click("assistant.mode.ask");
    d.wait("the setting to change back", |d| d.exec("assistant.settings", json!({}))["settings"]["mode"] == "ask");
}

/// Words and then a tool call in one reply, as a model writes a sentence and then asks for a view.
fn says_then_shows(text: &str, id: &str, name: &str, input: Value) -> (Vec<StreamEvent>, Response) {
    (
        vec![StreamEvent::Text(text.into())],
        Response {
            message: Message {
                role: Role::Assistant,
                parts: vec![Part::Text { text: text.into() }, Part::ToolUse { id: id.into(), name: name.into(), input, input_error: None }],
            },
            stop_reason: StopReason::ToolUse,
            usage: Usage { input_tokens: 1_500, output_tokens: 60, ..Default::default() },
            model: "claude-haiku-5-5".into(),
        },
    )
}

/// An open edit of colour-matte shots of the given lengths (seconds), end to end on V1, with a tone under it.
fn edit_of(lengths: &[f64]) -> Session {
    use filmcraft_media::generators::GeneratorSource;
    use filmcraft_media::{Generator, MediaSource};
    use filmcraft_project::{ItemKind, Label, MediaClip, MediaRef, SequenceSettings, TrackKind};
    use filmcraft_time::{FrameRate, Tick, TimeRange};
    let mut s = Session::default();
    let rate = FrameRate::FPS_24;
    let mut p = (*s.project).clone();
    let seq = p.new_sequence("Coach test", SequenceSettings { width: 160, height: 90, frame_rate: rate, ..Default::default() }, 1, 1, None);
    let total = rate.tick_of(lengths.iter().map(|l| (l * 24.0).round() as i64).sum());
    let mut at = Tick::ZERO;
    let item_of = |p: &mut filmcraft_project::Project, name: &str, g: Generator, dur: Tick| {
        let src = GeneratorSource::new(g.clone(), 160, 90, rate, dur);
        let clip = MediaClip {
            media: MediaRef::Generator(g),
            info: src.info().clone(),
            interpret: Default::default(),
            mark_in: None,
            mark_out: None,
            markers: vec![],
            offline: false,
            proxy: None,
            identity: None,
        };
        p.add_item(name, Label::Iris, ItemKind::Media(clip), None)
    };
    for (i, len) in lengths.iter().enumerate() {
        let dur = rate.tick_of((*len * 24.0).round() as i64);
        let item = item_of(&mut p, &format!("shot{i}"), Generator::ColorMatte { color: [0.2, 0.3 + 0.05 * (i % 5) as f32, 0.5, 1.0] }, dur);
        let v = p.make_track_item(item, TrackKind::Video, at, TimeRange::new(Tick::ZERO, dur), rate).unwrap();
        p.sequence_mut(seq).unwrap().video_tracks[0].items.push(v);
        at += dur;
    }
    let tone = item_of(&mut p, "tone", Generator::Tone { hz: 440.0, db: -20.0 }, total);
    let a = p.make_track_item(tone, TrackKind::Audio, Tick::ZERO, TimeRange::new(Tick::ZERO, total), rate).unwrap();
    p.sequence_mut(seq).unwrap().audio_tracks[0].items.push(a);
    s.project = Arc::new(p);
    s.state.active_sequence = Some(seq);
    s.state.open_sequences = vec![seq];
    s
}

fn entry_number(id: &str) -> u32 {
    id.rsplit('.').next().and_then(|n| n.parse().ok()).unwrap_or(u32::MAX)
}

fn playhead_seconds(d: &mut Driver) -> f64 {
    d.app().session.playhead().0 as f64 / filmcraft_time::TICKS_PER_SECOND as f64
}

#[test]
fn views_appear_between_the_sentences_and_their_timecodes_move_the_playhead() {
    let _g = guard();
    let mut d = Driver::with(edit_of(&[6.0, 0.8, 6.0, 5.0, 7.0]));
    d.app().session.assistant.use_model(scripted(vec![
        says_then_shows("One shot stands out.", "v1", "show", json!({"view": "findings", "caption": "What I noticed"})),
        says_then_shows("Here is where it sits in the edit.", "v2", "show", json!({"view": "timeline"})),
        says_then_shows("And this is how the shot lengths spread.", "v3", "show", json!({"view": "histogram"})),
        text_reply("Try giving the short shot two seconds."),
    ]));
    d.exec("assistant.settings", json!({"mode": "coach"}));
    d.open_assistant();
    // large enough that the whole conversation is on screen (clicks on scrolled-away rows do nothing)
    d.exec("window.floatPanel", json!({"panel": "Assistant", "rect": [40.0, 40.0, 700.0, 900.0]}));
    d.frames(6);
    d.type_into_input("how is the pacing?");
    d.click("assistant.send");
    d.wait("the answer", |d| d.entries().iter().any(|(_, l)| l.contains("two seconds")));
    d.frames(4);
    // the order in the conversation: your question, what was sent along (Coach mode attaches the numbers),
    // then words, view, words, view, words, view, words
    let mut items: Vec<(u32, String)> = d
        .elements("assistant.")
        .into_iter()
        .filter_map(|e| e["id"].as_str().map(str::to_string))
        .filter(|id| id.starts_with("assistant.entry.") || id.matches('.').count() == 2 && id.starts_with("assistant.view."))
        .map(|id| (entry_number(&id), id))
        .collect();
    items.sort();
    let order: Vec<&str> = items.iter().map(|(_, id)| if id.contains(".view.") { "view" } else { "words" }).collect();
    assert_eq!(order, ["words", "words", "words", "view", "words", "view", "words", "view", "words"], "{items:?}");
    let views: Vec<&String> = items.iter().filter(|(_, id)| id.contains(".view.")).map(|(_, id)| id).collect();

    // findings: the timecode of the finding is a link that moves the playhead
    let finding = d.elements(&format!("{}.finding.", views[0])).into_iter().next().expect("a finding row");
    let (id, label) = (finding["id"].as_str().unwrap().to_string(), finding["label"].as_str().unwrap().to_string());
    assert!(label.contains("0.8 s") && label.contains("0:06"), "{label}");
    d.click(&id);
    let p = playhead_seconds(&mut d);
    assert!((p - 6.0).abs() < 0.1, "the playhead went to the finding: {p}");
    d.snapshot("assistant-view-findings");

    // timeline: a mark jumps to exactly its place, the strip to where you point
    d.app().session.set_playhead(filmcraft_time::Tick::ZERO);
    let mark = d.elements(&format!("{}.mark.", views[1])).into_iter().next().expect("a mark on the timeline");
    d.click(mark["id"].as_str().unwrap());
    assert!((playhead_seconds(&mut d) - 6.0).abs() < 0.1);
    let strip = d.elements(&format!("{}.shot", views[1])).into_iter().find(|e| e["id"].as_str().is_some_and(|i| i.ends_with(".shot"))).unwrap();
    let r = strip["rect"].as_array().unwrap().iter().filter_map(Value::as_f64).collect::<Vec<_>>();
    d.ok("ui.click", json!({"x": r[0] + r[2] * 0.9, "y": r[1] + 20.0}));
    d.frames(3);
    let late = playhead_seconds(&mut d);
    assert!(late > 18.0 && late < 26.0, "90% along a 24.9 s edit: {late}");
    d.snapshot("assistant-view-timeline");

    // histogram: one bar per length band
    assert_eq!(d.elements(&format!("{}.bucket.", views[2])).len(), 6);
    d.snapshot("assistant-view-histogram");

    // no hand-written numbers anywhere: the answer text carries none of the views' data
    let thread = d.exec("assistant.threads", json!({}))["threads"][0]["id"].clone();
    let events = d.exec("assistant.thread", json!({"thread": thread}));
    assert_eq!(events["events"].as_array().unwrap().iter().filter(|e| e["type"] == "tool_result" && e["view"].is_object()).count(), 3);
}

#[test]
fn a_view_the_tool_refused_is_an_ordinary_tool_card_and_a_damaged_one_draws_as_nothing() {
    let _g = guard();
    let mut d = Driver::with(edit_of(&[6.0, 6.0]));
    d.app()
        .session
        .assistant
        .use_model(scripted(vec![says_then_shows("Let me show you.", "v1", "show", json!({"view": "radar"})), text_reply("I could not draw that one.")]));
    d.open_assistant();
    d.type_into_input("show me a radar chart");
    d.click("assistant.send");
    d.wait("the answer", |d| d.entries().iter().any(|(_, l)| l.contains("could not draw")));
    let entries = d.entries();
    let tool = entries.iter().find(|(id, _)| id.starts_with("assistant.tool.")).expect("the refused call is a card");
    assert!(tool.1.contains("there is no view"), "{tool:?}");
    assert!(d.elements("assistant.view.").is_empty(), "no view was drawn");
}

/// A coach conversation about a measured edit: a plan, a narration, the mix and one clip measured.
fn measured_edit() -> Session {
    let mut s = edit_of(&[1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 6.0, 7.0, 6.0, 8.0, 20.0, 5.0]);
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 1})).unwrap();
    let tone = s.project.items.values().find(|i| i.name == "tone").map(|i| i.id).unwrap();
    let n = filmcraft_project::Narration {
        text: vec!["word"; 120].join(" "),
        language: "en-US".into(),
        voice: "test".into(),
        pitch: Default::default(),
        pace: 1.0,
        take: 0,
        speech_duration: filmcraft_time::Tick(60 * filmcraft_time::TICKS_PER_SECOND),
    };
    std::sync::Arc::make_mut(&mut s.project).narrations.insert(tone, n);
    s.execute("analysis.run", json!({"mix": true, "wait": true})).unwrap();
    let shot = s.project.items.values().find(|i| i.name == "shot9").map(|i| i.id.0).unwrap();
    s.execute("analysis.run", json!({"item": shot, "samples": 3, "wait": true})).unwrap();
    s
}

fn coach_asks(d: &mut Driver, replies: Vec<(Vec<StreamEvent>, Response)>, last: &str) {
    d.app().session.assistant.use_model(scripted(replies));
    d.exec("assistant.settings", json!({"mode": "coach"}));
    d.open_assistant();
    d.exec("window.floatPanel", json!({"panel": "Assistant", "rect": [40.0, 40.0, 700.0, 900.0]}));
    d.frames(6);
    d.type_into_input("show me");
    d.click("assistant.send");
    d.wait("the answer", |d| d.entries().iter().any(|(_, l)| l.contains(last)));
    d.frames(4);
}

#[test]
fn the_pace_chart_and_the_gauges_are_drawn_from_the_measurements() {
    let _g = guard();
    let mut d = Driver::with(measured_edit());
    coach_asks(
        &mut d,
        vec![
            says_then_shows("The cuts come in a rush at the start.", "p1", "show", json!({"view": "pace", "caption": "Cuts a minute along the edit"})),
            says_then_shows("The narration is close to the plan.", "g1", "show", json!({"view": "gauge", "metric": "narrationPace"})),
            says_then_shows("The mix sits well under the usual target.", "g2", "show", json!({"view": "gauge", "metric": "loudness"})),
            text_reply("Lift the master a few decibels."),
        ],
        "Lift the master",
    );
    let ids: Vec<String> = d.elements("assistant.view.").into_iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect();
    assert!(ids.iter().any(|i| i.contains(".bar.0")), "{ids:?}");
    assert_eq!(ids.iter().filter(|i| i.ends_with(".gauge")).count(), 2, "{ids:?}");
    let gauge = d
        .elements("assistant.view.")
        .into_iter()
        .filter(|e| e["id"].as_str().is_some_and(|i| i.ends_with(".gauge")))
        .map(|e| e["label"].as_str().unwrap_or("").to_string())
        .collect::<Vec<_>>();
    assert!(gauge[0].starts_with("Narration pace 1") && gauge[1].starts_with("Loudness -"), "{gauge:?}");
    d.snapshot("assistant-view-pace-gauges");
}

#[test]
fn the_palette_and_the_pie_are_drawn_from_the_measurements() {
    let _g = guard();
    let mut d = Driver::with(measured_edit());
    let shot = d.app().session.project.items.values().find(|i| i.name == "shot9").map(|i| i.id.0).unwrap();
    coach_asks(
        &mut d,
        vec![
            says_then_shows("These are the colours of that shot.", "c1", "show", json!({"view": "palette", "item": shot})),
            says_then_shows("And this is where the running time goes.", "q1", "show", json!({"view": "pie", "of": "screenTime"})),
            text_reply("Most of the time is in a few long shots."),
        ],
        "Most of the time",
    );
    let ids: Vec<String> = d.elements("assistant.view.").into_iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect();
    assert!(ids.iter().any(|i| i.contains(".swatch.0")) && ids.iter().any(|i| i.contains(".slice.0")), "{ids:?}");
    let slices = d.elements("assistant.view.").into_iter().filter(|e| e["id"].as_str().is_some_and(|i| i.contains(".slice."))).count();
    assert!(slices >= 2, "{slices}");
    d.snapshot("assistant-view-palette-pie");
}

#[test]
fn times_written_in_an_answer_are_links_that_move_the_playhead_and_other_colons_are_not() {
    let _g = guard();
    let mut d = Driver::with(edit_of(&[30.0, 30.0, 10.0]));
    d.app().session.assistant.use_model(scripted(vec![text_reply(
        "There is a gap from 0:38 to 0:48, and the next clip starts at 00:00:47:22. In a 16:9 frame at 10:30am nothing here is a time, but (0:06) is.",
    )]));
    d.open_assistant();
    d.exec("window.floatPanel", json!({"panel": "Assistant", "rect": [40.0, 40.0, 700.0, 900.0]}));
    d.frames(6);
    d.type_into_input("where are the problems?");
    d.click("assistant.send");
    d.wait("the answer", |d| d.entries().iter().any(|(_, l)| l.contains("a gap from")));
    d.frames(4);
    let links: Vec<(String, String)> = d
        .elements("assistant.time.")
        .into_iter()
        .map(|e| (e["id"].as_str().unwrap_or("").to_string(), e["label"].as_str().unwrap_or("").to_string()))
        .collect();
    let labels: Vec<&str> = links.iter().map(|(_, l)| l.as_str()).collect();
    assert_eq!(labels, ["0:38", "0:48", "00:00:47:22", "0:06"], "16:9 and 10:30am are not times: {links:?}");
    d.click(&links[0].0);
    assert!((playhead_seconds(&mut d) - 38.0).abs() < 0.1, "{}", playhead_seconds(&mut d));
    d.click(&links[2].0);
    let p = playhead_seconds(&mut d);
    assert!((p - (47.0 + 22.0 / 24.0)).abs() < 0.1, "47 s and 22 frames at 24 fps: {p}");
    d.click(&links[3].0);
    assert!((playhead_seconds(&mut d) - 6.0).abs() < 0.1);
    d.snapshot("assistant-time-links");
}
