//! Planner panel (Window ▸ Planner; fork roadmap F08.4): plan a video before and while you cut it.
//! Pick a Format Pack and a length, press Create, and the beats appear with their lengths worked
//! out; edit each beat's name, length, intensity, purpose and narration; Build skeleton lays the
//! beats on the timeline as range markers (and offline placeholders for planned shots).
//!
//! This is the small first view of the plan. The A/V script, beat lane and board (roadmap F09) are
//! richer views on the same data. Every action is a `planner.*` command, so the panel, MCP and the
//! control channel share one path; the plan itself lives in the project, undoable.
//!
//! Text fields keep what you are typing until they lose focus, then make one edit (one undo step).
//!
//! Automation ids: `planner.pack`, `planner.minutes`, `planner.clues`, `planner.create`,
//! `planner.title`, `planner.length`, `planner.fit`, `planner.skeleton`, `planner.clear`,
//! `planner.total`, `planner.beat.<id>.name|length|intensity|status|purpose|narration`.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Color32, RichText, TextEdit};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::theme::Tokens;

const STATUSES: [&str; 5] = ["idea", "drafted", "sourced", "cut", "done"];

struct State {
    pack: String,
    /// What is typed in the length box (minutes).
    minutes: String,
    clues: u32,
}

impl Default for State {
    fn default() -> Self {
        Self { pack: String::new(), minutes: "10".into(), clues: 0 }
    }
}

fn status_label(s: &str) -> &'static str {
    match s {
        "drafted" => tl!("Drafted"),
        "sourced" => tl!("Sourced"),
        "cut" => tl!("Cut"),
        "done" => tl!("Done"),
        _ => tl!("Idea"),
    }
}

/// 722 seconds as `12:02`; an hour or more as `1:05:00`.
pub fn clock(ticks: i64) -> String {
    let n = (ticks.max(0) as f64 / filmcraft_time::TICKS_PER_SECOND as f64).round() as u64;
    if n >= 3600 { format!("{}:{:02}:{:02}", n / 3600, (n / 60) % 60, n % 60) } else { format!("{}:{:02}", n / 60, n % 60) }
}

/// `12:02`, `1:05:00`, `90` (seconds) or `1.5m` as seconds; None when it is none of these.
pub fn parse_clock(text: &str) -> Option<f64> {
    let t = text.trim();
    if t.is_empty() || t.len() > 12 {
        return None;
    }
    if let Some(m) = t.strip_suffix('m') {
        return m.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0).map(|v| v * 60.0);
    }
    let mut total = 0.0;
    for part in t.split(':') {
        let v: f64 = part.trim().parse().ok().filter(|v: &f64| v.is_finite() && *v >= 0.0)?;
        total = total * 60.0 + v;
    }
    Some(total).filter(|v| *v <= 12.0 * 3600.0)
}

/// A text box that keeps what you type until it loses focus; returns the new text then (when it
/// changed). `key` identifies the box.
pub(crate) fn field(
    ui: &mut egui::Ui,
    key: &str,
    current: &str,
    multiline: bool,
    width: f32,
    rows: usize,
    hint: &str,
    t: &Tokens,
) -> (egui::Response, Option<String>) {
    let buf = egui::Id::new(("planner-buffer", key));
    let mut text = ui.data(|d| d.get_temp::<String>(buf)).unwrap_or_else(|| current.to_string());
    let edit = if multiline { TextEdit::multiline(&mut text).desired_rows(rows) } else { TextEdit::singleline(&mut text) };
    let r = ui.add(edit.id(egui::Id::new(("planner-field", key))).desired_width(width).hint_text(RichText::new(hint).color(t.text_faint)));
    let mut out = None;
    if r.has_focus() || r.changed() {
        ui.data_mut(|d| d.insert_temp(buf, text.clone()));
    }
    if r.lost_focus() {
        ui.data_mut(|d| d.remove::<String>(buf));
        if text != current {
            out = Some(text);
        }
    }
    (r, out)
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: egui::Rect) {
    let t = app.tokens;
    let shared: Arc<Mutex<State>> =
        ui.data_mut(|d| d.get_temp_mut_or_insert_with(egui::Id::new("planner-state"), || Arc::new(Mutex::new(State::default()))).clone());
    let mut st = shared.lock().unwrap_or_else(PoisonError::into_inner);
    let packs = app.session.execute("planner.packs", json!({})).unwrap_or(Value::Null);
    let got = app.session.execute("planner.get", json!({})).unwrap_or(Value::Null);
    let has_sequence = app.session.active_sequence().is_some();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(rect.min + egui::vec2(10.0, 8.0), rect.max - egui::vec2(10.0, 4.0))));
    let ui = &mut child;
    let mut elems: Vec<(String, egui::Response, String)> = Vec::new();
    let mut commands: Vec<(&'static str, Value)> = Vec::new();
    let mut note: Option<String> = None;

    egui::ScrollArea::vertical().id_salt("planner-scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if got["exists"].as_bool() != Some(true) {
            create_form(ui, &t, &mut st, &packs, &mut elems, &mut commands);
            return;
        }
        let plan = &got["plan"];
        let sum = &got["summary"];
        // title and length
        let (r, new) = field(ui, "title", plan["title"].as_str().unwrap_or(""), false, ui.available_width(), 1, tl!("Title of the video"), &t);
        elems.push(("planner.title".into(), r, "Title of the video".into()));
        if let Some(v) = new {
            commands.push(("planner.update", json!({"title": v})));
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(tl!("Length")).size(12.0).color(t.text_dim));
            let (r, new) = field(ui, "length", &clock(plan["target"].as_i64().unwrap_or(0)), false, 70.0, 1, "12:00", &t);
            elems.push(("planner.length".into(), r, "Length of the video (minutes and seconds)".into()));
            if let Some(v) = new {
                match parse_clock(&v) {
                    Some(secs) => commands.push(("planner.update", json!({"seconds": secs}))),
                    None => note = Some(tl!("Write a length like 12:00, 90 (seconds) or 1.5m").to_string()),
                }
            }
        });
        let (planned, target) = (sum["plannedTicks"].as_i64().unwrap_or(0), sum["targetTicks"].as_i64().unwrap_or(0));
        let off = planned != target;
        let line = tlf!("{beats} beats · {planned} planned of {target}", beats = sum["beats"].as_u64().unwrap_or(0), planned = clock(planned), target = clock(target));
        let r = ui.label(RichText::new(line.clone()).size(12.0).color(if off { Color32::from_rgb(0xe8, 0xb3, 0x3c) } else { t.text_dim }));
        elems.push(("planner.total".into(), r, line));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if off && target > 0 {
                let b = ui.button(tl!("Fit beats to the length")).on_hover_text(tl!("Scales every beat in proportion so they add up to the length."));
                elems.push(("planner.fit".into(), b.clone(), "Fit beats to the length".into()));
                if b.clicked() {
                    commands.push(("planner.update", json!({"ticks": target, "fit": true})));
                }
            }
            let b = ui
                .add_enabled(has_sequence, egui::Button::new(tl!("Build skeleton")))
                .on_hover_text(tl!("Puts a marker for every beat on the timeline, and a placeholder clip for every planned shot. Run it again after changing the plan: markers move, nothing is doubled."));
            elems.push(("planner.skeleton".into(), b.clone(), "Build the skeleton on the timeline".into()));
            if b.clicked() {
                commands.push(("planner.buildSkeleton", json!({})));
            }
            let b = ui.button(tl!("Start over")).on_hover_text(tl!("Removes the plan and the markers and placeholder clips it put on the timeline. Undo brings it all back."));
            elems.push(("planner.clear".into(), b.clone(), "Remove the plan".into()));
            if b.clicked() {
                commands.push(("planner.clear", json!({})));
            }
        });
        ui.add_space(6.0);
        // sections and beats
        for sec in plan["sections"].as_array().into_iter().flatten() {
            ui.label(RichText::new(sec["name"].as_str().unwrap_or("")).size(12.5).strong().color(t.text));
            ui.add_space(2.0);
            for b in sec["beats"].as_array().into_iter().flatten() {
                beat_card(ui, &t, b, &mut elems, &mut commands);
            }
            ui.add_space(6.0);
        }
        if sum["lookPreset"].is_string() {
            ui.label(
                RichText::new(tlf!("This kind of video usually takes the look \"{name}\" (Effects ▸ Presets).", name = sum["lookPreset"].as_str().unwrap_or("")))
                    .size(11.0)
                    .color(t.text_faint),
            );
        }
    });
    drop(st);
    for (id, r, label) in &elems {
        app.auto.add_response(id, r, label);
    }
    for (cmd, p) in commands {
        match app.session.execute(cmd, p) {
            Ok(r) if cmd == "planner.buildSkeleton" => {
                app.ui.status =
                    tlf!("Skeleton built: {m} markers, {p} placeholders", m = r["markers"].as_u64().unwrap_or(0), p = r["placeholders"].as_u64().unwrap_or(0));
            }
            Ok(_) => {}
            Err(e) => app.ui.status = e.to_string(),
        }
    }
    if let Some(n) = note {
        app.ui.status = n;
    }
}

fn create_form(
    ui: &mut egui::Ui,
    t: &Tokens,
    st: &mut State,
    packs: &Value,
    elems: &mut Vec<(String, egui::Response, String)>,
    commands: &mut Vec<(&'static str, Value)>,
) {
    ui.add(
        egui::Label::new(
            RichText::new(tl!("Plan a video before you cut it. Pick the kind of video and how long it should be, and the beats are laid out for you to edit."))
                .size(12.0)
                .color(t.text_dim),
        )
        .wrap(),
    );
    ui.add_space(8.0);
    let list: Vec<Value> = packs["packs"].as_array().cloned().unwrap_or_default();
    if !list.iter().any(|p| p["id"].as_str() == Some(st.pack.as_str())) {
        st.pack = list.first().and_then(|p| p["id"].as_str()).unwrap_or("").to_string();
    }
    let current = list.iter().find(|p| p["id"].as_str() == Some(st.pack.as_str())).cloned().unwrap_or(Value::Null);
    ui.label(RichText::new(tl!("Kind of video")).size(12.0).strong().color(t.text));
    let cb = egui::ComboBox::from_id_salt("planner-pack").selected_text(current["name"].as_str().unwrap_or("")).width(ui.available_width()).show_ui(ui, |ui| {
        for p in &list {
            let id = p["id"].as_str().unwrap_or("");
            let label = if p["source"] == "custom" {
                format!("{} ({})", p["name"].as_str().unwrap_or(id), tl!("custom"))
            } else {
                p["name"].as_str().unwrap_or(id).to_string()
            };
            if ui.selectable_label(id == st.pack, label).clicked() {
                st.pack = id.to_string();
            }
        }
    });
    elems.push(("planner.pack".into(), cb.response, "Kind of video".into()));
    if let Some(s) = current["summary"].as_str().filter(|s| !s.is_empty()) {
        ui.add(egui::Label::new(RichText::new(s).size(11.5).color(t.text_dim)).wrap());
    }
    if let (Some(a), Some(b)) = (current["typicalMinutes"]["min"].as_f64(), current["typicalMinutes"]["max"].as_f64()) {
        ui.label(RichText::new(tlf!("Usually {a}–{b} minutes", a = a, b = b)).size(11.0).color(t.text_faint));
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(tl!("Length in minutes")).size(12.0).color(t.text_dim));
        let r = ui.add(TextEdit::singleline(&mut st.minutes).id(egui::Id::new("planner-minutes")).desired_width(48.0));
        elems.push(("planner.minutes".into(), r, "Length of the video in minutes".into()));
    });
    if let (Some(min), Some(max), Some(def)) = (current["repeat"]["min"].as_u64(), current["repeat"]["max"].as_u64(), current["repeat"]["default"].as_u64()) {
        if st.clues < min as u32 || st.clues > max as u32 {
            st.clues = def as u32;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(tl!("Number of repeating beats")).size(12.0).color(t.text_dim));
            let r = ui.add(egui::DragValue::new(&mut st.clues).range(min as u32..=max as u32));
            elems.push(("planner.clues".into(), r, "How many copies of the repeating beat".into()));
        });
    }
    ui.add_space(8.0);
    let minutes = st.minutes.trim().parse::<f64>().ok().filter(|m| m.is_finite() && (1.0..=600.0).contains(m));
    if minutes.is_none() {
        ui.label(RichText::new(tl!("Write the length as a number of minutes, from 1 to 600.")).size(11.0).color(Color32::from_rgb(0xe8, 0xb3, 0x3c)));
    }
    let b = ui.add_enabled(!st.pack.is_empty() && minutes.is_some(), egui::Button::new(tl!("Create plan")));
    elems.push(("planner.create".into(), b.clone(), "Create the plan".into()));
    if b.clicked() {
        let mut p = json!({"pack": st.pack, "minutes": minutes.unwrap_or(10.0)});
        if current["repeat"].is_object() {
            p["clues"] = json!(st.clues);
        }
        commands.push(("planner.create", p));
    }
    for problem in packs["problems"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        ui.add(
            egui::Label::new(RichText::new(format!("{}: {problem}", tl!("Skipped pack file"))).size(11.0).color(Color32::from_rgb(0xe8, 0xb3, 0x3c))).wrap(),
        );
    }
}

fn beat_card(ui: &mut egui::Ui, t: &Tokens, b: &Value, elems: &mut Vec<(String, egui::Response, String)>, commands: &mut Vec<(&'static str, Value)>) {
    let id = b["id"].as_u64().unwrap_or(0);
    let key = |f: &str| format!("beat.{id}.{f}");
    egui::Frame::new().fill(t.field_bg).stroke(egui::Stroke::new(1.0, t.field_border)).corner_radius(4.0).inner_margin(6.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let w = (ui.available_width() - 200.0).max(60.0);
            let (r, new) = field(ui, &key("name"), b["name"].as_str().unwrap_or(""), false, w, 1, tl!("Beat name"), t);
            elems.push((format!("planner.{}", key("name")), r, "Beat name".into()));
            if let Some(v) = new.filter(|v| !v.trim().is_empty()) {
                commands.push(("planner.updateBeat", json!({"beat": id, "name": v})));
            }
            let (r, new) = field(ui, &key("length"), &clock(b["target"].as_i64().unwrap_or(0)), false, 52.0, 1, "0:00", t);
            elems.push((format!("planner.{}", key("length")), r, "Beat length".into()));
            if let Some(secs) = new.as_deref().and_then(parse_clock) {
                commands.push(("planner.updateBeat", json!({"beat": id, "seconds": secs})));
            }
            let mut v = b["intensity"].as_f64().unwrap_or(0.0) / 10.0;
            let r = ui.add(egui::DragValue::new(&mut v).range(0.0..=10.0).speed(0.1).fixed_decimals(1)).on_hover_text(tl!("Intensity, 0 to 10"));
            elems.push((format!("planner.{}", key("intensity")), r.clone(), "Beat intensity".into()));
            if (r.drag_stopped() || r.lost_focus()) && ((v * 10.0).round() - b["intensity"].as_f64().unwrap_or(0.0)).abs() > 0.5 {
                commands.push(("planner.updateBeat", json!({"beat": id, "intensity": v})));
            }
            let s = b["status"].as_str().unwrap_or("idea");
            let r = crate::panels::assistant_widgets::pill(ui, status_label(s), s != "idea", true, t)
                .on_hover_text(tl!("How far this beat has got. Click to move it on."));
            elems.push((format!("planner.{}", key("status")), r.clone(), "Beat status".into()));
            if r.clicked() {
                let next = STATUSES.iter().position(|x| *x == s).map_or(0, |i| (i + 1) % STATUSES.len());
                commands.push(("planner.updateBeat", json!({"beat": id, "status": STATUSES[next]})));
            }
        });
        let (r, new) = field(ui, &key("purpose"), b["purpose"].as_str().unwrap_or(""), true, ui.available_width(), 2, tl!("What changes here?"), t);
        elems.push((format!("planner.{}", key("purpose")), r, "What changes in this beat".into()));
        if let Some(v) = new {
            commands.push(("planner.updateBeat", json!({"beat": id, "purpose": v})));
        }
        let (r, new) = field(ui, &key("narration"), b["narration"].as_str().unwrap_or(""), true, ui.available_width(), 3, tl!("Narration for this beat…"), t);
        elems.push((format!("planner.{}", key("narration")), r, "Narration for this beat".into()));
        if let Some(v) = new {
            commands.push(("planner.updateBeat", json!({"beat": id, "narration": v})));
        }
    });
    ui.add_space(4.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_are_shown_and_read_back_as_minutes_and_seconds() {
        let s = filmcraft_time::TICKS_PER_SECOND;
        assert_eq!(
            (clock(0), clock(61 * s), clock(720 * s), clock(3725 * s), clock(-5)),
            ("0:00".into(), "1:01".into(), "12:00".into(), "1:02:05".into(), "0:00".into())
        );
        assert_eq!(parse_clock("12:00"), Some(720.0));
        assert_eq!(parse_clock("1:02:05"), Some(3725.0));
        assert_eq!(parse_clock("90"), Some(90.0));
        assert_eq!(parse_clock(" 1.5m "), Some(90.0));
        assert_eq!(parse_clock("0:30"), Some(30.0));
        for bad in ["", "abc", "1:xx", "-5", "inf", "nan", "99:99:99:99", "13:00:00", "12345678901234", ":"] {
            assert_eq!(parse_clock(bad), None, "{bad}");
        }
    }
}
