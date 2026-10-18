//! Question and download cards in the conversation (fork roadmap F16.1b).
//!
//! The assistant asks by putting cards in the conversation and ending its turn. A **question** card shows the
//! question and its choices (and a box for the person's own words); a **download** card shows what the clip
//! is (picture, where it is from, its licence, size) and **Yes**, **No** and **Show me others**. Nothing is
//! sent by choosing: the cards of the latest turn form one group with one **Send answers** button under the
//! last of them, which sends every answer as one message. **Yes** starts the download at once, as the
//! person's own action; **Show me others** opens Find Media with the assistant's search typed in.
//!
//! The cards themselves are saved in the conversation; what has been chosen but not sent lives in the
//! engine (`assistant.cards`, `assistant.card`, `assistant.answerCards`), so the control channel sees and sets
//! the same state. This module only draws it and reports clicks.
//!
//! Automation ids (`<id>` is the card's id, `c1`, `c2`, …): `assistant.card.<id>` (the card),
//! `assistant.card.<id>.option.<n>`, `assistant.card.<id>.other` (the box for their own words),
//! `assistant.card.<id>.yes`, `.no`, `.others`, and `assistant.cards.send`.

use std::collections::HashMap;

use egui::{Color32, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use filmcraft_engine::provenance::Status;
use serde_json::{Value, json};

use super::assistant_widgets::{BODY, SMALL};
use crate::FilmcraftApp;
use crate::panels::rights;
use crate::theme::Tokens;

/// Choices and long texts are drawn up to these lengths, whatever a saved thread holds.
const MAX_OPTIONS: usize = 8;
const MAX_TEXT: usize = 400;
const THUMB: egui::Vec2 = vec2(112.0, 72.0);

pub type Elems = Vec<(String, egui::Response, String)>;

/// What a click on a card asks for. The panel runs it.
pub enum CardAction {
    /// `assistant.card` with these parameters (`option`, `other` or `decision`) for this card.
    Choose { card: String, params: Value },
    /// `assistant.answerCards`.
    Send,
}

fn short(s: &str) -> String {
    let t: String = s.chars().take(MAX_TEXT).collect();
    if s.chars().count() > MAX_TEXT { format!("{t}…") } else { t }
}

/// How a licence reads as a status, the way the Find Media panel works it out.
fn status_of(rights: &Value) -> Status {
    if rights["commercial"].as_bool() != Some(true) {
        Status::Risk
    } else if rights["attributionRequired"].as_bool() != Some(false) {
        Status::Credit
    } else {
        Status::Clear
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

/// A row for one choice: the label, a line of detail under it, filled when it is the chosen one.
fn choice_row(ui: &mut Ui, label: &str, detail: &str, selected: bool, enabled: bool, t: &Tokens) -> egui::Response {
    let w = ui.available_width();
    let text_w = (w - 26.0).max(40.0);
    let name = ui.painter().layout(label.to_string(), Tokens::ui(12.5), if selected { Color32::WHITE } else { t.text }, text_w);
    let more = (!detail.is_empty())
        .then(|| ui.painter().layout(detail.to_string(), Tokens::ui(SMALL), if selected { Color32::from_white_alpha(200) } else { t.text_faint }, text_w));
    let h = 12.0 + name.size().y + more.as_ref().map_or(0.0, |m| m.size().y + 2.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h.max(28.0)), if enabled { Sense::click() } else { Sense::hover() });
    let (fill, stroke) = match (selected, resp.hovered() && enabled) {
        (true, _) => (t.accent, Stroke::NONE),
        (false, true) => (t.hover, Stroke::new(1.0, t.field_border)),
        (false, false) => (t.panel_bg, Stroke::new(1.0, t.separator)),
    };
    ui.painter().rect_filled(rect, 4.0, fill);
    if stroke != Stroke::NONE {
        ui.painter().rect_stroke(rect, 4.0, stroke, StrokeKind::Inside);
    }
    let mut y = rect.min.y + 6.0;
    let at = pos2(rect.min.x + 10.0, y);
    y += name.size().y + 2.0;
    ui.painter().galley(at, name, t.text);
    if let Some(m) = more {
        ui.painter().galley(pos2(rect.min.x + 10.0, y), m, t.text_faint);
    }
    resp
}

/// A button for a decision on a download card: filled when it is the one chosen, accent when it is the main one.
fn decision_button(ui: &mut Ui, label: &str, chosen: bool, main: bool, enabled: bool, t: &Tokens) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), Tokens::ui(12.0), t.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 22.0, 26.0), if enabled { Sense::click() } else { Sense::hover() });
    let hot = resp.hovered() && enabled;
    let (fill, text) = if chosen {
        (t.accent, Color32::WHITE)
    } else if main && enabled {
        (if hot { t.accent_hover } else { t.accent }, Color32::WHITE)
    } else {
        (if hot { t.hover } else { t.panel_bg }, if enabled { t.text } else { t.text_faint })
    };
    ui.painter().rect_filled(rect, 4.0, fill);
    if !chosen && !(main && enabled) {
        ui.painter().rect_stroke(rect, 4.0, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
    }
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, Tokens::ui(12.0), text);
    resp
}

/// Draws one card. `open` is the engine's `assistant.cards` answer for the thread; `answers` is what the cards
/// that were sent were answered with; `running` is whether the assistant is still working.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    ui: &mut Ui,
    app: &mut FilmcraftApp,
    ctx: &egui::Context,
    view: &Value,
    thread: u64,
    open: &Value,
    answers: &HashMap<String, String>,
    running: bool,
    t: &Tokens,
    elems: &mut Elems,
    out: &mut Vec<CardAction>,
) {
    let id = view["id"].as_str().unwrap_or("").to_string();
    let download = view["kind"] == "download";
    let live = open["cards"].as_array().and_then(|a| a.iter().find(|c| c["id"].as_str() == Some(id.as_str())));
    let sent_as = answers.get(&id);
    let can_answer = live.is_some() && sent_as.is_none() && !running;
    let draft = live.map(|c| &c["draft"]).filter(|d| !d.is_null());
    let draft_kind = draft.and_then(|d| d["kind"].as_str()).unwrap_or("");
    let dl_state = live.and_then(|c| c["download"]["state"].as_str()).unwrap_or("");
    let started = draft_kind == "yes";

    let frame = egui::Frame::new().fill(t.field_bg).stroke(Stroke::new(1.0, t.field_border)).corner_radius(6.0).inner_margin(egui::Margin::symmetric(10, 8));
    let shown = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(if download { tl!("DOWNLOAD REQUEST") } else { tl!("QUESTION") }).size(10.5).strong().color(t.accent));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (text, color) = match (sent_as.is_some(), live.is_some()) {
                    (true, _) => (tl!("Answered"), t.text_faint),
                    (false, true) => (tl!("Waiting for your answer"), t.accent),
                    (false, false) => (tl!("Not answered"), t.text_faint),
                };
                ui.label(RichText::new(text).size(10.5).color(color));
            });
        });
        // an answered or closed card is only its question and its answer; the choices belong to a card still waiting
        let waiting_card = live.is_some() && sent_as.is_none();
        if download {
            draw_download(ui, app, ctx, view, id.as_str(), (can_answer, waiting_card), (draft_kind, dl_state, started), t, elems, out);
        } else {
            draw_question(ui, view, id.as_str(), thread, (can_answer, waiting_card), draft, t, elems, out);
        }
        if let Some(words) = sent_as {
            ui.add_space(2.0);
            let said = match words.strip_prefix("(in their own words) ") {
                Some(own) => format!("{} {}", tl!("Your words:"), short(own)),
                None => short(words),
            };
            ui.add(egui::Label::new(RichText::new(format!("→ {said}")).size(12.0).color(t.accent)).wrap());
        }
        // the group's one Send button sits under its last card
        let last_open = open["cards"].as_array().and_then(|a| a.last()).and_then(|c| c["id"].as_str()) == Some(id.as_str());
        if live.is_some() && sent_as.is_none() && last_open {
            ui.add_space(4.0);
            ui.separator();
            let waiting = open["waiting"].as_u64().unwrap_or(0);
            let total = open["cards"].as_array().map_or(0, Vec::len);
            let answered = open["answered"].as_u64().unwrap_or(0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(tlf!("{n} of {m} answered", n = answered, m = total)).size(11.0).color(t.text_faint));
                if waiting > 0 {
                    ui.label(RichText::new(tl!("Waiting for the download…")).size(11.0).color(t.text_dim));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ok = open["canSend"].as_bool() == Some(true);
                    let r = decision_button(ui, tl!("Send answers"), false, true, ok, t).on_hover_text(if running {
                        tl!("Edith is still working.")
                    } else if waiting > 0 {
                        tl!("Sends when the download has finished, so the clip is in the project for Edith.")
                    } else {
                        tl!("Sends all your answers to Edith in one message. A question you left alone goes as no answer.")
                    });
                    elems.push(("assistant.cards.send".into(), r.clone(), "Send answers".into()));
                    if ok && r.clicked() {
                        out.push(CardAction::Send);
                    }
                });
            });
        }
    });
    let label = if download { short(view["candidate"]["title"].as_str().unwrap_or("")) } else { short(view["question"].as_str().unwrap_or("")) };
    elems.push((format!("assistant.card.{id}"), shown.response, label));
}

#[allow(clippy::too_many_arguments)]
fn draw_question(
    ui: &mut Ui,
    view: &Value,
    id: &str,
    thread: u64,
    state: (bool, bool),
    draft: Option<&Value>,
    t: &Tokens,
    elems: &mut Elems,
    out: &mut Vec<CardAction>,
) {
    let (can_answer, waiting_card) = state;
    ui.add(egui::Label::new(RichText::new(short(view["question"].as_str().unwrap_or(""))).size(BODY).color(t.text)).wrap());
    if !waiting_card {
        return;
    }
    ui.add_space(4.0);
    let chosen = draft.filter(|d| d["kind"] == "option").and_then(|d| d["index"].as_u64());
    for (n, o) in view["options"].as_array().into_iter().flatten().take(MAX_OPTIONS).enumerate() {
        let label = short(o["label"].as_str().unwrap_or(""));
        let detail = short(o["detail"].as_str().unwrap_or(""));
        let r = choice_row(ui, &label, &detail, chosen == Some(n as u64), can_answer, t);
        elems.push((format!("assistant.card.{id}.option.{n}"), r.clone(), label.clone()));
        if can_answer && r.clicked() {
            out.push(CardAction::Choose { card: id.to_string(), params: json!({"option": n}) });
        }
    }
    if view["allowOther"] == true && (can_answer || draft.is_some_and(|d| d["kind"] == "other")) {
        let own = draft.filter(|d| d["kind"] == "other").and_then(|d| d["text"].as_str());
        if let Some(text) = own {
            ui.add(egui::Label::new(RichText::new(format!("{} {}", tl!("Your words:"), short(text))).size(12.0).color(t.accent)).wrap());
        }
        if can_answer {
            let key = egui::Id::new(("assistant-card-other", thread, id.to_string()));
            let mut typed: String = ui.data_mut(|d| d.get_temp::<String>(key)).unwrap_or_default();
            let r = ui.add(
                egui::TextEdit::singleline(&mut typed)
                    .id(key.with("field"))
                    .desired_width(ui.available_width())
                    .hint_text(RichText::new(tl!("Or in your own words…")).color(t.text_faint)),
            );
            elems.push((format!("assistant.card.{id}.other"), r.clone(), "Answer in your own words".into()));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !typed.trim().is_empty() {
                out.push(CardAction::Choose { card: id.to_string(), params: json!({"other": typed.trim()}) });
                typed.clear();
            }
            ui.data_mut(|d| d.insert_temp(key, typed));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_download(
    ui: &mut Ui,
    app: &mut FilmcraftApp,
    ctx: &egui::Context,
    view: &Value,
    id: &str,
    answerable: (bool, bool),
    state: (&str, &str, bool),
    t: &Tokens,
    elems: &mut Elems,
    out: &mut Vec<CardAction>,
) {
    let (draft_kind, dl_state, started) = state;
    let (can_answer, waiting_card) = answerable;
    let c = &view["candidate"];
    ui.add_space(4.0);
    ui.horizontal_top(|ui| {
        // the picture: the clip's own thumbnail while this session still has it, else a plain block
        let (rect, _) = ui.allocate_exact_size(THUMB, Sense::hover());
        ui.painter().rect_filled(rect, 4.0, t.panel_bg);
        let tex = c["id"].as_str().and_then(|cid| super::find_media::card_texture(app, ctx, cid));
        match tex {
            Some(tex) => {
                let ts = tex.size_vec2();
                let scale = (rect.width() / ts.x.max(1.0)).max(rect.height() / ts.y.max(1.0));
                let shown = Rect::from_center_size(rect.center(), ts * scale).intersect(rect);
                let uv = Rect::from_min_max(
                    pos2(0.5 + (shown.min.x - rect.center().x) / (ts.x * scale), 0.5 + (shown.min.y - rect.center().y) / (ts.y * scale)),
                    pos2(0.5 + (shown.max.x - rect.center().x) / (ts.x * scale), 0.5 + (shown.max.y - rect.center().y) / (ts.y * scale)),
                );
                ui.painter().image(tex.id(), shown, uv, Color32::WHITE);
            }
            None => {
                let word = c["kind"].as_str().unwrap_or("");
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, word, Tokens::ui(11.0), t.text_faint);
            }
        }
        ui.vertical(|ui| {
            ui.add(egui::Label::new(RichText::new(short(c["title"].as_str().unwrap_or(""))).font(Tokens::semibold(BODY)).color(t.text)).wrap());
            let by = c["author"].as_str().filter(|a| !a.is_empty()).map_or(String::new(), |a| format!(" · {} {}", tl!("by"), short(a)));
            ui.label(RichText::new(format!("{}{by}", short(c["sourceLabel"].as_str().unwrap_or("")))).size(SMALL).color(t.text_faint));
            let mut facts: Vec<String> = Vec::new();
            if let (Some(w), Some(h)) = (c["width"].as_u64(), c["height"].as_u64()) {
                facts.push(format!("{w}×{h}"));
            }
            if let Some(s) = c["seconds"].as_f64().filter(|s| s.is_finite() && *s >= 0.0) {
                facts.push(format!("{s:.0} s"));
            }
            if let Some(b) = c["bytes"].as_u64() {
                facts.push(megabytes(b));
            }
            if !facts.is_empty() {
                ui.label(RichText::new(facts.join(" · ")).size(SMALL).color(t.text_faint));
            }
            let status = status_of(&c["rights"]);
            ui.add(egui::Label::new(RichText::new(short(c["rightsLine"].as_str().unwrap_or(""))).size(SMALL).color(rights::color(status))).wrap());
        });
    });
    if let Some(why) = view["why"].as_str().filter(|w| !w.is_empty()) {
        ui.add_space(2.0);
        ui.add(egui::Label::new(RichText::new(format!("{} {}", tl!("Why this clip:"), short(why))).size(11.5).italics().color(t.text_dim)).wrap());
    }
    if !waiting_card {
        return;
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let choices: [(&str, &str, &str, bool); 3] =
            [("yes", tl!("Yes"), "decision.yes", true), ("no", tl!("No"), "decision.no", false), ("others", tl!("Show me others"), "decision.others", false)];
        for (kind, label, _, main) in choices {
            // after Yes the download has started and the card is settled
            let enabled = can_answer && !started;
            let r = decision_button(ui, label, draft_kind == kind, main && draft_kind.is_empty(), enabled || (draft_kind == kind), t);
            elems.push((format!("assistant.card.{id}.{kind}"), r.clone(), label.to_string()));
            if enabled && r.clicked() {
                out.push(CardAction::Choose { card: id.to_string(), params: json!({"decision": kind}) });
            }
        }
        match dl_state {
            "downloading" => {
                ui.label(RichText::new(tl!("Downloading…")).size(11.5).color(t.text_dim));
            }
            "done" => {
                ui.label(RichText::new(tl!("Added to the project")).size(11.5).color(rights::color(Status::Clear)));
            }
            "failed" => {
                ui.label(RichText::new(tl!("The download failed")).size(11.5).color(rights::color(Status::Credit)));
            }
            _ => {}
        }
    });
}
