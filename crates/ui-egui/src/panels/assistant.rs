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
//! `assistant.thread.<id>`), `assistant.chip.selection`, `assistant.chip.playhead`,
//! `assistant.starter.<n>`, `assistant.connect`, `assistant.cost`, `assistant.entry.<n>` (the text
//! of each message), `assistant.tool.<n>` (each tool card), `assistant.settings.thinking`,
//! `assistant.settings.effort`, `assistant.settings.steps`, `assistant.settings.cost`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId, Key, Modifiers, Rect, RichText, vec2};
use serde_json::{Value, json};

use super::assistant_view::{ChatLog, Entry, markdown};
use crate::FilmcraftApp;
use crate::dock::PanelKind;
use crate::theme::Tokens;

/// The warning amber used across panels.
const WARN: Color32 = Color32::from_rgb(0xe8, 0xb3, 0x3c);
/// Seconds between status refreshes while nothing is happening.
const STATUS_EVERY: f64 = 3.0;
const INPUT_HEIGHT: f32 = 64.0;

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
    chip_selection: bool,
    chip_playhead: bool,
    error: Option<String>,
    polled_at: f64,
    /// The slash commands (`/pacing`, …) as the engine lists them.
    commands: Vec<Value>,
    /// The latest thread was picked once at the start; after that "no thread" means "a new one".
    picked_initial: bool,
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
            chip_selection: true,
            chip_playhead: true,
            error: None,
            polled_at: f64::NEG_INFINITY,
            commands: Vec::new(),
            picked_initial: false,
        }
    }

    fn setting(&self, key: &str) -> Value {
        self.status.as_ref().and_then(|s| s.get("settings")).and_then(|s| s.get(key)).cloned().unwrap_or(Value::Null)
    }

    fn ready(&self) -> bool {
        self.status.as_ref().and_then(|s| s.get("ready")).and_then(Value::as_bool).unwrap_or(false)
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
}

/// Brings the panel's picture of the engine up to date: status, models, and the open thread's new
/// events. Cheap when nothing is going on.
fn refresh(app: &mut FilmcraftApp, chat: &mut Chat, now: f64, ctx: &egui::Context) {
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
            chat.usage_text = v["usageText"].as_str().unwrap_or("").to_string();
        }
    }
    if chat.running {
        ctx.request_repaint_after(std::time::Duration::from_millis(40));
    }
}

/// What the assistant is given along with the question, as the chips say: the playhead, and the
/// selected clips.
fn context_blocks(app: &FilmcraftApp, selection: bool, playhead: bool) -> Vec<Value> {
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
    out
}

fn md_job(ui: &egui::Ui, text: &str, color: Color32, size: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    let base = FontId::new(size, FontFamily::Proportional);
    for (n, line) in markdown(text).iter().enumerate() {
        if n > 0 {
            job.append("\n", 0.0, TextFormat { font_id: base.clone(), color, ..Default::default() });
        }
        for span in line {
            let font_id = if span.code { FontId::new(size - 1.0, FontFamily::Monospace) } else { base.clone() };
            let mut f = TextFormat { font_id, color, ..Default::default() };
            if span.bold {
                f.font_id = Tokens::semibold(size);
            }
            if span.code {
                f.background = Color32::from_white_alpha(14);
            }
            job.append(&span.text, 0.0, f);
        }
    }
    job
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

    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 6.0))));
    let ui = &mut child;
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

    // ---- header: model, thread, new, settings ... cost ----
    ui.horizontal(|ui| {
        let name = models
            .as_ref()
            .and_then(|m| m["models"].as_array())
            .and_then(|a| a.iter().find(|m| m["id"] == current_model.as_str()))
            .and_then(|m| m["name"].as_str())
            .map_or(current_model.clone(), str::to_string);
        let combo = egui::ComboBox::from_id_salt("assistant-model").selected_text(short(&name, 22)).width(140.0).show_ui(ui, |ui| {
            let list = models.as_ref().and_then(|m| m["models"].as_array()).cloned().unwrap_or_default();
            if list.is_empty() {
                ui.label(RichText::new(tl!("No models loaded yet")).color(t.text_faint));
            }
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
        elems.push(("assistant.model".into(), combo.response, format!("Model {name}")));

        let title = chat.thread.and_then(|id| threads.iter().find(|t| t.0 == id)).map_or(tl!("New thread").to_string(), |t| t.1.clone());
        let tcombo = egui::ComboBox::from_id_salt("assistant-thread").selected_text(short(&title, 18)).width(110.0).show_ui(ui, |ui| {
            for (id, title) in threads.iter().rev() {
                let r = ui.selectable_label(chat.thread == Some(*id), short(title, 40));
                elems.push((format!("assistant.thread.{id}"), r.clone(), title.clone()));
                if r.clicked() {
                    actions.push(Action::SelectThread(*id));
                }
            }
        });
        elems.push(("assistant.thread".into(), tcombo.response, format!("Thread {title}")));
        let r = ui.add_enabled(!running, egui::Button::new(tl!("New")));
        elems.push(("assistant.new".into(), r.clone(), "New thread".into()));
        if r.clicked() {
            actions.push(Action::NewThread);
        }
        ui.menu_button(tl!("Settings"), |ui| {
            let mut thinking = chat.setting("showThinking").as_bool().unwrap_or(true);
            let r = ui.checkbox(&mut thinking, tl!("Show a summary of its thinking"));
            elems.push(("assistant.settings.thinking".into(), r.clone(), "Show thinking".into()));
            if r.changed() {
                actions.push(Action::Settings(json!({"showThinking": thinking})));
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
            ui.label(RichText::new(tl!("The cost is an estimate from list prices.")).size(11.0).color(t.text_faint));
        });
    });
    // the cost meter, on its own line so a narrow panel never overlaps it with the buttons
    if !chat.usage_text.is_empty() {
        let r = ui.label(RichText::new(&chat.usage_text).size(11.0).color(t.text_dim));
        elems.push(("assistant.cost".into(), r, chat.usage_text.clone()));
    }
    ui.add_space(4.0);

    // ---- not connected ----
    if !ready {
        egui::Frame::new().fill(t.field_bg).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
            ui.label(RichText::new(tl!("Edith needs a model to talk to. Add your Anthropic key in Connections, and this panel will light up.")).color(WARN));
            let r = ui.button(tl!("Open Connections"));
            elems.push(("assistant.connect".into(), r.clone(), "Open Connections".into()));
            if r.clicked() {
                actions.push(Action::OpenConnections);
            }
        });
        ui.add_space(4.0);
    }

    // ---- the conversation ----
    let chips_h = 26.0;
    let transcript_h = (ui.available_height() - INPUT_HEIGHT - chips_h - 14.0).max(40.0);
    let thread = chat.thread;
    let empty = thread.is_none_or(|id| chat.logs.get(&id).is_none_or(|l| l.entries.is_empty()));
    egui::ScrollArea::vertical().id_salt("assistant-scroll").max_height(transcript_h).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        if empty && ready {
            ui.label(
                RichText::new(tl!("Ask about your edit. I can look at the project, the clips, the markers and any transcripts; I can't change anything."))
                    .color(t.text_dim),
            );
            ui.add_space(6.0);
            for (n, q) in [tl!("Summarize my edit"), tl!("Where does the pacing feel slow or uneven?"), tl!("What would make this cut feel more cinematic?")]
                .into_iter()
                .enumerate()
            {
                let r = ui.button(q);
                elems.push((format!("assistant.starter.{n}"), r.clone(), q.to_string()));
                if r.clicked() {
                    actions.push(Action::Send(q.to_string()));
                }
            }
        }
        if let Some(log) = thread.and_then(|id| chat.logs.get(&id)) {
            for (i, e) in log.entries.iter().enumerate() {
                match e {
                    Entry::User(text) => {
                        let f = egui::Frame::new().fill(t.field_bg).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8));
                        let r = f.show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(text).color(t.text));
                        });
                        elems.push((format!("assistant.entry.{i}"), r.response, text.clone()));
                    }
                    Entry::Assistant(text) => {
                        let r = ui.label(md_job(ui, text, t.text, 13.0));
                        elems.push((format!("assistant.entry.{i}"), r, text.clone()));
                    }
                    Entry::Thinking(text) => {
                        let h = egui::CollapsingHeader::new(RichText::new(tl!("Thinking…")).italics().size(12.0).color(t.text_faint))
                            .id_salt(("assistant-think", thread, i))
                            .default_open(false)
                            .show(ui, |ui| {
                                ui.label(RichText::new(text).italics().size(12.0).color(t.text_faint));
                            });
                        elems.push((format!("assistant.entry.{i}"), h.header_response, text.clone()));
                    }
                    Entry::Tool { name, input, result, is_error, .. } => {
                        let color = if *is_error { WARN } else { t.text_dim };
                        let waiting = if result.is_none() { "  …" } else { "" };
                        let title = if input.is_empty() {
                            format!("{} {name}{waiting}", tl!("Looked at"))
                        } else {
                            format!("{} {name}  ({input}){waiting}", tl!("Looked at"))
                        };
                        let h = egui::CollapsingHeader::new(RichText::new(title).size(12.0).color(color))
                            .id_salt(("assistant-tool", thread, i))
                            .default_open(false)
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical().max_height(160.0).id_salt(("assistant-tool-body", thread, i)).show(ui, |ui| {
                                    ui.label(RichText::new(result.as_deref().unwrap_or("")).monospace().size(11.0).color(t.text_dim));
                                });
                            });
                        elems.push((format!("assistant.tool.{i}"), h.header_response, format!("{name}: {}", short(result.as_deref().unwrap_or(""), 300))));
                    }
                    Entry::Note { text, warn } => {
                        let r = ui.label(RichText::new(text).size(12.0).color(if *warn { WARN } else { t.text_faint }));
                        elems.push((format!("assistant.entry.{i}"), r, text.clone()));
                    }
                }
                ui.add_space(4.0);
            }
        }
        if let Some(e) = &chat.error {
            ui.label(RichText::new(e).size(12.0).color(WARN));
        }
        if running {
            ui.label(RichText::new(tl!("Working…")).size(12.0).color(t.text_faint));
        }
    });

    // ---- chips ----
    ui.horizontal(|ui| {
        ui.label(RichText::new(tl!("Sends along:")).size(11.0).color(t.text_faint));
        let blocks = context_blocks(app, true, true);
        let text_of = |label: &str| blocks.iter().find(|b| b["label"] == label).and_then(|b| b["text"].as_str()).unwrap_or("nothing selected").to_string();
        let r = ui.selectable_label(chat.chip_selection, tl!("Selection")).on_hover_text(text_of("selection"));
        elems.push(("assistant.chip.selection".into(), r.clone(), format!("Selection {}", if chat.chip_selection { "on" } else { "off" })));
        if r.clicked() {
            chat.chip_selection = !chat.chip_selection;
        }
        let r = ui.selectable_label(chat.chip_playhead, tl!("Playhead")).on_hover_text(text_of("playhead"));
        elems.push(("assistant.chip.playhead".into(), r.clone(), format!("Playhead {}", if chat.chip_playhead { "on" } else { "off" })));
        if r.clicked() {
            chat.chip_playhead = !chat.chip_playhead;
        }
    });

    // ---- slash commands, while the message is just "/something" ----
    let typing_slash = chat.input.starts_with('/') && !chat.input.contains(char::is_whitespace);
    if typing_slash && ready {
        let prefix = chat.input[1..].to_ascii_lowercase();
        ui.horizontal_wrapped(|ui| {
            for c in chat.commands.iter().filter(|c| c["name"].as_str().is_some_and(|n| n.starts_with(&prefix))) {
                let name = c["name"].as_str().unwrap_or("");
                let r = ui.button(format!("/{name}")).on_hover_text(c["description"].as_str().unwrap_or(""));
                elems.push((format!("assistant.slash.{name}"), r.clone(), format!("/{name}")));
                if r.clicked() {
                    actions.push(Action::SetInput(format!("/{name} ")));
                }
            }
        });
    }

    // ---- input ----
    let input_id = egui::Id::new("assistant-input-field");
    let focused = ui.memory(|m| m.has_focus(input_id));
    // Enter sends; Shift+Enter makes a new line. Taken before the field sees the key.
    let enter = focused && ready && !running && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 70.0).max(60.0);
        let r = ui.add_enabled(
            ready,
            egui::TextEdit::multiline(&mut chat.input).id(input_id).desired_rows(2).desired_width(w).hint_text(tl!("Ask Edith about your edit…")),
        );
        elems.push(("assistant.input".into(), r, "Message".into()));
        ui.vertical(|ui| {
            if running {
                let r = ui.button(tl!("Stop"));
                elems.push(("assistant.stop".into(), r.clone(), "Stop".into()));
                if r.clicked() {
                    actions.push(Action::Stop);
                }
            } else {
                let can = ready && !chat.input.trim().is_empty();
                let r = ui.add_enabled(can, egui::Button::new(tl!("Send")));
                elems.push(("assistant.send".into(), r.clone(), "Send".into()));
                if r.clicked() || (enter && can) {
                    actions.push(Action::Send(chat.input.trim().to_string()));
                }
            }
        });
    });

    let (sel, play) = (chat.chip_selection, chat.chip_playhead);
    drop(chat);
    for (id, r, label) in &elems {
        app.auto.add_response(id, r, label);
    }
    let ctx = ui.ctx().clone();
    for a in actions {
        let mut chat = lock(&shared);
        match a {
            Action::Send(text) => {
                let mut params = json!({"text": text, "context": context_blocks(app, sel, play)});
                if let Some(id) = chat.thread {
                    params["thread"] = json!(id);
                }
                match app.session.execute("assistant.send", params) {
                    Ok(v) => {
                        chat.thread = v["thread"].as_u64();
                        chat.running = true;
                        chat.input.clear();
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
                match app.session.execute("assistant.settings", p) {
                    Ok(_) => chat.error = None,
                    Err(e) => chat.error = Some(e.to_string()),
                }
                chat.status_at = f64::NEG_INFINITY;
            }
            Action::OpenConnections => app.show_panel(PanelKind::Connections),
            Action::SetInput(text) => {
                chat.input = text;
                ctx.memory_mut(|m| m.request_focus(egui::Id::new("assistant-input-field")));
            }
        }
    }
}
