//! Views inside the conversation (fork roadmap F11.4): what the assistant's `show` tool puts between
//! its sentences. Each is drawn from a snapshot of the measurements taken when the tool ran (the
//! view is saved with the thread, so an old answer does not redraw with today's numbers); the model
//! only chose which view and what to point at.
//!
//! - **findings**: a card of the coach's findings, a severity dot, a timecode you can click to jump the
//!   playhead there, and the title.
//! - **timeline**: the edit as its shots in proportion to their length, plan sections behind them and
//!   findings marked; click anywhere to jump the playhead (a mark jumps to exactly its place).
//! - **histogram**: how many shots are in each length band, with the plan's target.
//! - **pace**: cuts a minute along the edit, with the plan's rate drawn across it.
//! - **gauge**: one number against its target and the band around it that is fine.
//! - **palette**: the colours of a measured clip as swatches in proportion.
//! - **pie**: a whole divided up (screen time by shot length, narration, silence).
//!
//! Everything in a snapshot is untrusted (a saved thread can be edited by hand): missing or odd
//! numbers draw as nothing rather than failing, and the number of things drawn is capped.
//!
//! Automation ids (`n` is the entry's number in the conversation): `assistant.view.<n>` (the whole view),
//! `assistant.view.<n>.finding.<id>` (a finding's timecode), `assistant.view.<n>.mark.<id>` (a mark on the
//! timeline), `assistant.view.<n>.shot` (the timeline strip), `assistant.view.<n>.bucket.<i>`.

use egui::{Align2, Color32, Rect, RichText, Sense, Shape, Stroke, Ui, pos2, vec2};
use filmcraft_time::TICKS_PER_SECOND;
use serde_json::Value;

use crate::panels::planner::clock;
use crate::theme::Tokens;

/// Findings drawn in one card, shots in one strip, marks on one strip, buckets in one histogram.
const MAX_ROWS: usize = 24;
const MAX_SHOTS: usize = 1500;
const MAX_MARKS: usize = 60;
const MAX_BUCKETS: usize = 12;
const LOOK: Color32 = Color32::from_rgb(0xe8, 0xb3, 0x3c);
const PROBLEM: Color32 = Color32::from_rgb(0xe5, 0x64, 0x5a);

pub type Elems = Vec<(String, egui::Response, String)>;

fn severity_color(v: &Value) -> Color32 {
    if v.as_str() == Some("problem") { PROBLEM } else { LOOK }
}

fn ticks(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().filter(|f| f.is_finite()).map(|f| f as i64))
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").chars().filter(|c| !c.is_control()).take(300).collect()
}

fn seconds_text(s: f64) -> String {
    if s < 10.0 { format!("{s:.1} s") } else { format!("{s:.0} s") }
}

/// Draws `view` and returns a place in the timeline (ticks) if the person clicked one.
pub fn draw(ui: &mut Ui, view: &Value, n: usize, t: &Tokens, elems: &mut Elems) -> Option<i64> {
    let mut seek = None;
    let frame = egui::Frame::new().fill(t.field_bg).corner_radius(6.0).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if let Some(c) = view["caption"].as_str().filter(|c| !c.is_empty()) {
            ui.label(RichText::new(text(&Value::String(c.to_string()))).size(11.5).strong().color(t.text_dim));
            ui.add_space(4.0);
        }
        match view["view"].as_str() {
            Some("findings") => seek = findings(ui, view, n, t, elems),
            Some("timeline") => seek = timeline(ui, view, n, t, elems),
            Some("histogram") => histogram(ui, view, n, t, elems),
            Some("pace") => pace(ui, view, n, t, elems),
            Some("gauge") => gauge(ui, view, n, t, elems),
            Some("palette") => palette(ui, view, n, t, elems),
            Some("pie") => pie(ui, view, n, t, elems),
            _ => {
                ui.label(RichText::new(tl!("This view cannot be shown.")).size(11.5).color(t.text_faint));
            }
        }
    });
    elems.push((format!("assistant.view.{n}"), frame.response, view["view"].as_str().unwrap_or("view").to_string()));
    seek
}

fn findings(ui: &mut Ui, view: &Value, n: usize, t: &Tokens, elems: &mut Elems) -> Option<i64> {
    let mut seek = None;
    let rows = view["findings"].as_array().map_or(&[][..], Vec::as_slice);
    for f in rows.iter().take(MAX_ROWS) {
        let start = ticks(&f["start"]).unwrap_or(0).max(0);
        let title = text(&f["title"]);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (dot, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
            ui.painter().circle_filled(dot.center(), 4.0, severity_color(&f["severity"]));
            let b = ui
                .add(egui::Button::new(RichText::new(clock(start)).monospace().size(11.5).color(t.accent)).frame(false))
                .on_hover_text(tl!("Jump the playhead here"));
            elems.push((format!("assistant.view.{n}.finding.{}", text(&f["id"])), b.clone(), title.clone()));
            if b.clicked() {
                seek = Some(start);
            }
            ui.add(egui::Label::new(RichText::new(&title).size(12.0).color(t.text)).wrap());
        });
    }
    let total = view["total"].as_u64().unwrap_or(rows.len() as u64);
    if total > rows.len() as u64 {
        ui.label(RichText::new(tlf!("{shown} of {total} findings", shown = rows.len(), total = total)).size(11.0).color(t.text_faint));
    }
    seek
}

fn timeline(ui: &mut Ui, view: &Value, n: usize, t: &Tokens, elems: &mut Elems) -> Option<i64> {
    let (from, to) = (ticks(&view["from"]).unwrap_or(0).max(0), ticks(&view["to"]).unwrap_or(0));
    if to <= from {
        return None;
    }
    let span = (to - from) as f64;
    let w = ui.available_width().max(60.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 58.0), Sense::click());
    let painter = ui.painter_at(rect);
    let x_of = |tick: i64| rect.left() + ((tick - from) as f64 / span).clamp(0.0, 1.0) as f32 * rect.width();
    let sections = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + 12.0));
    let band = Rect::from_min_max(pos2(rect.min.x, rect.min.y + 15.0), pos2(rect.max.x, rect.min.y + 41.0));
    for (i, s) in view["sections"].as_array().map_or(&[][..], Vec::as_slice).iter().take(12).enumerate() {
        let (a, b) = (ticks(&s["start"]).unwrap_or(0), ticks(&s["end"]).unwrap_or(0));
        if b <= a {
            continue;
        }
        let r = Rect::from_min_max(pos2(x_of(a), sections.min.y), pos2(x_of(b), sections.max.y));
        painter.rect_filled(r.shrink2(vec2(0.5, 0.0)), 2.0, t.hover.gamma_multiply(if i % 2 == 0 { 0.9 } else { 0.55 }));
        painter.with_clip_rect(r).text(r.left_center() + vec2(4.0, 0.0), Align2::LEFT_CENTER, text(&s["name"]), Tokens::ui(10.0), t.text_faint);
    }
    painter.rect_filled(band, 3.0, t.panel_bg);
    let mut drawn: Vec<(i64, i64, u64)> = Vec::new();
    for (i, s) in view["shots"].as_array().map_or(&[][..], Vec::as_slice).iter().take(MAX_SHOTS).enumerate() {
        let (a, b) = (ticks(&s[0]).unwrap_or(0), ticks(&s[1]).unwrap_or(0));
        if b <= a {
            continue;
        }
        let r = Rect::from_min_max(pos2(x_of(a), band.min.y), pos2(x_of(b).max(x_of(a) + 1.0), band.max.y));
        painter.rect_filled(r.shrink2(vec2(0.4, 0.0)), 1.5, t.accent.gamma_multiply(if i % 2 == 0 { 0.55 } else { 0.8 }));
        drawn.push((a, b, s[2].as_u64().unwrap_or(0)));
    }
    // findings: a triangle under the strip at each place
    let mut marks: Vec<(i64, Color32, String, Rect, String)> = Vec::new();
    for m in view["marks"].as_array().map_or(&[][..], Vec::as_slice).iter().take(MAX_MARKS) {
        let at = ticks(&m["start"]).unwrap_or(0);
        if at < from || at >= to {
            continue;
        }
        let x = x_of(at);
        let c = severity_color(&m["severity"]);
        let tip = pos2(x, band.max.y + 1.0);
        painter.add(Shape::convex_polygon(vec![tip, pos2(x - 5.0, band.max.y + 10.0), pos2(x + 5.0, band.max.y + 10.0)], c, Stroke::NONE));
        marks.push((at, c, text(&m["id"]), Rect::from_center_size(pos2(x, band.max.y + 7.0), vec2(14.0, 16.0)), text(&m["title"])));
    }
    // clicking: a mark jumps to exactly its place, anything else to where you pointed
    let mut seek = None;
    let pointer = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
    let tick_at = |p: egui::Pos2| from + (f64::from(((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0)) * span) as i64;
    for (at, _, id, mrect, title) in &marks {
        let r = ui.interact(*mrect, egui::Id::new(("assistant-view-mark", n, id.clone())), Sense::click()).on_hover_text(title);
        elems.push((format!("assistant.view.{n}.mark.{id}"), r.clone(), title.clone()));
        if r.clicked() {
            seek = Some(*at);
        }
    }
    if seek.is_none() && resp.clicked() {
        seek = pointer.map(tick_at);
    }
    if let Some(p) = resp.hover_pos().filter(|p| band.contains(*p)) {
        let at = tick_at(p);
        if let Some((a, b, _)) = drawn.iter().find(|(a, b, _)| at >= *a && at < *b) {
            resp.clone().on_hover_text(format!("{} · {}", clock(*a), seconds_text((b - a) as f64 / TICKS_PER_SECOND as f64)));
        }
    }
    elems.push((format!("assistant.view.{n}.shot"), resp, format!("{} shots", drawn.len())));
    ui.horizontal(|ui| {
        ui.label(RichText::new(clock(from)).size(10.5).color(t.text_faint));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let more = if view["truncated"] == true { format!("  ({})", tl!("the first shots only")) } else { String::new() };
            ui.label(RichText::new(format!("{}{more}", clock(to))).size(10.5).color(t.text_faint));
        });
    });
    seek
}

fn histogram(ui: &mut Ui, view: &Value, n: usize, t: &Tokens, elems: &mut Elems) {
    let buckets: Vec<&Value> = view["buckets"].as_array().map_or(Vec::new(), |a| a.iter().take(MAX_BUCKETS).collect());
    if buckets.is_empty() {
        return;
    }
    let counts: Vec<u64> = buckets.iter().map(|b| b["count"].as_u64().unwrap_or(0)).collect();
    let most = counts.iter().copied().max().unwrap_or(0).max(1);
    let mean = view["meanSeconds"].as_f64().filter(|m| m.is_finite());
    let target = view["targetSeconds"].as_f64().filter(|m| m.is_finite() && *m > 0.0);
    let holds = |b: &Value, s: f64| s >= b["lowerSeconds"].as_f64().unwrap_or(0.0) && b["upperSeconds"].as_f64().is_none_or(|u| s < u);
    let w = ui.available_width().max(60.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, 92.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let slot = rect.width() / buckets.len() as f32;
    for (i, b) in buckets.iter().enumerate() {
        let x0 = rect.left() + slot * i as f32;
        let h = 52.0 * counts.get(i).copied().unwrap_or(0) as f32 / most as f32;
        let bar = Rect::from_min_max(pos2(x0 + 4.0, rect.min.y + 66.0 - h - 12.0), pos2(x0 + slot - 4.0, rect.min.y + 54.0));
        let is_mean = mean.is_some_and(|m| holds(b, m));
        painter.rect_filled(bar, 2.0, if is_mean { t.accent } else { t.accent.gamma_multiply(0.6) });
        let c = counts.get(i).copied().unwrap_or(0);
        if c > 0 {
            painter.text(pos2(bar.center().x, bar.min.y - 2.0), Align2::CENTER_BOTTOM, c.to_string(), Tokens::ui(10.5), t.text_dim);
        }
        painter.text(pos2(bar.center().x, rect.min.y + 58.0), Align2::CENTER_TOP, text(&b["label"]), Tokens::ui(10.0), t.text_faint);
        if target.is_some_and(|tg| holds(b, tg)) {
            let c = pos2(bar.center().x, rect.min.y + 78.0);
            painter.add(Shape::convex_polygon(vec![pos2(c.x, c.y - 6.0), pos2(c.x - 5.0, c.y + 2.0), pos2(c.x + 5.0, c.y + 2.0)], LOOK, Stroke::NONE));
        }
        let r =
            ui.interact(Rect::from_min_max(pos2(x0, rect.min.y), pos2(x0 + slot, rect.max.y)), egui::Id::new(("assistant-view-bucket", n, i)), Sense::hover());
        let r = r.on_hover_text(format!("{}: {}", text(&b["label"]), tlf!("{count} shots", count = c)));
        elems.push((format!("assistant.view.{n}.bucket.{i}"), r, format!("{}: {c}", text(&b["label"]))));
    }
    let mut note = Vec::new();
    if let Some(m) = mean {
        note.push(tlf!("average {length}", length = seconds_text(m)));
    }
    if let Some(tg) = target {
        note.push(tlf!("plan: about {length} a shot (▲)", length = seconds_text(tg)));
    }
    if !note.is_empty() {
        ui.label(RichText::new(note.join("  ·  ")).size(11.0).color(t.text_faint));
    }
}

/// How many bars a pace view draws, swatches a palette, slices a pie.
const MAX_BARS: usize = 240;
const MAX_SWATCHES: usize = 8;
const MAX_SLICES: usize = 8;
/// Slice colours: distinct from each other and readable on the dark card.
const SLICE_COLORS: [Color32; 8] = [
    Color32::from_rgb(0x3b, 0x6e, 0xa8),
    Color32::from_rgb(0x4f, 0x8f, 0x6a),
    Color32::from_rgb(0xe8, 0xb3, 0x3c),
    Color32::from_rgb(0x84, 0x58, 0x9b),
    Color32::from_rgb(0x3e, 0x8f, 0x92),
    Color32::from_rgb(0xe5, 0x64, 0x5a),
    Color32::from_rgb(0x9a, 0x6a, 0x3e),
    Color32::from_rgb(0x7a, 0x7f, 0x8a),
];

fn finite(v: &Value) -> Option<f64> {
    v.as_f64().filter(|x| x.is_finite())
}

fn pace(ui: &mut Ui, view: &Value, n: usize, t: &Tokens, elems: &mut Elems) {
    let bars: Vec<f64> = view["cutsPerMinute"].as_array().map_or(Vec::new(), |a| a.iter().take(MAX_BARS).filter_map(finite).map(|v| v.max(0.0)).collect());
    if bars.is_empty() {
        return;
    }
    let target = finite(&view["targetPerMinute"]).filter(|v| *v > 0.0);
    let overall = finite(&view["overall"]);
    let top = bars.iter().copied().fold(0.0f64, f64::max).max(target.unwrap_or(0.0)).max(overall.unwrap_or(0.0)).max(1.0);
    let w = ui.available_width().max(60.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, 78.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let plot = Rect::from_min_max(pos2(rect.min.x, rect.min.y + 4.0), pos2(rect.max.x, rect.max.y - 14.0));
    painter.rect_filled(plot, 3.0, t.panel_bg);
    let slot = plot.width() / bars.len() as f32;
    let y_of = |v: f64| plot.max.y - (v / top) as f32 * (plot.height() - 4.0);
    let bar_seconds = finite(&view["barSeconds"]).unwrap_or(30.0);
    for (i, v) in bars.iter().enumerate() {
        let x0 = plot.min.x + slot * i as f32;
        let r = Rect::from_min_max(pos2(x0 + 0.5, y_of(*v)), pos2(x0 + (slot - 0.5).max(1.0), plot.max.y));
        painter.rect_filled(r, 1.5, t.accent.gamma_multiply(0.75));
        let hit =
            ui.interact(Rect::from_min_max(pos2(x0, plot.min.y), pos2(x0 + slot, plot.max.y)), egui::Id::new(("assistant-view-bar", n, i)), Sense::hover());
        let from = (i as f64 * bar_seconds) as i64 * TICKS_PER_SECOND;
        let hit = hit.on_hover_text(format!("{} · {:.1} {}", clock(from), v, tl!("cuts a minute")));
        elems.push((format!("assistant.view.{n}.bar.{i}"), hit, format!("{v:.1}")));
    }
    if let Some(tg) = target {
        let y = y_of(tg);
        painter.add(Shape::dashed_line(&[pos2(plot.min.x, y), pos2(plot.max.x, y)], Stroke::new(1.5, LOOK), 5.0, 4.0));
    }
    painter.text(pos2(rect.min.x, rect.max.y - 1.0), Align2::LEFT_BOTTOM, "0:00", Tokens::ui(10.0), t.text_faint);
    let end = (bars.len() as f64 * bar_seconds) as i64 * TICKS_PER_SECOND;
    painter.text(pos2(rect.max.x, rect.max.y - 1.0), Align2::RIGHT_BOTTOM, clock(end), Tokens::ui(10.0), t.text_faint);
    let mut note = Vec::new();
    if let Some(o) = overall {
        note.push(tlf!("overall {rate} cuts a minute", rate = format!("{o:.1}")));
    }
    if let Some(tg) = target {
        note.push(tlf!("plan: about {rate} (dashed)", rate = format!("{tg:.1}")));
    }
    if !note.is_empty() {
        ui.label(RichText::new(note.join("  ·  ")).size(11.0).color(t.text_faint));
    }
}

fn gauge(ui: &mut Ui, view: &Value, n: usize, t: &Tokens, elems: &mut Elems) {
    let (Some(value), Some(lo), Some(hi)) = (finite(&view["value"]), finite(&view["scaleMin"]), finite(&view["scaleMax"])) else { return };
    if hi <= lo {
        return;
    }
    let unit = text(&view["unit"]);
    let label = text(&view["label"]);
    let target = finite(&view["target"]);
    ui.horizontal(|ui| {
        ui.label(RichText::new(&label).size(12.0).color(t.text_dim));
        ui.label(RichText::new(format!("{value:.1} {unit}")).size(14.0).strong().color(t.text));
        if let Some(tg) = target {
            ui.label(RichText::new(tlf!("target {value}", value = format!("{tg:.1}"))).size(11.5).color(t.text_faint));
        }
    });
    let w = ui.available_width().max(60.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let x_of = |v: f64| rect.min.x + (((v - lo) / (hi - lo)).clamp(0.0, 1.0) as f32) * rect.width();
    let track = Rect::from_min_max(pos2(rect.min.x, rect.min.y + 8.0), pos2(rect.max.x, rect.min.y + 20.0));
    painter.rect_filled(track, 4.0, t.panel_bg);
    // the band that is fine
    if let (Some(a), Some(b)) = (finite(&view["low"]), finite(&view["high"])) {
        painter.rect_filled(
            Rect::from_min_max(pos2(x_of(a), track.min.y), pos2(x_of(b), track.max.y)),
            3.0,
            Color32::from_rgb(0x4f, 0x8f, 0x6a).gamma_multiply(0.7),
        );
    }
    if let Some(tg) = target {
        painter.line_segment([pos2(x_of(tg), track.min.y - 3.0), pos2(x_of(tg), track.max.y + 3.0)], Stroke::new(1.5, t.text));
    }
    let x = x_of(value);
    painter.add(Shape::convex_polygon(
        vec![pos2(x, track.min.y - 1.0), pos2(x - 5.0, track.min.y - 8.0), pos2(x + 5.0, track.min.y - 8.0)],
        t.accent,
        Stroke::NONE,
    ));
    painter.text(pos2(rect.min.x, rect.max.y), Align2::LEFT_BOTTOM, format!("{lo:.0}"), Tokens::ui(9.5), t.text_faint);
    painter.text(pos2(rect.max.x, rect.max.y), Align2::RIGHT_BOTTOM, format!("{hi:.0}"), Tokens::ui(9.5), t.text_faint);
    elems.push((format!("assistant.view.{n}.gauge"), resp, format!("{label} {value:.1} {unit}")));
}

fn palette(ui: &mut Ui, view: &Value, n: usize, t: &Tokens, elems: &mut Elems) {
    let colours: Vec<&Value> = view["colors"].as_array().map_or(Vec::new(), |a| a.iter().take(MAX_SWATCHES).collect());
    if colours.is_empty() {
        return;
    }
    let name = text(&view["name"]);
    if !name.is_empty() {
        ui.label(RichText::new(name).size(11.0).color(t.text_faint));
    }
    let shares: Vec<f32> = colours.iter().map(|c| finite(&c["share"]).unwrap_or(0.0).clamp(0.0, 1.0) as f32).collect();
    let total: f32 = shares.iter().sum::<f32>().max(f32::MIN_POSITIVE);
    let w = ui.available_width().max(60.0);
    let min = 26.0f32;
    let spare = (w - min * colours.len() as f32).max(0.0);
    let mut x = 0.0f32;
    let (rect, _) = ui.allocate_exact_size(vec2(w, 54.0), Sense::hover());
    for (i, (c, sh)) in colours.iter().zip(&shares).enumerate() {
        let width = min + spare * sh / total;
        let rgb: Vec<u8> =
            c["rgb"].as_array().map_or(Vec::new(), |a| a.iter().map(|v| (finite(v).unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() as u8).collect());
        let (r, g, b) = (rgb.first().copied().unwrap_or(0), rgb.get(1).copied().unwrap_or(0), rgb.get(2).copied().unwrap_or(0));
        let swatch = Rect::from_min_size(pos2(rect.min.x + x, rect.min.y), vec2((width - 2.0).max(1.0), 36.0));
        ui.painter().rect_filled(swatch, 3.0, Color32::from_rgb(r, g, b));
        let hex = format!("#{r:02x}{g:02x}{b:02x}");
        ui.painter().text(
            pos2(swatch.center().x, swatch.max.y + 3.0),
            Align2::CENTER_TOP,
            format!("{hex} · {:.0}%", sh / total * 100.0),
            Tokens::ui(9.5),
            t.text_faint,
        );
        let hit =
            ui.interact(swatch, egui::Id::new(("assistant-view-swatch", n, i)), Sense::hover()).on_hover_text(format!("{hex}, {:.0}%", sh / total * 100.0));
        elems.push((format!("assistant.view.{n}.swatch.{i}"), hit, hex));
        x += width;
    }
}

fn pie(ui: &mut Ui, view: &Value, n: usize, t: &Tokens, elems: &mut Elems) {
    let slices: Vec<(String, f64, String)> = view["slices"].as_array().map_or(Vec::new(), |a| {
        a.iter().take(MAX_SLICES).map(|s| (text(&s["label"]), finite(&s["share"]).unwrap_or(0.0).clamp(0.0, 1.0), text(&s["detail"]))).collect()
    });
    let total: f64 = slices.iter().map(|s| s.1).sum();
    if slices.is_empty() || total <= 0.0 {
        return;
    }
    ui.horizontal(|ui| {
        let size = 92.0;
        let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
        let (c, r) = (rect.center(), size / 2.0 - 2.0);
        let mut angle = -std::f32::consts::FRAC_PI_2;
        for (i, (_, share, _)) in slices.iter().enumerate() {
            let sweep = (share / total) as f32 * std::f32::consts::TAU;
            if sweep <= 0.0 {
                continue;
            }
            // a wedge of at most half a circle is convex, so it is one polygon with no seams; a wider one is
            // drawn as two halves
            let parts = if sweep > std::f32::consts::PI { 2 } else { 1 };
            for k in 0..parts {
                let (a0, a1) = (angle + sweep * k as f32 / parts as f32, angle + sweep * (k + 1) as f32 / parts as f32);
                let steps = (((a1 - a0) / 0.1).ceil() as usize).max(2);
                let mut pts = vec![c];
                pts.extend((0..=steps).map(|m| {
                    let a = a0 + (a1 - a0) * m as f32 / steps as f32;
                    pos2(c.x + r * a.cos(), c.y + r * a.sin())
                }));
                let edge = if parts == 1 { Stroke::new(1.0, t.field_bg) } else { Stroke::NONE };
                ui.painter().add(Shape::convex_polygon(pts, SLICE_COLORS[i % SLICE_COLORS.len()], edge));
            }
            angle += sweep;
        }
        ui.add_space(8.0);
        ui.vertical(|ui| {
            for (i, (label, share, detail)) in slices.iter().enumerate() {
                let line = if detail.is_empty() {
                    format!("{label}  {:.0}%", share / total * 100.0)
                } else {
                    format!("{label}  {:.0}%  ({detail})", share / total * 100.0)
                };
                ui.horizontal(|ui| {
                    let (sw, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                    ui.painter().rect_filled(sw, 2.0, SLICE_COLORS[i % SLICE_COLORS.len()]);
                    let r = ui.label(RichText::new(&line).size(11.5).color(t.text));
                    elems.push((format!("assistant.view.{n}.slice.{i}"), r, line.clone()));
                });
            }
        });
    });
}
