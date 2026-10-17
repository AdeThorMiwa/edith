//! Spotting Sheet panel (Window ▸ Spotting Sheet; fork roadmap F13.4): plan the sound of each beat.
//!
//! One card per beat of the plan: the ambience bed under it, its music (mood, where it comes in and
//! goes out), and the effects and deliberate silences in the order they happen. Times are the video's
//! (as on the exported sheet): the card shows where the beat is, and a time typed in a cue is a time in
//! the video, not in the beat. **Put cues on the timeline** turns the plan into markers; **Export as
//! text…** and **Copy as text** give the sheet as plain text.
//!
//! The panel is a view over `sound.sheet` and the `planner.*` sound commands, so a plan is shared with
//! the assistant, the A/V script and the beat board. Text fields keep what you type until they lose
//! focus, then make one edit (one undo step).
//!
//! Automation ids: `spot.place`, `spot.clear`, `spot.export`, `spot.copy`, `spot.summary`, `spot.error`,
//! `spot.empty`, `spot.beat.<id>` (its header: click to go there), and on a card `spot.beat.<id>.ambience`,
//! `.musicAdd`, `.musicMood`, `.musicIn`, `.musicOut`, `.musicEnd`, `.musicRemove`, `.addEffect`,
//! `.addSilence`; on a cue `spot.cue.<id>.kind`, `.at`, `.length`, `.text`, `.remove`.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Color32, RichText, Sense};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use super::planner::{clock, field, parse_clock};
use crate::FilmcraftApp;
use crate::native_dialogs::FileDialog;

const MAX_CARDS: usize = 400;
const MAX_CUES_SHOWN: usize = 60;
const LOOK: Color32 = Color32::from_rgb(0xe8, 0xb3, 0x3c);

#[derive(Default)]
struct State {
    error: Option<String>,
}

type Shared = Arc<Mutex<State>>;

fn text_of(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: egui::Rect) {
    let t = app.tokens;
    let shared: Shared =
        ui.data_mut(|d| d.get_temp_mut_or_insert_with(egui::Id::new("spotting-sheet-state"), || Arc::new(Mutex::new(State::default()))).clone());
    // the sheet needs a plan, and asking without one is an error: ask `planner.get` first, which never fails
    let has_plan = app.session.execute("planner.get", json!({})).is_ok_and(|v| v["exists"] == json!(true));
    let sheet = has_plan.then(|| app.session.execute("sound.sheet", json!({})).ok()).flatten();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(rect.min + egui::vec2(10.0, 8.0), rect.max - egui::vec2(10.0, 4.0))));
    let ui = &mut child;
    let mut elems: Vec<(String, egui::Response, String)> = Vec::new();
    let mut commands: Vec<(&'static str, Value)> = Vec::new();
    let mut seek: Option<i64> = None;
    let mut export = false;
    let mut copy: Option<String> = None;
    let mut problems: Vec<String> = Vec::new();
    let error = shared.lock().unwrap_or_else(PoisonError::into_inner).error.clone();

    egui::ScrollArea::vertical().id_salt("spotting-sheet-scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let Some(sheet) = &sheet else {
            let r = ui.label(
                RichText::new(tl!("No plan yet: open Window ▸ Planner and make one. Its beats are what the spotting sheet plans the sound for."))
                    .size(12.0)
                    .color(t.text_faint),
            );
            elems.push(("spot.empty".into(), r, "No plan yet".into()));
            return;
        };
        let beats: Vec<&Value> = sheet["beats"].as_array().map_or(Vec::new(), |a| a.iter().take(MAX_CARDS).collect());
        let planned = beats.iter().filter(|b| b["planned"] == true).count();
        let summary = tlf!("{planned} of {total} beats have a sound plan", planned = planned, total = beats.len());
        let r = ui.label(RichText::new(summary.clone()).size(12.0).strong().color(t.text));
        elems.push(("spot.summary".into(), r, summary));
        ui.horizontal_wrapped(|ui| {
            let p = ui
                .button(tl!("Put cues on the timeline"))
                .on_hover_text(tl!("Makes a marker for each bed, music and cue. Asking again moves them; none of your own markers are touched."));
            elems.push(("spot.place".into(), p.clone(), "Put the sound plan on the timeline as markers".into()));
            if p.clicked() {
                commands.push(("sound.placeCues", json!({})));
            }
            let c = ui.button(tl!("Take them off")).on_hover_text(tl!("Removes the markers this panel put on the timeline."));
            elems.push(("spot.clear".into(), c.clone(), "Remove the sound plan's markers".into()));
            if c.clicked() {
                commands.push(("sound.placeCues", json!({"clear": true})));
            }
            let e = ui.button(tl!("Export as text…"));
            elems.push(("spot.export".into(), e.clone(), "Save the spotting sheet as a text file".into()));
            export |= e.clicked();
            let k = ui.button(tl!("Copy as text"));
            elems.push(("spot.copy".into(), k.clone(), "Copy the spotting sheet as text".into()));
            if k.clicked() {
                copy = Some(text_of(&sheet["text"]));
            }
        });
        if let Some(e) = &error {
            let r = ui.add(egui::Label::new(RichText::new(e).size(11.5).color(LOOK)).wrap());
            elems.push(("spot.error".into(), r, e.clone()));
        }
        ui.add_space(6.0);
        for b in beats {
            beat_card(ui, &t, b, &mut elems, &mut commands, &mut seek, &mut problems);
            ui.add_space(8.0);
        }
    });
    for (id, r, label) in &elems {
        app.auto.add_response(id, r, label);
    }
    if let Some(at) = seek {
        app.session.set_playhead(Tick(at.max(0)));
    }
    if let Some(text) = copy {
        ui.ctx().copy_text(text);
        app.ui.status = tl!("Spotting sheet copied.").to_string();
    }
    if let Some(first) = problems.into_iter().next() {
        shared.lock().unwrap_or_else(PoisonError::into_inner).error = Some(first);
    }
    for (cmd, p) in commands {
        let r = app.session.execute(cmd, p);
        let mut st = shared.lock().unwrap_or_else(PoisonError::into_inner);
        match r {
            Ok(v) => {
                st.error = None;
                if cmd == "sound.placeCues" {
                    app.ui.status = if v["placed"].as_u64().unwrap_or(0) > 0 {
                        tlf!("Put {n} sound cues on the timeline.", n = v["placed"])
                    } else {
                        tl!("Took the sound cues off the timeline.").to_string()
                    };
                }
            }
            Err(e) => st.error = Some(e.to_string()),
        }
    }
    if export {
        app.pick_ui(FileDialog::SaveAs { filter: tl!("Text").into(), exts: vec!["txt".into()], name: "spotting-sheet.txt".into() }, move |app, paths| {
            if let Some(p) = paths.into_iter().next() {
                match app.session.execute("sound.sheet", json!({"path": p})) {
                    Ok(_) => app.ui.status = tl!("Spotting sheet saved.").to_string(),
                    Err(e) => app.ui.status = e.to_string(),
                }
            }
        });
    }
}

fn beat_card(
    ui: &mut egui::Ui,
    t: &crate::theme::Tokens,
    b: &Value,
    elems: &mut Vec<(String, egui::Response, String)>,
    commands: &mut Vec<(&'static str, Value)>,
    seek: &mut Option<i64>,
    problems: &mut Vec<String>,
) {
    let id = b["id"].as_u64().unwrap_or(0);
    let start = b["start"].as_i64().unwrap_or(0);
    let length = b["length"].as_i64().unwrap_or(0);
    let name = text_of(&b["name"]);
    let start_s = start as f64 / TICKS_PER_SECOND as f64;
    egui::Frame::new().fill(t.field_bg).corner_radius(6.0).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        // header: where the beat is; a click goes there
        let head = ui.add(
            egui::Label::new(RichText::new(format!("{name}   {}–{}", clock(start), clock(start.saturating_add(length)))).size(12.5).strong().color(t.text))
                .sense(Sense::click()),
        );
        let head = head.on_hover_text(format!("{}: {}", text_of(&b["section"]), tl!("click to go there")));
        elems.push((format!("spot.beat.{id}"), head.clone(), name.clone()));
        if head.clicked() {
            *seek = Some(start);
        }
        // the bed
        ui.horizontal(|ui| {
            ui.label(RichText::new(tl!("Ambience")).size(11.5).color(t.text_dim));
            let (r, new) = field(
                ui,
                &format!("spot-amb-{id}"),
                &text_of(&b["ambience"]),
                false,
                ui.available_width(),
                1,
                tl!("the room tone or bed under this beat, e.g. wind over a shore, low"),
                t,
            );
            elems.push((format!("spot.beat.{id}.ambience"), r, "Ambience under this beat".into()));
            if let Some(v) = new {
                commands.push(("planner.setSound", json!({"beat": id, "ambience": v})));
            }
        });
        // the music
        match b["music"].as_object() {
            None => {
                let a = ui.small_button(tl!("Add music"));
                elems.push((format!("spot.beat.{id}.musicAdd"), a.clone(), "Add a music cue".into()));
                if a.clicked() {
                    commands.push(("planner.setSound", json!({"beat": id, "music": "mood to be chosen"})));
                }
            }
            Some(m) => {
                let enters = m.get("enters").and_then(Value::as_i64).unwrap_or(start);
                let leaves = m.get("leaves").and_then(Value::as_i64).unwrap_or(start.saturating_add(length));
                let to_end = m.get("toEnd").and_then(Value::as_bool).unwrap_or(true);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tl!("Music")).size(11.5).color(t.text_dim));
                    let (r, new) = field(ui, &format!("spot-mood-{id}"), &text_of(&m["mood"]), false, 190.0, 1, tl!("mood"), t);
                    elems.push((format!("spot.beat.{id}.musicMood"), r, "Music mood".into()));
                    if let Some(v) = new
                        && !v.trim().is_empty()
                    {
                        commands.push(("planner.setSound", json!({"beat": id, "music": v})));
                    }
                    ui.label(RichText::new(tl!("in")).size(11.0).color(t.text_faint));
                    let (r, new) = field(ui, &format!("spot-in-{id}"), &clock(enters), false, 56.0, 1, "0:00", t);
                    elems.push((format!("spot.beat.{id}.musicIn"), r, "Where the music comes in".into()));
                    if let Some(v) = new {
                        match parse_clock(&v) {
                            Some(abs) if abs >= start_s => commands.push(("planner.setSound", json!({"beat": id, "musicInSeconds": abs - start_s}))),
                            _ => problems.push(tl!("A time looks like 1:25, and cannot be before the beat starts.").to_string()),
                        }
                    }
                    ui.label(RichText::new(tl!("out")).size(11.0).color(t.text_faint));
                    if to_end {
                        let e = ui.small_button(tl!("end of beat")).on_hover_text(tl!("Click to give it a time."));
                        elems.push((format!("spot.beat.{id}.musicEnd"), e.clone(), "The music plays to the end of the beat".into()));
                        if e.clicked() {
                            let beat_s = length as f64 / TICKS_PER_SECOND as f64;
                            let in_s = (enters - start) as f64 / TICKS_PER_SECOND as f64;
                            commands.push(("planner.setSound", json!({"beat": id, "musicOutSeconds": (beat_s * 0.9).max(in_s + 0.5)})));
                        }
                    } else {
                        let (r, new) = field(ui, &format!("spot-out-{id}"), &clock(leaves), false, 56.0, 1, "0:00", t);
                        elems.push((format!("spot.beat.{id}.musicOut"), r, "Where the music goes out".into()));
                        if let Some(v) = new {
                            match parse_clock(&v) {
                                Some(abs) if abs >= start_s => commands.push(("planner.setSound", json!({"beat": id, "musicOutSeconds": abs - start_s}))),
                                _ => problems.push(tl!("A time looks like 1:25, and cannot be before the beat starts.").to_string()),
                            }
                        }
                        let e = ui.small_button(tl!("to the end")).on_hover_text(tl!("Let it play to the end of the beat."));
                        elems.push((format!("spot.beat.{id}.musicEnd"), e.clone(), "Let the music play to the end of the beat".into()));
                        if e.clicked() {
                            commands.push(("planner.setSound", json!({"beat": id, "musicToEnd": true})));
                        }
                    }
                    let x = ui.small_button(tl!("Remove music"));
                    elems.push((format!("spot.beat.{id}.musicRemove"), x.clone(), "Remove the music cue".into()));
                    if x.clicked() {
                        commands.push(("planner.setSound", json!({"beat": id, "music": Value::Null})));
                    }
                });
            }
        }
        // the cues, in the order they happen
        for c in b["cues"].as_array().map_or(&[][..], Vec::as_slice).iter().take(MAX_CUES_SHOWN) {
            let cid = c["id"].as_u64().unwrap_or(0);
            let silence = c["kind"] == "silence";
            let at = c["start"].as_i64().unwrap_or(start);
            ui.horizontal(|ui| {
                let k = ui
                    .small_button(if silence { tl!("Silence") } else { tl!("Effect") })
                    .on_hover_text(tl!("Click to switch between an effect and a silence."));
                elems.push((format!("spot.cue.{cid}.kind"), k.clone(), if silence { "Silence" } else { "Effect" }.into()));
                if k.clicked() {
                    commands.push((
                        "planner.updateCue",
                        if silence { json!({"cue": cid, "kind": "effect"}) } else { json!({"cue": cid, "kind": "silence", "lengthSeconds": 2}) },
                    ));
                }
                let (r, new) = field(ui, &format!("spot-at-{cid}"), &clock(at), false, 56.0, 1, "0:00", t);
                elems.push((format!("spot.cue.{cid}.at"), r, "Time in the video".into()));
                if let Some(v) = new {
                    match parse_clock(&v) {
                        Some(abs) if abs >= start_s => commands.push(("planner.updateCue", json!({"cue": cid, "atSeconds": abs - start_s}))),
                        _ => problems.push(tl!("A time looks like 1:25, and cannot be before the beat starts.").to_string()),
                    }
                }
                if silence {
                    let secs = c["length"].as_i64().unwrap_or(0) as f64 / TICKS_PER_SECOND as f64;
                    let (r, new) = field(ui, &format!("spot-len-{cid}"), &format!("{secs:.1}"), false, 44.0, 1, "s", t);
                    elems.push((format!("spot.cue.{cid}.length"), r, "How long the silence lasts, in seconds".into()));
                    if let Some(v) = new {
                        match v.trim().trim_end_matches('s').trim().parse::<f64>() {
                            Ok(x) if x.is_finite() && x > 0.0 => commands.push(("planner.updateCue", json!({"cue": cid, "lengthSeconds": x}))),
                            _ => problems.push(tl!("A silence needs a length in seconds, more than 0.").to_string()),
                        }
                    }
                    ui.label(RichText::new("s").size(11.0).color(t.text_faint));
                }
                let (r, new) = field(
                    ui,
                    &format!("spot-text-{cid}"),
                    &text_of(&c["text"]),
                    false,
                    (ui.available_width() - 30.0).max(60.0),
                    1,
                    if silence { tl!("why it is quiet") } else { tl!("what the sound is") },
                    t,
                );
                elems.push((format!("spot.cue.{cid}.text"), r, "What the cue is".into()));
                if let Some(v) = new {
                    commands.push(("planner.updateCue", json!({"cue": cid, "text": v})));
                }
                let x = ui.small_button("✕").on_hover_text(tl!("Remove this cue"));
                elems.push((format!("spot.cue.{cid}.remove"), x.clone(), "Remove this cue".into()));
                if x.clicked() {
                    commands.push(("planner.removeCue", json!({"cue": cid})));
                }
            });
        }
        ui.horizontal(|ui| {
            let e = ui.small_button(tl!("+ Effect"));
            elems.push((format!("spot.beat.{id}.addEffect"), e.clone(), "Add an effect".into()));
            if e.clicked() {
                commands.push(("planner.addCue", json!({"beat": id, "kind": "effect", "atSeconds": 1})));
            }
            let s = ui.small_button(tl!("+ Silence"));
            elems.push((format!("spot.beat.{id}.addSilence"), s.clone(), "Add a deliberate silence".into()));
            if s.clicked() {
                commands.push(("planner.addCue", json!({"beat": id, "kind": "silence", "atSeconds": 1, "lengthSeconds": 2})));
            }
        });
        let note = text_of(&b["soundNote"]);
        if !note.trim().is_empty() {
            ui.add(egui::Label::new(RichText::new(format!("{}: {}", tl!("Note"), note.trim())).size(10.5).color(t.text_faint)).wrap());
        }
    });
}
