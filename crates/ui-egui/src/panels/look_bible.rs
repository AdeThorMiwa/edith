//! Look Bible panel (Window ▸ Look Bible; fork roadmap F12.7c): a small board of pictures that say how
//! a film should feel.
//!
//! A *look* is a named board. Put pictures on it from an image or film file (a note of why you may use
//! the picture is required) or from the frame at the playhead, write down what you want (the picture
//! shape, the grain, the LUT), and read what the pictures measure on average: brightness, contrast,
//! saturation, warmth and the colours that recur. **Compare** sets the frame at the playhead beside the
//! look in plain words, and **Match to Look** grades the selected clip towards the selected picture.
//! Looks are global: every project sees the same ones, and the pictures are kept small.
//!
//! The panel is a view over the `look.*` commands and holds only what you are typing.
//!
//! Automation ids: `looks.empty`, `looks.new.name`, `looks.new.add`, `looks.pick.<id>`, `looks.name`,
//! `looks.notes`, `looks.aspect`, `looks.grain`, `looks.lut`, `looks.remove`, `looks.pic.<id>`,
//! `looks.pic.<id>.caption`, `looks.pic.<id>.remove`, `looks.path`, `looks.browse`, `looks.seconds`,
//! `looks.rights`, `looks.caption`, `looks.addFile`, `looks.addFrame`, `looks.compare`,
//! `looks.row.<metric>`, `looks.match`, `looks.swatch.<i>`, `looks.numbers`, `looks.error`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use egui::{Color32, RichText, Sense, TextEdit, pos2, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::native_dialogs::FileDialog;

const LOOK: Color32 = Color32::from_rgb(0xe8, 0xb3, 0x3c);
const FINE: Color32 = Color32::from_rgb(0x6f, 0xb8, 0x8a);
const PICTURE_H: f32 = 84.0;
const MAX_ROWS: usize = 12;
const MAX_SWATCHES: usize = 6;

const FILE_EXTENSIONS: [&str; 24] = [
    "png", "jpg", "jpeg", "webp", "tif", "tiff", "bmp", "gif", "tga", "dpx", "mp4", "m4v", "mov", "mkv", "webm", "avi", "mxf", "mpg", "mpeg", "ts", "mts",
    "m2ts", "wav", "flac",
];

#[derive(Default)]
struct State {
    selected: Option<String>,
    selected_still: Option<String>,
    new_name: String,
    path: String,
    seconds: String,
    rights: String,
    caption: String,
    error: Option<String>,
    comparing: bool,
    confirm_remove: Option<String>,
    confirm_remove_pic: Option<String>,
    /// What each text field of the selected look holds while you type (`name`, `notes`, `cap.<id>`…).
    edits: HashMap<String, String>,
    /// The look the `edits` were loaded from, and whether a field had the cursor last frame.
    loaded_for: Option<String>,
    editing: bool,
    /// The comparison last asked for: what it was about, and the answer.
    compare_cache: Option<(String, Result<Value, String>)>,
}

type Shared = Arc<Mutex<State>>;

fn lock(s: &Shared) -> std::sync::MutexGuard<'_, State> {
    s.lock().unwrap_or_else(PoisonError::into_inner)
}

fn text_of(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn percent(v: &Value) -> String {
    format!("{:.0}%", v.as_f64().unwrap_or(0.0) * 100.0)
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: egui::Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let shared: Shared = ui.data_mut(|d| d.get_temp_mut_or_insert_with(egui::Id::new("look-bible-state"), || Arc::new(Mutex::new(State::default()))).clone());
    let mut st = lock(&shared);
    let list = app.session.execute("look.list", json!({})).unwrap_or(Value::Null);
    let looks: Vec<Value> = list["looks"].as_array().cloned().unwrap_or_default();
    // the selected look: the one chosen, else the first; none if there are none (or it was removed)
    if st.selected.as_ref().is_none_or(|id| !looks.iter().any(|l| l["id"] == json!(id))) {
        st.selected = looks.first().and_then(|l| l["id"].as_str().map(str::to_string));
        st.selected_still = None;
    }
    let sel = st.selected.clone();
    let detail: Option<Value> = sel.as_ref().and_then(|id| app.session.execute("look.get", json!({"id": id})).ok());
    let pictures: Vec<Value> = detail.as_ref().and_then(|d| d["pictures"].as_array().cloned()).unwrap_or_default();
    // a picture to match to: the one picked, or the only one
    if st.selected_still.as_ref().is_none_or(|id| !pictures.iter().any(|p| p["id"] == json!(id))) {
        st.selected_still = (pictures.len() == 1).then(|| text_of(&pictures[0]["id"]));
    }
    // the text fields show what the look holds, except while you are typing in one
    if !st.editing || st.loaded_for != sel {
        st.edits.clear();
        if let Some(d) = &detail {
            for k in ["name", "notes", "aspect", "grain", "lut"] {
                st.edits.insert(k.to_string(), text_of(&d[k]));
            }
            for p in &pictures {
                st.edits.insert(format!("cap.{}", text_of(&p["id"])), text_of(&p["caption"]));
            }
        }
        st.loaded_for = sel.clone();
    }
    let revision = app.session.revision;
    let playhead = app.session.playhead().0;

    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(rect.min + vec2(10.0, 8.0), rect.max - vec2(10.0, 4.0))));
    let ui = &mut child;
    let mut elems: Vec<(String, egui::Response, String)> = Vec::new();
    let mut commands: Vec<(&'static str, Value)> = Vec::new();
    let mut browse = false;
    let mut focus = false;

    egui::ScrollArea::vertical().id_salt("look-bible-scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(
            egui::Label::new(
                RichText::new(tl!(
                    "Pictures that say how your film should feel. Keep only pictures you have the right to use: each one carries a note of why."
                ))
                .size(12.0)
                .color(t.text_dim),
            )
            .wrap(),
        );
        ui.add_space(6.0);
        // ---- the looks, and a new one
        ui.horizontal_wrapped(|ui| {
            for l in &looks {
                let id = text_of(&l["id"]);
                let name = text_of(&l["name"]);
                let r = ui.selectable_label(sel.as_deref() == Some(id.as_str()), format!("{name} ({})", l["stills"].as_u64().unwrap_or(0)));
                elems.push((format!("looks.pick.{id}"), r.clone(), name));
                if r.clicked() {
                    st.selected = Some(id);
                    st.selected_still = None;
                    st.confirm_remove = None;
                    st.error = None;
                }
            }
        });
        ui.horizontal(|ui| {
            let r = ui.add(
                TextEdit::singleline(&mut st.new_name)
                    .id(egui::Id::new("looks-new-name"))
                    .desired_width((ui.available_width() - 90.0).max(60.0))
                    .hint_text(RichText::new(tl!("Name of a new look")).color(t.text_faint)),
            );
            elems.push(("looks.new.name".into(), r, "Name of a new look".into()));
            let b = ui.add_enabled(!st.new_name.trim().is_empty(), egui::Button::new(tl!("New look")));
            elems.push(("looks.new.add".into(), b.clone(), "Make a new look".into()));
            if b.clicked() {
                commands.push(("look.create", json!({"name": st.new_name.trim()})));
            }
        });
        if looks.is_empty() {
            let r = ui.label(RichText::new(tl!("No looks yet. Name one above to start a board.")).size(12.0).color(t.text_faint));
            elems.push(("looks.empty".into(), r, "No looks yet".into()));
        }
        let (Some(look_id), Some(detail)) = (sel.clone(), detail.as_ref()) else {
            if let Some(e) = st.error.clone() {
                ui.label(RichText::new(e).size(11.5).color(LOOK));
            }
            return;
        };
        ui.add_space(8.0);
        // ---- what the look is
        let mut text_field =
            |ui: &mut egui::Ui, st: &mut State, key: &str, auto: &str, label: &str, hint: &str, multiline: bool, commands: &mut Vec<(&'static str, Value)>| {
                let current = text_of(&detail[key]);
                let buf = st.edits.entry(key.to_string()).or_insert_with(|| current.clone());
                let edit = if multiline { TextEdit::multiline(buf).desired_rows(2) } else { TextEdit::singleline(buf) };
                let r =
                    ui.add(edit.id(egui::Id::new(("looks-field", key))).desired_width(ui.available_width()).hint_text(RichText::new(hint).color(t.text_faint)));
                focus |= r.has_focus();
                let changed = r.lost_focus() && *buf != current && (key != "name" || !buf.trim().is_empty());
                if changed {
                    commands.push(("look.set", json!({"id": look_id, key: buf.as_str()})));
                }
                elems.push((auto.into(), r, label.into()));
            };
        text_field(ui, &mut st, "name", "looks.name", "Name of the look", tl!("Name"), false, &mut commands);
        text_field(ui, &mut st, "notes", "looks.notes", "Notes on the look", tl!("What this look is about: light, colour, mood…"), true, &mut commands);
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 16.0) / 3.0;
            for (key, auto, label, hint) in [
                ("aspect", "looks.aspect", "Picture shape wanted", tl!("Picture shape, e.g. 2.39:1")),
                ("grain", "looks.grain", "Grain wanted", tl!("Grain, e.g. heavy 35 mm")),
                ("lut", "looks.lut", "LUT", tl!("LUT, e.g. Kodak 2383")),
            ] {
                ui.allocate_ui(vec2(w, 22.0), |ui| text_field(ui, &mut st, key, auto, label, hint, false, &mut commands));
            }
        });
        ui.add_space(8.0);
        // ---- the pictures
        ui.label(RichText::new(tl!("Pictures")).size(12.5).strong().color(t.text));
        if pictures.is_empty() {
            ui.label(RichText::new(tl!("No pictures yet. Add one below.")).size(11.5).color(t.text_faint));
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for p in &pictures {
                let sid = text_of(&p["id"]);
                let picked = st.selected_still.as_deref() == Some(sid.as_str());
                ui.vertical(|ui| {
                    let tex = super::look_pictures::still(app, &ctx, &look_id, &sid);
                    let (img_rect, r) = match tex {
                        Some((tid, size)) => {
                            let w = (PICTURE_H * size.x / size.y.max(1.0)).clamp(24.0, 170.0);
                            let r = ui.add(egui::Image::new(egui::load::SizedTexture::new(tid, vec2(w, PICTURE_H))).corner_radius(3.0).sense(Sense::click()));
                            (r.rect, r)
                        }
                        None => {
                            let (rc, r) = ui.allocate_exact_size(vec2(PICTURE_H * 16.0 / 9.0, PICTURE_H), Sense::click());
                            ui.painter().rect_filled(rc, 3.0, t.field_border.gamma_multiply(0.5));
                            ui.painter().text(
                                rc.center(),
                                egui::Align2::CENTER_CENTER,
                                tl!("picture not available"),
                                crate::theme::Tokens::ui(10.5),
                                t.text_faint,
                            );
                            (rc, r)
                        }
                    };
                    if picked {
                        ui.painter().rect_stroke(img_rect.expand(2.0), 4.0, egui::Stroke::new(2.0, t.accent), egui::StrokeKind::Outside);
                    }
                    let source = text_of(&p["source"]);
                    let at = p["timeSeconds"]
                        .as_f64()
                        .map(|s| format!(" · {}", super::planner::clock((s * filmcraft_time::TICKS_PER_SECOND as f64) as i64)))
                        .unwrap_or_default();
                    let r = r.on_hover_text(format!("{source}{at}\n{}", text_of(&p["rights"])));
                    elems.push((format!("looks.pic.{sid}"), r.clone(), format!("Picture from {source}")));
                    if r.clicked() {
                        st.selected_still = if picked { None } else { Some(sid.clone()) };
                    }
                    let key = format!("cap.{sid}");
                    let current = text_of(&p["caption"]);
                    let buf = st.edits.entry(key.clone()).or_insert_with(|| current.clone());
                    let c = ui.add(
                        TextEdit::singleline(buf)
                            .id(egui::Id::new(("looks-caption", &sid)))
                            .desired_width(img_rect.width().max(90.0))
                            .hint_text(RichText::new(tl!("Caption")).color(t.text_faint)),
                    );
                    focus |= c.has_focus();
                    if c.lost_focus() && *buf != current {
                        commands.push(("look.setStill", json!({"look": look_id, "still": sid, "caption": buf.as_str()})));
                    }
                    elems.push((format!("looks.pic.{sid}.caption"), c, "Caption".into()));
                    let confirming = st.confirm_remove_pic.as_deref() == Some(sid.as_str());
                    let x = ui.small_button(if confirming { tl!("Really remove?") } else { tl!("Remove") });
                    elems.push((format!("looks.pic.{sid}.remove"), x.clone(), "Remove this picture".into()));
                    if x.clicked() {
                        if confirming {
                            commands.push(("look.removeStill", json!({"look": look_id, "still": sid})));
                            st.confirm_remove_pic = None;
                        } else {
                            st.confirm_remove_pic = Some(sid.clone());
                        }
                    }
                });
            }
        });
        ui.add_space(6.0);
        // ---- add a picture
        ui.label(RichText::new(tl!("Add a picture")).size(12.5).strong().color(t.text));
        ui.horizontal(|ui| {
            let r = ui.add(
                TextEdit::singleline(&mut st.path)
                    .id(egui::Id::new("looks-path"))
                    .desired_width((ui.available_width() - 150.0).max(60.0))
                    .hint_text(RichText::new(tl!("Path to an image or film file")).color(t.text_faint)),
            );
            focus |= r.has_focus();
            elems.push(("looks.path".into(), r, "Path to an image or film file".into()));
            let s = ui
                .add(
                    TextEdit::singleline(&mut st.seconds)
                        .id(egui::Id::new("looks-seconds"))
                        .desired_width(60.0)
                        .hint_text(RichText::new(tl!("at (s)")).color(t.text_faint)),
                )
                .on_hover_text(tl!("For a film: the moment to take, in seconds. Left empty it is the middle."));
            focus |= s.has_focus();
            elems.push(("looks.seconds".into(), s, "Moment in the film, in seconds".into()));
            let b = ui.button(tl!("Browse…"));
            elems.push(("looks.browse".into(), b.clone(), "Choose a file".into()));
            browse |= b.clicked();
        });
        let r = ui.add(
            TextEdit::singleline(&mut st.rights)
                .id(egui::Id::new("looks-rights"))
                .desired_width(ui.available_width())
                .hint_text(RichText::new(tl!("Why you may use it: public domain, your own, a licence…")).color(t.text_faint)),
        );
        focus |= r.has_focus();
        elems.push(("looks.rights".into(), r, "Why you may use this picture".into()));
        let r = ui.add(
            TextEdit::singleline(&mut st.caption)
                .id(egui::Id::new("looks-add-caption"))
                .desired_width(ui.available_width())
                .hint_text(RichText::new(tl!("Caption (optional)")).color(t.text_faint)),
        );
        focus |= r.has_focus();
        elems.push(("looks.caption".into(), r, "Caption for the new picture".into()));
        ui.horizontal(|ui| {
            let can = !st.path.trim().is_empty() && !st.rights.trim().is_empty();
            let b = ui.add_enabled(can, egui::Button::new(tl!("Add from file")));
            elems.push(("looks.addFile".into(), b.clone(), "Add the picture from the file".into()));
            if b.clicked() {
                let mut p = json!({"look": look_id, "path": st.path.trim(), "rights": st.rights.trim()});
                if !st.seconds.trim().is_empty() {
                    match st.seconds.trim().parse::<f64>() {
                        Ok(v) if v.is_finite() && v >= 0.0 => p["seconds"] = json!(v),
                        _ => st.error = Some(tl!("The moment must be a number of seconds, 0 or more.").to_string()),
                    }
                }
                if !st.caption.trim().is_empty() {
                    p["caption"] = json!(st.caption.trim());
                }
                if st.error.is_none() {
                    commands.push(("look.addStill", p));
                }
            }
            let f = ui.button(tl!("Add this frame")).on_hover_text(tl!("Keeps the Program frame at the playhead."));
            elems.push(("looks.addFrame".into(), f.clone(), "Add the frame at the playhead".into()));
            if f.clicked() {
                let mut p = json!({"look": look_id});
                if !st.caption.trim().is_empty() {
                    p["caption"] = json!(st.caption.trim());
                }
                commands.push(("look.addStill", p));
            }
        });
        if let Some(e) = st.error.clone() {
            let r = ui.add(egui::Label::new(RichText::new(&e).size(11.5).color(LOOK)).wrap());
            elems.push(("looks.error".into(), r, e));
        }
        ui.add_space(8.0);
        // ---- what the pictures measure
        if let Some(m) = detail["measured"].as_object().filter(|_| !pictures.is_empty()) {
            ui.label(RichText::new(tl!("What the pictures measure")).size(12.5).strong().color(t.text));
            let colours: Vec<&Value> = m.get("palette").and_then(Value::as_array).map_or(Vec::new(), |a| a.iter().take(MAX_SWATCHES).collect());
            if !colours.is_empty() {
                let w = ui.available_width().max(60.0);
                let (rc, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
                let total: f64 = colours.iter().map(|c| c["share"].as_f64().unwrap_or(0.0).max(0.0)).sum::<f64>().max(f64::MIN_POSITIVE);
                let mut x = 0.0f32;
                for (i, c) in colours.iter().enumerate() {
                    let share = c["share"].as_f64().unwrap_or(0.0).max(0.0) / total;
                    let width = (share as f32 * w).max(6.0);
                    let rgb: Vec<u8> = c["rgb"]
                        .as_array()
                        .map_or(Vec::new(), |a| a.iter().map(|v| (v.as_f64().unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() as u8).collect());
                    let col = Color32::from_rgb(rgb.first().copied().unwrap_or(0), rgb.get(1).copied().unwrap_or(0), rgb.get(2).copied().unwrap_or(0));
                    let sw = egui::Rect::from_min_size(pos2(rc.min.x + x, rc.min.y), vec2((width - 2.0).max(1.0), 30.0));
                    ui.painter().rect_filled(sw, 3.0, col);
                    let hit = ui.interact(sw, egui::Id::new(("looks-swatch", i)), Sense::hover()).on_hover_text(format!(
                        "#{:02x}{:02x}{:02x} · {:.0}%",
                        col.r(),
                        col.g(),
                        col.b(),
                        share * 100.0
                    ));
                    elems.push((format!("looks.swatch.{i}"), hit, format!("#{:02x}{:02x}{:02x}", col.r(), col.g(), col.b())));
                    x += width;
                }
            }
            let warmth = m.get("colourTemperature").and_then(Value::as_f64).map_or(String::new(), |k| {
                if k < 4500.0 {
                    tlf!("warm ({k} K)", k = format!("{k:.0}"))
                } else if k > 6500.0 {
                    tlf!("cool ({k} K)", k = format!("{k:.0}"))
                } else {
                    tlf!("neutral ({k} K)", k = format!("{k:.0}"))
                }
            });
            let line = tlf!(
                "brightness {b} · contrast {c} · saturation {s} · {w}",
                b = percent(&m["lumaMean"]),
                c = percent(&m["contrast"]),
                s = percent(&m["saturation"]),
                w = warmth
            );
            let r = ui.label(RichText::new(line.clone()).size(11.5).color(t.text_dim));
            elems.push(("looks.numbers".into(), r, line));
            ui.add_space(8.0);
        }
        // ---- compare, and match
        ui.horizontal(|ui| {
            let c = ui.add_enabled(!pictures.is_empty(), egui::Button::new(tl!("Compare with the frame at the playhead")).selected(st.comparing));
            elems.push(("looks.compare".into(), c.clone(), "Compare the playhead frame with this look".into()));
            if c.clicked() {
                st.comparing = !st.comparing;
            }
            let can_match = st.selected_still.is_some();
            let m = ui.add_enabled(can_match, egui::Button::new(tl!("Match to Look"))).on_hover_text(if can_match {
                tl!("Grades the selected clip towards the picked picture. One undo step.")
            } else {
                tl!("Pick a picture above first (click it), and select a clip on the timeline.")
            });
            elems.push(("looks.match".into(), m.clone(), "Grade the selected clip towards the picked picture".into()));
            if m.clicked()
                && let Some(still) = st.selected_still.clone()
            {
                commands.push(("look.match", json!({"look": look_id, "still": still})));
            }
        });
        if st.comparing && !pictures.is_empty() {
            let key = format!("{look_id}|{revision}|{playhead}|{}|{}", pictures.len(), pictures.last().map_or(String::new(), |p| text_of(&p["id"])));
            let cached = st.compare_cache.as_ref().filter(|(k, _)| *k == key).map(|(_, v)| v.clone());
            let answer = cached.unwrap_or_else(|| {
                let v = app.session.execute("look.compare", json!({"look": look_id})).map_err(|e| e.to_string());
                st.compare_cache = Some((key, v.clone()));
                v
            });
            match answer {
                Ok(v) => {
                    ui.label(
                        RichText::new(tlf!(
                            "{close} close, {differ} differ",
                            close = v["close"].as_u64().unwrap_or(0),
                            differ = v["differ"].as_u64().unwrap_or(0)
                        ))
                        .size(11.5)
                        .strong()
                        .color(t.text),
                    );
                    for row in v["rows"].as_array().into_iter().flatten().take(MAX_ROWS) {
                        let fine = row["verdict"] == "close";
                        let words = text_of(&row["words"]);
                        let l = ui.add(
                            egui::Label::new(RichText::new(format!("{} {words}", if fine { "✓" } else { "•" })).size(11.5).color(if fine {
                                FINE
                            } else {
                                t.text
                            }))
                            .wrap(),
                        );
                        elems.push((format!("looks.row.{}", text_of(&row["metric"])), l, words));
                    }
                }
                Err(e) => {
                    ui.label(RichText::new(e).size(11.5).color(t.text_faint));
                }
            }
        }
        ui.add_space(10.0);
        let confirming = st.confirm_remove.as_deref() == Some(look_id.as_str());
        let x = ui.button(if confirming { tl!("Really remove this look and its pictures?") } else { tl!("Remove this look") });
        elems.push(("looks.remove".into(), x.clone(), "Remove this look".into()));
        if x.clicked() {
            if confirming {
                commands.push(("look.remove", json!({"id": look_id})));
                st.confirm_remove = None;
            } else {
                st.confirm_remove = Some(look_id.clone());
            }
        }
    });
    st.editing = focus;
    drop(st);
    for (id, r, label) in &elems {
        app.auto.add_response(id, r, label);
    }
    for (cmd, p) in commands {
        match app.session.execute(cmd, p) {
            Ok(r) => {
                let mut s = lock(&shared);
                s.error = None;
                match cmd {
                    "look.create" => {
                        s.selected = r["look"].as_str().map(str::to_string);
                        s.selected_still = None;
                        s.new_name.clear();
                    }
                    "look.addStill" => {
                        s.path.clear();
                        s.seconds.clear();
                        s.caption.clear();
                        s.selected_still = r["still"].as_str().map(str::to_string);
                        s.compare_cache = None;
                    }
                    "look.match" => app.ui.status = tl!("Matched the clip to the look.").to_string(),
                    _ => {}
                }
            }
            Err(e) => lock(&shared).error = Some(e.to_string()),
        }
    }
    if browse {
        let s = shared.clone();
        app.pick_ui(
            FileDialog::OpenFile { filter: tl!("Picture or film").into(), exts: FILE_EXTENSIONS.iter().map(|e| (*e).to_string()).collect() },
            move |_, paths| {
                if let Some(p) = paths.into_iter().next() {
                    lock(&s).path = p;
                }
            },
        );
    }
}
