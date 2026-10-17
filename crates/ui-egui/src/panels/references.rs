//! References panel (Window ▸ References; fork roadmap F12.6): films you admire, kept as numbers.
//!
//! Point it at a film file you have the right to use, say why you may (public domain, your own
//! export, a licence), and it measures the film in the background and keeps its style fingerprint:
//! how long its shots are, how its pace moves, how the picture and the sound feel. Nothing of the film
//! is kept. Then, for each reference, you can read what its numbers suggest, compare your edit with it
//! in plain words, make it the coach's target for this project, share it as a small file, or remove it.
//!
//! The panel is a view over the `reference.*` commands: it holds only what you typed. The library is
//! global, so every project sees the same references.
//!
//! Automation ids: `references.path`, `references.browse`, `references.name`, `references.rights`,
//! `references.add`, `references.progress`, `references.cancel`, `references.error`,
//! `references.import`, `references.card.<id>`, `references.adopted.<id>`, `references.adopt.<id>`,
//! `references.compare.<id>`, `references.details.<id>`, `references.export.<id>`,
//! `references.rename.<id>`, `references.rename.field`, `references.remove.<id>`,
//! `references.reading.<id>.<n>`, `references.row.<metric>`, `references.empty`.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Color32, RichText, TextEdit};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::native_dialogs::FileDialog;

const LOOK: Color32 = Color32::from_rgb(0xe8, 0xb3, 0x3c);
const FINE: Color32 = Color32::from_rgb(0x6f, 0xb8, 0x8a);
/// Rows of a comparison and readings of a reference shown.
const MAX_ROWS: usize = 24;

#[derive(Default)]
struct State {
    path: String,
    name: String,
    rights: String,
    /// The measuring job that is running.
    job: Option<u64>,
    error: Option<String>,
    details: Option<String>,
    comparing: Option<String>,
    confirm_remove: Option<String>,
    renaming: Option<(String, String)>,
    /// The comparison last asked for: the project's revision, the reference, the answer.
    compare_cache: Option<(u64, String, Result<Value, String>)>,
}

type Shared = Arc<Mutex<State>>;

fn lock(s: &Shared) -> std::sync::MutexGuard<'_, State> {
    s.lock().unwrap_or_else(PoisonError::into_inner)
}

fn clock(seconds: f64) -> String {
    let n = seconds.max(0.0) as u64;
    if n >= 3600 { format!("{}:{:02}:{:02}", n / 3600, (n / 60) % 60, n % 60) } else { format!("{}:{:02}", n / 60, n % 60) }
}

fn length(seconds: f64) -> String {
    if seconds < 10.0 { format!("{seconds:.1} s") } else { format!("{seconds:.0} s") }
}

const FILM_EXTENSIONS: [&str; 16] = ["mp4", "m4v", "mov", "mkv", "webm", "avi", "mxf", "mpg", "mpeg", "ts", "mts", "m2ts", "wav", "mp3", "flac", "m4a"];

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: egui::Rect) {
    let t = app.tokens;
    let shared: Shared = ui.data_mut(|d| d.get_temp_mut_or_insert_with(egui::Id::new("references-state"), || Arc::new(Mutex::new(State::default()))).clone());
    let mut st = lock(&shared);
    let list = app.session.execute("reference.list", json!({})).unwrap_or(Value::Null);
    let adopted = app.session.execute("reference.adopted", json!({})).ok().and_then(|v| v["adopted"]["id"].as_str().map(str::to_string));
    // a measuring job: its progress, and what it left when it ended
    let mut progress: Option<(f32, String)> = None;
    if let Some(job) = st.job {
        match app.session.execute("reference.result", json!({"job": job})) {
            Ok(v) if v["state"] == "done" => {
                st.job = None;
                st.path.clear();
                st.name.clear();
                st.rights.clear();
                app.ui.status = tl!("Added to your references.").to_string();
            }
            Ok(v) if v["state"] == "failed" => {
                st.job = None;
                st.error = Some(v["error"].as_str().unwrap_or("").to_string());
            }
            Ok(_) => {
                progress = app.session.jobs.iter().find(|j| j.id == job).map(|j| {
                    let v = j.to_json();
                    (v["progress"].as_f64().unwrap_or(0.0) as f32, v["status"].as_str().unwrap_or("").to_string())
                });
            }
            Err(_) => st.job = None,
        }
    }
    let revision = app.session.revision;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(rect.min + egui::vec2(10.0, 8.0), rect.max - egui::vec2(10.0, 4.0))));
    let ui = &mut child;
    let mut elems: Vec<(String, egui::Response, String)> = Vec::new();
    let mut commands: Vec<(&'static str, Value)> = Vec::new();
    let mut browse = false;
    let mut import = false;
    let mut export: Option<(String, String)> = None;

    egui::ScrollArea::vertical().id_salt("references-scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(
            egui::Label::new(
                RichText::new(tl!("Films you admire, kept as numbers: how long their shots are, how their pace moves, how the picture and sound feel. Nothing of the film is kept. Use only films you have the right to use."))
                    .size(12.0)
                    .color(t.text_dim),
            )
            .wrap(),
        );
        ui.add_space(8.0);
        // ---- add one
        ui.label(RichText::new(tl!("Add a reference")).size(12.5).strong().color(t.text));
        ui.horizontal(|ui| {
            let r = ui.add(TextEdit::singleline(&mut st.path).id(egui::Id::new("references-path")).desired_width((ui.available_width() - 84.0).max(60.0)).hint_text(RichText::new(tl!("Path to a film file")).color(t.text_faint)));
            elems.push(("references.path".into(), r, "Path to a film file".into()));
            let b = ui.button(tl!("Browse…"));
            elems.push(("references.browse".into(), b.clone(), "Choose a film file".into()));
            browse |= b.clicked();
        });
        let r = ui.add(TextEdit::singleline(&mut st.name).id(egui::Id::new("references-name")).desired_width(ui.available_width()).hint_text(RichText::new(tl!("Name (the file's name if you leave it)")).color(t.text_faint)));
        elems.push(("references.name".into(), r, "Name".into()));
        let r = ui.add(
            TextEdit::singleline(&mut st.rights)
                .id(egui::Id::new("references-rights"))
                .desired_width(ui.available_width())
                .hint_text(RichText::new(tl!("Why you may use it: public domain, your own export, a licence…")).color(t.text_faint)),
        );
        elems.push(("references.rights".into(), r, "Why you may use this film".into()));
        let can = st.job.is_none() && !st.path.trim().is_empty() && !st.rights.trim().is_empty();
        ui.horizontal(|ui| {
            let b = ui.add_enabled(can, egui::Button::new(tl!("Measure and add"))).on_hover_text(tl!("Reads the film and keeps only its numbers. A long film takes a few minutes; you can carry on."));
            elems.push(("references.add".into(), b.clone(), "Measure the film and add it".into()));
            if b.clicked() {
                let mut p = json!({"path": st.path.trim(), "rights": st.rights.trim()});
                if !st.name.trim().is_empty() {
                    p["name"] = json!(st.name.trim());
                }
                commands.push(("reference.add", p));
                st.error = None;
            }
            let i = ui.button(tl!("Import a fingerprint…")).on_hover_text(tl!("Add a reference someone shared as a small file: numbers only."));
            elems.push(("references.import".into(), i.clone(), "Import a fingerprint file".into()));
            import |= i.clicked();
        });
        if let Some((fraction, status)) = &progress {
            ui.add(egui::ProgressBar::new(*fraction).text(status.as_str()));
            let c = ui.button(tl!("Stop"));
            elems.push(("references.progress".into(), c.clone(), format!("{status} ({:.0}%)", fraction * 100.0)));
            elems.push(("references.cancel".into(), c.clone(), "Stop measuring".into()));
            if c.clicked()
                && let Some(job) = st.job
            {
                commands.push(("jobs.cancel", json!({"job": job})));
            }
        }
        if let Some(e) = st.error.clone() {
            let r = ui.add(egui::Label::new(RichText::new(&e).size(11.5).color(LOOK)).wrap());
            elems.push(("references.error".into(), r, e));
        }
        ui.add_space(10.0);
        // ---- the library
        let refs: Vec<Value> = list["references"].as_array().cloned().unwrap_or_default();
        if refs.is_empty() {
            let r = ui.label(RichText::new(tl!("No references yet.")).size(12.0).color(t.text_faint));
            elems.push(("references.empty".into(), r, "No references yet".into()));
        }
        for r in &refs {
            let id = r["id"].as_str().unwrap_or("").to_string();
            let is_adopted = adopted.as_deref() == Some(id.as_str());
            let name = r["name"].as_str().unwrap_or("").to_string();
            let card = egui::Frame::new().fill(t.field_bg).corner_radius(6.0).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    if st.renaming.as_ref().is_some_and(|(rid, _)| *rid == id) {
                        let mut done = false;
                        if let Some((_, text)) = st.renaming.as_mut() {
                            let e = ui.add(TextEdit::singleline(text).id(egui::Id::new(("references-rename", &id))).desired_width(200.0));
                            elems.push(("references.rename.field".into(), e.clone(), "New name".into()));
                            e.request_focus();
                            done = e.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter));
                        }
                        if done
                            && let Some((rid, text)) = st.renaming.take()
                            && !text.trim().is_empty()
                            && text.trim() != name
                        {
                            commands.push(("reference.rename", json!({"id": rid, "name": text.trim()})));
                        }
                    } else {
                        ui.label(RichText::new(&name).size(13.0).strong().color(t.text));
                    }
                    if is_adopted {
                        let p = ui.label(RichText::new(tl!("the coach aims at this")).size(11.0).strong().color(FINE));
                        elems.push((format!("references.adopted.{id}"), p, "Adopted as this project's target".into()));
                    }
                });
                ui.label(RichText::new(r["rights"].as_str().unwrap_or("")).size(11.0).color(t.text_faint));
                let shots = &r["shots"];
                let stats = format!(
                    "{} · {} {} · {:.1} {} · {}{}",
                    clock(r["durationSeconds"].as_f64().unwrap_or(0.0)),
                    tl!("shots average"),
                    length(shots["meanSeconds"].as_f64().unwrap_or(0.0)),
                    shots["cutsPerMinute"].as_f64().unwrap_or(0.0),
                    tl!("cuts a minute"),
                    match r["arc"].as_str() {
                        Some("quickening") => tl!("pace quickens"),
                        Some("slowing") => tl!("pace slows"),
                        _ => tl!("pace steady"),
                    },
                    r["integratedLufs"].as_f64().map_or(String::new(), |l| format!(" · {l:.0} LUFS"))
                );
                ui.label(RichText::new(stats).size(11.5).color(t.text_dim));
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let a = ui.button(if is_adopted { tl!("Stop using as target") } else { tl!("Use as target") });
                    elems.push((format!("references.adopt.{id}"), a.clone(), "Use as the coach's target".into()));
                    if a.clicked() {
                        commands.push(("reference.adopt", if is_adopted { json!({"clear": true}) } else { json!({"id": id}) }));
                    }
                    let c = ui.selectable_label(st.comparing.as_deref() == Some(id.as_str()), tl!("Compare"));
                    elems.push((format!("references.compare.{id}"), c.clone(), "Compare my edit with it".into()));
                    if c.clicked() {
                        st.comparing = if st.comparing.as_deref() == Some(id.as_str()) { None } else { Some(id.clone()) };
                    }
                    let d = ui.selectable_label(st.details.as_deref() == Some(id.as_str()), tl!("What it suggests"));
                    elems.push((format!("references.details.{id}"), d.clone(), "What its numbers suggest".into()));
                    if d.clicked() {
                        st.details = if st.details.as_deref() == Some(id.as_str()) { None } else { Some(id.clone()) };
                    }
                    let e = ui.button(tl!("Export…"));
                    elems.push((format!("references.export.{id}"), e.clone(), "Export as a file".into()));
                    if e.clicked() {
                        export = Some((id.clone(), name.clone()));
                    }
                    let n = ui.button(tl!("Rename"));
                    elems.push((format!("references.rename.{id}"), n.clone(), "Rename".into()));
                    if n.clicked() {
                        st.renaming = Some((id.clone(), name.clone()));
                    }
                    let confirming = st.confirm_remove.as_deref() == Some(id.as_str());
                    let x = ui.button(if confirming { tl!("Really remove?") } else { tl!("Remove") });
                    elems.push((format!("references.remove.{id}"), x.clone(), "Remove from the library".into()));
                    if x.clicked() {
                        if confirming {
                            commands.push(("reference.remove", json!({"id": id})));
                            st.confirm_remove = None;
                        } else {
                            st.confirm_remove = Some(id.clone());
                        }
                    }
                });
                // what the numbers suggest
                if st.details.as_deref() == Some(id.as_str()) {
                    match app.session.execute("reference.get", json!({"id": id})) {
                        Ok(v) => {
                            for (n, line) in v["readings"].as_array().into_iter().flatten().take(MAX_ROWS).enumerate() {
                                let text = line.as_str().unwrap_or("").to_string();
                                let l = ui.add(egui::Label::new(RichText::new(format!("• {text}")).size(11.5).color(t.text_dim)).wrap());
                                elems.push((format!("references.reading.{id}.{n}"), l, text));
                            }
                        }
                        Err(e) => {
                            ui.label(RichText::new(e.to_string()).size(11.5).color(LOOK));
                        }
                    }
                }
                // your edit against it
                if st.comparing.as_deref() == Some(id.as_str()) {
                    let cached = st.compare_cache.as_ref().filter(|(rev, cid, _)| *rev == revision && *cid == id).map(|(_, _, v)| v.clone());
                    let answer = cached.unwrap_or_else(|| {
                        let v = app.session.execute("reference.compare", json!({"id": id})).map_err(|e| e.to_string());
                        st.compare_cache = Some((revision, id.clone(), v.clone()));
                        v
                    });
                    match answer {
                        Ok(v) => {
                            let c = &v["comparison"];
                            ui.label(RichText::new(tlf!("“{sequence}” against “{reference}”: {close} close, {differ} differ", sequence = v["sequence"]["name"].as_str().unwrap_or(""), reference = name, close = c["close"].as_u64().unwrap_or(0), differ = c["differ"].as_u64().unwrap_or(0))).size(11.5).strong().color(t.text));
                            for row in c["rows"].as_array().into_iter().flatten().take(MAX_ROWS) {
                                let fine = row["verdict"] == "close";
                                let words = row["words"].as_str().unwrap_or("").to_string();
                                let l = ui.add(egui::Label::new(RichText::new(format!("{} {words}", if fine { "✓" } else { "•" })).size(11.5).color(if fine { FINE } else { t.text })).wrap());
                                elems.push((format!("references.row.{}", row["metric"].as_str().unwrap_or("")), l, words));
                            }
                            for m in c["missing"].as_array().into_iter().flatten() {
                                ui.label(RichText::new(tlf!("not compared: {part}", part = m.as_str().unwrap_or(""))).size(11.0).color(t.text_faint));
                            }
                        }
                        Err(e) => {
                            ui.label(RichText::new(e).size(11.5).color(t.text_faint));
                        }
                    }
                }
            });
            elems.push((format!("references.card.{id}"), card.response, name));
            ui.add_space(6.0);
        }
    });
    drop(st);
    for (id, r, label) in &elems {
        app.auto.add_response(id, r, label);
    }
    for (cmd, p) in commands {
        match app.session.execute(cmd, p) {
            Ok(r) if cmd == "reference.add" => lock(&shared).job = r["job"].as_u64(),
            Ok(_) => {}
            Err(e) => lock(&shared).error = Some(e.to_string()),
        }
    }
    if browse {
        let s = shared.clone();
        app.pick_ui(
            FileDialog::OpenFile { filter: tl!("Film or sound").into(), exts: FILM_EXTENSIONS.iter().map(|e| (*e).to_string()).collect() },
            move |_, paths| {
                if let Some(p) = paths.into_iter().next() {
                    lock(&s).path = p;
                }
            },
        );
    }
    if import {
        app.pick_ui(FileDialog::OpenFile { filter: tl!("Fingerprint").into(), exts: vec!["json".into()] }, |app, paths| {
            if let Some(p) = paths.into_iter().next()
                && let Err(e) = app.session.execute("reference.import", json!({"path": p}))
            {
                app.ui.status = e.to_string();
            } else {
                app.ui.status = tl!("Fingerprint imported.").to_string();
            }
        });
    }
    if let Some((id, name)) = export {
        let file =
            format!("{}.fingerprint.json", name.chars().filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-').collect::<String>().trim().replace(' ', "-"));
        app.pick_ui(FileDialog::SaveAs { filter: tl!("Fingerprint").into(), exts: vec!["json".into()], name: file }, move |app, paths| {
            if let Some(p) = paths.into_iter().next() {
                match app.session.execute("reference.export", json!({"id": id, "path": p})) {
                    Ok(_) => app.ui.status = tl!("Fingerprint saved.").to_string(),
                    Err(e) => app.ui.status = e.to_string(),
                }
            }
        });
    }
}
