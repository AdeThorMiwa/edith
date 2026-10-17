//! Sound Map panel (Window ▸ Sound Map; fork roadmap F13.4): the sound of the video made visible.
//!
//! The timeline's audio in lanes (narration, music, ambience, effects, and unsorted when something is in
//! it), each drawn where it sounds. **Measure loudness** adds a loudness curve to every lane. Deliberate
//! silences from the sound plan are shaded across the lanes, the findings (no ambience, an ambience
//! that stops, dead air, a room tone that jumps) are marked on the ruler and listed below with a
//! timecode you can click, and a click anywhere on the lanes moves the playhead there.
//!
//! The panel is a view over `sound.map` and the spotting sheet's silences; it holds only the job it
//! started and a one-second cache of the answer.
//!
//! Automation ids: `soundmap.measure`, `soundmap.progress`, `soundmap.cancel`, `soundmap.empty`,
//! `soundmap.lane.<name>`, `soundmap.finding.<n>` (a mark on the ruler), `soundmap.row.<n>` (a line of
//! the list), `soundmap.seek.<n>` (its timecode), `soundmap.unsorted`, `soundmap.error`.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Align2, Color32, Rect, RichText, Sense, Stroke, pos2, vec2};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::theme::Tokens;

const LANE_H: f32 = 48.0;
const LABEL_W: f32 = 118.0;
const RULER_H: f32 = 22.0;
/// The loudness range drawn in a lane, LUFS.
const FLOOR_LUFS: f64 = -60.0;
const CEIL_LUFS: f64 = -10.0;
const MAX_FINDINGS: usize = 40;
const REFRESH_SECONDS: f64 = 1.0;
const LOOK: Color32 = Color32::from_rgb(0xe8, 0xb3, 0x3c);
const PROBLEM: Color32 = Color32::from_rgb(0xe5, 0x64, 0x5a);

#[derive(Default)]
struct State {
    job: Option<u64>,
    error: Option<String>,
    /// The answer last fetched, when and at which revision of the project.
    cache: Option<(f64, u64, Value, Vec<(i64, i64)>)>,
}

type Shared = Arc<Mutex<State>>;

fn lane_color(name: &str) -> Color32 {
    match name {
        "narration" => Color32::from_rgb(0x4f, 0x8c, 0xff),
        "music" => Color32::from_rgb(0xa0, 0x70, 0xd8),
        "ambience" => Color32::from_rgb(0x3f, 0xb6, 0xa8),
        "effects" => Color32::from_rgb(0xe8, 0xa2, 0x3c),
        _ => Color32::from_rgb(0x8a, 0x8f, 0x98),
    }
}

fn lane_label(name: &str) -> &'static str {
    match name {
        "narration" => tl!("Narration"),
        "music" => tl!("Music"),
        "ambience" => tl!("Ambience"),
        "effects" => tl!("Effects"),
        _ => tl!("Unsorted"),
    }
}

fn clock(ticks: i64) -> String {
    super::planner::clock(ticks)
}

/// A step between ruler marks that gives about `want` marks over `seconds`.
fn ruler_step(seconds: f64, want: f64) -> f64 {
    let raw = (seconds / want.max(1.0)).max(0.1);
    [1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0].into_iter().find(|s| *s >= raw).unwrap_or(7200.0)
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let now = ui.input(|i| i.time);
    let shared: Shared = ui.data_mut(|d| d.get_temp_mut_or_insert_with(egui::Id::new("sound-map-state"), || Arc::new(Mutex::new(State::default()))).clone());
    let mut st = shared.lock().unwrap_or_else(PoisonError::into_inner);
    let revision = app.session.revision;
    // a measuring job: its progress, and what it left when it ended
    let mut progress: Option<(f32, String)> = None;
    if let Some(job) = st.job {
        match app.session.jobs.iter().find(|j| j.id == job) {
            Some(j) if j.progress.finished.load(std::sync::atomic::Ordering::Relaxed) => {
                let failed = j.result.lock().unwrap_or_else(PoisonError::into_inner).as_ref().and_then(|r| r.as_ref().err().cloned());
                st.error = failed.filter(|e| e != "stopped");
                st.job = None;
                st.cache = None;
            }
            Some(j) => {
                let v = j.to_json();
                progress = Some((v["progress"].as_f64().unwrap_or(0.0) as f32, v["status"].as_str().unwrap_or("").to_string()));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(400));
            }
            None => st.job = None,
        }
    }
    // the map, fetched again when the project changes and at most once a second otherwise
    let stale = st.cache.as_ref().is_none_or(|(at, rev, _, _)| *rev != revision || now - *at > REFRESH_SECONDS);
    if stale {
        let map = app.session.execute("sound.map", json!({})).unwrap_or(Value::Null);
        // the sheet needs a plan, and asking without one is an error: ask `planner.get` first, which never fails
        let has_plan = app.session.execute("planner.get", json!({})).is_ok_and(|v| v["exists"] == json!(true));
        let silences: Vec<(i64, i64)> = has_plan
            .then(|| app.session.execute("sound.sheet", json!({})).ok())
            .flatten()
            .map(|v| {
                v["beats"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|b| b["cues"].as_array().into_iter().flatten())
                    .filter(|c| c["kind"] == "silence")
                    .filter_map(|c| Some((c["start"].as_i64()?, c["start"].as_i64()?.saturating_add(c["length"].as_i64()?))))
                    .collect()
            })
            .unwrap_or_default();
        st.cache = Some((now, revision, map, silences));
    }
    let (map, silences) = st.cache.as_ref().map(|(_, _, m, s)| (m.clone(), s.clone())).unwrap_or((Value::Null, Vec::new()));
    let playhead = app.session.playhead().0;

    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(rect.min + vec2(10.0, 8.0), rect.max - vec2(10.0, 4.0))));
    let ui = &mut child;
    let mut elems: Vec<(String, egui::Response, String)> = Vec::new();
    let mut seek: Option<i64> = None;
    let mut measure = false;
    let mut cancel = false;

    egui::ScrollArea::vertical().id_salt("sound-map-scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let duration = map["duration"].as_i64().unwrap_or(0);
        if map.is_null() || duration <= 0 {
            let r = ui.label(RichText::new(tl!("Nothing on the audio tracks yet. Put narration, music or ambience on the timeline to see the sound map.")).size(12.0).color(t.text_faint));
            elems.push(("soundmap.empty".into(), r, "No sound on the timeline yet".into()));
            return;
        }
        // ---- the measuring bar
        ui.horizontal(|ui| {
            let measured = map["measured"].as_bool().unwrap_or(false);
            let b = ui.add_enabled(st.job.is_none(), egui::Button::new(if measured { tl!("Measure again") } else { tl!("Measure loudness") }));
            elems.push(("soundmap.measure".into(), b.clone(), "Measure the loudness of each lane".into()));
            measure |= b.clicked();
            ui.label(
                RichText::new(if measured {
                    tl!("Loudness measured: the curves show how loud each lane is.")
                } else {
                    tl!("Not measured yet: lanes show where each sound is. Measuring adds loudness and finds more.")
                })
                .size(11.0)
                .color(t.text_faint),
            );
        });
        if let Some((fraction, status)) = &progress {
            ui.add(egui::ProgressBar::new(*fraction).text(status.as_str()));
            let c = ui.button(tl!("Stop"));
            elems.push(("soundmap.progress".into(), c.clone(), format!("{status} ({:.0}%)", fraction * 100.0)));
            elems.push(("soundmap.cancel".into(), c.clone(), "Stop measuring".into()));
            cancel |= c.clicked();
        }
        if let Some(e) = st.error.clone() {
            let r = ui.add(egui::Label::new(RichText::new(&e).size(11.5).color(LOOK)).wrap());
            elems.push(("soundmap.error".into(), r, e));
        }
        ui.add_space(6.0);
        // ---- the lanes
        let lanes: Vec<&Value> = map["lanes"].as_array().map_or(Vec::new(), |a| a.iter().collect());
        let width = (ui.available_width() - LABEL_W - 4.0).max(120.0);
        let total_h = RULER_H + lanes.len() as f32 * (LANE_H + 4.0);
        let (area, _) = ui.allocate_exact_size(vec2(LABEL_W + width + 4.0, total_h), Sense::hover());
        let painter = ui.painter_at(area);
        let left = area.min.x + LABEL_W;
        let dur_s = duration as f64 / TICKS_PER_SECOND as f64;
        let x_of = |ticks: i64| left + (ticks as f64 / duration as f64).clamp(0.0, 1.0) as f32 * width;
        // ruler
        let step = ruler_step(dur_s, width as f64 / 80.0);
        let mut at = 0.0;
        while at <= dur_s + 1e-6 {
            let x = x_of((at * TICKS_PER_SECOND as f64) as i64);
            painter.line_segment([pos2(x, area.min.y + RULER_H - 5.0), pos2(x, area.min.y + RULER_H)], Stroke::new(1.0, t.separator));
            painter.text(pos2(x + 2.0, area.min.y + 2.0), Align2::LEFT_TOP, clock((at * TICKS_PER_SECOND as f64) as i64), Tokens::ui(9.5), t.text_faint);
            at += step;
        }
        let lanes_top = area.min.y + RULER_H;
        let lanes_bottom = lanes_top + lanes.len() as f32 * (LANE_H + 4.0) - 4.0;
        for (i, lane) in lanes.iter().enumerate() {
            let name = lane["lane"].as_str().unwrap_or("other");
            let col = lane_color(name);
            let top = lanes_top + i as f32 * (LANE_H + 4.0);
            let row = Rect::from_min_size(pos2(left, top), vec2(width, LANE_H));
            painter.rect_filled(row, 3.0, t.field_bg);
            painter.text(pos2(area.min.x, top + 4.0), Align2::LEFT_TOP, lane_label(name), Tokens::semibold(11.5), col);
            let covered = lane["coveredSeconds"].as_f64().unwrap_or(0.0);
            let detail = match lane["medianLufs"].as_f64() {
                Some(l) => tlf!("{l} LUFS typical", l = format!("{l:.0}")),
                None if covered <= 0.0 => tl!("nothing here").to_string(),
                None => tlf!("{p}% of the video", p = format!("{:.0}", lane["share"].as_f64().unwrap_or(0.0) * 100.0)),
            };
            painter.text(pos2(area.min.x, top + 20.0), Align2::LEFT_TOP, detail, Tokens::ui(10.0), t.text_faint);
            let measured_lane = lane["levels"].as_array().is_some_and(|a| !a.is_empty());
            for s in lane["spans"].as_array().into_iter().flatten() {
                let (Some(a), Some(b)) = (s[0].as_i64(), s[1].as_i64()) else { continue };
                let r = Rect::from_min_max(pos2(x_of(a), top + 2.0), pos2(x_of(b).max(x_of(a) + 1.0), top + LANE_H - 2.0));
                painter.rect_filled(r, 2.0, col.gamma_multiply(if measured_lane { 0.22 } else { 0.7 }));
            }
            if let Some(levels) = lane["levels"].as_array() {
                let hop = map["hopSeconds"].as_f64().unwrap_or(0.5).max(0.01);
                let y_of = |l: f64| top + LANE_H - 3.0 - (((l.clamp(FLOOR_LUFS, CEIL_LUFS) - FLOOR_LUFS) / (CEIL_LUFS - FLOOR_LUFS)) as f32) * (LANE_H - 6.0);
                let mut prev: Option<egui::Pos2> = None;
                for (k, v) in levels.iter().enumerate() {
                    let x = x_of(((k as f64 + 0.5) * hop * TICKS_PER_SECOND as f64) as i64);
                    match v.as_f64().filter(|l| l.is_finite()) {
                        Some(l) => {
                            let p = pos2(x, y_of(l));
                            if let Some(q) = prev {
                                painter.line_segment([q, p], Stroke::new(1.4, col));
                            }
                            prev = Some(p);
                        }
                        None => prev = None,
                    }
                }
            }
            // a click or a drag on the lane moves the playhead
            let resp = ui.interact(row, egui::Id::new(("sound-map-lane", name)), Sense::click_and_drag());
            if let Some(p) = resp.interact_pointer_pos().filter(|_| resp.clicked() || resp.dragged()) {
                seek = Some((((p.x - left) / width).clamp(0.0, 1.0) as f64 * duration as f64) as i64);
            }
            let names: Vec<String> = lane["clips"].as_array().into_iter().flatten().take(6).filter_map(|c| c["name"].as_str().map(str::to_string)).collect();
            let resp = if names.is_empty() { resp } else { resp.on_hover_text(names.join("\n")) };
            elems.push((format!("soundmap.lane.{name}"), resp, lane_label(name).to_string()));
        }
        // planned silences, across the lanes
        for (a, b) in &silences {
            let r = Rect::from_min_max(pos2(x_of(*a), lanes_top), pos2(x_of(*b).max(x_of(*a) + 2.0), lanes_bottom));
            painter.rect_filled(r, 0.0, Color32::from_rgb(0xd9, 0x6a, 0x8a).gamma_multiply(0.22));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_rgb(0xd9, 0x6a, 0x8a).gamma_multiply(0.7)), egui::StrokeKind::Inside);
        }
        // the playhead
        let px = x_of(playhead.clamp(0, duration));
        painter.line_segment([pos2(px, area.min.y + 2.0), pos2(px, lanes_bottom)], Stroke::new(1.5, t.accent));
        // findings on the ruler
        let findings: Vec<&Value> = map["findings"].as_array().map_or(Vec::new(), |a| a.iter().take(MAX_FINDINGS).collect());
        for (n, f) in findings.iter().enumerate() {
            let Some(start) = f["start"].as_i64() else { continue };
            let col = if f["severity"] == "problem" { PROBLEM } else { LOOK };
            let c = pos2(x_of(start), area.min.y + RULER_H - 8.0);
            let hit = Rect::from_center_size(c, vec2(14.0, 14.0));
            painter.add(egui::Shape::convex_polygon(vec![pos2(c.x, c.y + 5.0), pos2(c.x - 5.0, c.y - 5.0), pos2(c.x + 5.0, c.y - 5.0)], col, Stroke::NONE));
            let r = ui.interact(hit, egui::Id::new(("sound-map-finding", n)), Sense::click()).on_hover_text(text_of(&f["title"]));
            if r.clicked() {
                seek = Some(start);
            }
            elems.push((format!("soundmap.finding.{n}"), r, text_of(&f["title"])));
        }
        // ---- what is wrong, in words
        ui.add_space(8.0);
        let unsorted = lanes.iter().find(|l| l["lane"] == "other").and_then(|l| l["clips"].as_array().map(Vec::len)).unwrap_or(0);
        if unsorted > 0 {
            let r = ui.add(
                egui::Label::new(
                    RichText::new(tlf!(
                        "{n} sound clips are not sorted into a lane. Mark each as Dialogue, Music, SFX or Ambience in Essential Sound, and the map can tell you what is missing.",
                        n = unsorted
                    ))
                    .size(11.5)
                    .color(t.text_dim),
                )
                .wrap(),
            );
            elems.push(("soundmap.unsorted".into(), r, "Clips not sorted into a lane".into()));
            ui.add_space(4.0);
        }
        if findings.is_empty() {
            ui.label(RichText::new(tl!("Nothing stands out in the sound.")).size(12.0).color(t.text_faint));
        }
        for (n, f) in findings.iter().enumerate() {
            let col = if f["severity"] == "problem" { PROBLEM } else { LOOK };
            let start = f["start"].as_i64().unwrap_or(0);
            let resp = ui
                .horizontal_wrapped(|ui| {
                    ui.label(RichText::new("●").size(10.0).color(col));
                    let link = ui.add(egui::Label::new(RichText::new(clock(start)).size(11.5).strong().color(t.accent)).sense(Sense::click()));
                    elems.push((format!("soundmap.seek.{n}"), link.clone(), clock(start)));
                    if link.clicked() {
                        seek = Some(start);
                    }
                    ui.add(egui::Label::new(RichText::new(text_of(&f["title"])).size(12.0).strong().color(t.text)).wrap());
                })
                .response;
            elems.push((format!("soundmap.row.{n}"), resp, text_of(&f["title"])));
            ui.add(egui::Label::new(RichText::new(text_of(&f["detail"])).size(11.0).color(t.text_dim)).wrap());
            ui.add_space(4.0);
        }
    });
    drop(st);
    for (id, r, label) in &elems {
        app.auto.add_response(id, r, label);
    }
    if let Some(at) = seek {
        app.session.set_playhead(Tick(at.max(0)));
    }
    let mut st = shared.lock().unwrap_or_else(PoisonError::into_inner);
    if measure {
        match app.session.execute("analysis.run", json!({"sound": true, "force": true})) {
            Ok(v) => {
                st.error = None;
                st.job = v["job"].as_u64();
                st.cache = None;
            }
            Err(e) => st.error = Some(e.to_string()),
        }
    }
    if cancel && let Some(job) = st.job {
        let _ = app.session.execute("jobs.cancel", json!({"job": job}));
    }
}

fn text_of(v: &Value) -> String {
    v.as_str().unwrap_or("").chars().filter(|c| !c.is_control()).take(400).collect()
}
