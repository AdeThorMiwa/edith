//! Beat board (fork roadmap F09.3): the plan as cards, one column per section (act). A card shows a
//! beat's name, length, status, intensity and the start of its purpose. Drag a card to put it
//! somewhere else in its column or in another column; the plan changes order and nothing else:
//! the clips and markers you have already put on the timeline stay where they are. When the
//! plan's order no longer matches the markers, a line says so and **Rebuild markers** moves them
//! to match (`planner.refreshLinks`).
//!
//! Every change is a `planner.*` command (`planner.move`, `planner.updateSection`,
//! `planner.addBeat`, `planner.refreshLinks`), so it is one undo step and the control channel and
//! MCP do the same thing. Click a card to select the beat (the A/V Script and the Beat Lane follow).
//!
//! Automation ids: `board.empty`, `board.hint`, `board.rebuild`, `board.section.<id>` (the title
//! box), `board.beat.<id>`, `board.addBeat.<section>`. To reorder through the control channel drag
//! `board.beat.<id>` onto another card (it goes before that card) or onto `board.addBeat.<section>`
//! (it goes to the end of that section).

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::panels::beat_lane::SECTION_COLORS;
use crate::panels::planner::{clock, field, status_label};
use crate::theme::Tokens;

const COL_W: f32 = 196.0;
const COL_GAP: f32 = 8.0;
const HEAD_H: f32 = 28.0;
const CARD_H: f32 = 84.0;
const CARD_GAP: f32 = 6.0;
const ADD_H: f32 = 24.0;
const NOTE_H: f32 = 20.0;
const BANNER_H: f32 = 30.0;
const AMBER: Color32 = Color32::from_rgb(0xe8, 0xb3, 0x3c);

/// One column as it is laid out: where it is and where its cards are.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub section: u64,
    pub rect: Rect,
    pub cards: Vec<(u64, Rect)>,
}

/// Where a card dropped at `pos` goes: the section and the place in it, counted as if the dragged
/// card were already out of its column (which is what `planner.move` does). The nearest column
/// takes the card; a card goes before every card whose middle is below the pointer.
pub fn drop_slot(columns: &[Column], dragged: u64, pos: Pos2) -> Option<(u64, usize)> {
    let distance = |c: &Column| {
        if pos.x < c.rect.min.x {
            c.rect.min.x - pos.x
        } else if pos.x > c.rect.max.x {
            pos.x - c.rect.max.x
        } else {
            0.0
        }
    };
    let column = columns.iter().min_by(|a, b| distance(a).total_cmp(&distance(b)))?;
    let at = column.cards.iter().filter(|(id, r)| *id != dragged && r.center().y < pos.y).count();
    Some((column.section, at))
}

/// Whether dropping `beat` on (`section`, `at`) leaves the plan as it is.
pub fn is_noop(sections: &[Value], beat: u64, section: u64, at: usize) -> bool {
    for s in sections {
        let ids: Vec<u64> = s["beats"].as_array().into_iter().flatten().filter_map(|b| b["id"].as_u64()).collect();
        if let Some(i) = ids.iter().position(|id| *id == beat) {
            return s["id"].as_u64() == Some(section) && i == at.min(ids.len().saturating_sub(1));
        }
    }
    true
}

/// True when the beats that have been laid on the timeline are no longer in the plan's order
/// there: the markers should follow the plan.
pub fn order_is_stale(sections: &[Value]) -> bool {
    let starts: Vec<i64> = sections.iter().flat_map(|s| s["beats"].as_array().into_iter().flatten()).filter_map(|b| b["link"]["start"].as_i64()).collect();
    starts.windows(2).any(|w| w[1] < w[0])
}

/// The first line of a beat's purpose, for the card.
fn first_line(text: &str) -> &str {
    text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("")
}

fn paint_card(painter: &egui::Painter, rect: Rect, beat: &Value, color: Color32, t: &Tokens, selected: bool, hovered: bool, faded: bool) {
    let alpha = if faded { 0.35 } else { 1.0 };
    let fade = |c: Color32| c.gamma_multiply(alpha);
    painter.rect_filled(rect, 4.0, fade(if selected || hovered { t.hover } else { t.field_bg }));
    painter.rect_filled(Rect::from_min_max(rect.min, pos2(rect.min.x + 4.0, rect.max.y)), 4.0, fade(color));
    if selected {
        painter.rect_stroke(rect, 4.0, Stroke::new(1.5, fade(Color32::WHITE)), StrokeKind::Inside);
    }
    let left = rect.min.x + 12.0;
    let right = rect.max.x - 8.0;
    let length = clock(beat["target"].as_i64().unwrap_or(0));
    let length_galley = painter.layout_no_wrap(length, Tokens::ui(11.0), fade(t.text_dim));
    painter.galley(pos2(right - length_galley.size().x, rect.min.y + 8.0), length_galley.clone(), Color32::WHITE);
    painter.with_clip_rect(Rect::from_min_max(pos2(left, rect.min.y), pos2(right - length_galley.size().x - 6.0, rect.max.y))).text(
        pos2(left, rect.min.y + 8.0),
        Align2::LEFT_TOP,
        beat["name"].as_str().unwrap_or(""),
        Tokens::semibold(12.0),
        fade(t.text),
    );
    painter.text(
        pos2(left, rect.min.y + 26.0),
        Align2::LEFT_TOP,
        status_label(beat["status"].as_str().unwrap_or("idea")),
        Tokens::ui(10.5),
        fade(t.text_faint),
    );
    // intensity: 0 to 10 in tenths
    let intensity = f32::from(beat["intensity"].as_u64().unwrap_or(0).min(100) as u8);
    let track = Rect::from_min_max(pos2(left, rect.min.y + 43.0), pos2(right - 26.0, rect.min.y + 47.0));
    painter.rect_filled(track, 2.0, fade(t.panel_bg));
    painter.rect_filled(Rect::from_min_max(track.min, pos2(track.min.x + track.width() * (intensity / 100.0), track.max.y)), 2.0, fade(AMBER));
    painter.text(pos2(right, track.center().y), Align2::RIGHT_CENTER, format!("{:.1}", intensity / 10.0), Tokens::ui(10.0), fade(t.text_faint));
    let purpose = first_line(beat["purpose"].as_str().unwrap_or(""));
    if !purpose.is_empty() {
        let mut job = egui::text::LayoutJob::simple(purpose.to_string(), Tokens::ui(10.5), fade(t.text_dim), (right - left).max(10.0));
        job.wrap.max_rows = 2;
        let galley = painter.layout_job(job);
        painter.with_clip_rect(rect).galley(pos2(left, rect.min.y + 54.0), galley, Color32::WHITE);
    }
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let got = app.session.execute("planner.get", json!({})).unwrap_or(Value::Null);
    ui.painter().rect_filled(rect, 0.0, t.panel_bg);
    let mut elems: Vec<(String, egui::Response, String)> = Vec::new();
    let mut commands: Vec<(&'static str, Value)> = Vec::new();
    let mut select: Option<u64> = None;
    let mut status: Option<String> = None;
    let sections: Vec<Value> = got["plan"]["sections"].as_array().cloned().unwrap_or_default();
    if got["exists"].as_bool() != Some(true) {
        let r = ui.allocate_rect(rect, Sense::hover());
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, tl!("No plan yet: open Window ▸ Planner"), Tokens::ui(12.0), t.text_faint);
        app.auto.add_response("board.empty", &r, "No plan yet");
        return;
    }
    let stale = order_is_stale(&sections);
    let selected = app.ui.planner.selected_beat;
    let ctx = ui.ctx().clone();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 6.0))));
    let ui = &mut child;
    let viewport_h = rect.height();
    let mut dropped: Option<(u64, Pos2)> = None;
    let mut dragging: Option<(u64, Rect, Pos2)> = None;

    egui::ScrollArea::both().id_salt("beat-board-scroll").auto_shrink([false, false]).show(ui, |ui| {
        let notes_h = NOTE_H + if stale { BANNER_H } else { 0.0 };
        let tallest = sections.iter().map(|s| s["beats"].as_array().map_or(0, Vec::len)).max().unwrap_or(0);
        let body_h = HEAD_H + 6.0 + tallest as f32 * (CARD_H + CARD_GAP) + ADD_H;
        let total = vec2((sections.len() as f32 * (COL_W + COL_GAP)).max(COL_W), (notes_h + body_h).max(viewport_h - 12.0));
        let (area, _) = ui.allocate_exact_size(total, Sense::hover());
        let visible = ui.clip_rect();
        let painter = ui.painter().clone();
        let mut y = area.min.y;
        let hint = painter.text(
            pos2(area.min.x + 2.0, y + NOTE_H / 2.0),
            Align2::LEFT_CENTER,
            tl!("Drag a beat to reorder the plan. Clips on the timeline don't move."),
            Tokens::ui(11.0),
            t.text_faint,
        );
        elems.push((
            "board.hint".into(),
            ui.interact(hint.expand2(vec2(0.0, 2.0)).intersect(visible), egui::Id::new("board-hint"), Sense::hover()),
            "How the board works".into(),
        ));
        y += NOTE_H;
        if stale {
            let band = Rect::from_min_size(pos2(area.min.x, y + 2.0), vec2(total.x.min(560.0), BANNER_H - 6.0));
            painter.rect_filled(band, 4.0, AMBER.gamma_multiply(0.18));
            painter.text(
                band.left_center() + vec2(8.0, 0.0),
                Align2::LEFT_CENTER,
                tl!("The plan changed order; the timeline markers are as before."),
                Tokens::ui(11.5),
                t.text,
            );
            let button = Rect::from_center_size(pos2(band.max.x - 62.0, band.center().y), vec2(110.0, band.height() - 4.0));
            let resp = ui
                .put(button, egui::Button::new(tl!("Rebuild markers")))
                .on_hover_text(tl!("Moves the markers (and the placeholder clips) to follow the plan's order. Clips you have cut are not moved."));
            elems.push(("board.rebuild".into(), resp.clone(), "Rebuild markers".into()));
            if resp.clicked() {
                commands.push(("planner.refreshLinks", json!({})));
            }
            y += BANNER_H;
        }
        let mut columns: Vec<Column> = Vec::new();
        for (ci, sec) in sections.iter().enumerate() {
            let sid = sec["id"].as_u64().unwrap_or(0);
            let color = SECTION_COLORS[ci % SECTION_COLORS.len()];
            let x = area.min.x + ci as f32 * (COL_W + COL_GAP);
            let col = Rect::from_min_max(pos2(x, y), pos2(x + COL_W, area.max.y));
            painter.rect_filled(col, 5.0, t.field_bg.gamma_multiply(0.45));
            painter.rect_filled(Rect::from_min_size(col.min, vec2(COL_W, 3.0)), 2.0, color);
            // the section's title, editable
            let title = Rect::from_min_size(pos2(x + 4.0, y + 5.0), vec2(COL_W - 8.0, HEAD_H - 6.0));
            let (resp, new) = ui
                .scope_builder(egui::UiBuilder::new().max_rect(title), |ui| {
                    field(ui, &format!("board.section.{sid}"), sec["name"].as_str().unwrap_or(""), false, COL_W - 8.0, 1, "", &t)
                })
                .inner;
            elems.push((format!("board.section.{sid}"), resp, "Section name".into()));
            if let Some(name) = new.filter(|n| !n.trim().is_empty()) {
                commands.push(("planner.updateSection", json!({"section": sid, "name": name})));
            }
            let mut cards = Vec::new();
            let mut cy = y + HEAD_H + 6.0;
            let beats: Vec<Value> = sec["beats"].as_array().cloned().unwrap_or_default();
            for b in &beats {
                let bid = b["id"].as_u64().unwrap_or(0);
                let card = Rect::from_min_size(pos2(x + 4.0, cy), vec2(COL_W - 8.0, CARD_H));
                cards.push((bid, card));
                let resp = ui.interact(card.intersect(visible), egui::Id::new(("board-beat", bid)), Sense::click_and_drag());
                let being_dragged = resp.dragged();
                paint_card(&painter, card, b, color, &t, selected == Some(bid), resp.hovered() && !being_dragged, being_dragged);
                if being_dragged {
                    let grab = ctx.input(|i| i.pointer.press_origin()).map_or(vec2(20.0, 12.0), |o| o - card.min);
                    if let Some(p) = ctx.pointer_latest_pos() {
                        dragging = Some((bid, card, p - grab));
                    }
                }
                if resp.clicked() {
                    select = Some(bid);
                }
                if resp.drag_stopped() {
                    dropped = ctx.pointer_latest_pos().map(|p| (bid, p));
                }
                elems.push((format!("board.beat.{bid}"), resp, b["name"].as_str().unwrap_or("").to_string()));
                cy += CARD_H + CARD_GAP;
            }
            let add = Rect::from_min_size(pos2(x + 4.0, cy), vec2(COL_W - 8.0, ADD_H));
            let resp = ui.put(add, egui::Button::new(tl!("Add beat")).fill(Color32::TRANSPARENT));
            elems.push((format!("board.addBeat.{sid}"), resp.clone(), "Add beat".into()));
            if resp.clicked() {
                commands.push(("planner.addBeat", json!({"section": sid, "name": tl!("New beat")})));
            }
            columns.push(Column { section: sid, rect: col, cards });
        }
        // the card being carried, and where it would land
        if let Some((bid, from, at)) = dragging {
            let target = ctx.pointer_latest_pos().filter(|p| rect.contains(*p)).and_then(|p| drop_slot(&columns, bid, p));
            let fg = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("board-drag")));
            if let Some((col, slot)) =
                target.filter(|(s, a)| !is_noop(&sections, bid, *s, *a)).and_then(|(sid, slot)| columns.iter().find(|c| c.section == sid).map(|c| (c, slot)))
            {
                let others: Vec<&(u64, Rect)> = col.cards.iter().filter(|(id, _)| *id != bid).collect();
                let line_y = match others.get(slot) {
                    Some((_, r)) => r.min.y - CARD_GAP / 2.0,
                    None => others.last().map_or(col.rect.min.y + HEAD_H + 3.0, |(_, r)| r.max.y + CARD_GAP / 2.0),
                };
                fg.line_segment([pos2(col.rect.min.x + 6.0, line_y), pos2(col.rect.max.x - 6.0, line_y)], Stroke::new(2.5, t.accent));
            }
            if let Some(beat) = sections.iter().flat_map(|s| s["beats"].as_array().into_iter().flatten()).find(|b| b["id"].as_u64() == Some(bid)) {
                let ci = sections.iter().position(|s| s["beats"].as_array().into_iter().flatten().any(|b| b["id"].as_u64() == Some(bid))).unwrap_or(0);
                paint_card(&fg, Rect::from_min_size(at, from.size()), beat, SECTION_COLORS[ci % SECTION_COLORS.len()], &t, true, false, false);
            }
        }
        if let Some((bid, p)) = dropped {
            let cancelled = ctx.input(|i| i.key_down(egui::Key::Escape));
            let slot = drop_slot(&columns, bid, p).filter(|(s, a)| !is_noop(&sections, bid, *s, *a));
            if let (false, true, Some((sid, at))) = (cancelled, rect.contains(p), slot) {
                commands.push(("planner.move", json!({"beat": bid, "to": sid, "at": at})));
            }
        }
    });
    for (id, r, label) in &elems {
        app.auto.add_response(id, r, label);
    }
    if let Some(b) = select {
        app.ui.planner.selected_beat = Some(b);
        app.ui.planner.selected_line = None;
    }
    for (cmd, p) in commands {
        match app.session.execute(cmd, p) {
            Ok(r) if cmd == "planner.refreshLinks" => {
                status = Some(tlf!(
                    "Markers rebuilt: {m} markers, {p} placeholders",
                    m = r["markers"].as_u64().unwrap_or(0),
                    p = r["placeholders"].as_u64().unwrap_or(0)
                ));
            }
            Ok(_) => {}
            Err(e) => status = Some(e.to_string()),
        }
    }
    if let Some(s) = status {
        app.ui.status = s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(section: u64, x: f32, beats: &[u64]) -> Column {
        let cards = beats.iter().enumerate().map(|(i, id)| (*id, Rect::from_min_size(pos2(x, 40.0 + i as f32 * 80.0), vec2(180.0, 74.0)))).collect();
        Column { section, rect: Rect::from_min_size(pos2(x, 0.0), vec2(196.0, 600.0)), cards }
    }

    #[test]
    fn a_card_goes_before_every_card_whose_middle_is_below_the_pointer() {
        let cols = [col(1, 0.0, &[10, 11, 12]), col(2, 200.0, &[20])];
        // cards are 74 high from y=40, 120, 200: middles at 77, 157, 237
        assert_eq!(drop_slot(&cols, 99, pos2(50.0, 10.0)), Some((1, 0)));
        assert_eq!(drop_slot(&cols, 99, pos2(50.0, 77.0)), Some((1, 0)), "exactly on a middle goes before it");
        assert_eq!(drop_slot(&cols, 99, pos2(50.0, 100.0)), Some((1, 1)));
        assert_eq!(drop_slot(&cols, 99, pos2(50.0, 590.0)), Some((1, 3)));
        assert_eq!(drop_slot(&cols, 99, pos2(250.0, 590.0)), Some((2, 1)));
    }

    #[test]
    fn the_dragged_card_does_not_count_itself() {
        let cols = [col(1, 0.0, &[10, 11, 12])];
        assert_eq!(drop_slot(&cols, 10, pos2(50.0, 590.0)), Some((1, 2)), "last of the other two");
        assert_eq!(drop_slot(&cols, 12, pos2(50.0, 100.0)), Some((1, 1)));
    }

    #[test]
    fn the_nearest_column_takes_a_drop_beside_or_between_columns() {
        let cols = [col(1, 0.0, &[10]), col(2, 200.0, &[])];
        assert_eq!(drop_slot(&cols, 10, pos2(-300.0, 10.0)).map(|s| s.0), Some(1));
        assert_eq!(drop_slot(&cols, 10, pos2(198.0, 10.0)).map(|s| s.0), Some(1), "equally near: the first");
        assert_eq!(drop_slot(&cols, 10, pos2(900.0, 10.0)), Some((2, 0)));
        assert_eq!(drop_slot(&[], 10, pos2(0.0, 0.0)), None);
        let _ = drop_slot(&cols, 10, pos2(f32::NAN, f32::NAN)); // a hostile position does not panic
    }

    #[test]
    fn a_drop_that_changes_nothing_is_ignored() {
        let sections = vec![json!({"id": 1, "beats": [{"id": 10}, {"id": 11}, {"id": 12}]}), json!({"id": 2, "beats": []})];
        assert!(is_noop(&sections, 11, 1, 1), "back where it was");
        assert!(is_noop(&sections, 12, 1, 5), "past the end of its own column is the end");
        assert!(!is_noop(&sections, 11, 1, 0));
        assert!(!is_noop(&sections, 11, 2, 0), "another column is a change");
        assert!(is_noop(&sections, 99, 1, 0), "an unknown beat is nothing to do");
        assert!(is_noop(&[], 1, 1, 0));
    }

    #[test]
    fn markers_are_stale_when_the_laid_out_beats_are_out_of_order() {
        let beat = |id: u64, start: Option<i64>| match start {
            Some(s) => json!({"id": id, "link": {"start": s, "duration": 5, "marker": id}}),
            None => json!({"id": id, "link": null}),
        };
        let ok = vec![json!({"id": 1, "beats": [beat(1, Some(0)), beat(2, None), beat(3, Some(500))]})];
        assert!(!order_is_stale(&ok), "beats without markers are skipped");
        let swapped = vec![json!({"id": 1, "beats": [beat(3, Some(500)), beat(1, Some(0))]})];
        assert!(order_is_stale(&swapped));
        assert!(!order_is_stale(&[]) && !order_is_stale(&[json!({"id": 1})]));
        let tied = vec![json!({"id": 1, "beats": [beat(1, Some(0)), beat(2, Some(0))]})];
        assert!(!order_is_stale(&tied), "two zero-length beats at the same place are in order");
    }
}
