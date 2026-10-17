//! What the Assistant panel is drawn with, in the app's own style: flat icon buttons, the same
//! dark fields and dropdowns as the monitors, small pill toggles, and answers laid out from blocks
//! (paragraphs with a steady line height, lists with a hanging indent, code, tables).

use egui::text::{LayoutJob, TextFormat};
use egui::{Align, Color32, FontFamily, FontId, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::panels::assistant_view::{Block, Span};
use crate::theme::Tokens;

/// Size of answer text. The rest of the app reads 12; a conversation is read for longer, so it is
/// a point larger, with a line height that keeps lines apart without looking double-spaced.
pub const BODY: f32 = 13.0;
pub const SMALL: f32 = 11.0;

/// Line height for text of `size`.
pub fn line_height(size: f32) -> f32 {
    (size * 1.5).round()
}

/// One styled run of text with a fixed line height, so bold, code and plain text share a row.
fn format(font_id: FontId, color: Color32, size: f32) -> TextFormat {
    TextFormat { font_id, color, line_height: Some(line_height(size)), valign: Align::Center, ..Default::default() }
}

/// Spans as one wrapping text, in `color`.
pub fn spans_job(ui: &Ui, spans: &[Span], color: Color32, size: f32, all_bold: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    for span in spans {
        let font = if span.code {
            FontId::new(size - 1.0, FontFamily::Monospace)
        } else if span.bold || all_bold {
            Tokens::semibold(size)
        } else {
            Tokens::ui(size)
        };
        let mut f = format(font, color, size);
        if span.code {
            f.background = Color32::from_white_alpha(14);
        }
        job.append(&span.text, 0.0, f);
    }
    job
}

/// A small pill toggle ("Selection", "Frame"): filled when on, outlined when off, dimmed when it
/// cannot be used.
pub fn pill(ui: &mut Ui, label: &str, on: bool, enabled: bool, t: &Tokens) -> Response {
    ui.add_enabled_ui(enabled, |ui| pill_inner(ui, label, on, enabled, t)).inner
}

fn pill_inner(ui: &mut Ui, label: &str, on: bool, enabled: bool, t: &Tokens) -> Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), Tokens::ui(SMALL), t.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 16.0, 20.0), if enabled { Sense::click() } else { Sense::hover() });
    let (fill, stroke, text) = match (on && enabled, resp.hovered() && enabled) {
        (true, _) => (t.accent, Stroke::NONE, Color32::WHITE),
        (false, true) => (t.hover, Stroke::new(1.0, t.field_border), t.text),
        (false, false) => (Color32::TRANSPARENT, Stroke::new(1.0, t.separator), if enabled { t.text_dim } else { t.text_faint }),
    };
    ui.painter().rect_filled(rect, 10.0, fill);
    if stroke != Stroke::NONE {
        ui.painter().rect_stroke(rect, 10.0, stroke, StrokeKind::Inside);
    }
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, Tokens::ui(SMALL), text);
    resp
}

/// A full-width button for a suggested question: the same dark field as the app's inputs.
pub fn list_button(ui: &mut Ui, label: &str, t: &Tokens) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    ui.painter().rect_filled(rect, 4.0, if resp.hovered() { t.hover } else { t.field_bg });
    ui.painter().rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0, if resp.hovered() { Color32::from_rgb(0x4b, 0x4b, 0x4b) } else { t.field_border }),
        StrokeKind::Inside,
    );
    ui.painter().text(pos2(rect.min.x + 10.0, rect.center().y), egui::Align2::LEFT_CENTER, label, Tokens::ui(12.0), t.text);
    resp
}

/// The round Send button (an arrow), or Stop (a square) while a turn runs.
pub fn round_button(ui: &mut Ui, stop: bool, enabled: bool, t: &Tokens) -> Response {
    ui.add_enabled_ui(enabled, |ui| round_inner(ui, stop, enabled, t)).inner
}

fn round_inner(ui: &mut Ui, stop: bool, enabled: bool, t: &Tokens) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), if enabled { Sense::click() } else { Sense::hover() });
    let fill = match (enabled, resp.is_pointer_button_down_on(), resp.hovered()) {
        (false, ..) => t.field_border,
        (true, true, _) => t.pressed,
        (true, false, true) => t.accent_hover,
        _ => t.accent,
    };
    ui.painter().circle_filled(rect.center(), 13.0, fill);
    let ink = if enabled { Color32::WHITE } else { t.text_faint };
    let c = rect.center();
    if stop {
        ui.painter().rect_filled(egui::Rect::from_center_size(c, vec2(9.0, 9.0)), 1.5, ink);
    } else {
        let s = Stroke::new(1.8, ink);
        ui.painter().line_segment([pos2(c.x, c.y + 5.0), pos2(c.x, c.y - 5.0)], s);
        ui.painter().line_segment([pos2(c.x - 4.5, c.y - 0.5), pos2(c.x, c.y - 5.0)], s);
        ui.painter().line_segment([pos2(c.x + 4.5, c.y - 0.5), pos2(c.x, c.y - 5.0)], s);
    }
    resp
}

/// What an answer needs to turn the times in its text into links: somewhere to register them for
/// automation and to leave the places clicked, and the frame rate that gives `00:00:38:23` its frames.
pub struct Links<'a> {
    pub elems: &'a mut Vec<(String, Response, String)>,
    /// Timeline positions (ticks) the person clicked.
    pub seek: &'a mut Vec<i64>,
    pub rate: filmcraft_time::FrameRate,
    /// Which message these belong to, and how many links it has so far (for their ids).
    pub entry: usize,
    pub next: usize,
}

/// Spans as wrapping text in which every written time (`0:38`, `00:00:38:23`) is a link that moves the
/// playhead there. The text is laid out once, and each time gets a clickable rectangle over its glyphs.
pub fn linked_text(ui: &mut Ui, spans: &[Span], color: Color32, size: f32, all_bold: bool, t: &Tokens, links: &mut Links) {
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    // each time: where it starts and ends in the text (in characters), and its numbers
    let mut found: Vec<(usize, usize, Vec<u32>)> = Vec::new();
    let mut chars = 0usize;
    for span in spans {
        let font = if span.code {
            FontId::new(size - 1.0, FontFamily::Monospace)
        } else if span.bold || all_bold {
            Tokens::semibold(size)
        } else {
            Tokens::ui(size)
        };
        let mut plain = format(font.clone(), color, size);
        if span.code {
            plain.background = Color32::from_white_alpha(14);
        }
        let mut at = 0usize;
        for r in crate::panels::assistant_view::timecodes(&span.text) {
            if span.code {
                break;
            }
            let (before, time) = (span.text.get(at..r.start).unwrap_or(""), span.text.get(r.start..r.end).unwrap_or(""));
            job.append(before, 0.0, plain.clone());
            chars += before.chars().count();
            let mut link = format(font.clone(), t.accent, size);
            link.underline = Stroke::new(1.0, t.accent.gamma_multiply(0.7));
            job.append(time, 0.0, link);
            let n = time.chars().count();
            found.push((chars, chars + n, r.parts));
            chars += n;
            at = r.end;
        }
        let rest = span.text.get(at..).unwrap_or("");
        job.append(rest, 0.0, plain);
        chars += rest.chars().count();
    }
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(rect.min, galley.clone(), color);
    for (from, to, parts) in found {
        let (a, b) = (galley.pos_from_cursor(egui::text::CCursor::new(from)), galley.pos_from_cursor(egui::text::CCursor::new(to)));
        // a time that wraps onto a second line is clickable on its first line
        let right = if (b.min.y - a.min.y).abs() < 1.0 { b.min.x } else { galley.size().x };
        let r = Rect::from_min_max(rect.min + vec2(a.min.x, a.min.y), rect.min + vec2(right.max(a.min.x + 4.0), a.max.y));
        let id = format!("assistant.time.{}.{}", links.entry, links.next);
        let resp = ui.interact(r, egui::Id::new((id.as_str(), "time-link")), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let label = crate::panels::assistant_view::timecode_label(&parts);
        links.elems.push((id, resp.clone(), label));
        links.next += 1;
        if resp.clicked()
            && let Some(ticks) = crate::panels::assistant_view::timecode_ticks(&parts, links.rate)
        {
            links.seek.push(ticks);
        }
    }
}

/// An answer, drawn block by block with even spacing. The times in it are links (see [`linked_text`]).
pub fn answer(ui: &mut Ui, id: (Option<u64>, usize), blocks: &[Block], t: &Tokens, links: &mut Links) {
    ui.spacing_mut().item_spacing.y = 6.0;
    for (n, b) in blocks.iter().enumerate() {
        match b {
            Block::Paragraph(spans) => {
                linked_text(ui, spans, t.text, BODY, false, t, links);
            }
            Block::Heading(spans) => {
                ui.add_space(2.0);
                linked_text(ui, spans, t.text, BODY + 1.0, true, t, links);
            }
            Block::Item { depth, marker, spans } => {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.add_space(*depth as f32 * 16.0 + 4.0);
                    // the marker is laid out like the text beside it, so both sit on the same baseline
                    let mut job = LayoutJob::default();
                    job.append(marker, 0.0, format(Tokens::ui(BODY), t.text_dim, BODY));
                    let galley = ui.fonts_mut(|f| f.layout_job(job));
                    let width = if marker == "\u{2022}" { 10.0 } else { 20.0 };
                    let (r, _) = ui.allocate_exact_size(vec2(width, line_height(BODY)), Sense::hover());
                    ui.painter().galley(pos2(r.max.x - galley.size().x, r.min.y), galley, t.text_dim);
                    linked_text(ui, spans, t.text, BODY, false, t, links);
                });
            }
            Block::Code(code) => {
                egui::Frame::new().fill(t.field_bg).stroke(Stroke::new(1.0, t.field_border)).corner_radius(4.0).inner_margin(egui::Margin::same(8)).show(
                    ui,
                    |ui| {
                        ui.set_width(ui.available_width());
                        let mut job = LayoutJob::default();
                        job.wrap.max_width = ui.available_width();
                        job.append(code, 0.0, format(Tokens::mono(SMALL + 0.5), t.text_dim, SMALL + 0.5));
                        ui.add(egui::Label::new(job).wrap());
                    },
                );
            }
            Block::Table { header, rows } => {
                egui::Frame::new().stroke(Stroke::new(1.0, t.separator)).corner_radius(4.0).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                    egui::Grid::new((id, "table", n)).spacing([16.0, 3.0]).show(ui, |ui| {
                        for cell in header {
                            ui.add(egui::Label::new(spans_job(ui, cell, t.text_dim, SMALL + 0.5, true)));
                        }
                        ui.end_row();
                        for row in rows {
                            for cell in row {
                                linked_text(ui, cell, t.text, SMALL + 1.0, false, t, links);
                            }
                            ui.end_row();
                        }
                    });
                });
            }
            Block::Rule => {
                ui.add_space(2.0);
                ui.separator();
            }
        }
    }
}

/// The small icons under a message.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ActionIcon {
    Copy,
    Retry,
    Edit,
}

fn paint_action_icon(painter: &egui::Painter, c: egui::Pos2, icon: ActionIcon, color: Color32) {
    let stroke = Stroke::new(1.3, color);
    let p = |x: f32, y: f32| pos2(c.x + x, c.y + y);
    match icon {
        ActionIcon::Copy => {
            // a page in front of another: the front one whole, the back one as an L
            painter.rect_stroke(egui::Rect::from_min_max(p(-2.0, -2.0), p(4.5, 4.5)), 1.5, stroke, StrokeKind::Inside);
            painter.line_segment([p(-4.5, 2.0), p(-4.5, -4.5)], stroke);
            painter.line_segment([p(-4.5, -4.5), p(2.0, -4.5)], stroke);
        }
        ActionIcon::Retry => {
            // an open circle with an arrowhead: go round again
            let (r, from, to) = (4.6_f32, 0.6_f32, 5.6_f32);
            let steps = 14;
            let pts: Vec<egui::Pos2> = (0..=steps)
                .map(|i| {
                    let a = from + (to - from) * i as f32 / steps as f32;
                    p(r * a.cos(), r * a.sin())
                })
                .collect();
            painter.add(egui::Shape::line(pts.clone(), stroke));
            if let Some(end) = pts.last() {
                painter.line_segment([*end, pos2(end.x - 3.2, end.y - 0.4)], stroke);
                painter.line_segment([*end, pos2(end.x - 0.6, end.y - 3.2)], stroke);
            }
        }
        ActionIcon::Edit => {
            // a pencil: body, tip and the line it writes on
            painter.line_segment([p(-1.0, 1.0), p(3.6, -3.6)], Stroke::new(2.6, color));
            painter.line_segment([p(-4.2, 4.2), p(-1.0, 1.0)], Stroke::new(1.3, color));
            painter.line_segment([p(0.5, 5.0), p(5.0, 5.0)], stroke);
        }
    }
}

/// A small flat icon button under a message: faint, brighter and lightly filled under the pointer.
pub fn action_icon(ui: &mut Ui, icon: ActionIcon, tip: &str, t: &Tokens) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 20.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    paint_action_icon(ui.painter(), rect.center(), icon, if resp.hovered() { t.text } else { t.text_faint });
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The row of icons under a message. Always takes the same height, so nothing jumps when the icons
/// appear; they are drawn only when `shown`. Returns the row's area and one response per icon
/// (none when hidden).
pub fn action_row(ui: &mut Ui, shown: bool, icons: &[(ActionIcon, &str)], t: &Tokens) -> (egui::Rect, Vec<Response>) {
    let out = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.set_min_height(20.0);
        if shown { icons.iter().map(|(i, tip)| action_icon(ui, *i, tip, t)).collect::<Vec<_>>() } else { Vec::new() }
    });
    (out.response.rect, out.inner)
}
