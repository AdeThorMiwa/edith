//! Assistant panel (Window ▸ Assistant; fork roadmap F03): ask Edith about your edit. A header with
//! the model and thread pickers, the conversation (messages, tool cards, thinking summaries), the
//! context chips that say what the assistant is given, and the input.
//!
//! The panel holds no conversation state of its own beyond what it draws: the engine's
//! `assistant.*` commands are the truth, and each frame the panel reads the new events of the open
//! thread and folds them into a [`ChatLog`]. The assistant only looks at the project; nothing here
//! can change it.
//!
//! Automation ids: `assistant.input`, `assistant.send`, `assistant.stop`, `assistant.new`,
//! `assistant.model` (and `assistant.model.<id>` for each choice), `assistant.mode` (and `assistant.mode.ask|coach`), `assistant.thread` (and
//! `assistant.thread.<id>`), `assistant.settings.chip.<name>` (what is sent with each question, in Settings),
//! `assistant.draft` (the text being typed, as its label), `assistant.copy.<n>` / `assistant.retry.<n>` / `assistant.edit.<n>` (links under a message), `assistant.edit.cancel`, `assistant.continue`, `assistant.rename` / `assistant.rename.field`, `assistant.starter.<n>`, `assistant.connect`, `assistant.cost`, `assistant.entry.<n>` (the text
//! of each message), `assistant.tool.<n>` (each tool card), `assistant.settings.thinking`,
//! `assistant.settings.effort`, `assistant.settings.steps`, `assistant.settings.cost`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use egui::{Color32, Key, Modifiers, Rect, RichText, vec2};
use serde_json::{Value, json};

use super::assistant_view::{ChatLog, Entry};
use super::assistant_widgets::ActionIcon;
use crate::FilmcraftApp;
use crate::dock::PanelKind;
use crate::theme::Tokens;

/// The warning amber used across panels.
const WARN: Color32 = Color32::from_rgb(0xe8, 0xb3, 0x3c);
/// Seconds between status refreshes while nothing is happening.
const STATUS_EVERY: f64 = 3.0;

struct Chat {
    thread: Option<u64>,
    logs: HashMap<u64, ChatLog>,
    input: String,
    status: Option<Value>,
    status_at: f64,
    models: Option<Value>,
    models_at: f64,
    usage_text: String,
    running: bool,
    error: Option<String>,
    polled_at: f64,
    /// The slash commands (`/pacing`, …) as the engine lists them.
    commands: Vec<Value>,
    /// The thread whose Delete button was pressed once (a second press deletes it).
    delete_armed: Option<u64>,
    /// The latest thread was picked once at the start; after that "no thread" means "a new one".
    picked_initial: bool,
    /// Renaming the open thread: the text so far.
    renaming: Option<String>,
    rename_fresh: bool,
    /// Editing an earlier message (its turn number); Send then makes a new thread.
    editing: Option<usize>,
    /// Which earlier question Up has recalled (0 is the latest).
    hist_pos: Option<usize>,
    /// When the panel was last drawn: a gap means it was just opened, and the input takes the cursor.
    last_shown: f64,
    focus_input: bool,
    /// How the last turn of the open thread ended (`done`, `cut_off`, `step_limit`, …).
    finish_kind: String,
    /// The message whose Copy / Retry / Edit links are showing.
    hover_entry: Option<usize>,
    /// The engine's `assistant.cards` answer for the open thread (the question and download cards waiting for
    /// the person), and when it was read (fork roadmap F16.1).
    cards: Value,
    cards_at: f64,
    /// Where the project is in a workflow (`workflow.state`) and the workflows there are (`workflow.list`), with when
    /// each was read (fork roadmap F16.4).
    workflow: Value,
    workflow_at: f64,
    workflows: Value,
    workflows_at: f64,
    /// Project files attached to the message being written (media items), sent with it (fork roadmap F16.1c).
    attached: Vec<u64>,
    /// What was typed to narrow the list of files in the attach menu.
    attach_filter: String,
}

impl Chat {
    fn new() -> Self {
        Self {
            thread: None,
            logs: HashMap::new(),
            input: String::new(),
            status: None,
            status_at: f64::NEG_INFINITY,
            models: None,
            models_at: f64::NEG_INFINITY,
            usage_text: String::new(),
            running: false,
            error: None,
            polled_at: f64::NEG_INFINITY,
            commands: Vec::new(),
            delete_armed: None,
            picked_initial: false,
            renaming: None,
            rename_fresh: false,
            editing: None,
            hist_pos: None,
            last_shown: f64::NEG_INFINITY,
            focus_input: true,
            finish_kind: String::new(),
            hover_entry: None,
            cards: Value::Null,
            cards_at: f64::NEG_INFINITY,
            workflow: Value::Null,
            workflow_at: f64::NEG_INFINITY,
            workflows: Value::Null,
            workflows_at: f64::NEG_INFINITY,
            attached: Vec::new(),
            attach_filter: String::new(),
        }
    }

    fn setting(&self, key: &str) -> Value {
        self.status.as_ref().and_then(|s| s.get("settings")).and_then(|s| s.get(key)).cloned().unwrap_or(Value::Null)
    }

    fn ready(&self) -> bool {
        self.status.as_ref().and_then(|s| s.get("ready")).and_then(Value::as_bool).unwrap_or(false)
    }

    /// Whether a chip is on: it is a saved setting, so it is the same next time.
    fn chip(&self, name: &str) -> bool {
        self.setting("chips").get(name).and_then(Value::as_bool).unwrap_or(matches!(name, "selection" | "playhead" | "frame"))
    }

    /// The provider has many models and none is chosen yet: nothing is picked for the person.
    fn needs_model(&self) -> bool {
        self.status.as_ref().and_then(|s| s.get("needsModel")).and_then(Value::as_bool).unwrap_or(false)
    }
}

type Shared = Arc<Mutex<Chat>>;

fn lock(s: &Shared) -> MutexGuard<'_, Chat> {
    s.lock().unwrap_or_else(PoisonError::into_inner)
}

enum Action {
    Send(String),
    Stop,
    NewThread,
    SelectThread(u64),
    Settings(Value),
    OpenConnections,
    SetInput(String),
    Delete(u64),
    StartRename,
    Rename(u64, String),
    CancelRename,
    /// Ask again in a new thread from the start of a turn: the same question, or an edited one.
    Branch {
        turn: usize,
        text: Option<String>,
    },
    StartEdit {
        turn: usize,
        text: String,
    },
    CancelEdit,
    Copy(String),
    /// Move the playhead to a place a view pointed at (ticks).
    Seek(i64),
    /// An answer chosen on a card (`assistant.card`).
    CardChoose {
        card: String,
        params: Value,
    },
    /// Send the cards' answers as one message (`assistant.answerCards`).
    CardsSend,
    /// Forget which workflow the project is in (`workflow.clear`).
    ClearWorkflow,
}

/// Brings the panel's picture of the engine up to date: status, models, and the open thread's new
/// events. Cheap when nothing is going on.
fn refresh(app: &mut FilmcraftApp, chat: &mut Chat, now: f64, ctx: &egui::Context) {
    if chat.delete_armed.is_some() && chat.delete_armed != chat.thread {
        chat.delete_armed = None;
    }
    if now - chat.status_at >= STATUS_EVERY || chat.status.is_none() {
        chat.status_at = now;
        if let Ok(v) = app.session.execute("assistant.status", json!({})) {
            if !chat.picked_initial {
                chat.picked_initial = true;
                chat.thread = v["threads"].as_array().and_then(|t| t.last()).and_then(|t| t["id"].as_u64());
            }
            chat.status = Some(v);
        }
    }
    if chat.commands.is_empty()
        && let Ok(v) = app.session.execute("assistant.commands", json!({}))
    {
        chat.commands = v["commands"].as_array().cloned().unwrap_or_default();
    }
    let updating = chat.models.as_ref().and_then(|m| m.get("updating")).and_then(Value::as_bool).unwrap_or(false);
    if chat.ready() && (chat.models.is_none() || (updating && now - chat.models_at >= 0.5)) {
        chat.models_at = now;
        if let Ok(v) = app.session.execute("assistant.models", json!({})) {
            chat.models = Some(v);
        }
    }
    // where the project is in a workflow, and the workflows there are: cheap, and neither can fail
    if now - chat.workflow_at >= 1.0 {
        chat.workflow_at = now;
        if let Ok(v) = app.session.execute("workflow.state", json!({})) {
            chat.workflow = v;
        }
    }
    if now - chat.workflows_at >= 10.0 {
        chat.workflows_at = now;
        if let Ok(v) = app.session.execute("workflow.list", json!({})) {
            chat.workflows = v;
        }
    }
    let Some(thread) = chat.thread else { return };
    let since = chat.logs.get(&thread).map_or(0, |l| l.next);
    // while a turn runs, every frame; otherwise now and then, so a thread picked from the list loads
    if chat.running || now - chat.polled_at >= 1.0 {
        chat.polled_at = now;
        if let Ok(v) = app.session.execute("assistant.thread", json!({"thread": thread, "since": since})) {
            let log = chat.logs.entry(thread).or_default();
            for ev in v["events"].as_array().into_iter().flatten() {
                log.apply(ev);
            }
            log.next = v["next"].as_u64().unwrap_or(since);
            chat.running = v["running"].as_bool().unwrap_or(false);
            chat.finish_kind = v["finish"]["kind"].as_str().unwrap_or("").to_string();
            chat.usage_text = v["usageText"].as_str().unwrap_or("").to_string();
        }
    }
    // the question and download cards waiting for an answer: cheap, and only while the thread has any
    let has_cards = chat
        .logs
        .get(&thread)
        .is_some_and(|l| l.entries.iter().any(|e| matches!(e, Entry::Tool { view: Some(v), .. } if matches!(v["kind"].as_str(), Some("ask" | "download")))));
    if has_cards {
        let downloading = chat.cards["waiting"].as_u64().unwrap_or(0) > 0;
        if now - chat.cards_at >= if downloading || chat.running { 0.2 } else { 0.5 }
            && let Ok(v) = app.session.execute("assistant.cards", json!({"thread": thread}))
        {
            chat.cards = v;
            chat.cards_at = now;
        }
        if downloading {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
    } else if !chat.cards.is_null() {
        chat.cards = Value::Null;
    }
    if chat.running {
        ctx.request_repaint_after(std::time::Duration::from_millis(40));
    }
}

/// What the assistant is given along with the question, as the chips say: the playhead, and the
/// selected clips.
/// Which chips are on.
#[derive(Clone, Copy)]
struct Chips {
    selection: bool,
    playhead: bool,
    transcript: bool,
    markers: bool,
    frame: bool,
}

/// The most project files attached to one message (the engine refuses more).
const MAX_ATTACHED: usize = 6;

fn context_blocks(app: &FilmcraftApp, chips: Chips, attached: &[u64]) -> Vec<Value> {
    let (selection, playhead) = (chips.selection, chips.playhead);
    let s = &app.session;
    let Some(q) = s.active_sequence() else { return Vec::new() };
    let rate = q.settings.frame_rate;
    let tc = |t| filmcraft_time::format_timecode_frames(rate.frame_at(t), rate, q.settings.drop_frame);
    let mut out = Vec::new();
    if playhead {
        out.push(json!({"label": "playhead", "text": format!("The playhead is at {}.", tc(s.playhead()))}));
    }
    if selection && !s.state.selection.is_empty() {
        let lines: Vec<String> = s
            .state
            .selection
            .iter()
            .take(10)
            .filter_map(|c| q.video_tracks.iter().chain(q.audio_tracks.iter()).find_map(|t| t.items.iter().find(|i| i.id == *c)))
            .map(|i| format!("clip #{} {} ({} to {})", i.id.0, i.name, tc(i.start), tc(i.end())))
            .collect();
        if !lines.is_empty() {
            out.push(json!({"label": "selection", "text": format!("Selected: {}.", lines.join("; "))}));
        }
    }
    // what the engine gathers itself, when the message is sent
    for (on, kind) in [(chips.transcript, "transcript"), (chips.markers, "markers"), (chips.frame, "frame")] {
        if on {
            out.push(json!({"kind": kind}));
        }
    }
    // files the person attached: only ones still in the project
    for id in attached.iter().take(MAX_ATTACHED) {
        if s.project.items.contains_key(&filmcraft_project::ItemId(*id)) {
            out.push(json!({"kind": "item", "item": id}));
        }
    }
    out
}

/// The media in the project the attach menu offers: (id, name, a word for what it is, length in seconds).
fn project_files(app: &FilmcraftApp) -> Vec<(u64, String, &'static str, f64)> {
    app.session
        .project
        .items
        .values()
        .filter_map(|it| match &it.kind {
            filmcraft_project::ItemKind::Media(m) => {
                let kind = match m.info.kind {
                    filmcraft_media::MediaKind::AudioOnly => tl!("audio"),
                    filmcraft_media::MediaKind::Still | filmcraft_media::MediaKind::ImageSequence => tl!("picture"),
                    _ if m.info.video.is_some() => tl!("video"),
                    _ => tl!("audio"),
                };
                Some((it.id.0, it.name.clone(), kind, m.info.duration.seconds()))
            }
            _ => None,
        })
        .collect()
}

fn short(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let now = ui.input(|i| i.time);
    let shared: Shared = ui.data_mut(|d| d.get_temp_mut_or_insert_with(egui::Id::new("assistant-chat"), || Arc::new(Mutex::new(Chat::new()))).clone());
    let mut chat = lock(&shared);
    refresh(app, &mut chat, now, ui.ctx());
    // just opened (or its tab came forward): the cursor goes to the input
    if now - chat.last_shown > 0.4 {
        chat.focus_input = true;
    }
    chat.last_shown = now;

    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(rect.min + vec2(10.0, 8.0), rect.max - vec2(10.0, 4.0))));
    let ui = &mut child;
    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
    let mut elems: Vec<(String, egui::Response, String)> = Vec::new();
    let mut actions: Vec<Action> = Vec::new();
    // places in the timeline the person clicked in the text of an answer (ticks)
    let mut seeks: Vec<i64> = Vec::new();
    let ready = chat.ready();
    let running = chat.running;
    let models = chat.models.clone();
    let current_model = chat.setting("model").as_str().unwrap_or("").to_string();
    let threads: Vec<(u64, String)> = chat
        .status
        .as_ref()
        .and_then(|s| s["threads"].as_array())
        .map(|a| a.iter().filter_map(|t| Some((t["id"].as_u64()?, t["title"].as_str()?.to_string()))).collect())
        .unwrap_or_default();
    let sees = models
        .as_ref()
        .and_then(|m| m["models"].as_array())
        .and_then(|a| a.iter().find(|m| m["id"] == current_model.as_str()))
        .and_then(|m| m["capabilities"]["images"].as_bool())
        // until the model list says so, a model is not assumed to see: no picture goes by surprise
        .unwrap_or(false);

    // ---- top bar: the thread, then New, Delete and Settings as flat icons, as in the monitors ----
    ui.horizontal(|ui| {
        let title = chat.thread.and_then(|id| threads.iter().find(|t| t.0 == id)).map_or(tl!("New thread").to_string(), |t| t.1.clone());
        let w = (ui.available_width() - 100.0).clamp(80.0, 260.0);
        let (trect, _) = ui.allocate_exact_size(vec2(w, 24.0), egui::Sense::hover());
        if let Some(buf) = chat.renaming.as_mut() {
            let field = ui.put(trect, egui::TextEdit::singleline(buf).id(egui::Id::new("assistant-rename-field")).font(Tokens::ui(12.0)).desired_width(w));
            if chat.rename_fresh {
                field.request_focus();
                chat.rename_fresh = false;
            }
            elems.push(("assistant.rename.field".into(), field.clone(), "Thread name".into()));
            if field.lost_focus() {
                match (ui.input(|i| i.key_pressed(Key::Enter)), chat.thread) {
                    (true, Some(id)) => actions.push(Action::Rename(id, chat.renaming.clone().unwrap_or_default())),
                    _ => actions.push(Action::CancelRename),
                }
            }
        }
        let tresp = if chat.renaming.is_some() {
            ui.interact(egui::Rect::NOTHING, egui::Id::new("assistant-thread-dd-hidden"), egui::Sense::hover())
        } else {
            let r = crate::widgets::dropdown_text(ui, trect, &short(&title, 34), &t, egui::Id::new("assistant-thread-dd"));
            elems.push(("assistant.thread".into(), r.clone(), format!("Thread {title}")));
            r
        };
        egui::Popup::menu(&tresp).show(|ui| {
            for (id, title) in threads.iter().rev() {
                let r = ui.selectable_label(chat.thread == Some(*id), short(title, 40));
                elems.push((format!("assistant.thread.{id}"), r.clone(), title.clone()));
                if r.clicked() {
                    actions.push(Action::SelectThread(*id));
                }
            }
            if chat.thread.is_some_and(|id| threads.iter().any(|t| t.0 == id)) {
                ui.separator();
                let r = ui.button(tl!("Rename this thread"));
                elems.push(("assistant.rename".into(), r.clone(), "Rename this thread".into()));
                if r.clicked() {
                    actions.push(Action::StartRename);
                }
            }
        });
        let r = ui.add_enabled_ui(!running, |ui| crate::icons::button(ui, crate::icons::Icon::Plus, 24.0, false, &t, tl!("New thread"))).inner;
        elems.push(("assistant.new".into(), r.clone(), "New thread".into()));
        if r.clicked() && !running {
            actions.push(Action::NewThread);
        }
        if let Some(id) = chat.thread.filter(|id| threads.iter().any(|t| t.0 == *id)) {
            let armed = chat.delete_armed == Some(id);
            let tip = if armed { tl!("Click again to delete") } else { tl!("Delete this thread") };
            let r = ui.add_enabled_ui(!running, |ui| crate::icons::button(ui, crate::icons::Icon::Trash, 24.0, armed, &t, tip)).inner;
            elems.push(("assistant.delete".into(), r.clone(), if armed { "Click again to delete".into() } else { "Delete thread".into() }));
            if r.clicked() && !running {
                if armed {
                    actions.push(Action::Delete(id));
                } else {
                    chat.delete_armed = Some(id);
                }
            }
            if armed {
                ui.label(RichText::new(tl!("Click again to delete")).size(11.0).color(WARN));
            }
        }
        let wr = crate::icons::button(ui, crate::icons::Icon::Wrench, 24.0, false, &t, tl!("Settings"));
        elems.push(("assistant.settings.menu".into(), wr.clone(), "Settings".into()));
        egui::Popup::menu(&wr).show(|ui| {
            ui.set_min_width(240.0);
            let mut thinking = chat.setting("showThinking").as_bool().unwrap_or(true);
            let r = ui.checkbox(&mut thinking, tl!("Show a summary of its thinking"));
            elems.push(("assistant.settings.thinking".into(), r.clone(), "Show thinking".into()));
            if r.changed() {
                actions.push(Action::Settings(json!({"showThinking": thinking})));
            }
            let mut auto = chat.setting("autoTitle").as_bool().unwrap_or(true);
            let r = ui.checkbox(&mut auto, tl!("Let the model name new threads"));
            elems.push(("assistant.settings.autotitle".into(), r.clone(), "Name threads automatically".into()));
            if r.changed() {
                actions.push(Action::Settings(json!({"autoTitle": auto})));
            }
            let effort = chat.setting("effort").as_str().map(str::to_string);
            ui.horizontal(|ui| {
                ui.label(tl!("Effort"));
                let c = egui::ComboBox::from_id_salt("assistant-effort")
                    .selected_text(effort.clone().unwrap_or_else(|| tl!("Model default").to_string()))
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(effort.is_none(), tl!("Model default")).clicked() {
                            actions.push(Action::Settings(json!({"effort": null})));
                        }
                        for e in ["low", "medium", "high", "max"] {
                            if ui.selectable_label(effort.as_deref() == Some(e), e).clicked() {
                                actions.push(Action::Settings(json!({"effort": e})));
                            }
                        }
                    });
                elems.push(("assistant.settings.effort".into(), c.response, "Effort".into()));
            });
            ui.horizontal(|ui| {
                ui.label(tl!("Most steps per question"));
                let mut steps = chat.setting("maxSteps").as_u64().unwrap_or(12) as u32;
                let r = ui.add(egui::DragValue::new(&mut steps).range(1..=40));
                elems.push(("assistant.settings.steps".into(), r.clone(), "Steps".into()));
                if r.changed() {
                    actions.push(Action::Settings(json!({"maxSteps": steps})));
                }
            });
            ui.horizontal(|ui| {
                let limit = chat.setting("maxCostUsd").as_f64();
                let mut on = limit.is_some();
                if ui.checkbox(&mut on, tl!("Stop a thread at about $")).changed() {
                    actions.push(Action::Settings(json!({"maxCostUsd": if on { json!(1.0) } else { Value::Null }})));
                }
                if let Some(mut c) = limit {
                    let r = ui.add(egui::DragValue::new(&mut c).range(0.05..=1000.0).speed(0.05).fixed_decimals(2));
                    elems.push(("assistant.settings.cost".into(), r.clone(), "Cost limit".into()));
                    if r.changed() {
                        actions.push(Action::Settings(json!({"maxCostUsd": c})));
                    }
                }
            });
            ui.separator();
            ui.label(RichText::new(tl!("Sent with each question")).size(11.0).color(t.text_faint));
            for (name, label, english, hover) in [
                ("selection", tl!("Selection"), "Selection", tl!("The clips you have selected.")),
                ("playhead", tl!("Playhead"), "Playhead", tl!("Where the playhead is.")),
                ("transcript", tl!("Transcript"), "Transcript", tl!("What is said within 20 seconds of the playhead.")),
                ("markers", tl!("Markers"), "Markers", tl!("Every marker in this sequence.")),
                (
                    "frame",
                    tl!("Frame"),
                    "Frame",
                    if sees {
                        tl!("A picture of the Program monitor at the playhead. Costs more than text.")
                    } else {
                        tl!("This model cannot look at pictures.")
                    },
                ),
            ] {
                let enabled = name != "frame" || sees;
                let mut on = chat.chip(name);
                let r = ui.add_enabled(enabled, egui::Checkbox::new(&mut on, label)).on_hover_text(hover).on_disabled_hover_text(hover);
                elems.push((format!("assistant.settings.chip.{name}"), r.clone(), format!("{english} {}", if on && enabled { "on" } else { "off" })));
                if r.changed() {
                    actions.push(Action::Settings(json!({"chips": {name: on}})));
                }
            }
            ui.separator();
            ui.label(RichText::new(tl!("The cost is an estimate from list prices.")).size(11.0).color(t.text_faint));
        });
    });
    // the cost meter, under the bar so a narrow panel never overlaps it with the buttons
    if !chat.usage_text.is_empty() {
        let r = ui.label(RichText::new(&chat.usage_text).size(11.0).color(t.text_faint));
        elems.push(("assistant.cost".into(), r, chat.usage_text.clone()));
    }
    // the workflow the project is in, and how far: one line, with a way to stop following it
    if chat.workflow["active"] == true && chat.workflow["known"] == true {
        let w = &chat.workflow;
        let name = w["name"].as_str().unwrap_or("").replace('-', " ");
        let stage = tlf!(
            "Stage {n} of {m}: {name}",
            n = w["stage"].as_u64().unwrap_or(0),
            m = w["stages"].as_u64().unwrap_or(0),
            name = w["stageName"].as_str().unwrap_or("")
        );
        let status = match w["status"].as_str().unwrap_or("working") {
            "waiting" => tl!("waiting for you"),
            "done" => tl!("done"),
            "blocked" => tl!("stuck"),
            _ => tl!("working"),
        };
        let text = format!("{name} · {stage} · {status}");
        // the tag, the name and the cross on one line; where it is, and the note, under them
        ui.horizontal(|ui| {
            ui.label(RichText::new(tl!("WORKFLOW")).size(10.5).strong().color(t.accent));
            let r = ui.add(egui::Label::new(RichText::new(&name).size(12.0).color(t.text)).truncate());
            elems.push(("assistant.workflow".into(), r, text.clone()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let x = ui.small_button("×").on_hover_text(tl!("Stop following this workflow"));
                elems.push(("assistant.workflow.clear".into(), x.clone(), "Stop following this workflow".into()));
                if x.clicked() {
                    actions.push(Action::ClearWorkflow);
                }
            });
        });
        ui.add(egui::Label::new(RichText::new(format!("{stage} · {status}")).size(11.5).color(t.text_dim)).wrap());
        if let Some(note) = w["note"].as_str().filter(|n| !n.is_empty()) {
            ui.add(egui::Label::new(RichText::new(note).size(11.0).color(t.text_faint)).wrap());
        }
        ui.add_space(4.0);
    }
    // notes from the engine: a damaged settings file, conversations that could not be restored
    for key in ["settingsNote", "restoreNote"] {
        if let Some(note) = chat.status.as_ref().and_then(|st| st[key].as_str()) {
            ui.label(RichText::new(note).size(11.0).color(WARN));
        }
    }

    // ---- not connected ----
    if !ready {
        egui::Frame::new().fill(t.field_bg).stroke(egui::Stroke::new(1.0, t.field_border)).corner_radius(6.0).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(tl!(
                    "Edith needs a model to talk to. Connect a service in Connections (or pick Ollama or LM Studio for a model on this computer), and this panel will light up."
                ))
                .color(WARN),
            );
            let r = ui.button(tl!("Open Connections"));
            elems.push(("assistant.connect".into(), r.clone(), "Open Connections".into()));
            if r.clicked() {
                actions.push(Action::OpenConnections);
            }
        });
    }

    // ---- what the composer below will take: its height decides how tall the conversation is ----
    // the chips of attached files add a row above the field
    let composer_h = 90.0 + if chat.attached.is_empty() { 0.0 } else { 28.0 };
    let hint_h = if ready && chat.needs_model() { 18.0 } else { 0.0 };
    let slash_h = if chat.input.starts_with('/') && !chat.input.contains(char::is_whitespace) && ready { 30.0 } else { 0.0 };

    // ---- the conversation ----
    let mut new_hover: Option<usize> = None;
    let transcript_h = (ui.available_height() - composer_h - hint_h - slash_h - 8.0).max(40.0);
    let thread = chat.thread;
    let empty = thread.is_none_or(|id| chat.logs.get(&id).is_none_or(|l| l.entries.is_empty()));
    egui::ScrollArea::vertical().id_salt("assistant-scroll").max_height(transcript_h).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if empty && ready {
            ui.add_space(14.0);
            ui.label(RichText::new(tl!("Ask about your edit")).font(Tokens::semibold(15.0)).color(t.text));
            ui.label(
                RichText::new(tl!("I can look at the project, the clips, the markers and any transcripts. I can't change anything."))
                    .size(12.0)
                    .color(t.text_dim),
            );
            ui.add_space(6.0);
            for (n, q) in [tl!("Summarize my edit"), tl!("Where does the pacing feel slow or uneven?"), tl!("What would make this cut feel more cinematic?")]
                .into_iter()
                .enumerate()
            {
                let r = super::assistant_widgets::list_button(ui, q, &t);
                elems.push((format!("assistant.starter.{n}"), r.clone(), q.to_string()));
                if r.clicked() {
                    actions.push(Action::Send(q.to_string()));
                }
            }
            // workflows: guidance for a bigger job, put in the box for the person to read and send
            let flows: Vec<Value> = chat.workflows["workflows"].as_array().cloned().unwrap_or_default();
            if !flows.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new(tl!("Workflows")).size(11.0).color(t.text_faint));
                for w in flows.iter().take(8) {
                    let name = w["name"].as_str().unwrap_or("");
                    let shown = name.replace('-', " ");
                    let shown = shown.chars().next().map_or(String::new(), |c| c.to_uppercase().collect::<String>() + &shown[c.len_utf8()..]);
                    let r = super::assistant_widgets::list_button(ui, &shown, &t).on_hover_text(w["description"].as_str().unwrap_or(""));
                    elems.push((format!("assistant.workflow.start.{name}"), r.clone(), shown));
                    if r.clicked() {
                        actions.push(Action::SetInput(format!("Let's use the “{name}” workflow.")));
                    }
                }
            }
        }
        if let Some(log) = thread.and_then(|id| chat.logs.get(&id)) {
            ui.spacing_mut().item_spacing.y = 6.0;
            let last = log.entries.len().saturating_sub(1);
            let last_user = log.entries.iter().rposition(|e| matches!(e, Entry::User(_)));
            let last_answer = log.entries.iter().rposition(|e| matches!(e, Entry::Assistant(_)));
            let mut turn = 0usize;
            for (i, e) in log.entries.iter().enumerate() {
                match e {
                    Entry::User(text) => {
                        if i > 0 {
                            ui.add_space(10.0);
                        }
                        let f = egui::Frame::new()
                            .fill(t.field_bg)
                            .stroke(egui::Stroke::new(1.0, t.field_border))
                            .corner_radius(6.0)
                            .inner_margin(egui::Margin::symmetric(10, 8));
                        let r = f.show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.add(egui::Label::new(RichText::new(text).size(super::assistant_widgets::BODY).color(t.text)).wrap());
                        });
                        let this_turn = turn;
                        turn += 1;
                        let shown = !running && (last_user == Some(i) || chat.hover_entry == Some(i));
                        let (row, links) =
                            super::assistant_widgets::action_row(ui, shown, &[(ActionIcon::Edit, tl!("Edit")), (ActionIcon::Copy, tl!("Copy"))], &t);
                        for (n, r) in links.iter().enumerate() {
                            let (id, label) = if n == 0 { ("edit", "Edit message") } else { ("copy", "Copy message") };
                            elems.push((format!("assistant.{id}.{i}"), r.clone(), label.to_string()));
                            if r.clicked() {
                                actions.push(if n == 0 { Action::StartEdit { turn: this_turn, text: text.clone() } } else { Action::Copy(text.clone()) });
                            }
                        }
                        if ui.rect_contains_pointer(r.response.rect.union(row)) {
                            new_hover = Some(i);
                        }
                        elems.push((format!("assistant.entry.{i}"), r.response, text.clone()));
                    }
                    Entry::Assistant(text) => {
                        let rate = app.session.active_sequence().map_or(filmcraft_time::FrameRate::FPS_24, |q| q.settings.frame_rate);
                        let mut links = super::assistant_widgets::Links { elems: &mut elems, seek: &mut seeks, rate, entry: i, next: 0 };
                        let r = ui.scope(|ui| super::assistant_widgets::answer(ui, (thread, i), &super::assistant_view::blocks(text), &t, &mut links)).response;
                        let shown = !running && (last_answer == Some(i) || chat.hover_entry == Some(i));
                        let (row, links) =
                            super::assistant_widgets::action_row(ui, shown, &[(ActionIcon::Copy, tl!("Copy")), (ActionIcon::Retry, tl!("Retry"))], &t);
                        for (n, l) in links.iter().enumerate() {
                            let (id, label) = if n == 0 { ("copy", "Copy answer") } else { ("retry", "Ask again") };
                            elems.push((format!("assistant.{id}.{i}"), l.clone(), label.to_string()));
                            if l.clicked() {
                                actions.push(if n == 0 { Action::Copy(text.clone()) } else { Action::Branch { turn: turn.saturating_sub(1), text: None } });
                            }
                        }
                        if ui.rect_contains_pointer(r.rect.union(row)) {
                            new_hover = Some(i);
                        }
                        elems.push((format!("assistant.entry.{i}"), r, text.clone()));
                    }
                    Entry::Thinking(text) => {
                        // open while it streams in, folded once the answer starts
                        let live = running && i == last;
                        let h = egui::CollapsingHeader::new(RichText::new(tl!("Thinking…")).italics().size(11.5).color(t.text_faint))
                            .id_salt(("assistant-think", thread, i))
                            .open(live.then_some(true))
                            .default_open(false)
                            .show(ui, |ui| {
                                ui.add(egui::Label::new(RichText::new(text).italics().size(11.5).color(t.text_faint)).wrap());
                            });
                        elems.push((format!("assistant.entry.{i}"), h.header_response, text.clone()));
                    }
                    Entry::Tool { view: Some(view), is_error: false, .. } if matches!(view["kind"].as_str(), Some("ask" | "download")) => {
                        // a question or a download request: a card, answered here, sent together with its turn's others
                        ui.add_space(2.0);
                        let ctx = ui.ctx().clone();
                        if let Some(th) = thread {
                            let mut taken = Vec::new();
                            super::assistant_cards::draw(ui, app, &ctx, view, th, &chat.cards, &log.answers, running, &t, &mut elems, &mut taken);
                            actions.extend(taken.into_iter().map(|a| match a {
                                super::assistant_cards::CardAction::Choose { card, params } => Action::CardChoose { card, params },
                                super::assistant_cards::CardAction::Send => Action::CardsSend,
                            }));
                        }
                        ui.add_space(2.0);
                    }
                    Entry::Tool { view: Some(view), is_error: false, .. } => {
                        // a view the assistant placed in its answer: drawn in place, between its sentences
                        ui.add_space(2.0);
                        let ctx = ui.ctx().clone();
                        let mut pics = |p: super::assistant_views::Pic<'_>| match p {
                            super::assistant_views::Pic::Still(look, still) => super::look_pictures::still(app, &ctx, look, still),
                            super::assistant_views::Pic::Inline(b64) => super::look_pictures::inline(&ctx, b64),
                        };
                        if let Some(at) = super::assistant_views::draw(ui, view, i, &t, &mut elems, &mut pics) {
                            actions.push(Action::Seek(at));
                        }
                        ui.add_space(2.0);
                    }
                    Entry::Tool { name, input, result, is_error, pictures, .. } => {
                        let color = if *is_error { WARN } else { t.text_faint };
                        let waiting = if result.is_none() {
                            "  …".to_string()
                        } else if *pictures > 0 {
                            format!("  [{}]", tl!("with a picture"))
                        } else {
                            String::new()
                        };
                        // the tools that change the edit are not "looking"
                        let verb = if matches!(name.as_str(), "begin_version" | "edit" | "undo_steps" | "redo_step") {
                            tl!("Changed the edit with")
                        } else {
                            tl!("Looked at")
                        };
                        let title = if input.is_empty() { format!("{verb} {name}{waiting}") } else { format!("{verb} {name}  ({input}){waiting}") };
                        let h = egui::CollapsingHeader::new(RichText::new(title).size(11.5).color(color))
                            .id_salt(("assistant-tool", thread, i))
                            .default_open(false)
                            .show(ui, |ui| {
                                egui::Frame::new().fill(t.field_bg).corner_radius(4.0).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    egui::ScrollArea::vertical().max_height(160.0).id_salt(("assistant-tool-body", thread, i)).show(ui, |ui| {
                                        ui.add(
                                            egui::Label::new(RichText::new(result.as_deref().unwrap_or("")).monospace().size(11.0).color(t.text_dim)).wrap(),
                                        );
                                    });
                                });
                            });
                        elems.push((format!("assistant.tool.{i}"), h.header_response, format!("{name}: {}", short(result.as_deref().unwrap_or(""), 300))));
                    }
                    Entry::Attached(labels) => {
                        let text = format!("{} {}", tl!("Sent along:"), labels.join(", "));
                        let r = ui.label(RichText::new(&text).size(11.0).color(t.text_faint));
                        elems.push((format!("assistant.entry.{i}"), r, text));
                    }
                    Entry::Note { text, warn } => {
                        let r = ui.label(RichText::new(text).size(11.5).color(if *warn { WARN } else { t.text_faint }));
                        elems.push((format!("assistant.entry.{i}"), r, text.clone()));
                    }
                }
            }
        }
        if let Some(e) = &chat.error {
            ui.label(RichText::new(e).size(12.0).color(WARN));
        }
        if running {
            let line = if chat.usage_text.is_empty() { tl!("Working…").to_string() } else { format!("{} · {}", tl!("Working…"), chat.usage_text) };
            ui.label(RichText::new(line).size(11.5).color(t.text_faint));
        } else if thread.is_some() && matches!(chat.finish_kind.as_str(), "cut_off" | "step_limit") {
            let r = ui.button(tl!("Continue"));
            elems.push(("assistant.continue".into(), r.clone(), "Continue".into()));
            if r.clicked() {
                actions.push(Action::Send("Please continue.".to_string()));
            }
        }
    });
    chat.hover_entry = new_hover;

    if hint_h > 0.0 {
        ui.label(RichText::new(tl!("Choose a model below to start.")).size(11.0).color(WARN));
    }

    // ---- slash commands, while the message is just "/something" ----
    let typing_slash = chat.input.starts_with('/') && !chat.input.contains(char::is_whitespace);
    if typing_slash && ready {
        let prefix = chat.input[1..].to_ascii_lowercase();
        ui.horizontal_wrapped(|ui| {
            for c in chat.commands.iter().filter(|c| c["name"].as_str().is_some_and(|n| n.starts_with(&prefix))) {
                let name = c["name"].as_str().unwrap_or("");
                let r = super::assistant_widgets::pill(ui, &format!("/{name}"), false, true, &t).on_hover_text(c["description"].as_str().unwrap_or(""));
                elems.push((format!("assistant.slash.{name}"), r.clone(), format!("/{name}")));
                if r.clicked() {
                    actions.push(Action::SetInput(format!("/{name} ")));
                }
            }
        });
    }

    // ---- the composer: one dark field holding the text, and under it the provider, the model and Send ----
    let input_id = egui::Id::new("assistant-input-field");
    if chat.focus_input && ready {
        ui.memory_mut(|m| m.request_focus(input_id));
        chat.focus_input = false;
    }
    let focused = ui.memory(|m| m.has_focus(input_id));
    // Esc stops a turn that is running, or drops an edit; taken before the field, which would let go of focus
    let nothing_focused = ui.memory(|m| m.focused().is_none());
    if (running || chat.editing.is_some()) && (focused || nothing_focused) && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        actions.push(if running { Action::Stop } else { Action::CancelEdit });
    }
    // Up in an empty input brings back your last question, again for older ones; Down goes forward
    if focused && ready && !running {
        let asked: Vec<String> = thread
            .and_then(|id| chat.logs.get(&id))
            .map(|l| l.entries.iter().rev().filter_map(|e| if let Entry::User(t) = e { Some(t.clone()) } else { None }).collect())
            .unwrap_or_default();
        let at_recalled = chat.hist_pos.and_then(|p| asked.get(p)).is_some_and(|t| *t == chat.input);
        if (chat.input.is_empty() || at_recalled) && !asked.is_empty() {
            if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
                let p = chat.hist_pos.map_or(0, |p| p + 1).min(asked.len() - 1);
                chat.input = asked[p].clone();
                chat.hist_pos = Some(p);
            } else if at_recalled && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
                match chat.hist_pos {
                    Some(0) | None => {
                        chat.input.clear();
                        chat.hist_pos = None;
                    }
                    Some(p) => {
                        chat.input = asked[p - 1].clone();
                        chat.hist_pos = Some(p - 1);
                    }
                }
            }
        }
    }
    if chat.editing.is_some() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(tl!("Editing an earlier message. It is sent as a new thread.")).size(11.0).color(WARN));
            let r = ui.small_button(tl!("Cancel"));
            elems.push(("assistant.edit.cancel".into(), r.clone(), "Cancel editing".into()));
            if r.clicked() {
                actions.push(Action::CancelEdit);
            }
        });
    }
    // Enter sends; Shift+Enter makes a new line. Taken before the field sees the key.
    let enter = focused && ready && !running && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
    // files attached to this message, as chips that come off with a click; one that left the project is dropped
    let files = project_files(app);
    chat.attached.retain(|id| files.iter().any(|f| f.0 == *id));
    if !chat.attached.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for id in chat.attached.clone() {
                let name = files.iter().find(|f| f.0 == id).map(|f| short(&f.1, 28)).unwrap_or_default();
                let r = super::assistant_widgets::pill(ui, &format!("{name}  ×"), true, true, &t).on_hover_text(tl!("Take this file off the message"));
                elems.push((format!("assistant.attached.{id}"), r.clone(), format!("Attached: {name}")));
                if r.clicked() {
                    chat.attached.retain(|x| *x != id);
                }
            }
        });
    }
    let border = if focused { t.accent } else { t.field_border };
    egui::Frame::new().fill(t.field_bg).stroke(egui::Stroke::new(1.0, border)).corner_radius(6.0).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let r = ui.add_enabled(
            ready,
            egui::TextEdit::multiline(&mut chat.input)
                .id(input_id)
                .frame(egui::Frame::NONE)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .font(Tokens::ui(super::assistant_widgets::BODY))
                .hint_text(RichText::new(tl!("Ask Edith about your edit…")).color(t.text_faint)),
        );
        elems.push(("assistant.draft".into(), r.clone(), chat.input.clone()));
        elems.push(("assistant.input".into(), r, "Message".into()));
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            // attach a file: the project's own media only (importing stays in the Project and Media panels)
            // a fixed id: the chips above shift the button's automatic one, and the open menu is keyed to it
            let (arect, _) = ui.allocate_exact_size(vec2(24.0, 24.0), egui::Sense::hover());
            let aresp = ui.interact(arect, egui::Id::new("assistant-attach-button"), egui::Sense::click());
            ui.painter().rect_filled(arect, 4.0, if aresp.hovered() { t.hover } else { t.field_bg });
            ui.painter().rect_stroke(arect, 4.0, egui::Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
            crate::icons::paint(ui.painter(), Rect::from_center_size(arect.center(), vec2(12.0, 12.0)), crate::icons::Icon::Plus, t.text_dim);
            let aresp = aresp.on_hover_text(tl!("Attach a file from the project"));
            elems.push(("assistant.attach".into(), aresp.clone(), "Attach a file from the project".into()));
            // stays open while several files are picked; a click outside closes it
            egui::Popup::menu(&aresp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                ui.set_min_width(280.0);
                let f = ui.add(egui::TextEdit::singleline(&mut chat.attach_filter).id(egui::Id::new("assistant-attach-filter")).hint_text(tl!("Search the project…")).desired_width(f32::INFINITY));
                elems.push(("assistant.attach.search".into(), f, "Search the project's files".into()));
                let q = chat.attach_filter.trim().to_lowercase();
                let shown: Vec<&(u64, String, &str, f64)> = files.iter().filter(|f| q.is_empty() || f.1.to_lowercase().contains(&q)).take(200).collect();
                if files.is_empty() {
                    ui.label(RichText::new(tl!("No files in the project yet.")).color(t.text_faint));
                } else if shown.is_empty() {
                    ui.label(RichText::new(tl!("Nothing matches.")).color(t.text_faint));
                }
                let full = chat.attached.len() >= MAX_ATTACHED;
                if full {
                    ui.label(RichText::new(tl!("Up to 6 files can go with one message.")).size(11.0).color(WARN));
                }
                egui::ScrollArea::vertical().max_height(280.0).id_salt("assistant-attach-list").show(ui, |ui| {
                    for (id, name, kind, secs) in shown {
                        let on = chat.attached.contains(id);
                        let label = format!("{}   ·  {kind}, {secs:.0} s", short(name, 34));
                        let r = ui.add_enabled(on || !full, egui::Button::selectable(on, label));
                        elems.push((format!("assistant.attach.item.{id}"), r.clone(), name.to_string()));
                        if r.clicked() {
                            if on {
                                chat.attached.retain(|x| x != id);
                            } else {
                                chat.attached.push(*id);
                            }
                        }
                    }
                });
            });
            // provider
            let providers: Vec<Value> = chat.status.as_ref().and_then(|s| s["providers"].as_array()).cloned().unwrap_or_default();
            let current_service = chat.setting("service").as_str().unwrap_or("").to_string();
            let service_name = |p: &Value| p["label"].as_str().unwrap_or("").split(" (").next().unwrap_or("").to_string();
            let shown = providers.iter().find(|p| p["id"] == current_service.as_str()).map_or(current_service.clone(), service_name);
            let painter = ui.painter().clone();
            let chip_w = |text: &str| painter.layout_no_wrap(text.to_string(), Tokens::ui(12.0), t.text).size().x + 34.0;
            let pw = chip_w(&short(&shown, 16));
            let (prect, _) = ui.allocate_exact_size(vec2(pw, 24.0), egui::Sense::hover());
            let presp = crate::widgets::dropdown_text(ui, prect, &short(&shown, 16), &t, egui::Id::new("assistant-provider-dd"));
            elems.push(("assistant.provider".into(), presp.clone(), format!("Provider {shown}")));
            egui::Popup::menu(&presp).show(|ui| {
                for p in &providers {
                    let id = p["id"].as_str().unwrap_or("").to_string();
                    let mut label = service_name(p);
                    if p["ready"] != true {
                        label = format!("{label} · {}", tl!("not connected"));
                    }
                    let r = ui.selectable_label(id == current_service, &label);
                    elems.push((format!("assistant.provider.{id}"), r.clone(), label));
                    if r.clicked() {
                        actions.push(Action::Settings(json!({"service": id})));
                    }
                }
            });
            // model
            let name = models
                .as_ref()
                .and_then(|m| m["models"].as_array())
                .and_then(|a| a.iter().find(|m| m["id"] == current_model.as_str()))
                .and_then(|m| m["name"].as_str())
                .map_or(current_model.clone(), str::to_string);
            let shown_model = if current_model.is_empty() { tl!("Choose a model").to_string() } else { short(&name, 24) };
            // the mode picker follows the model picker, so leave it room
            let mode_id = chat.setting("mode").as_str().unwrap_or("ask").to_string();
            let mode_label = |id: &str| match id {
                "coach" => tl!("Coach"),
                "edit" => tl!("Edit"),
                _ => tl!("Ask"),
            };
            let mode_w = chip_w(mode_label(&mode_id));
            let mw = chip_w(&shown_model).min((ui.available_width() - 40.0 - mode_w - 4.0).max(80.0));
            let (mrect, _) = ui.allocate_exact_size(vec2(mw, 24.0), egui::Sense::hover());
            let mresp = crate::widgets::dropdown_text(ui, mrect, &shown_model, &t, egui::Id::new("assistant-model-dd"));
            elems.push(("assistant.model".into(), mresp.clone(), format!("Model {shown_model}")));
            egui::Popup::menu(&mresp).show(|ui| {
                ui.set_min_width(260.0);
                let list = models.as_ref().and_then(|m| m["models"].as_array()).cloned().unwrap_or_default();
                if list.is_empty() {
                    ui.label(RichText::new(tl!("No models loaded yet")).color(t.text_faint));
                }
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for m in &list {
                        let (id, label) = (m["id"].as_str().unwrap_or("").to_string(), m["name"].as_str().unwrap_or("").to_string());
                        let badges: Vec<&str> = m["badges"].as_array().map(|b| b.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                        let r = ui.selectable_label(id == current_model, &label).on_hover_text(badges.join("  ·  "));
                        elems.push((format!("assistant.model.{id}"), r.clone(), label));
                        if r.clicked() {
                            actions.push(Action::Settings(json!({"model": id})));
                        }
                    }
                });
            });
            // mode
            let (drect, _) = ui.allocate_exact_size(vec2(mode_w, 24.0), egui::Sense::hover());
            let dresp = crate::widgets::dropdown_text(ui, drect, mode_label(&mode_id), &t, egui::Id::new("assistant-mode-dd"))
                .on_hover_text(tl!("How Edith works with you: Ask answers questions about your edit; Coach teaches from its measurements; Edit changes your edit, in a version you decide on."));
            elems.push(("assistant.mode".into(), dresp.clone(), format!("Mode {}", mode_label(&mode_id))));
            egui::Popup::menu(&dresp).show(|ui| {
                ui.set_min_width(220.0);
                for (id, hint) in [
                    ("ask", tl!("Look at your edit and answer your questions, using its measurements when they help.")),
                    ("coach", tl!("Teach from the measurements: what they mean, why it matters, and one thing to try.")),
                    ("edit", tl!("Change your edit, only inside a version that you can look at, edit alongside, and commit or discard.")),
                ] {
                    let r = ui.selectable_label(id == mode_id, mode_label(id)).on_hover_text(hint);
                    elems.push((format!("assistant.mode.{id}"), r.clone(), mode_label(id).to_string()));
                    if r.clicked() {
                        actions.push(Action::Settings(json!({"mode": id})));
                    }
                }
            });
            // Send, or Stop while a turn runs
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if running {
                    let r = super::assistant_widgets::round_button(ui, true, true, &t).on_hover_text(tl!("Stop"));
                    elems.push(("assistant.stop".into(), r.clone(), "Stop".into()));
                    if r.clicked() {
                        actions.push(Action::Stop);
                    }
                } else {
                    let can = ready && !chat.needs_model() && !chat.input.trim().is_empty();
                    let r = super::assistant_widgets::round_button(ui, false, can, &t).on_hover_text(tl!("Send"));
                    elems.push(("assistant.send".into(), r.clone(), "Send".into()));
                    if (r.clicked() && can) || (enter && can) {
                        let text = chat.input.trim().to_string();
                        actions.push(match chat.editing {
                            Some(turn) => Action::Branch { turn, text: Some(text) },
                            None => Action::Send(text),
                        });
                    }
                }
            });
        });
    });

    let chips = Chips {
        selection: chat.chip("selection"),
        playhead: chat.chip("playhead"),
        transcript: chat.chip("transcript"),
        markers: chat.chip("markers"),
        frame: chat.chip("frame") && sees,
    };
    let attached = chat.attached.clone();
    drop(chat);
    for (id, r, label) in &elems {
        app.auto.add_response(id, r, label);
    }
    actions.extend(seeks.into_iter().map(Action::Seek));
    let ctx = ui.ctx().clone();
    for a in actions {
        let mut chat = lock(&shared);
        match a {
            Action::Send(text) => {
                let mut params = json!({"text": text, "context": context_blocks(app, chips, &attached)});
                if let Some(id) = chat.thread {
                    params["thread"] = json!(id);
                }
                match app.session.execute("assistant.send", params) {
                    Ok(v) => {
                        chat.thread = v["thread"].as_u64();
                        chat.running = true;
                        chat.input.clear();
                        chat.attached.clear();
                        chat.hist_pos = None;
                        chat.finish_kind.clear();
                        chat.error = None;
                        chat.status_at = f64::NEG_INFINITY;
                        chat.polled_at = f64::NEG_INFINITY;
                    }
                    Err(e) => chat.error = Some(e.to_string()),
                }
            }
            Action::Stop => {
                if let Some(id) = chat.thread {
                    let _ = app.session.execute("assistant.cancel", json!({"thread": id}));
                }
            }
            Action::StartRename => {
                let title = chat.thread.and_then(|id| threads.iter().find(|t| t.0 == id)).map(|t| t.1.clone()).unwrap_or_default();
                chat.renaming = Some(title);
                chat.rename_fresh = true;
            }
            Action::CancelRename => chat.renaming = None,
            Action::Rename(id, title) => {
                match app.session.execute("assistant.renameThread", json!({"thread": id, "title": title})) {
                    Ok(_) => chat.error = None,
                    Err(e) => chat.error = Some(e.to_string()),
                }
                chat.renaming = None;
                chat.status_at = f64::NEG_INFINITY;
            }
            Action::StartEdit { turn, text } => {
                chat.editing = Some(turn);
                chat.input = text;
                chat.focus_input = true;
            }
            Action::CancelEdit => {
                chat.editing = None;
                chat.input.clear();
            }
            Action::Copy(text) => {
                ctx.copy_text(text);
                app.ui.status = tl!("Copied to the clipboard.").to_string();
            }
            Action::Seek(ticks) => app.session.set_playhead(filmcraft_time::Tick(ticks.max(0))),
            Action::CardChoose { card, params } => {
                let Some(id) = chat.thread else { continue };
                let mut p = params;
                p["thread"] = json!(id);
                p["card"] = json!(card);
                match app.session.execute("assistant.card", p) {
                    Ok(v) => {
                        chat.error = None;
                        // Show me others: Find Media opens with Edith's search typed in, and searches
                        if let Some(text) = v["findMedia"]["text"].as_str() {
                            let kind = v["findMedia"]["kind"].as_str().unwrap_or("video");
                            super::find_media::prefill(&ctx, text, kind);
                            if let Err(e) = app.session.execute("hub.search", json!({"text": text, "kinds": [kind]})) {
                                chat.error = Some(e.to_string());
                            }
                            app.show_panel(PanelKind::FindMedia);
                        }
                    }
                    Err(e) => chat.error = Some(e.to_string()),
                }
                chat.cards_at = f64::NEG_INFINITY;
            }
            Action::ClearWorkflow => {
                if let Err(e) = app.session.execute("workflow.clear", json!({})) {
                    chat.error = Some(e.to_string());
                }
                chat.workflow_at = f64::NEG_INFINITY;
            }
            Action::CardsSend => {
                let Some(id) = chat.thread else { continue };
                match app.session.execute("assistant.answerCards", json!({"thread": id})) {
                    Ok(_) => {
                        chat.running = true;
                        chat.finish_kind.clear();
                        chat.error = None;
                        chat.status_at = f64::NEG_INFINITY;
                        chat.polled_at = f64::NEG_INFINITY;
                    }
                    Err(e) => chat.error = Some(e.to_string()),
                }
                chat.cards_at = f64::NEG_INFINITY;
            }
            Action::Branch { turn, text } => {
                let Some(from) = chat.thread else { continue };
                let mut params = json!({"thread": from, "turn": turn, "context": context_blocks(app, chips, &attached)});
                if let Some(t) = &text {
                    params["text"] = json!(t);
                }
                match app.session.execute("assistant.branch", params) {
                    Ok(v) => {
                        chat.thread = v["thread"].as_u64();
                        chat.running = true;
                        chat.attached.clear();
                        chat.editing = None;
                        chat.hist_pos = None;
                        if text.is_some() {
                            chat.input.clear();
                        }
                        chat.error = None;
                        chat.finish_kind.clear();
                        chat.status_at = f64::NEG_INFINITY;
                        chat.polled_at = f64::NEG_INFINITY;
                    }
                    Err(e) => chat.error = Some(e.to_string()),
                }
            }
            Action::NewThread => {
                chat.thread = None;
                chat.error = None;
                chat.usage_text.clear();
            }
            Action::SelectThread(id) => {
                chat.thread = Some(id);
                chat.polled_at = f64::NEG_INFINITY;
            }
            Action::Settings(p) => {
                if p.get("service").is_some() {
                    // The models belong to the provider: forget the old list so the new one is fetched.
                    chat.models = None;
                }
                match app.session.execute("assistant.settings", p) {
                    Ok(_) => chat.error = None,
                    Err(e) => chat.error = Some(e.to_string()),
                }
                chat.status_at = f64::NEG_INFINITY;
            }
            Action::OpenConnections => app.show_panel(PanelKind::Connections),
            Action::Delete(id) => {
                if app.session.execute("assistant.deleteThread", json!({"thread": id})).is_ok() {
                    chat.logs.remove(&id);
                    chat.thread = None;
                    chat.usage_text.clear();
                }
                chat.delete_armed = None;
                chat.status_at = f64::NEG_INFINITY;
            }
            Action::SetInput(text) => {
                chat.input = text;
                ctx.memory_mut(|m| m.request_focus(egui::Id::new("assistant-input-field")));
            }
        }
    }
}
