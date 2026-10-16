//! Storyboard (fork roadmap F09.4): one tile per planned shot, in plan order, grouped under the
//! beat each belongs to. A tile shows the picture (a frame of the shot's source), or, while the shot
//! has none, what it should show written on a plain box. Under it you can edit the kind of shot, the
//! camera move, the length and the written intent. Click the picture to use the clip selected in
//! the Project panel; **No picture** takes it off again.
//!
//! It is a view on the plan, like the A/V Script and the Beat Board: every change is a `planner.*`
//! command (`updateShot`, `setShotSource`, `addShot`, `delete`), one undo step each.
//!
//! Automation ids: `story.empty`, `story.shot.<id>` (the picture), `story.shot.<id>.type|move|length|
//! intent|clear|delete`, `story.addShot.<beat>`.

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::panels::beat_lane::SECTION_COLORS;
use crate::panels::planner::{field, parse_clock};
use crate::theme::Tokens;

const TILE_W: f32 = 204.0;
const PICTURE_H: f32 = TILE_W * 9.0 / 16.0;
const TILE_H: f32 = PICTURE_H + 6.0 + 22.0 + 4.0 + 46.0 + 4.0;
const GAP: f32 = 8.0;

/// A shot's length for the tile: `3`, `3.5`, `0` (seconds, one decimal at most).
pub fn seconds_text(ticks: i64) -> String {
    let tenths = (ticks.max(0) as f64 / TICKS_PER_SECOND as f64 * 10.0).round() as i64;
    if tenths % 10 == 0 { format!("{}", tenths / 10) } else { format!("{}.{}", tenths / 10, tenths % 10) }
}

fn picture(app: &mut FilmcraftApp, ui: &mut egui::Ui, ctx: &egui::Context, rect: Rect, shot: &Value, t: &Tokens) -> egui::Response {
    let id = shot["id"].as_u64().unwrap_or(0);
    let resp = ui.interact(rect.intersect(ui.clip_rect()), egui::Id::new(("story-picture", id)), Sense::click());
    let painter = ui.painter();
    let tex = shot["source"].as_u64().and_then(|i| app.thumbnail(ctx, filmcraft_project::ItemId(i), Tick::ZERO, 240));
    match tex {
        Some((tid, _)) => {
            painter.image(tid, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            painter.rect_filled(rect, 4.0, t.field_bg);
            let intent = shot["intent"].as_str().unwrap_or("").trim();
            if intent.is_empty() {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    if shot["source"].is_u64() { tl!("Picture") } else { tl!("No picture") },
                    Tokens::ui(11.0),
                    t.text_faint,
                );
            } else {
                // what it should show, written on the box
                let mut job = egui::text::LayoutJob::simple(intent.to_string(), Tokens::ui(12.0), t.text_dim, rect.width() - 20.0);
                job.wrap.max_rows = 5;
                let galley = painter.layout_job(job);
                painter.with_clip_rect(rect).galley(rect.min + vec2(10.0, 10.0), galley, Color32::WHITE);
            }
        }
    }
    painter.rect_stroke(rect, 4.0, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
    resp
}

#[allow(clippy::too_many_arguments)]
fn tile(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    area: Rect,
    shot: &Value,
    selection: Option<filmcraft_project::ItemId>,
    elems: &mut Vec<(String, egui::Response, String)>,
    commands: &mut Vec<(&'static str, Value)>,
    note: &mut Option<String>,
) {
    let t = app.tokens;
    let id = shot["id"].as_u64().unwrap_or(0);
    let source = shot["source"].as_u64();
    let pic = Rect::from_min_size(area.min, vec2(TILE_W, PICTURE_H));
    let resp = picture(app, ui, ctx, pic, shot, &t).on_hover_text(if selection.is_some() {
        tl!("Use the clip selected in the Project panel as this shot's picture.")
    } else {
        tl!("Select a clip in the Project panel, then press here to use it as the picture.")
    });
    elems.push((format!("story.shot.{id}"), resp.clone(), "The shot's picture".into()));
    if resp.clicked()
        && let Some(item) = selection
    {
        commands.push(("planner.setShotSource", json!({"shot": id, "item": item.0})));
    }
    // the small buttons over the picture's corner
    if source.is_some() {
        let r = Rect::from_min_size(pic.min + vec2(4.0, 4.0), vec2(74.0, 18.0));
        let c = ui.put(r, egui::Button::new(egui::RichText::new(tl!("No picture")).size(10.5)).small());
        elems.push((format!("story.shot.{id}.clear"), c.clone(), "Remove the shot's picture".into()));
        if c.clicked() {
            commands.push(("planner.setShotSource", json!({"shot": id, "item": Value::Null})));
        }
    }
    let r = Rect::from_min_size(pos2(pic.max.x - 24.0, pic.min.y + 4.0), vec2(20.0, 18.0));
    let d = ui.put(r, egui::Button::new("✕").small()).on_hover_text(tl!("Delete this shot"));
    elems.push((format!("story.shot.{id}.delete"), d.clone(), "Delete this shot".into()));
    if d.clicked() {
        commands.push(("planner.delete", json!({"shot": id})));
    }
    // kind of shot, camera move, length
    let row = Rect::from_min_size(pos2(area.min.x, pic.max.y + 6.0), vec2(TILE_W, 22.0));
    let (w_type, w_move, w_len) = (68.0, 78.0, TILE_W - 68.0 - 78.0 - 8.0);
    let cells = [
        ("type", w_type, shot["shotType"].as_str().unwrap_or("").to_string(), tl!("Shot")),
        ("move", w_move, shot["cameraMove"].as_str().unwrap_or("").to_string(), tl!("Move")),
        ("length", w_len, seconds_text(shot["target"].as_i64().unwrap_or(0)), tl!("Seconds")),
    ];
    let mut x = row.min.x;
    for (key, w, current, hint) in cells {
        let cell = Rect::from_min_size(pos2(x, row.min.y), vec2(w, row.height()));
        let (r, new) =
            ui.scope_builder(egui::UiBuilder::new().max_rect(cell), |ui| field(ui, &format!("story.{id}.{key}"), &current, false, w, 1, hint, &t)).inner;
        let r = r.on_hover_text(match key {
            "type" => tl!("Kind of shot, such as wide or close-up"),
            "move" => tl!("Camera move, such as push in or still"),
            _ => tl!("Length in seconds"),
        });
        elems.push((format!("story.shot.{id}.{key}"), r, key.to_string()));
        if let Some(v) = new {
            match key {
                "type" => commands.push(("planner.updateShot", json!({"shot": id, "shotType": v.trim()}))),
                "move" => commands.push(("planner.updateShot", json!({"shot": id, "cameraMove": v.trim()}))),
                _ => match parse_clock(&v) {
                    Some(seconds) => commands.push(("planner.updateShot", json!({"shot": id, "seconds": seconds}))),
                    None => *note = Some(tl!("Write a length in seconds, like 3 or 3.5").to_string()),
                },
            }
        }
        x += w + 4.0;
    }
    // what it should show
    let intent = Rect::from_min_size(pos2(area.min.x, row.max.y + 4.0), vec2(TILE_W, 46.0));
    let (r, new) = ui
        .scope_builder(egui::UiBuilder::new().max_rect(intent), |ui| {
            field(ui, &format!("story.{id}.intent"), shot["intent"].as_str().unwrap_or(""), true, TILE_W, 2, tl!("What the picture shows"), &t)
        })
        .inner;
    elems.push((format!("story.shot.{id}.intent"), r, "What the shot should show".into()));
    if let Some(v) = new {
        commands.push(("planner.updateShot", json!({"shot": id, "intent": v})));
    }
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    ui.painter().rect_filled(rect, 0.0, t.panel_bg);
    let got = app.session.execute("planner.get", json!({})).unwrap_or(Value::Null);
    let mut elems: Vec<(String, egui::Response, String)> = Vec::new();
    let mut commands: Vec<(&'static str, Value)> = Vec::new();
    let mut note: Option<String> = None;
    if got["exists"].as_bool() != Some(true) {
        let r = ui.allocate_rect(rect, Sense::hover());
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, tl!("No plan yet: open Window ▸ Planner"), Tokens::ui(12.0), t.text_faint);
        app.auto.add_response("story.empty", &r, "No plan yet");
        return;
    }
    let selection = app.session.state.project_selection.first().copied();
    let ctx = ui.ctx().clone();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(10.0, 8.0))));
    let ui = &mut child;
    egui::ScrollArea::vertical().id_salt("storyboard-scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.set_width(ui.available_width());
        for (si, sec) in got["plan"]["sections"].as_array().into_iter().flatten().enumerate() {
            let color = SECTION_COLORS[si % SECTION_COLORS.len()];
            for b in sec["beats"].as_array().into_iter().flatten() {
                let bid = b["id"].as_u64().unwrap_or(0);
                ui.horizontal(|ui| {
                    let (bar, _) = ui.allocate_exact_size(vec2(4.0, 16.0), Sense::hover());
                    ui.painter().rect_filled(bar, 2.0, color);
                    ui.label(egui::RichText::new(b["name"].as_str().unwrap_or("")).size(12.5).strong().color(t.text));
                    ui.label(egui::RichText::new(sec["name"].as_str().unwrap_or("")).size(11.0).color(t.text_faint));
                });
                ui.add_space(4.0);
                // an explicit grid: the wrapping layouts centre items on a row and stagger the tiles
                let shots: Vec<Value> = b["shots"].as_array().cloned().unwrap_or_default();
                let per_row = (((ui.available_width() + GAP) / (TILE_W + GAP)).floor() as usize).max(1);
                // the "Add shot" button is the cell after the last shot; a row that holds only it is short
                let last_row = shots.len() / per_row;
                let height = if shots.is_empty() {
                    30.0
                } else if shots.len().is_multiple_of(per_row) {
                    last_row as f32 * (TILE_H + GAP) + PICTURE_H
                } else {
                    last_row as f32 * (TILE_H + GAP) + TILE_H
                };
                let (_, grid) = ui.allocate_space(vec2(per_row as f32 * (TILE_W + GAP), height));
                // widgets placed by `put` and `scope_builder` move the cursor of the Ui they are in, so the
                // grid draws in a child and the next beat starts below it
                let mut cell_ui = ui.new_child(egui::UiBuilder::new().max_rect(grid));
                let g = &mut cell_ui;
                for (n, shot) in shots.iter().enumerate() {
                    let at = pos2(grid.min.x + (n % per_row) as f32 * (TILE_W + GAP), grid.min.y + (n / per_row) as f32 * (TILE_H + GAP));
                    tile(app, g, &ctx, Rect::from_min_size(at, vec2(TILE_W, TILE_H)), shot, selection, &mut elems, &mut commands, &mut note);
                }
                let n = shots.len();
                let at = pos2(grid.min.x + (n % per_row) as f32 * (TILE_W + GAP), grid.min.y + (n / per_row) as f32 * (TILE_H + GAP));
                let area = Rect::from_min_size(at, if shots.is_empty() { vec2(TILE_W, 30.0) } else { vec2(84.0, PICTURE_H) });
                let label = if shots.is_empty() { tl!("No shots yet. Add shot") } else { tl!("Add shot") };
                let add = g.put(area, egui::Button::new(label).fill(Color32::TRANSPARENT));
                elems.push((format!("story.addShot.{bid}"), add.clone(), "Add a shot to this beat".into()));
                if add.clicked() {
                    commands.push(("planner.addShot", json!({"beat": bid})));
                }
                ui.add_space(10.0);
            }
        }
    });
    for (id, r, label) in &elems {
        app.auto.add_response(id, r, label);
    }
    for (cmd, p) in commands {
        if let Err(e) = app.session.execute(cmd, p) {
            note = Some(e.to_string());
        }
    }
    if let Some(n) = note {
        app.ui.status = n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shots_length_shows_whole_seconds_or_one_decimal() {
        let s = |x: f64| (x * TICKS_PER_SECOND as f64) as i64;
        assert_eq!(seconds_text(0), "0");
        assert_eq!(seconds_text(s(3.0)), "3");
        assert_eq!(seconds_text(s(3.5)), "3.5");
        assert_eq!(seconds_text(s(3.04)), "3", "under a tenth rounds away");
        assert_eq!(seconds_text(s(2.96)), "3");
        assert_eq!(seconds_text(-5), "0", "hostile values do not panic or go negative");
        let _ = seconds_text(i64::MAX);
        let _ = seconds_text(i64::MIN);
    }
}
