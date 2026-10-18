//! Headless UI test of the question and download cards in the Assistant panel (fork roadmap F16.1b): several
//! questions in one turn answered by clicking and sent as one message; a download request that downloads only
//! on Yes, with Show me others opening Find Media. The model is scripted and the stock sites are a local server.

#![allow(dead_code)]

use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use edith_ai::scripted::ScriptedModel;
use edith_ai::{Message, Part, Response, Role, StopReason, StreamEvent, Usage};
use edith_net::NetState;
use edith_net::http::{Client, ClientConfig};
use edith_net::testserver::{Server, response, serve};
use edith_sources::Sources;
use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};
use zeroize::Zeroizing;

static LOCK: Mutex<()> = Mutex::new(());

fn guard() -> MutexGuard<'static, ()> {
    let g = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = edith_net::secrets::install(edith_net::secrets::in_memory().unwrap());
    edith_net::shared().set_offline(false);
    g
}

fn png() -> Vec<u8> {
    let mut out = Vec::new();
    let img = image::RgbaImage::from_pixel(16, 9, image::Rgba([60, 120, 200, 255]));
    image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

struct Rig {
    sites: Server,
    files: Server,
}

fn rig() -> (Rig, Sources) {
    let image = png();
    let files = serve(move |_, _| response(200, &[("content-type", "image/png")], &image));
    let furl = files.url.clone();
    let sites = serve(move |_, req| {
        let json = |b: String| response(200, &[("content-type", "application/json")], b.as_bytes());
        if req.starts_with("GET /v1/search") {
            json(format!(
                r#"{{"total_results":2,"photos":[
                {{"id":1,"width":800,"height":600,"url":"https://www.pexels.com/photo/1/","photographer":"Sam","alt":"A pier at dawn","src":{{"large2x":"{furl}/big1.jpg","medium":"{furl}/t1.png"}}}},
                {{"id":2,"width":800,"height":600,"url":"https://www.pexels.com/photo/2/","photographer":"Lee","alt":"Fog on the water","src":{{"large2x":"{furl}/big2.jpg","medium":"{furl}/t2.png"}}}}]}}"#
            ))
        } else if req.starts_with("GET /apiv2/search") {
            json(format!(
                r#"{{"count":1,"results":[{{"id":77,"name":"Fog horn","username":"sound man","license":"https://creativecommons.org/licenses/by-nc/4.0/","url":"https://freesound.org/s/77/","duration":8,"previews":{{"preview-hq-mp3":"{furl}/horn.mp3"}}}}]}}"#
            ))
        } else {
            response(404, &[], b"")
        }
    });
    let client = Arc::new(Client::new(
        ClientConfig { allow_loopback_http: true, min_host_interval: Duration::ZERO, timeout: Duration::from_secs(5), ..ClientConfig::default() },
        Arc::new(NetState::new()),
    ));
    let sources = Sources::new(client, Arc::new(|_| Ok(Zeroizing::new("KEY-0123456789".to_string())))).with_origin(&sites.url);
    (Rig { sites, files }, sources)
}

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    snapshots: Option<std::path::PathBuf>,
}

impl Driver {
    fn new(sources: Sources) -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        session.hub.use_sources(Arc::new(sources));
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

    fn element(&mut self, id: &str) -> Option<Value> {
        self.ok("ui.elements", json!({"prefix": id})).as_array().and_then(|a| a.iter().find(|e| e["id"] == json!(id)).cloned())
    }

    fn has(&mut self, id: &str) -> bool {
        self.element(id).is_some()
    }

    fn label(&mut self, id: &str) -> String {
        self.element(id).and_then(|e| e["label"].as_str().map(str::to_string)).unwrap_or_default()
    }

    fn wait(&mut self, what: &str, mut done: impl FnMut(&mut Driver) -> bool) {
        let started = Instant::now();
        while !done(self) {
            assert!(started.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
            self.frames(3);
            std::thread::sleep(Duration::from_millis(10));
        }
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

fn tool_reply(calls: &[(&str, &str, Value)]) -> (Vec<StreamEvent>, Response) {
    let parts =
        calls.iter().map(|(id, name, input)| Part::ToolUse { id: (*id).into(), name: (*name).into(), input: input.clone(), input_error: None }).collect();
    (
        vec![],
        Response {
            message: Message { role: Role::Assistant, parts },
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

impl Driver {
    fn enabled(&mut self, id: &str) -> bool {
        self.ok("ui.elements", json!({"prefix": id}))
            .as_array()
            .and_then(|a| a.iter().find(|e| e["id"] == json!(id)).cloned())
            .is_some_and(|e| e["enabled"] != json!(false))
    }

    fn cards(&mut self, thread: u64) -> Value {
        self.exec("assistant.cards", json!({"thread": thread}))
    }
}

fn start(d: &mut Driver, replies: Vec<(Vec<StreamEvent>, Response)>, question: &str) -> (u64, Arc<ScriptedModel>) {
    let model = scripted(replies);
    d.harness.state_mut().session.assistant.use_model(model.clone());
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Assistant"}));
    d.frames(4);
    d.click("assistant.input");
    d.ok("ui.type", json!({"text": question}));
    d.click("assistant.send");
    d.wait("the cards", |d| d.has("assistant.card.c1"));
    let thread = d.exec("assistant.threads", json!({}))["threads"][0]["id"].as_u64().unwrap();
    d.wait("the turn to end", |d| d.exec("assistant.thread", json!({"thread": thread}))["running"] == json!(false));
    d.frames(6);
    (thread, model)
}

#[test]
fn questions_are_answered_by_clicking_and_go_back_as_one_message() {
    let _g = guard();
    let (_rig, sources) = rig();
    let mut d = Driver::new(sources);
    let (thread, model) = start(
        &mut d,
        vec![
            tool_reply(&[
                (
                    "a",
                    "ask_user",
                    json!({"question": "What tone should the film have?", "options": ["Slow and contemplative", {"label": "Tense", "description": "short shots, a low drone"}]}),
                ),
                ("b", "ask_user", json!({"question": "Is the harbour footage yours?", "options": ["Yes", "No"], "allowOther": true})),
            ]),
            text_reply("Two questions for you."),
            text_reply("Slow it is."),
        ],
        "cut my narration",
    );
    // both cards are there with their choices, and one Send under the last
    for id in [
        "assistant.card.c1",
        "assistant.card.c1.option.0",
        "assistant.card.c1.option.1",
        "assistant.card.c2",
        "assistant.card.c2.option.0",
        "assistant.card.c2.other",
        "assistant.cards.send",
    ] {
        assert!(d.has(id), "{id} missing");
    }
    d.snapshot("assistant-cards-questions");
    // choosing is not sending
    let before = d.exec("assistant.thread", json!({"thread": thread}))["next"].clone();
    d.click("assistant.card.c1.option.1");
    d.wait("the choice", |d| d.cards(thread)["cards"][0]["draft"]["label"] == json!("Tense"));
    d.click("assistant.card.c2.other");
    d.ok("ui.type", json!({"text": "Some of it is stock"}));
    d.ok("ui.key", json!({"key": "Enter"}));
    d.wait("their own words", |d| d.cards(thread)["cards"][1]["draft"]["kind"] == json!("other"));
    assert_eq!(d.exec("assistant.thread", json!({"thread": thread}))["next"], before, "nothing sent yet");
    d.snapshot("assistant-cards-chosen");
    // Send answers: one message, then the cards read as answered
    d.click("assistant.cards.send");
    d.wait("the next turn", |_| model.requests().len() == 3);
    d.wait("done", |d| d.exec("assistant.thread", json!({"thread": thread}))["running"] == json!(false));
    let said = model
        .requests()
        .last()
        .unwrap()
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .rev()
        .find_map(|m| m.parts.iter().find_map(|p| if let Part::Text { text, .. } = p { Some(text.clone()) } else { None }))
        .unwrap();
    assert_eq!(
        said,
        "Here are my answers to your questions:\n1. What tone should the film have?: Tense\n2. Is the harbour footage yours?: (in their own words) Some of it is stock"
    );
    d.frames(6);
    assert!(!d.has("assistant.cards.send"), "no group is open any more");
    assert!(
        d.has("assistant.card.c1") && !d.has("assistant.card.c1.option.0") && !d.has("assistant.card.c2.other"),
        "an answered card keeps its question and answer, not its choices"
    );
    d.snapshot("assistant-cards-answered");
}

fn png_bytes() -> Vec<u8> {
    png()
}

#[test]
fn a_download_request_downloads_only_on_yes_and_show_me_others_opens_find_media() {
    let _g = guard();
    let (_rig, sources) = rig();
    let mut d = Driver::new(sources);
    let dir = std::env::temp_dir().join(format!("edith-cards-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    d.exec("file.projectSettings.scratchDisks", json!({"captured": dir.to_string_lossy()}));
    let items = d.harness.state_mut().session.project.items.len();
    let (thread, model) = start(
        &mut d,
        vec![
            tool_reply(&[("m", "media_search", json!({"text": "pier at dawn", "kind": "image"}))]),
            tool_reply(&[
                ("a", "request_download", json!({"candidate": "pexels:photo-1", "why": "It is the harbour the narration opens on."})),
                ("b", "request_download", json!({"candidate": "pexels:photo-2", "why": "A calmer alternative."})),
            ]),
            text_reply("Two clips to choose from."),
            text_reply("Thanks."),
        ],
        "find me the harbour shot",
    );
    for id in ["assistant.card.c1", "assistant.card.c1.yes", "assistant.card.c1.no", "assistant.card.c1.others", "assistant.card.c2.yes"] {
        assert!(d.has(id), "{id} missing");
    }
    // the card says where it is from and what the licence allows
    let label =
        d.ok("ui.elements", json!({"prefix": "assistant.card.c1"})).as_array().unwrap().iter().find(|e| e["id"] == "assistant.card.c1").unwrap()["label"]
            .as_str()
            .unwrap()
            .to_string();
    assert!(label.contains("A pier at dawn"), "{label}");
    d.snapshot("assistant-cards-download");
    assert_eq!(d.harness.state_mut().session.project.items.len(), items, "nothing downloaded yet");
    // Yes on the first: it downloads at once and the Send waits for it
    d.click("assistant.card.c1.yes");
    d.wait("the download to start", |d| d.cards(thread)["cards"][0]["draft"]["kind"] == json!("yes"));
    // the other card: show me others opens Find Media with Edith's words typed in and searched
    d.click("assistant.card.c2.others");
    d.wait("Find Media", |d| d.has("panel.tab.FindMedia") && d.has("find.query"));
    d.wait("its results", |d| d.has("find.card.pexels:photo-1"));
    d.wait("the download", |d| d.cards(thread)["cards"][0]["download"]["state"] == json!("done"));
    assert_eq!(d.harness.state_mut().session.project.items.len(), items + 1);
    // back in the conversation the group can be sent
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Assistant"}));
    d.frames(4);
    d.wait("Send", |d| d.has("assistant.cards.send") && d.enabled("assistant.cards.send"));
    d.snapshot("assistant-cards-downloaded");
    d.click("assistant.cards.send");
    d.wait("the reply", |_| model.requests().len() == 4);
    let said = model
        .requests()
        .last()
        .unwrap()
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .rev()
        .find_map(|m| m.parts.iter().find_map(|p| if let Part::Text { text, .. } = p { Some(text.clone()) } else { None }))
        .unwrap();
    assert!(said.contains("yes, it is now in the project as item") && said.contains("look at other options themselves in Find Media"), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_project_file_is_attached_from_the_menu_and_goes_with_the_message() {
    let _g = guard();
    let (_rig, sources) = rig();
    let mut d = Driver::new(sources);
    let model = scripted(vec![text_reply("I see it.")]);
    d.harness.state_mut().session.assistant.use_model(model.clone());
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Assistant"}));
    d.frames(4);
    let media = |d: &mut Driver, name: &str| {
        d.harness
            .state_mut()
            .session
            .project
            .items
            .values()
            .find(|i| i.name.contains(name) && matches!(i.kind, filmcraft_project::ItemKind::Media(_)))
            .map(|i| i.id.0)
            .unwrap()
    };
    let ocean = media(&mut d, "Ocean_Sunset");
    let neon = media(&mut d, "Neon_Loop");
    assert!(d.has("assistant.attach"));
    // the menu lists the project's own files
    d.click("assistant.attach");
    d.wait("the file list", |d| d.has(&format!("assistant.attach.item.{ocean}")));
    assert!(d.has(&format!("assistant.attach.item.{neon}")), "every media item of the project is listed");
    d.snapshot("assistant-attach-menu");
    d.click(&format!("assistant.attach.item.{ocean}"));
    d.wait("the chip", |d| d.has(&format!("assistant.attached.{ocean}")));
    // a second file, and one comes off again with a click on its chip
    d.click(&format!("assistant.attach.item.{neon}"));
    d.wait("two chips", |d| d.has(&format!("assistant.attached.{neon}")));
    d.click(&format!("assistant.attached.{neon}"));
    d.wait("one chip", |d| !d.has(&format!("assistant.attached.{neon}")) && d.has(&format!("assistant.attached.{ocean}")));
    d.snapshot("assistant-attached");
    // sent with the message, and the chip is spent
    d.click("assistant.input");
    d.ok("ui.type", json!({"text": "is this shot good enough?"}));
    d.click("assistant.send");
    d.wait("the answer", |_| model.requests().len() == 1);
    let text: String = model
        .requests()
        .first()
        .unwrap()
        .messages
        .iter()
        .flat_map(|m| m.parts.iter())
        .filter_map(|p| if let Part::Text { text, .. } = p { Some(text.clone()) } else { None })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("file: Ocean_Sunset") && text.contains(&format!("#{ocean} ")) && !text.contains("Neon_Loop"), "{text}");
    d.wait("the chip to go", |d| !d.has(&format!("assistant.attached.{ocean}")));
}

#[test]
fn a_workflow_is_started_from_the_list_shown_with_its_stage_and_stopped_with_the_cross() {
    let _g = guard();
    let (_rig, sources) = rig();
    let mut d = Driver::new(sources);
    d.harness.state_mut().session.assistant.use_model(scripted(vec![text_reply("ok")]));
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Assistant"}));
    d.frames(4);
    // the shipped workflow is offered, with what it is for as its tip; there is no strip until one is followed
    d.wait("the workflow list", |d| d.has("assistant.workflow.start.rough-cut-from-narration"));
    assert!(!d.has("assistant.workflow"));
    d.snapshot("assistant-workflows-list");
    // choosing it writes a message for the person to read and send
    d.click("assistant.workflow.start.rough-cut-from-narration");
    d.wait("the draft", |d| d.label("assistant.draft").contains("rough-cut-from-narration"));
    // where the project is in it shows as a strip
    d.exec(
        "workflow.progress",
        json!({"workflow": "rough-cut-from-narration", "stage": 2, "status": "waiting", "note": "Six ideas written as beats; waiting for your changes."}),
    );
    d.wait("the strip", |d| d.has("assistant.workflow"));
    let text = d.label("assistant.workflow");
    assert!(text.contains("rough cut from narration") && text.contains("Stage 2 of") && text.contains("waiting for you"), "{text}");
    assert!(d.has("assistant.workflow.clear"));
    d.snapshot("assistant-workflow-strip");
    // the cross stops following it, and the project forgets
    d.click("assistant.workflow.clear");
    d.wait("the strip to go", |d| !d.has("assistant.workflow"));
    assert_eq!(d.exec("workflow.state", json!({}))["active"], false);
    // a workflow of the person's own in the folder joins the list
}

const SCRIPT: &str =
    "The harbour was quiet that morning. [pause 1s] Nobody saw the small boat leave. By noon the fog had lifted, and the water lay flat and bright.";

#[test]
fn a_script_is_offered_a_voice_and_yes_makes_the_narration_on_the_timeline() {
    let _g = guard();
    let (_rig, sources) = rig();
    let mut d = Driver::new(sources);
    let dir = std::env::temp_dir().join(format!("edith-narration-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    d.exec("file.projectSettings.scratchDisks", json!({"captured": dir.to_string_lossy()}));
    let (thread, _model) = start(
        &mut d,
        vec![
            tool_reply(&[("n", "request_narration", json!({"text": SCRIPT, "voice": "basic-female", "why": "It reads as narration."}))]),
            text_reply("Shall I make it?"),
            text_reply("Thank you."),
        ],
        "here is my script",
    );
    for id in ["assistant.card.c1", "assistant.card.c1.yes", "assistant.card.c1.no", "assistant.card.c1.others", "assistant.cards.send"] {
        assert!(d.has(id), "{id} missing");
    }
    let label =
        d.ok("ui.elements", json!({"prefix": "assistant.card.c1"})).as_array().unwrap().iter().find(|e| e["id"] == "assistant.card.c1").unwrap()["label"]
            .as_str()
            .unwrap()
            .to_string();
    assert!(label.starts_with("The harbour was quiet"), "{label}");
    d.snapshot("assistant-narration-offer");
    let narrations = |d: &mut Driver| d.harness.state_mut().session.project.narrations.len();
    assert_eq!(narrations(&mut d), 0, "an offer makes nothing");
    // Yes: the voice is made and a narration appears on the timeline
    d.click("assistant.card.c1.yes");
    d.wait("the narration", |d| d.cards(thread)["cards"][0]["narration"]["state"] == json!("done"));
    assert_eq!(narrations(&mut d), 1);
    d.frames(6);
    d.snapshot("assistant-narration-made");
    d.wait("Send", |d| d.enabled("assistant.cards.send"));
    d.click("assistant.cards.send");
    d.wait("the next turn", |d| {
        d.exec("assistant.thread", json!({"thread": thread}))["running"] == json!(false)
            && d.exec("assistant.thread", json!({"thread": thread}))["events"].as_array().unwrap().iter().any(|e| e["type"] == "cards_answered")
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn choosing_the_voice_yourself_opens_text_to_speech_with_the_words_in_it() {
    let _g = guard();
    let (_rig, sources) = rig();
    let mut d = Driver::new(sources);
    start(
        &mut d,
        vec![tool_reply(&[("n", "request_narration", json!({"text": SCRIPT, "voice": "basic-female"}))]), text_reply("Shall I make it?")],
        "here is my script",
    );
    d.click("assistant.card.c1.others");
    d.wait("Text to Speech", |d| d.has("tts.text"));
    assert_eq!(d.harness.state_mut().ui.tts.text, SCRIPT);
    assert_eq!(d.harness.state_mut().session.project.narrations.len(), 0, "nothing was made");
}
