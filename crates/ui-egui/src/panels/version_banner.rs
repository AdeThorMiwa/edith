//! The version banner (fork roadmap F14.3): a strip above the editor that appears while a version is
//! open, so you always know whether you are working on a version of the edit or on the edit itself.
//!
//! It names the version (rename it in place), says how many steps it has and who made them, and has
//! **See original** (look at the committed edit, read-only, and back), **Changes** (what the version
//! changes against the original in plain words, and its steps, each one a place to rewind to),
//! **Commit** (the whole version becomes one undo step) and **Discard** (asks twice). It is drawn from
//! the session directly and holds only whether the list is open and whether a discard is waiting for
//! its second click.
//!
//! Automation ids: `version.banner`, `version.title`, `version.peek`, `version.changes`,
//! `version.commit`, `version.discard`, `version.line.<n>` (a sentence of the summary),
//! `version.step.<n>` (a step) and `version.step.<n>.rewind`.

use egui::{Align, Color32, Layout, Rect, RichText, Sense, pos2, vec2};
use filmcraft_engine::version::{Actor, Info};
use serde_json::json;

use crate::FilmcraftApp;

const ROW_H: f32 = 36.0;
const LIST_H: f32 = 176.0;
const AMBER: Color32 = Color32::from_rgb(0xe8, 0xb3, 0x3c);
const MAX_LINES: usize = 30;

fn open_id() -> egui::Id {
    egui::Id::new("version-banner-changes-open")
}

fn confirm_id() -> egui::Id {
    egui::Id::new("version-banner-confirm-discard")
}

fn summary_id() -> egui::Id {
    egui::Id::new("version-banner-summary")
}

/// How tall the banner is this frame: nothing when no version is open.
pub fn height(app: &FilmcraftApp, ctx: &egui::Context) -> f32 {
    if !app.session.versions.is_open() {
        return 0.0;
    }
    ROW_H + if ctx.data(|d| d.get_temp::<bool>(open_id())).unwrap_or(false) { LIST_H } else { 0.0 }
}

fn actor_word(a: Actor) -> &'static str {
    match a {
        Actor::You => tl!("you"),
        Actor::Assistant => tl!("assistant"),
    }
}

/// The summary lines, worked out again only when the project has changed.
fn summary(app: &FilmcraftApp, ctx: &egui::Context) -> Vec<String> {
    let rev = app.session.revision;
    if let Some((r, lines)) = ctx.data(|d| d.get_temp::<(u64, Vec<String>)>(summary_id()))
        && r == rev
    {
        return lines;
    }
    let lines: Vec<String> = app.session.version_changes().map(|d| d.lines).unwrap_or_default();
    ctx.data_mut(|d| d.insert_temp(summary_id(), (rev, lines.clone())));
    lines
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let Some(info) = app.session.version_info() else { return };
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let expanded = ctx.data(|d| d.get_temp::<bool>(open_id())).unwrap_or(false);
    let confirming = ctx.data(|d| d.get_temp::<bool>(confirm_id())).unwrap_or(false);
    let mut elems: Vec<(String, egui::Response, String)> = Vec::new();
    let mut commands: Vec<(&'static str, serde_json::Value)> = Vec::new();
    let tint = if info.peeking { AMBER.gamma_multiply(0.22) } else { t.header_bg };
    ui.painter().rect_filled(rect, 0.0, tint);
    ui.painter().rect_filled(Rect::from_min_size(rect.min, vec2(rect.width(), 2.0)), 0.0, if info.peeking { AMBER } else { t.accent });
    let row = Rect::from_min_size(rect.min + vec2(10.0, 2.0), vec2(rect.width() - 20.0, ROW_H - 4.0));
    let mut toggle_list = false;
    let mut clear_confirm = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(row), |ui| {
        ui.horizontal_centered(|ui| {
            ui.label(RichText::new(tl!("VERSION")).size(10.5).strong().color(if info.peeking { AMBER } else { t.text_faint }));
            if info.peeking {
                ui.label(RichText::new(tl!("Viewing the original: read-only")).size(12.0).strong().color(AMBER));
            } else {
                let (r, new) = super::planner::field(ui, "version-title", &info.title, false, 220.0, 1, tl!("Name of this version"), &t);
                elems.push(("version.title".into(), r, "Name of this version".into()));
                if let Some(v) = new
                    && !v.trim().is_empty()
                {
                    commands.push(("version.rename", json!({"title": v})));
                }
            }
            let (you, assistant) = (info.steps.iter().filter(|s| s.by == Actor::You).count(), info.steps.iter().filter(|s| s.by == Actor::Assistant).count());
            let steps_text = match (info.steps.len(), assistant) {
                (0, _) => tl!("no changes yet").to_string(),
                (1, 0) => tl!("1 step").to_string(),
                (n, 0) => tlf!("{n} steps", n = n),
                (n, a) => tlf!("{n} steps ({you} by you, {a} by the assistant)", n = n, you = you, a = a),
            };
            ui.label(RichText::new(steps_text).size(11.0).color(t.text_dim));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let d = ui.button(if confirming { tl!("Really discard?") } else { tl!("Discard") });
                elems.push(("version.discard".into(), d.clone(), "Discard this version".into()));
                if d.clicked() {
                    if confirming {
                        commands.push(("version.discard", json!({})));
                        clear_confirm = true;
                    } else {
                        ctx.data_mut(|m| m.insert_temp(confirm_id(), true));
                    }
                } else if confirming && !d.hovered() && ui.input(|i| i.pointer.any_click()) {
                    clear_confirm = true;
                }
                let c = ui.add_enabled(!info.peeking, egui::Button::new(RichText::new(tl!("Commit")).strong()).fill(t.accent));
                elems.push(("version.commit".into(), c.clone(), "Commit this version into the edit".into()));
                if c.clicked() {
                    commands.push(("version.commit", json!({})));
                }
                let l = ui.selectable_label(expanded, if expanded { tl!("Changes ▴") } else { tl!("Changes ▾") });
                elems.push(("version.changes".into(), l.clone(), "Show what this version changes".into()));
                toggle_list |= l.clicked();
                let p = ui.selectable_label(info.peeking, if info.peeking { tl!("Back to the version") } else { tl!("See original") });
                elems.push(("version.peek".into(), p.clone(), "Look at the original, read-only".into()));
                if p.clicked() {
                    commands.push(("version.peek", json!({})));
                }
            });
        });
    });
    if expanded {
        let list = Rect::from_min_max(pos2(rect.min.x + 10.0, rect.min.y + ROW_H), pos2(rect.max.x - 10.0, rect.max.y - 4.0));
        list_panel(ui, list, app, &info, &mut elems, &mut commands);
    }
    let whole = ui.interact(Rect::from_min_size(rect.min, vec2(rect.width(), 2.0)), egui::Id::new("version-banner-edge"), Sense::hover());
    elems.push(("version.banner".into(), whole, info.title.clone()));
    if toggle_list {
        ctx.data_mut(|m| m.insert_temp(open_id(), !expanded));
    }
    if clear_confirm {
        ctx.data_mut(|m| m.insert_temp(confirm_id(), false));
    }
    for (id, r, label) in &elems {
        app.auto.add_response(id, r, label);
    }
    for (cmd, p) in commands {
        let r = app.session.execute(cmd, p);
        match (cmd, r) {
            ("version.commit", Ok(v)) => {
                app.ui.status = if v["committed"] == true {
                    tlf!("Committed “{title}” as one undo step.", title = v["title"].as_str().unwrap_or(""))
                } else {
                    tl!("The version changed nothing, so there is nothing to commit.").to_string()
                };
            }
            ("version.discard", Ok(_)) => app.ui.status = tl!("Discarded the version. Your edit is as it was.").to_string(),
            (_, Err(e)) => app.ui.status = e.to_string(),
            _ => {}
        }
    }
}

fn list_panel(
    ui: &mut egui::Ui,
    rect: Rect,
    app: &FilmcraftApp,
    info: &Info,
    elems: &mut Vec<(String, egui::Response, String)>,
    commands: &mut Vec<(&'static str, serde_json::Value)>,
) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let lines = summary(app, &ctx);
    let half = (rect.width() - 12.0) / 2.0;
    let left = Rect::from_min_size(rect.min, vec2(half, rect.height()));
    let right = Rect::from_min_size(pos2(rect.min.x + half + 12.0, rect.min.y), vec2(half, rect.height()));
    ui.scope_builder(egui::UiBuilder::new().max_rect(left), |ui| {
        ui.label(RichText::new(tl!("What this version changes")).size(11.5).strong().color(t.text));
        egui::ScrollArea::vertical().id_salt("version-summary").auto_shrink([false, false]).show(ui, |ui| {
            if lines.is_empty() {
                ui.label(RichText::new(tl!("Nothing yet: it is the same as the original.")).size(11.5).color(t.text_faint));
            }
            for (n, l) in lines.iter().take(MAX_LINES).enumerate() {
                let r = ui.add(egui::Label::new(RichText::new(format!("• {l}")).size(11.5).color(t.text_dim)).wrap());
                elems.push((format!("version.line.{n}"), r, l.clone()));
            }
        });
    });
    ui.scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
        ui.label(RichText::new(tl!("Steps, oldest first")).size(11.5).strong().color(t.text));
        egui::ScrollArea::vertical().id_salt("version-steps").auto_shrink([false, false]).show(ui, |ui| {
            let n = info.steps.len();
            if n == 0 {
                ui.label(RichText::new(tl!("No steps yet.")).size(11.5).color(t.text_faint));
            }
            for (i, s) in info.steps.iter().enumerate() {
                ui.horizontal(|ui| {
                    let row = ui.label(RichText::new(format!("{}. {}", i + 1, s.label)).size(11.5).color(t.text_dim));
                    elems.push((format!("version.step.{i}"), row, s.label.clone()));
                    ui.label(RichText::new(actor_word(s.by)).size(10.5).color(if s.by == Actor::Assistant { t.accent } else { t.text_faint }));
                    // keeping this step and everything before it takes back the ones after
                    if i + 1 < n {
                        let b = ui.small_button(tl!("Rewind to here")).on_hover_text(tl!("Takes back the steps after this one."));
                        elems.push((format!("version.step.{i}.rewind"), b.clone(), "Rewind the version to this step".into()));
                        if b.clicked() {
                            commands.push(("version.rewind", json!({"steps": n - 1 - i})));
                        }
                    }
                });
            }
        });
    });
}
