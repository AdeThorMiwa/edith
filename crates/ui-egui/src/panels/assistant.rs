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
//! `assistant.model` (and `assistant.model.<id>` for each choice), `assistant.thread` (and
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

fn context_blocks(app: &FilmcraftApp, chips: Chips) -> Vec<Value> {
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
    out
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
    let composer_h = 90.0;
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
                        let r = ui.scope(|ui| super::assistant_widgets::answer(ui, (thread, i), &super::assistant_view::blocks(text), &t)).response;
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
                    Entry::Tool { name, input, result, is_error, pictures, .. } => {
                        let color = if *is_error { WARN } else { t.text_faint };
                        let waiting = if result.is_none() {
                            "  …".to_string()
                        } else if *pictures > 0 {
                            format!("  [{}]", tl!("with a picture"))
                        } else {
                            String::new()
                        };
                        let title = if input.is_empty() {
                            format!("{} {name}{waiting}", tl!("Looked at"))
                        } else {
                            format!("{} {name}  ({input}){waiting}", tl!("Looked at"))
                        };
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
            let mw = chip_w(&shown_model).min((ui.available_width() - 40.0).max(80.0));
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
    drop(chat);
    for (id, r, label) in &elems {
        app.auto.add_response(id, r, label);
    }
    let ctx = ui.ctx().clone();
    for a in actions {
        let mut chat = lock(&shared);
        match a {
            Action::Send(text) => {
                let mut params = json!({"text": text, "context": context_blocks(app, chips)});
                if let Some(id) = chat.thread {
                    params["thread"] = json!(id);
                }
                match app.session.execute("assistant.send", params) {
                    Ok(v) => {
                        chat.thread = v["thread"].as_u64();
                        chat.running = true;
                        chat.input.clear();
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
            Action::Branch { turn, text } => {
                let Some(from) = chat.thread else { continue };
                let mut params = json!({"thread": from, "turn": turn, "context": context_blocks(app, chips)});
                if let Some(t) = &text {
                    params["text"] = json!(t);
                }
                match app.session.execute("assistant.branch", params) {
                    Ok(v) => {
                        chat.thread = v["thread"].as_u64();
                        chat.running = true;
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
