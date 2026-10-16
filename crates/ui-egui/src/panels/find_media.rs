//! Find Media panel (Window ▸ Find Media; fork roadmap F05.3): ask free stock sites for video,
//! pictures and sound in plain words, look at what comes back with its licence, shortlist what you
//! like, and add a choice to the project. Adding downloads the file into the project's `Sources`
//! bin and records where it came from (see the Credits and Rights panel).
//!
//! The panel is a view over the Source Hub (`hub.*` commands): it holds only what you typed, which
//! results are shortlisted-only, and the pictures it has decoded. Nothing is downloaded until you
//! press Add.
//!
//! Automation ids: `find.query`, `find.search`, `find.kind.video|image|audio`, `find.shortlist`,
//! `find.source.<id>` (each site's state), `find.card.<result id>`, `find.star.<result id>`,
//! `find.get.<result id>`, `find.connections`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use egui::{Align2, Color32, Key, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, pos2, vec2};
use filmcraft_engine::provenance::Status;
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::dock::PanelKind;
use crate::panels::rights;
use crate::theme::Tokens;

const KINDS: [(&str, &str); 3] = [("video", "Video"), ("image", "Photos"), ("audio", "Sound")];
const CARD_W: f32 = 140.0;
const THUMB_H: f32 = 80.0;

struct State {
    query: String,
    kinds: [bool; 3],
    shortlist_only: bool,
    /// The latest `hub.results` and `hub.shortlist`, and when they were read.
    results: Value,
    shortlist: Value,
    read_at: f64,
    textures: HashMap<String, TextureHandle>,
    searched: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            query: String::new(),
            kinds: [true, false, false],
            shortlist_only: false,
            results: Value::Null,
            shortlist: Value::Null,
            read_at: f64::NEG_INFINITY,
            textures: HashMap::new(),
            searched: false,
        }
    }
}

fn kind_label(k: &str) -> &'static str {
    match k {
        "video" => tl!("Video"),
        "image" => tl!("Photos"),
        _ => tl!("Sound"),
    }
}

/// What a result's licence means, the same way the ledger works it out.
fn status_of(c: &Value) -> Status {
    let r = &c["rights"];
    if r["commercial"].as_bool() != Some(true) {
        Status::Risk
    } else if r["attributionRequired"].as_bool() != Some(false) {
        Status::Credit
    } else {
        Status::Clear
    }
}

fn star(p: &egui::Painter, c: egui::Pos2, on: bool, color: Color32) {
    let pts: Vec<egui::Pos2> = (0..10)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
            let r = if i % 2 == 0 { 6.5 } else { 2.8 };
            pos2(c.x + r * a.cos(), c.y + r * a.sin())
        })
        .collect();
    if on {
        p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
    } else {
        p.add(egui::Shape::closed_line(pts, Stroke::new(1.2, color)));
    }
}

fn arrow_down(p: &egui::Painter, c: egui::Pos2, color: Color32) {
    let s = Stroke::new(1.6, color);
    p.line_segment([pos2(c.x, c.y - 5.0), pos2(c.x, c.y + 3.0)], s);
    p.line_segment([pos2(c.x - 3.5, c.y), pos2(c.x, c.y + 3.5)], s);
    p.line_segment([pos2(c.x + 3.5, c.y), pos2(c.x, c.y + 3.5)], s);
    p.line_segment([pos2(c.x - 5.0, c.y + 6.0), pos2(c.x + 5.0, c.y + 6.0)], s);
}

fn seconds_text(s: f64) -> String {
    let n = s.max(0.0) as u64;
    if n >= 3600 { format!("{}:{:02}:{:02}", n / 3600, (n / 60) % 60, n % 60) } else { format!("{}:{:02}", n / 60, n % 60) }
}

/// A texture for a result's picture, decoding it the first time its bytes are there.
fn texture(app: &mut FilmcraftApp, ctx: &egui::Context, st: &mut State, id: &str) -> Option<TextureHandle> {
    if let Some(t) = st.textures.get(id) {
        return Some(t.clone());
    }
    let c = app.session.hub.candidate(id)?;
    let bytes = app.session.hub.thumbnail(&c)?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return None;
    }
    if st.textures.len() > 300 {
        st.textures.clear();
    }
    let tex = ctx.load_texture(format!("find-{id}"), egui::ColorImage::from_rgba_unmultiplied([w, h], img.as_raw()), egui::TextureOptions::LINEAR);
    st.textures.insert(id.to_string(), tex.clone());
    Some(tex)
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let now = ui.input(|i| i.time);
    let shared: Arc<Mutex<State>> =
        ui.data_mut(|d| d.get_temp_mut_or_insert_with(egui::Id::new("find-media-state"), || Arc::new(Mutex::new(State::default()))).clone());
    let mut st = shared.lock().unwrap_or_else(PoisonError::into_inner);
    let running = st.results["running"].as_bool().unwrap_or(false);
    if now - st.read_at > if running { 0.25 } else { 1.0 } {
        st.results = app.session.execute("hub.results", json!({})).unwrap_or(Value::Null);
        st.shortlist = app.session.execute("hub.shortlist", json!({})).unwrap_or(Value::Null);
        st.read_at = now;
    }
    if running {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
    }
    let providers = app.session.execute("hub.providers", json!({})).unwrap_or(Value::Null);
    let available = providers["available"].as_bool().unwrap_or(false);

    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(rect.min + vec2(10.0, 8.0), rect.max - vec2(10.0, 4.0))));
    let ui = &mut child;
    let ctx = ui.ctx().clone();
    let mut elems: Vec<(String, egui::Response, String)> = Vec::new();
    let mut commands: Vec<(&'static str, Value)> = Vec::new();
    let mut open_connections = false;
    let mut open_url: Option<String> = None;

    // ---- the request: words, what kind, and Search ----
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 34.0).max(80.0);
        let r = ui.add(
            egui::TextEdit::singleline(&mut st.query)
                .id(egui::Id::new("find-query"))
                .desired_width(w)
                .hint_text(RichText::new(tl!("Describe what you need…")).color(t.text_faint)),
        );
        elems.push(("find.query".into(), r.clone(), "What to look for".into()));
        let submitted = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let (rc, go) = ui.allocate_exact_size(vec2(26.0, 24.0), Sense::click());
        ui.painter().rect_filled(rc, 4.0, if go.hovered() { t.accent_hover } else { t.accent });
        crate::icons::paint(ui.painter(), Rect::from_center_size(rc.center(), vec2(14.0, 14.0)), crate::icons::Icon::Search, Color32::WHITE);
        let go = go.on_hover_text(tl!("Search"));
        elems.push(("find.search".into(), go.clone(), "Search".into()));
        let q = st.query.trim().to_string();
        if (go.clicked() || submitted) && !q.is_empty() {
            let kinds: Vec<&str> = KINDS.iter().zip(st.kinds).filter(|(_, on)| *on).map(|((k, _), _)| *k).collect();
            commands.push(("hub.search", json!({"text": q, "kinds": kinds})));
            st.searched = true;
            st.shortlist_only = false;
        }
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (n, (k, _)) in KINDS.iter().enumerate() {
            let r = crate::panels::assistant_widgets::pill(ui, kind_label(k), st.kinds[n], true, &t);
            elems.push((format!("find.kind.{k}"), r.clone(), format!("{} {}", kind_label(k), if st.kinds[n] { "on" } else { "off" })));
            if r.clicked() {
                st.kinds[n] = !st.kinds[n];
                if !st.kinds.iter().any(|b| *b) {
                    st.kinds[n] = true;
                }
            }
        }
        let count = st.shortlist["shortlist"].as_array().map_or(0, Vec::len);
        let r = crate::panels::assistant_widgets::pill(ui, &tlf!("Shortlist ({n})", n = count), st.shortlist_only, true, &t);
        elems.push(("find.shortlist".into(), r.clone(), format!("Shortlist, {count} chosen")));
        if r.clicked() {
            st.shortlist_only = !st.shortlist_only;
        }
    });

    // ---- what each site said ----
    let sources: Vec<Value> = st.results["sources"].as_array().cloned().unwrap_or_default();
    let missing_key = providers["providers"].as_array().is_some_and(|p| p.iter().any(|x| x["connected"].as_bool() == Some(false)));
    ui.horizontal_wrapped(|ui| {
        for sv in &sources {
            let id = sv["id"].as_str().unwrap_or("");
            let name =
                providers["providers"].as_array().and_then(|p| p.iter().find(|x| x["id"] == id)).and_then(|x| x["label"].as_str()).unwrap_or(id).to_string();
            let (text, color) = match sv["state"].as_str() {
                Some("searching") => (format!("{name} …"), t.text_faint),
                Some("failed") => (format!("{name}: {}", sv["message"].as_str().unwrap_or("")), rights::color(Status::Credit)),
                _ => (tlf!("{name}: {n} found", name = name, n = sv["found"].as_u64().unwrap_or(0)), t.text_faint),
            };
            let r = ui.label(RichText::new(&text).size(11.0).color(color));
            elems.push((format!("find.source.{id}"), r, text));
        }
    });
    if !available {
        ui.label(RichText::new(tl!("This build cannot reach the internet, so media cannot be searched.")).size(12.0).color(rights::color(Status::Credit)));
    } else if missing_key && !st.searched {
        let r = ui.link(tl!("Add your free Pexels, Pixabay and Freesound keys in Connections"));
        elems.push(("find.connections".into(), r.clone(), "Open Connections".into()));
        open_connections = r.clicked();
    }

    // ---- the grid ----
    let list: Vec<Value> = if st.shortlist_only {
        st.shortlist["shortlist"].as_array().cloned().unwrap_or_default()
    } else {
        st.results["candidates"].as_array().cloned().unwrap_or_default()
    };
    let credit_row = ui.available_height() > 60.0;
    let grid_h = (ui.available_height() - if credit_row { 36.0 } else { 0.0 }).max(60.0);
    egui::ScrollArea::vertical().id_salt("find-scroll").max_height(grid_h).auto_shrink([false, false]).show(ui, |ui| {
        if list.is_empty() {
            let msg = if st.shortlist_only {
                tl!("Nothing shortlisted yet. Press the star on a result to keep it here.")
            } else if st.searched && !running {
                tl!("Nothing found. Try other words, or another kind.")
            } else if !st.searched {
                tl!("Describe the shot or sound you need, for example \"a lone figure walking through fog at dawn\".")
            } else {
                ""
            };
            ui.add_space(10.0);
            ui.add(egui::Label::new(RichText::new(msg).size(12.0).color(t.text_dim)).wrap());
        }
        let cols = ((ui.available_width() + 6.0) / (CARD_W + 6.0)).floor().max(1.0) as usize;
        for row in list.chunks(cols) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for c in row {
                    let id = c["id"].as_str().unwrap_or("").to_string();
                    let (card, resp) = ui.allocate_exact_size(vec2(CARD_W, THUMB_H + 44.0), Sense::click());
                    if !ui.is_rect_visible(card) {
                        continue;
                    }
                    let thumb = Rect::from_min_size(card.min, vec2(CARD_W, THUMB_H));
                    let p = ui.painter();
                    p.rect_filled(thumb, 4.0, t.field_bg);
                    match texture(app, &ctx, &mut st, &id) {
                        Some(tex) => {
                            let ts = tex.size_vec2();
                            let scale = (thumb.width() / ts.x).max(thumb.height() / ts.y);
                            let shown = Rect::from_center_size(thumb.center(), ts * scale).intersect(thumb);
                            let uv = Rect::from_min_max(
                                pos2(0.5 + (shown.min.x - thumb.center().x) / (ts.x * scale), 0.5 + (shown.min.y - thumb.center().y) / (ts.y * scale)),
                                pos2(0.5 + (shown.max.x - thumb.center().x) / (ts.x * scale), 0.5 + (shown.max.y - thumb.center().y) / (ts.y * scale)),
                            );
                            ui.painter().with_clip_rect(thumb).image(tex.id(), shown, uv, Color32::WHITE);
                        }
                        None => {
                            let word = kind_label(c["kind"].as_str().unwrap_or("video"));
                            p.text(thumb.center(), Align2::CENTER_CENTER, word, Tokens::ui(11.0), t.text_faint);
                        }
                    }
                    let p = ui.painter();
                    p.rect_stroke(
                        thumb,
                        4.0,
                        Stroke::new(1.0, if resp.hovered() { Color32::from_rgb(0x4b, 0x4b, 0x4b) } else { t.field_border }),
                        StrokeKind::Inside,
                    );
                    // licence dot, bottom left of the picture
                    let status = status_of(c);
                    p.circle_filled(pos2(thumb.min.x + 10.0, thumb.max.y - 10.0), 5.5, rights::color(status));
                    if let Some(s) = c["seconds"].as_f64() {
                        let g = p.layout_no_wrap(seconds_text(s), Tokens::ui(10.0), Color32::WHITE);
                        let box_ = Rect::from_min_size(pos2(thumb.max.x - g.size().x - 10.0, thumb.max.y - 17.0), vec2(g.size().x + 6.0, 13.0));
                        p.rect_filled(box_, 2.0, Color32::from_black_alpha(170));
                        p.galley(pos2(box_.min.x + 3.0, box_.min.y + 1.0), g, Color32::WHITE);
                    }
                    // shortlist star, top right
                    let star_r = Rect::from_center_size(pos2(thumb.max.x - 13.0, thumb.min.y + 13.0), vec2(22.0, 22.0));
                    let star_resp = ui.interact(star_r, egui::Id::new(("find-star", &id)), Sense::click());
                    let on = c["shortlisted"].as_bool().unwrap_or(false);
                    ui.painter().circle_filled(star_r.center(), 11.0, Color32::from_black_alpha(if star_resp.hovered() { 200 } else { 130 }));
                    star(ui.painter(), star_r.center(), on, if on { Color32::from_rgb(0xe8, 0xb3, 0x3c) } else { Color32::WHITE });
                    elems.push((format!("find.star.{id}"), star_resp.clone(), format!("{} shortlist", if on { "Remove from" } else { "Add to" })));
                    if star_resp.clicked() {
                        commands.push(("hub.shortlist", json!({"id": id, "on": !on})));
                        st.read_at = f64::NEG_INFINITY;
                    }
                    // title, author, and Add
                    let title = c["title"].as_str().filter(|s| !s.is_empty()).unwrap_or(c["source"].as_str().unwrap_or(""));
                    let p = ui.painter();
                    p.with_clip_rect(Rect::from_min_size(pos2(card.min.x, thumb.max.y), vec2(CARD_W - 28.0, 44.0))).text(
                        pos2(card.min.x + 2.0, thumb.max.y + 10.0),
                        Align2::LEFT_CENTER,
                        title,
                        Tokens::ui(11.5),
                        t.text,
                    );
                    p.with_clip_rect(Rect::from_min_size(pos2(card.min.x, thumb.max.y + 18.0), vec2(CARD_W - 28.0, 22.0))).text(
                        pos2(card.min.x + 2.0, thumb.max.y + 29.0),
                        Align2::LEFT_CENTER,
                        c["author"].as_str().unwrap_or(""),
                        Tokens::ui(10.5),
                        t.text_faint,
                    );
                    let get_r = Rect::from_center_size(pos2(card.max.x - 13.0, thumb.max.y + 22.0), vec2(24.0, 24.0));
                    let state = c["acquired"]["state"].as_str();
                    let get = ui.interact(
                        get_r,
                        egui::Id::new(("find-get", &id)),
                        if state == Some("downloading") || state == Some("done") { Sense::hover() } else { Sense::click() },
                    );
                    match state {
                        Some("downloading") => {
                            ui.put(get_r.shrink(5.0), egui::Spinner::new().size(14.0));
                        }
                        Some("done") => {
                            let p = ui.painter();
                            let c0 = get_r.center();
                            p.line_segment([pos2(c0.x - 5.0, c0.y), pos2(c0.x - 1.5, c0.y + 4.0)], Stroke::new(2.0, rights::color(Status::Clear)));
                            p.line_segment([pos2(c0.x - 1.5, c0.y + 4.0), pos2(c0.x + 5.5, c0.y - 4.5)], Stroke::new(2.0, rights::color(Status::Clear)));
                        }
                        _ => {
                            if get.hovered() {
                                ui.painter().rect_filled(get_r, 4.0, t.hover);
                            }
                            arrow_down(
                                ui.painter(),
                                get_r.center(),
                                if state == Some("failed") {
                                    rights::color(Status::Risk)
                                } else if get.hovered() {
                                    t.text
                                } else {
                                    t.text_dim
                                },
                            );
                        }
                    }
                    let tip = match state {
                        Some("done") => tl!("In the project").to_string(),
                        Some("downloading") => tl!("Downloading…").to_string(),
                        Some("failed") => tlf!("Failed: {why}. Click to try again.", why = c["acquired"]["message"].as_str().unwrap_or("")),
                        _ => tl!("Add to the project").to_string(),
                    };
                    let get = get.on_hover_text(tip);
                    elems.push((format!("find.get.{id}"), get.clone(), format!("Add {title} to the project")));
                    if get.clicked() {
                        commands.push(("hub.acquire", json!({"id": id})));
                        st.read_at = f64::NEG_INFINITY;
                    }
                    // the whole card: what it is and what its licence allows
                    let rights_text = format!(
                        "{} · {}",
                        c["rights"]["name"].as_str().unwrap_or(""),
                        match (c["rights"]["commercial"].as_bool(), c["rights"]["attributionRequired"].as_bool()) {
                            (Some(true), Some(false)) => tl!("free to use, no credit needed"),
                            (Some(true), Some(true)) => tl!("free to use, credit required"),
                            (Some(false), _) => tl!("NOT for commercial use"),
                            _ => tl!("terms not stated: check before using"),
                        }
                    );
                    let size = match (c["width"].as_u64(), c["height"].as_u64()) {
                        (Some(w), Some(h)) => format!("\n{w}×{h}"),
                        _ => String::new(),
                    };
                    let note = c["download"]["note"].as_str().filter(|n| !n.is_empty()).map(|n| format!("\n{n}")).unwrap_or_default();
                    let hover = format!("{title}\n{} · {}\n{rights_text}{size}{note}", c["author"].as_str().unwrap_or(""), c["source"].as_str().unwrap_or(""));
                    let resp = resp.on_hover_text(hover);
                    elems.push((format!("find.card.{id}"), resp.clone(), format!("{title}: {rights_text}")));
                    if resp.double_clicked()
                        && let Some(u) = c["sourceUrl"].as_str().filter(|u| !u.is_empty())
                    {
                        open_url = Some(u.to_string());
                    }
                }
            });
        }
    });
    // what the sites ask to be shown next to their results
    if credit_row && !sources.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for sv in &sources {
                let id = sv["id"].as_str().unwrap_or("");
                if let Some(info) = edith_sources::info(id) {
                    let r =
                        ui.add(egui::Label::new(RichText::new(crate::i18n::t(info.credit)).size(10.5).color(t.text_faint).underline()).sense(Sense::click()));
                    elems.push((format!("find.credit.{id}"), r.clone(), info.credit.to_string()));
                    if r.clicked() {
                        open_url = Some(info.credit_url.to_string());
                    }
                }
            }
        });
    }
    drop(st);
    for (id, r, label) in &elems {
        app.auto.add_response(id, r, label);
    }
    for (cmd, p) in commands {
        if let Err(e) = app.session.execute(cmd, p) {
            app.ui.status = e.to_string();
        }
        shared.lock().unwrap_or_else(PoisonError::into_inner).read_at = f64::NEG_INFINITY;
    }
    if open_connections {
        app.show_panel(PanelKind::Connections);
    }
    if let Some(u) = open_url {
        crate::links::open(app, &ctx, &u);
    }
}
