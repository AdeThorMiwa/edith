//! What the Assistant panel is drawn with, in the app's own style: flat icon buttons, the same
//! dark fields and dropdowns as the monitors, small pill toggles, and answers laid out from blocks
//! (paragraphs with a steady line height, lists with a hanging indent, code, tables).

use egui::text::{LayoutJob, TextFormat};
use egui::{Align, Color32, FontFamily, FontId, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

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

/// An answer, drawn block by block with even spacing.
pub fn answer(ui: &mut Ui, id: (Option<u64>, usize), blocks: &[Block], t: &Tokens) {
    ui.spacing_mut().item_spacing.y = 6.0;
    for (n, b) in blocks.iter().enumerate() {
        match b {
            Block::Paragraph(spans) => {
                ui.add(egui::Label::new(spans_job(ui, spans, t.text, BODY, false)).wrap());
            }
            Block::Heading(spans) => {
                ui.add_space(2.0);
                ui.add(egui::Label::new(spans_job(ui, spans, t.text, BODY + 1.0, true)).wrap());
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
                    ui.add(egui::Label::new(spans_job(ui, spans, t.text, BODY, false)).wrap());
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
                                ui.add(egui::Label::new(spans_job(ui, cell, t.text, SMALL + 1.0, false)));
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

/// A small faint text link ("Copy", "Retry"): brighter under the pointer.
pub fn action_link(ui: &mut Ui, label: &str, t: &Tokens) -> Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), Tokens::ui(SMALL), t.text_faint);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + vec2(8.0, 2.0), Sense::click());
    let color = if resp.hovered() { t.text } else { t.text_faint };
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, Tokens::ui(SMALL), color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The row of links under a message. Always takes the same height, so nothing jumps when the links
/// appear; they are drawn only when `shown`. Returns the row's area and one response per link
/// (none when hidden).
pub fn action_row(ui: &mut Ui, shown: bool, labels: &[&str], t: &Tokens) -> (egui::Rect, Vec<Response>) {
    let out = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.set_min_height(16.0);
        if shown { labels.iter().map(|l| action_link(ui, l, t)).collect::<Vec<_>>() } else { Vec::new() }
    });
    (out.response.rect, out.inner)
}
