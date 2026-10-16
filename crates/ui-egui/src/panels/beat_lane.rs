//! Beat lane (fork roadmap F09.2): the plan's beats as coloured ranges in a strip directly above the
//! timeline, on the timeline's own scale (same zoom and scroll, same header width), so a beat sits
//! above the clips it covers. A line through the beats shows the intensity you planned (0 to 10).
//! For a beat that has been laid on the timeline (`planner.buildSkeleton`) a thin bar under it
//! shows the planned length against what the marker now covers, amber when they are more than 5%
//! apart.
//!
//! Click a beat to move the playhead to its start and select it (the A/V Script follows). It reads
//! the same `planner.lines` data as every other planner view and changes nothing itself.
//!
//! Automation ids: `lane.beat.<id>` (one per visible beat), `lane.empty` (when there is no plan).

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::theme::Tokens;

/// A beat whose actual length is this much off its planned length is flagged.
const OFF: f64 = 0.05;
const SECTION_COLORS: [Color32; 6] = [
    Color32::from_rgb(0x3b, 0x6e, 0xa8),
    Color32::from_rgb(0x4f, 0x8f, 0x6a),
    Color32::from_rgb(0x9a, 0x6a, 0x3e),
    Color32::from_rgb(0x84, 0x58, 0x9b),
    Color32::from_rgb(0x3e, 0x8f, 0x92),
    Color32::from_rgb(0x9b, 0x56, 0x62),
];

/// Pixels from the left of the lane for time `t`, given the timeline's scroll (seconds), zoom
/// (pixels per second) and header width: the same formula the timeline uses.
pub fn x_of(left: f32, header_w: f32, scroll: f64, pps: f64, t: Tick) -> f32 {
    left + header_w + ((t.0 as f64 / TICKS_PER_SECOND as f64 - scroll) * pps) as f32
}

/// The time at pixel `x` (the inverse of [`x_of`]); zero for a zoom of nothing.
pub fn tick_at(left: f32, header_w: f32, scroll: f64, pps: f64, x: f32) -> Tick {
    if pps <= 0.0 || !pps.is_finite() {
        return Tick::ZERO;
    }
    Tick::from_seconds_f64(scroll + f64::from(x - left - header_w) / pps)
}

/// Whether a beat's actual length is more than [`OFF`] away from its planned length.
pub fn is_off(actual: i64, planned: i64) -> bool {
    if planned <= 0 {
        return actual > 0;
    }
    ((actual - planned).abs() as f64) / (planned as f64) > OFF
}

/// One beat as the lane draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub beat: u64,
    pub name: String,
    pub purpose: String,
    pub section: usize,
    pub start: i64,
    /// Where the coloured range ends: the marker's end when laid on the timeline, else the planned end.
    pub end: i64,
    pub planned: i64,
    pub actual: Option<i64>,
    /// Intensity 0 to 10.
    pub intensity: f64,
}

/// The places of every beat, from `planner.lines`.
pub fn places(lines: &Value) -> Vec<Place> {
    let mut sections: Vec<u64> = Vec::new();
    let mut out = Vec::new();
    for b in lines["beats"].as_array().into_iter().flatten() {
        let sec = b["section"].as_u64().unwrap_or(0);
        if !sections.contains(&sec) {
            sections.push(sec);
        }
        let planned = b["targetTicks"].as_i64().unwrap_or(0).max(0);
        let actual = b["actualTicks"].as_i64().map(|a| a.max(0));
        let linked = b["link"].is_object();
        let start = if linked { b["link"]["start"].as_i64().unwrap_or(0) } else { b["plannedStartTicks"].as_i64().unwrap_or(0) }.max(0);
        let length = if linked { actual.unwrap_or(planned) } else { planned };
        out.push(Place {
            beat: b["beat"].as_u64().unwrap_or(0),
            name: b["name"].as_str().unwrap_or("").to_string(),
            purpose: b["purpose"].as_str().unwrap_or("").to_string(),
            section: sections.iter().position(|s| *s == sec).unwrap_or(0),
            start,
            end: start.saturating_add(length),
            planned,
            actual: if linked { actual } else { None },
            intensity: f64::from(b["intensity"].as_u64().unwrap_or(0).min(100) as u32) / 10.0,
        });
    }
    out
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let painter = ui.painter().clone();
    painter.rect_filled(rect, 0.0, t.panel_bg);
    // only ask when there is a plan: a command that fails is logged as an error event, and this
    // panel is open all the time
    let lines = if filmcraft_engine::planner::get(&app.session.project).is_some() {
        app.session.execute("planner.lines", json!({})).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let v = app.ui.timeline.clone();
    let header_w = v.header_w;
    // the label block on the left lines up with the timeline's track headers
    let head = Rect::from_min_max(rect.min, pos2(rect.min.x + header_w, rect.max.y));
    painter.rect_filled(head, 0.0, t.field_bg);
    let mut elems: Vec<(String, egui::Response, String)> = Vec::new();
    if lines["beats"].as_array().is_none_or(Vec::is_empty) {
        painter.text(pos2(head.min.x + 10.0, head.center().y), Align2::LEFT_CENTER, tl!("Plan"), Tokens::semibold(11.5), t.text_faint);
        let r = ui.allocate_rect(Rect::from_min_max(pos2(rect.min.x + header_w, rect.min.y), rect.max), Sense::hover());
        painter.text(
            pos2(rect.min.x + header_w + 12.0, rect.center().y),
            Align2::LEFT_CENTER,
            tl!("No plan yet: open Window ▸ Planner"),
            Tokens::ui(11.5),
            t.text_faint,
        );
        elems.push(("lane.empty".into(), r, "No plan yet".into()));
        for (id, r, label) in &elems {
            app.auto.add_response(id, r, label);
        }
        return;
    }
    painter.text(pos2(head.min.x + 10.0, head.min.y + 14.0), Align2::LEFT_CENTER, tl!("Plan"), Tokens::semibold(11.5), t.text);
    let planned = lines["plannedTicks"].as_i64().unwrap_or(0);
    let target = lines["targetTicks"].as_i64().unwrap_or(0);
    painter.text(
        pos2(head.min.x + 10.0, head.min.y + 30.0),
        Align2::LEFT_CENTER,
        format!("{} / {}", crate::panels::planner::clock(planned), crate::panels::planner::clock(target)),
        Tokens::ui(10.5),
        if planned == target { t.text_dim } else { Color32::from_rgb(0xe8, 0xb3, 0x3c) },
    );
    let places = places(&lines);
    let lane = Rect::from_min_max(pos2(rect.min.x + header_w, rect.min.y), rect.max);
    let clip = painter.with_clip_rect(lane);
    let h = rect.height();
    let curve_top = rect.min.y + 6.0;
    let curve_bottom = rect.min.y + (h * 0.45).max(14.0);
    let band = Rect::from_min_max(pos2(lane.min.x, rect.min.y + h * 0.5), pos2(lane.max.x, rect.max.y - 10.0));
    let x = |ticks: i64| x_of(rect.min.x, header_w, v.scroll, v.pps, Tick(ticks));
    let selected = app.ui.planner.selected_beat;
    let mut seek: Option<(u64, i64)> = None;
    // the intensity curve: a point at the middle of each beat's range
    let curve: Vec<Pos2> = places
        .iter()
        .map(|p| {
            let mid = p.start / 2 + p.end / 2;
            pos2(x(mid), curve_bottom - (curve_bottom - curve_top) * (p.intensity as f32 / 10.0))
        })
        .collect();
    if curve.len() > 1 {
        clip.add(egui::Shape::line(curve.clone(), Stroke::new(1.5, Color32::from_rgba_unmultiplied(0xe8, 0xb3, 0x3c, 200))));
    }
    for c in &curve {
        clip.circle_filled(*c, 2.0, Color32::from_rgb(0xe8, 0xb3, 0x3c));
    }
    for p in &places {
        let (x0, x1) = (x(p.start), x(p.end));
        if x1 < lane.min.x - 1.0 || x0 > lane.max.x + 1.0 {
            continue;
        }
        let r = Rect::from_min_max(pos2(x0, band.min.y), pos2(x1.max(x0 + 2.0), band.max.y));
        let color = SECTION_COLORS[p.section % SECTION_COLORS.len()];
        let visible = r.intersect(lane);
        let resp = ui.interact(visible, egui::Id::new(("lane-beat", p.beat)), Sense::click());
        let on = resp.hovered() || selected == Some(p.beat);
        clip.rect_filled(r, 3.0, if on { color } else { color.gamma_multiply(0.8) });
        if selected == Some(p.beat) {
            clip.rect_stroke(r, 3.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Inside);
        }
        if r.width() > 34.0 {
            clip.with_clip_rect(r.shrink2(vec2(3.0, 0.0))).text(
                r.left_center() + vec2(5.0, 0.0),
                Align2::LEFT_CENTER,
                &p.name,
                Tokens::ui(11.0),
                Color32::WHITE,
            );
        }
        // planned against actual, for a beat laid on the timeline
        if let Some(actual) = p.actual {
            let bar_end = x(p.start.saturating_add(p.planned));
            let bar = Rect::from_min_max(pos2(x0, band.max.y + 2.0), pos2(bar_end.max(x0 + 1.0), band.max.y + 5.0));
            let amber = is_off(actual, p.planned);
            clip.rect_filled(bar, 1.0, if amber { Color32::from_rgb(0xe8, 0xb3, 0x3c) } else { Color32::from_rgba_unmultiplied(255, 255, 255, 110) });
        }
        let tip = match p.actual {
            Some(a) => tlf!(
                "{name}: planned {planned}, on the timeline {actual}. {purpose}",
                name = p.name,
                planned = crate::panels::planner::clock(p.planned),
                actual = crate::panels::planner::clock(a),
                purpose = p.purpose
            ),
            None => tlf!("{name}: planned {planned}. {purpose}", name = p.name, planned = crate::panels::planner::clock(p.planned), purpose = p.purpose),
        };
        let resp = resp.on_hover_text(tip);
        elems.push((format!("lane.beat.{}", p.beat), resp.clone(), p.name.clone()));
        if resp.clicked() {
            seek = Some((p.beat, p.start));
        }
    }
    for (id, r, label) in &elems {
        app.auto.add_response(id, r, label);
    }
    if let Some((beat, start)) = seek {
        app.ui.planner.selected_beat = Some(beat);
        app.ui.planner.selected_line = None;
        app.session.set_playhead(Tick(start));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lane_uses_the_timelines_own_time_to_pixel_formula_and_it_inverts() {
        let (left, header, scroll, pps) = (100.0f32, 180.0f32, 2.5f64, 40.0f64);
        let one = |s: f64| Tick((s * TICKS_PER_SECOND as f64) as i64);
        assert_eq!(x_of(left, header, scroll, pps, one(2.5)), 280.0, "the scroll position is at the start of the content");
        assert_eq!(x_of(left, header, scroll, pps, one(3.5)), 320.0);
        assert!(x_of(left, header, scroll, pps, one(0.0)) < 280.0, "before the scroll position is off to the left");
        for s in [0.0, 1.0, 17.25, 600.0] {
            let x = x_of(left, header, scroll, pps, one(s));
            assert!((tick_at(left, header, scroll, pps, x).0 - one(s).0).abs() < TICKS_PER_SECOND / 20, "{s}");
        }
        for bad in [0.0, -5.0, f64::NAN, f64::INFINITY] {
            let _ = tick_at(left, header, scroll, bad, 300.0);
            let _ = x_of(left, header, scroll, bad, one(1.0));
        }
        assert_eq!(tick_at(left, header, scroll, 0.0, 300.0), Tick::ZERO);
    }

    #[test]
    fn a_beat_is_flagged_when_its_actual_length_is_more_than_five_percent_off() {
        assert!(!is_off(100, 100) && !is_off(104, 100) && !is_off(96, 100));
        assert!(is_off(106, 100) && is_off(94, 100) && is_off(200, 100) && is_off(0, 100));
        assert!(!is_off(0, 0) && is_off(5, 0), "nothing planned and nothing there is fine");
        assert!(!is_off(i64::MAX, i64::MAX));
    }

    #[test]
    fn places_come_from_the_plan_and_prefer_the_laid_out_range() {
        let lines = json!({"beats": [
            {"beat": 1, "section": 10, "name": "A", "purpose": "p", "targetTicks": 100, "plannedStartTicks": 0, "link": null, "actualTicks": null, "intensity": 80},
            {"beat": 2, "section": 10, "name": "B", "purpose": "", "targetTicks": 200, "plannedStartTicks": 100, "link": {"start": 5000, "duration": 200, "marker": 7}, "actualTicks": 150, "intensity": 55},
            {"beat": 3, "section": 11, "name": "C", "targetTicks": 50, "plannedStartTicks": 300, "link": null, "actualTicks": null, "intensity": 250},
        ]});
        let p = places(&lines);
        assert_eq!(p.len(), 3);
        assert_eq!((p[0].start, p[0].end, p[0].actual, p[0].section, p[0].intensity), (0, 100, None, 0, 8.0));
        assert_eq!(
            (p[1].start, p[1].end, p[1].planned, p[1].actual),
            (5000, 5150, 200, Some(150)),
            "a laid-out beat sits where its marker is, as long as it is"
        );
        assert_eq!((p[2].section, p[2].intensity), (1, 10.0), "sections are numbered in order and an intensity is capped at 10");
        assert!(places(&Value::Null).is_empty() && places(&json!({"beats": 5})).is_empty());
        // hostile numbers do not overflow
        let hostile = json!({"beats": [{"beat": 1, "section": 1, "targetTicks": i64::MAX, "link": {"start": i64::MAX, "duration": 1, "marker": 1}, "actualTicks": i64::MAX, "intensity": u64::MAX}]});
        let h = places(&hostile);
        assert!(h[0].end >= h[0].start);
    }
}
