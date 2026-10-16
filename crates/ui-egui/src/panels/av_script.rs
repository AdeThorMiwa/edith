//! A/V Script panel (Window ▸ A/V Script; fork roadmap F09.1): the plan's narration beside its
//! visuals, one row per sentence. The left column is the sentence (editable); the right column is
//! the shots for that sentence, each with its picture (when it has one) and what it should show.
//! The time on each row is estimated from the words at the pack's speaking pace; click it to move
//! the timeline's playhead there.
//!
//! It is one lens on the plan: every edit is a `planner.*` command (`setLine`, `addLine`,
//! `updateShot`, `setShotSource`, …), so the beat board, the beat lane and the narration all see it
//! at once, and each edit is one undo step. Text boxes keep what you type until they lose focus.
//!
//! Automation ids: `script.beat.<id>` (the beat's header), `script.line.<beat>.<n>.text|time|delete`,
//! `script.addLine.<beat>`, `script.send.<beat>`, `script.addShot.<beat>.<n>`,
//! `script.shot.<id>.intent|picture|clear|delete`. State: `ui.planner` (`PlannerView`).

use egui::{Color32, Rect, RichText, Sense, Stroke, StrokeKind, vec2};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::dock::PanelKind;
use crate::panels::planner::{clock, field};
use crate::theme::Tokens;

const THUMB_W: f32 = 64.0;

/// What the plan panels share: which beat and sentence you are on. Serde, so agents can read it
/// (`ui.inspect`) and the lane, board and script agree.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PlannerView {
    pub selected_beat: Option<u64>,
    pub selected_line: Option<u32>,
}

fn secs_text(seconds: f64) -> String {
    format!("~{:.0} s", seconds.max(0.0).round())
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    // only ask when there is a plan: a command that fails is logged as an error event
    let got = if filmcraft_engine::planner::get(&app.session.project).is_some() {
        app.session.execute("planner.lines", json!({})).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(rect.min + vec2(10.0, 8.0), rect.max - vec2(10.0, 4.0))));
    let ui = &mut child;
    let ctx = ui.ctx().clone();
    let mut elems: Vec<(String, egui::Response, String)> = Vec::new();
    let mut commands: Vec<(&'static str, Value)> = Vec::new();
    let mut seek: Option<i64> = None;
    let mut select: Option<(u64, Option<u32>)> = None;
    let mut send: Option<u64> = None;
    let selected = app.ui.planner.clone();
    let selection = app.session.state.project_selection.first().copied();

    egui::ScrollArea::vertical().id_salt("av-script-scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let Some(beats) = got["beats"].as_array() else {
            ui.add(
                egui::Label::new(RichText::new(tl!("No plan yet. Open Window ▸ Planner and create one from a Format Pack.")).size(12.0).color(t.text_dim))
                    .wrap(),
            );
            return;
        };
        let full = ui.available_width();
        let left = (full * 0.5).max(120.0);
        let right = (full - left - 24.0).max(120.0);
        for b in beats {
            let bid = b["beat"].as_u64().unwrap_or(0);
            let is_sel = selected.selected_beat == Some(bid);
            // ---- the beat's header
            let head = egui::Frame::new().fill(if is_sel { t.hover } else { t.field_bg }).corner_radius(3.0).inner_margin(5.0).show(ui, |ui| {
                ui.set_width(full - 10.0);
                ui.horizontal(|ui| {
                    let name =
                        ui.add(egui::Label::new(RichText::new(b["name"].as_str().unwrap_or("")).size(12.5).strong().color(t.text)).sense(Sense::click()));
                    elems.push((format!("script.beat.{bid}"), name.clone(), b["name"].as_str().unwrap_or("").into()));
                    if name.clicked() {
                        select = Some((bid, None));
                    }
                    ui.label(RichText::new(clock(b["targetTicks"].as_i64().unwrap_or(0))).size(11.5).color(t.text_dim));
                    ui.label(RichText::new(b["sectionName"].as_str().unwrap_or("")).size(11.0).color(t.text_faint));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let r = ui.small_button(tl!("Narrate")).on_hover_text(tl!("Put this beat's narration in the Text to Speech panel."));
                        elems.push((format!("script.send.{bid}"), r.clone(), "Send the beat's narration to Text to Speech".into()));
                        if r.clicked() {
                            send = Some(bid);
                        }
                    });
                });
                if let Some(p) = b["purpose"].as_str().filter(|p| !p.is_empty()) {
                    ui.add(egui::Label::new(RichText::new(p).size(11.0).color(t.text_faint)).wrap());
                }
            });
            let _ = head;
            // ---- one row per sentence
            let lines = b["lines"].as_array().cloned().unwrap_or_default();
            let shots: Vec<Value> = b["shots"].as_array().cloned().unwrap_or_default();
            for l in &lines {
                let i = l["index"].as_u64().unwrap_or(0) as u32;
                let row_sel = selected.selected_beat == Some(bid) && selected.selected_line == Some(i);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    // left: time, sentence
                    ui.allocate_ui(vec2(left, 10.0), |ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                let start = l["startTicks"].as_i64().unwrap_or(0);
                                let label = format!("{}  {}", clock(start), secs_text(l["seconds"].as_f64().unwrap_or(0.0)));
                                let r = ui.add(
                                    egui::Label::new(RichText::new(label.clone()).size(10.5).color(if row_sel { t.accent } else { t.text_faint }))
                                        .sense(Sense::click()),
                                );
                                let r = r.on_hover_text(tl!("Jump the timeline here."));
                                elems.push((format!("script.line.{bid}.{i}.time"), r.clone(), format!("Jump to {label}")));
                                if r.clicked() {
                                    seek = Some(start);
                                    select = Some((bid, Some(i)));
                                }
                                let x = ui.small_button("✕").on_hover_text(tl!("Delete this sentence"));
                                elems.push((format!("script.line.{bid}.{i}.delete"), x.clone(), "Delete this sentence".into()));
                                if x.clicked() {
                                    commands.push(("planner.setLine", json!({"beat": bid, "line": i, "text": ""})));
                                }
                            });
                            let (r, new) = field(ui, &format!("line.{bid}.{i}"), l["text"].as_str().unwrap_or(""), true, left - 4.0, 2, tl!("Narration"), &t);
                            elems.push((format!("script.line.{bid}.{i}.text"), r.clone(), "Sentence of narration".into()));
                            if r.has_focus() {
                                select = Some((bid, Some(i)));
                            }
                            if let Some(v) = new.map(|v| v.trim().to_string()).filter(|v| !v.contains('\n')) {
                                commands.push(("planner.setLine", json!({"beat": bid, "line": i, "text": v})));
                            }
                        });
                    });
                    // right: the shots for this sentence
                    ui.vertical(|ui| {
                        for x in shots.iter().filter(|x| x["line"].as_u64() == Some(u64::from(i))) {
                            shot_row(ui, app, &ctx, &t, x, selection, right, &mut elems, &mut commands);
                        }
                        let r = ui.small_button(tl!("+ visual")).on_hover_text(tl!("Add a shot for this sentence."));
                        elems.push((format!("script.addShot.{bid}.{i}"), r.clone(), "Add a shot for this sentence".into()));
                        if r.clicked() {
                            commands.push(("planner.addShot", json!({"beat": bid, "line": i, "intent": ""})));
                        }
                    });
                });
                ui.add_space(4.0);
            }
            // shots for the beat as a whole, and a way to add a sentence
            let whole: Vec<&Value> = shots.iter().filter(|x| x["line"].is_null()).collect();
            if !whole.is_empty() {
                ui.label(RichText::new(tl!("Whole beat")).size(10.5).color(t.text_faint));
                for x in whole {
                    shot_row(ui, app, &ctx, &t, x, selection, right, &mut elems, &mut commands);
                }
            }
            ui.horizontal(|ui| {
                let r = ui.small_button(tl!("+ sentence"));
                elems.push((format!("script.addLine.{bid}"), r.clone(), "Add a sentence to this beat".into()));
                if r.clicked() {
                    commands.push(("planner.addLine", json!({"beat": bid, "text": tl!("New sentence.")})));
                }
                let r = ui.small_button(tl!("+ visual for the beat"));
                elems.push((format!("script.addShot.{bid}"), r.clone(), "Add a shot for the whole beat".into()));
                if r.clicked() {
                    commands.push(("planner.addShot", json!({"beat": bid, "intent": ""})));
                }
            });
            ui.add_space(8.0);
        }
    });
    for (id, r, label) in &elems {
        app.auto.add_response(id, r, label);
    }
    for (cmd, p) in commands {
        if let Err(e) = app.session.execute(cmd, p) {
            app.ui.status = e.to_string();
        }
    }
    if let Some((beat, line)) = select {
        app.ui.planner.selected_beat = Some(beat);
        app.ui.planner.selected_line = line;
    }
    if let Some(ticks) = seek {
        app.session.set_playhead(Tick(ticks));
    }
    if let Some(bid) = send {
        // read after the edits above, so a sentence typed a moment ago is in it
        let plan = app.session.execute("planner.get", json!({})).unwrap_or(Value::Null);
        let text = plan["plan"]["sections"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|sec| sec["beats"].as_array().into_iter().flatten())
            .find(|b| b["id"].as_u64() == Some(bid))
            .and_then(|b| b["narration"].as_str())
            .unwrap_or("")
            .to_string();
        app.ui.tts.text = text;
        app.ui.tts.loaded_from = None;
        app.show_panel(PanelKind::TextToSpeech);
        app.ui.status = tl!("The beat's narration is in the Text to Speech panel.").to_string();
    }
}

#[allow(clippy::too_many_arguments)]
fn shot_row(
    ui: &mut egui::Ui,
    app: &mut FilmcraftApp,
    ctx: &egui::Context,
    t: &Tokens,
    x: &Value,
    selection: Option<filmcraft_project::ItemId>,
    width: f32,
    elems: &mut Vec<(String, egui::Response, String)>,
    commands: &mut Vec<(&'static str, Value)>,
) {
    let id = x["id"].as_u64().unwrap_or(0);
    let source = x["source"].as_u64();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        // the picture, or a plain box saying it is still to be found
        let (r, resp) = ui.allocate_exact_size(vec2(THUMB_W, THUMB_W * 9.0 / 16.0), Sense::click());
        let tex = source.and_then(|i| app.thumbnail(ctx, filmcraft_project::ItemId(i), Tick::ZERO, 160));
        match tex {
            Some((tid, _)) => {
                ui.painter().image(tid, r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                ui.painter().rect_filled(r, 3.0, t.field_bg);
                ui.painter().text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    if source.is_some() { tl!("Picture") } else { tl!("No picture") },
                    Tokens::ui(10.0),
                    t.text_faint,
                );
            }
        }
        ui.painter().rect_stroke(r, 3.0, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
        let hover = if selection.is_some() {
            tl!("Use the clip selected in the Project panel as this shot's picture.")
        } else {
            tl!("Select a clip in the Project panel, then press here to use it as the picture.")
        };
        let resp = resp.on_hover_text(hover);
        elems.push((format!("script.shot.{id}.picture"), resp.clone(), "Use the selected project clip as this shot's picture".into()));
        if resp.clicked()
            && let Some(item) = selection
        {
            commands.push(("planner.setShotSource", json!({"shot": id, "item": item.0})));
        }
        ui.vertical(|ui| {
            let (r, new) = field(
                ui,
                &format!("shot.{id}"),
                x["intent"].as_str().unwrap_or(""),
                true,
                (width - THUMB_W - 12.0).max(70.0),
                2,
                tl!("What the picture shows"),
                t,
            );
            elems.push((format!("script.shot.{id}.intent"), r, "What the shot should show".into()));
            if let Some(v) = new {
                commands.push(("planner.updateShot", json!({"shot": id, "intent": v})));
            }
            ui.horizontal(|ui| {
                if source.is_some() {
                    let c = ui.small_button(tl!("No picture"));
                    elems.push((format!("script.shot.{id}.clear"), c.clone(), "Remove the shot's picture".into()));
                    if c.clicked() {
                        commands.push(("planner.setShotSource", json!({"shot": id, "item": Value::Null})));
                    }
                }
                let d = ui.small_button("✕").on_hover_text(tl!("Delete this shot"));
                elems.push((format!("script.shot.{id}.delete"), d.clone(), "Delete this shot".into()));
                if d.clicked() {
                    commands.push(("planner.delete", json!({"shot": id})));
                }
            });
        });
    });
}
