//! Credits and Rights panel (Window ▸ Credits and Rights; fork roadmap F04.2): what this project
//! allows (commercial use, material that needs a credit, AI-generated material), a live check of
//! the open sequence against it, and the credits block to paste into a video description.
//!
//! The panel only reads: the policy is changed through `provenance.policy`, the check is
//! `provenance.audit`, and the credits are `provenance.credits`. The check is redone whenever the
//! project changes.
//!
//! Automation ids: `rights.policy.commercial`, `rights.policy.attribution`, `rights.policy.synthetic`,
//! `rights.summary`, `rights.finding.<n>` (a click moves the playhead there), `rights.format.<name>`,
//! `rights.credits`, `rights.copy`.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Rect, RichText, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::theme::Tokens;

fn formats() -> [(&'static str, &'static str); 3] {
    [("text", tl!("Plain")), ("markdown", tl!("Markdown")), ("youtube", tl!("YouTube"))]
}

#[derive(Default)]
struct State {
    /// The project revision and sequence the numbers below were made for.
    key: Option<(u64, Option<u64>)>,
    policy: Value,
    audit: Value,
    credits: Value,
    format: String,
    format_shown: String,
}

fn colour(severity: &str, t: &Tokens) -> egui::Color32 {
    match severity {
        "error" => crate::panels::rights::color(filmcraft_engine::provenance::Status::Risk),
        "warning" => crate::panels::rights::color(filmcraft_engine::provenance::Status::Credit),
        _ => t.text_faint,
    }
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let shared: Arc<Mutex<State>> =
        ui.data_mut(|d| d.get_temp_mut_or_insert_with(egui::Id::new("rights-state"), || Arc::new(Mutex::new(State::default()))).clone());
    let mut st = shared.lock().unwrap_or_else(PoisonError::into_inner);
    if st.format.is_empty() {
        st.format = "text".to_string();
    }
    // redo the numbers when the project, the open sequence or the credits format changed
    let key = (app.session.revision, app.session.state.active_sequence.map(|s| s.0));
    if st.key != Some(key) || st.format != st.format_shown {
        st.policy = app.session.execute("provenance.policy", json!({})).unwrap_or(Value::Null);
        st.audit = app.session.execute("provenance.audit", json!({})).unwrap_or(Value::Null);
        let f = st.format.clone();
        st.credits = app.session.execute("provenance.credits", json!({"format": f})).unwrap_or(Value::Null);
        st.key = Some(key);
        st.format_shown = st.format.clone();
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(rect.min + vec2(10.0, 8.0), rect.max - vec2(10.0, 4.0))));
    let ui = &mut child;
    let mut elems: Vec<(String, egui::Response, String)> = Vec::new();
    let mut commands: Vec<(&'static str, Value)> = Vec::new();
    let mut goto: Option<f64> = None;
    let mut copy: Option<String> = None;

    egui::ScrollArea::vertical().id_salt("rights-scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(RichText::new(tl!("What this project allows")).font(Tokens::semibold(13.0)).color(t.text));
        for (key, field, label, hint) in [
            (
                "commercial",
                "commercial",
                tl!("This video is commercial (monetized)"),
                tl!("Material that is not licensed for commercial use is then an error."),
            ),
            ("attribution", "attributionOk", tl!("Allow material that needs a credit"), tl!("Off: anything that asks for a credit is an error.")),
            ("synthetic", "syntheticOk", tl!("Allow AI-generated material"), tl!("Off: anything marked AI-generated is an error.")),
        ] {
            let mut on = st.policy["policy"][field].as_bool().unwrap_or(true);
            let r = ui.checkbox(&mut on, label).on_hover_text(hint);
            elems.push((format!("rights.policy.{key}"), r.clone(), format!("{label}: {}", if on { "on" } else { "off" })));
            if r.changed() {
                commands.push(("provenance.policy", json!({field: on})));
            }
        }

        ui.add_space(8.0);
        let (errors, warnings, notes) =
            (st.audit["errors"].as_u64().unwrap_or(0), st.audit["warnings"].as_u64().unwrap_or(0), st.audit["notes"].as_u64().unwrap_or(0));
        let checked = st.audit["clipsChecked"].as_u64().unwrap_or(0);
        let summary = if st.audit.is_null() {
            tl!("Open a sequence to check its rights.").to_string()
        } else if errors == 0 && warnings == 0 {
            tlf!("All {n} clips in this edit are clear.", n = checked)
        } else {
            tlf!("{errors} to fix · {warnings} to check · {notes} notes, across {n} clips", errors = errors, warnings = warnings, notes = notes, n = checked)
        };
        let color = if errors > 0 {
            colour("error", &t)
        } else if warnings > 0 {
            colour("warning", &t)
        } else {
            crate::panels::rights::color(filmcraft_engine::provenance::Status::Clear)
        };
        let r = ui.label(RichText::new(&summary).font(Tokens::semibold(13.0)).color(color));
        elems.push(("rights.summary".into(), r, summary.clone()));
        let findings = st.audit["findings"].as_array().cloned().unwrap_or_default();
        for (n, f) in findings.iter().enumerate() {
            let severity = f["severity"].as_str().unwrap_or("note");
            let message = f["message"].as_str().unwrap_or("").to_string();
            let mut at: Vec<&str> = f["uses"].as_array().map(|u| u.iter().filter_map(|x| x["timecode"].as_str()).collect()).unwrap_or_default();
            // a clip with sound is two clips at the same moment: say each moment once
            at.dedup();
            at.truncate(6);
            let first = f["uses"][0]["seconds"].as_f64();
            let r = egui::Frame::new()
                .fill(t.field_bg)
                .stroke(egui::Stroke::new(1.0, t.field_border))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_top(|ui| {
                        let (dot, _) = ui.allocate_exact_size(vec2(10.0, 16.0), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 4.0, colour(severity, &t));
                        ui.vertical(|ui| {
                            ui.add(egui::Label::new(RichText::new(&message).size(12.0).color(t.text)).wrap());
                            if !at.is_empty() {
                                ui.label(RichText::new(format!("{} {}", tl!("At"), at.join(", "))).size(11.0).color(t.text_faint));
                            }
                        });
                    });
                })
                .response
                .interact(egui::Sense::click())
                .on_hover_text(tl!("Click to go there"))
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            elems.push((format!("rights.finding.{n}"), r.clone(), format!("{severity}: {message}")));
            if r.clicked() {
                goto = first;
            }
        }

        ui.add_space(10.0);
        ui.label(RichText::new(tl!("Credits")).font(Tokens::semibold(13.0)).color(t.text));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (name, label) in formats() {
                let r = crate::panels::assistant_widgets::pill(ui, label, st.format == name, true, &t);
                elems.push((format!("rights.format.{name}"), r.clone(), format!("{name} format")));
                if r.clicked() {
                    st.format = name.to_string();
                }
            }
        });
        let text = st.credits["text"].as_str().unwrap_or("").to_string();
        if text.is_empty() {
            ui.label(RichText::new(tl!("Nothing in this edit needs a credit.")).size(12.0).color(t.text_dim));
        } else {
            let mut shown = text.clone();
            let r = ui.add(egui::TextEdit::multiline(&mut shown).desired_width(f32::INFINITY).desired_rows(4).font(Tokens::ui(12.0)).interactive(true));
            elems.push(("rights.credits".into(), r, text.clone()));
            let c = ui.button(tl!("Copy"));
            elems.push(("rights.copy".into(), c.clone(), "Copy the credits".into()));
            if c.clicked() {
                copy = Some(text);
            }
        }
        let missing = st.credits["missingCreditText"].as_array().map_or(0, Vec::len);
        let unrecorded = st.credits["unrecorded"].as_u64().unwrap_or(0);
        if missing > 0 {
            ui.label(
                RichText::new(tlf!("{n} credit lines have no author or credit text. Add one so the line is complete.", n = missing))
                    .size(11.0)
                    .color(colour("warning", &t)),
            );
        }
        if unrecorded > 0 {
            ui.label(
                RichText::new(tlf!("{n} clips in this edit have no record, so no credit can be listed for them.", n = unrecorded))
                    .size(11.0)
                    .color(t.text_faint),
            );
        }
        ui.add_space(6.0);
        ui.label(RichText::new(tl!("This reports what each source says about itself. It is not legal advice.")).size(11.0).color(t.text_faint));
    });
    drop(st);
    for (id, r, label) in &elems {
        app.auto.add_response(id, r, label);
    }
    for (cmd, p) in commands {
        if let Err(e) = app.session.execute(cmd, p) {
            app.ui.status = e.to_string();
        }
    }
    if let Some(sec) = goto {
        let _ = app.session.execute("playhead.set", json!({"seconds": sec}));
    }
    if let Some(text) = copy {
        ui.ctx().copy_text(text);
        app.ui.status = tl!("Copied to the clipboard.").to_string();
    }
}
