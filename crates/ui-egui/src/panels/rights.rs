//! Rights badges and tags in the Project panel (fork roadmap F04): a small round badge on each
//! media item that has a provenance record (green: free to use; amber: credit asked for or not
//! ruled out; red: not for commercial use or terms not known; violet: AI-generated), with the
//! record in a tooltip, and a Rights submenu to tag an item as your own footage or as licensed.
//! An item with no record shows nothing.
//!
//! Automation ids: `<panel>.item.<id>.rights` (label: the status and what it means) and
//! `project.itemMenu.rights.own` / `.licensed` / `.clear`.

use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use filmcraft_engine::provenance::{self, Status};
use filmcraft_project::{ItemId, ItemKind};
use serde_json::json;

use crate::FilmcraftApp;
use crate::panels::project::Actions;
use crate::theme::Tokens;

/// The badge colour for a status.
pub fn color(s: Status) -> Color32 {
    match s {
        Status::Clear => Color32::from_rgb(0x4c, 0xaf, 0x6a),
        Status::Credit => Color32::from_rgb(0xe8, 0xb3, 0x3c),
        Status::Risk => Color32::from_rgb(0xe8, 0x5c, 0x5c),
        Status::Synthetic => Color32::from_rgb(0x9b, 0x7b, 0xf0),
    }
}

/// A few words for a status.
pub fn name(s: Status) -> &'static str {
    match s {
        Status::Clear => tl!("Free to use"),
        Status::Credit => tl!("Credit needed"),
        Status::Risk => tl!("Check the terms"),
        Status::Synthetic => tl!("AI-generated"),
    }
}

/// Draws the badge for `item` centred at `at`, if it has a record. `auto` is its automation id.
/// Returns the width taken (0 when nothing was drawn), so a caller can place what follows.
pub fn badge(app: &mut FilmcraftApp, ui: &egui::Ui, item: ItemId, at: egui::Pos2, auto: &str) -> f32 {
    if app.session.project.ext.is_empty() {
        return 0.0;
    }
    let Some(rec) = provenance::get(&app.session.project, item) else { return 0.0 };
    let status = rec.status();
    let rect = Rect::from_center_size(at, vec2(15.0, 15.0));
    let p = ui.painter();
    p.circle_filled(at, 7.0, color(status));
    let ink = Color32::from_black_alpha(220);
    let (stroke, font) = (Stroke::new(1.6, ink), Tokens::semibold(9.0));
    match status {
        Status::Clear => {
            p.line_segment([pos2(at.x - 3.2, at.y + 0.2), pos2(at.x - 0.9, at.y + 2.6)], stroke);
            p.line_segment([pos2(at.x - 0.9, at.y + 2.6), pos2(at.x + 3.4, at.y - 2.6)], stroke);
        }
        Status::Credit => {
            p.text(at, Align2::CENTER_CENTER, "©", font, ink);
        }
        Status::Risk => {
            p.text(at, Align2::CENTER_CENTER, "!", font, ink);
        }
        Status::Synthetic => {
            p.text(at, Align2::CENTER_CENTER, "AI", Tokens::semibold(7.5), ink);
        }
    }
    let summary = rec.summary();
    let resp = ui.interact(rect, egui::Id::new(("rights-badge", auto)), Sense::hover());
    resp.on_hover_text(format!("{}\n{summary}", crate::i18n::t(name(status))));
    app.auto.add(auto, rect, &format!("{}: {summary}", name(status)));
    16.0
}

/// The Rights submenu of an item's context menu: tag it, or clear the tag. Only media has a record.
pub fn menu(app: &mut FilmcraftApp, ui: &mut egui::Ui, id: ItemId, kind: &ItemKind, actions: &mut Actions) {
    if !matches!(kind, ItemKind::Media(_) | ItemKind::Subclip { .. }) {
        return;
    }
    let has = provenance::get(&app.session.project, id).is_some();
    let r = ui.menu_button(tl!("Rights"), |ui| {
        for (key, label, tag) in [("own", tl!("My footage"), "own"), ("licensed", tl!("Licensed to me"), "licensed")] {
            let b = ui.button(label);
            app.auto.add(&format!("project.itemMenu.rights.{key}"), b.rect, label);
            if b.clicked() {
                actions.push(("provenance.set".into(), json!({"item": id.0, "tag": tag})));
                ui.close();
            }
        }
        let b = ui.add_enabled(has, egui::Button::new(tl!("Clear the record")));
        app.auto.add("project.itemMenu.rights.clear", b.rect, "Clear the record");
        if b.clicked() {
            actions.push(("provenance.set".into(), json!({"item": id.0, "clear": true})));
            ui.close();
        }
    });
    app.auto.add("project.itemMenu.rights", r.response.rect, tl!("Rights"));
}
