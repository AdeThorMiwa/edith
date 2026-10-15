//! Connections panel (Window ▸ Connections; fork roadmap F02): the Offline switch, every service
//! Edith can use with whether a key is stored, a masked field to paste one, and the redacted
//! request log. Also the Offline indicator at the right of the status bar.
//!
//! Keys go straight from the masked field to the OS keychain (`engine::net_commands::save_secret`,
//! which is not a command). They are never part of `ui.inspect` state: the field's text lives in
//! egui's temporary memory only, and no automation label contains it.
//!
//! Services that sign in through their own page (OpenRouter) get a **Connect** button: it opens the
//! provider's page, shows "Waiting for the browser…" with Cancel, and ends with the key in the
//! keychain (the key never passes through this panel). Pasting a key stays available.
//!
//! Automation ids: `connections.offline`, `connections.<service>.key` / `.save` / `.remove` /
//! `.getKey` / `.connect` / `.cancel` / `.reopen` / `.disconnect`, `connections.log`,
//! `connections.log.clear`; status bar: `status.offline`.

use std::sync::Arc;

use egui::{Align2, Color32, Rect, RichText, Sense, vec2};
use filmcraft_engine::net_commands::connect::{self, Attempt};
use filmcraft_engine::net_commands::probe::{self, Probe};
use filmcraft_engine::net_commands::{self as net, Kind, Outcome, Service};
use filmcraft_engine::panels::Level;
use serde_json::json;

use crate::FilmcraftApp;
use crate::theme::Tokens;

/// How long a keychain answer is reused before the panel asks again (seconds).
const REFRESH_SECS: f64 = 5.0;
/// Requests shown in the log.
const LOG_ROWS: usize = 50;
/// The warning amber the Events panel uses.
const WARN: Color32 = Color32::from_rgb(0xe8, 0xb3, 0x3c);

struct Cache {
    at: f64,
    /// [`connect::changes`] when this was read: a sign-in starting or ending makes it stale.
    changes: u64,
    statuses: Vec<(&'static Service, Option<bool>)>,
    problem: Option<String>,
}

enum Action {
    Offline(bool),
    Save(&'static Service, String),
    Remove(&'static Service),
    Connect(&'static Service),
    Probe(&'static Service),
    CancelConnect(u64),
    Reopen(String),
    Disconnect(&'static Service),
    GetKey(&'static str),
    ClearLog,
}

fn cache_id() -> egui::Id {
    egui::Id::new("connections-cache")
}

fn key_id(service: &str) -> egui::Id {
    egui::Id::new(("connections-key", service.to_string()))
}

/// The stored-state of every service, from the cache while it is fresh (the keychain is asked
/// once per refresh, never once per frame).
fn statuses(ui: &egui::Ui, now: f64) -> Arc<Cache> {
    let changes = connect::changes();
    if let Some(c) = ui.data(|d| d.get_temp::<Arc<Cache>>(cache_id()))
        && now - c.at <= REFRESH_SECS
        && c.changes == changes
    {
        return c;
    }
    let (list, problem) = net::service_statuses();
    let c = Arc::new(Cache { at: now, changes, statuses: list.into_iter().map(|s| (s.service, s.stored)).collect(), problem });
    ui.data_mut(|d| d.insert_temp(cache_id(), c.clone()));
    c
}

fn kind_label(k: Kind) -> String {
    match k {
        Kind::SignIn => tl!("Sign in").to_string(),
        Kind::ApiKey => tl!("API key").to_string(),
        Kind::Local => tl!("On this computer").to_string(),
    }
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let now = ui.input(|i| i.time);
    let cache = statuses(ui, now);
    let secrets = net::secrets_available();
    let offline = net::is_offline();
    let requests = net::recent_requests(LOG_ROWS);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 6.0))));
    let ui = &mut child;
    let mut elems: Vec<(String, egui::Response, String)> = Vec::new();
    let mut actions: Vec<Action> = Vec::new();
    egui::ScrollArea::vertical().id_salt("connections-scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.label(RichText::new(tl!("Network")).strong().color(t.text));
        let mut off = offline;
        let r = ui.checkbox(&mut off, tl!("Offline mode: make no network requests"));
        elems.push(("connections.offline".into(), r.clone(), format!("Offline mode {}", if offline { "on" } else { "off" })));
        if r.changed() {
            actions.push(Action::Offline(off));
        }
        ui.label(
            RichText::new(tl!("While Offline is on, everything that needs the internet stops at once. Local features keep working."))
                .size(11.0)
                .color(t.text_faint),
        );
        ui.add_space(10.0);

        ui.label(RichText::new(tl!("Services")).strong().color(t.text));
        if !secrets {
            ui.label(RichText::new(tl!("This build has no keychain support, so keys cannot be saved.")).color(WARN));
        } else if let Some(p) = &cache.problem {
            ui.label(RichText::new(tlf!("The keychain cannot be used: {why}", why = p)).color(WARN));
        }
        for (service, stored) in &cache.statuses {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
                let color = if *stored == Some(true) { egui::Color32::from_rgb(0x4c, 0xaf, 0x6a) } else { t.text_faint };
                ui.painter().circle_filled(dot.center(), 4.0, color);
                ui.label(RichText::new(service.label).color(t.text));
                let mut tag = kind_label(service.kind);
                if service.free {
                    tag.push_str(" · ");
                    tag.push_str(tl!("free option"));
                }
                ui.label(RichText::new(tag).size(11.0).color(t.text_dim));
                if *stored == Some(true) {
                    ui.label(RichText::new(tl!("Connected")).size(11.0).color(color));
                }
            });
            ui.label(RichText::new(service.note).size(11.0).color(t.text_faint));
            if service.kind == Kind::Local {
                // A model server on this computer: nothing to store, only a button to look for it.
                let found = probe::state(service.id);
                ui.horizontal(|ui| {
                    let r = ui.add_enabled(found != Some(Probe::Checking), egui::Button::new(tl!("Check")));
                    elems.push((format!("connections.{}.check", service.id), r.clone(), format!("Check for {}", service.label)));
                    if r.clicked() {
                        actions.push(Action::Probe(service));
                    }
                    match &found {
                        Some(Probe::Checking) => {
                            ui.label(RichText::new(tl!("Checking…")).size(11.0).color(t.text_dim));
                        }
                        Some(Probe::Up { models }) => {
                            ui.label(
                                RichText::new(tlf!("Running · models: {count}", count = models)).size(11.0).color(egui::Color32::from_rgb(0x4c, 0xaf, 0x6a)),
                            );
                        }
                        Some(Probe::Down(why)) => {
                            ui.label(RichText::new(why).size(11.0).color(WARN));
                        }
                        None => {}
                    }
                });
                continue;
            }
            let (Some(_), true) = (service.secret, secrets) else { continue };
            let attempt = connect::attempt(service.id);
            if service.kind == Kind::SignIn {
                ui.horizontal(|ui| match &attempt {
                    Some(Attempt::Waiting { job, url }) => {
                        ui.spinner();
                        ui.label(RichText::new(tl!("Waiting for the browser…")).color(t.text_dim));
                        let r = ui.button(tl!("Cancel"));
                        elems.push((format!("connections.{}.cancel", service.id), r.clone(), format!("Cancel signing in to {}", service.label)));
                        if r.clicked() {
                            actions.push(Action::CancelConnect(*job));
                        }
                        let r = ui.link(tl!("Open the page again"));
                        elems.push((format!("connections.{}.reopen", service.id), r.clone(), format!("Open the {} page again", service.label)));
                        if r.clicked() {
                            actions.push(Action::Reopen(url.clone()));
                        }
                    }
                    other => {
                        if *stored == Some(true) {
                            let r = ui.button(tl!("Disconnect"));
                            elems.push((format!("connections.{}.disconnect", service.id), r.clone(), format!("Disconnect {}", service.label)));
                            if r.clicked() {
                                actions.push(Action::Disconnect(service));
                            }
                        } else {
                            let r = ui.button(tl!("Connect"));
                            elems.push((format!("connections.{}.connect", service.id), r.clone(), format!("Connect {}", service.label)));
                            if r.clicked() {
                                actions.push(Action::Connect(service));
                            }
                        }
                        if let Some(Attempt::Failed(why)) = other {
                            ui.label(RichText::new(tlf!("Sign-in did not finish: {why}", why = why)).size(11.0).color(WARN));
                        }
                    }
                });
            }
            ui.horizontal(|ui| {
                let mut buf: String = ui.data(|d| d.get_temp(key_id(service.id))).unwrap_or_default();
                let r = ui.add(egui::TextEdit::singleline(&mut buf).password(true).hint_text(tl!("Paste a key")).desired_width(240.0));
                ui.data_mut(|d| d.insert_temp(key_id(service.id), buf.clone()));
                elems.push((format!("connections.{}.key", service.id), r.clone(), format!("{} key", service.label)));
                let r = ui.add_enabled(!buf.trim().is_empty(), egui::Button::new(tl!("Save")));
                elems.push((format!("connections.{}.save", service.id), r.clone(), format!("Save {} key", service.label)));
                if r.clicked() {
                    actions.push(Action::Save(service, buf));
                }
                if *stored == Some(true) && service.kind != Kind::SignIn {
                    let r = ui.button(tl!("Remove"));
                    elems.push((format!("connections.{}.remove", service.id), r.clone(), format!("Remove {} key", service.label)));
                    if r.clicked() {
                        actions.push(Action::Remove(service));
                    }
                }
                if let Some(url) = service.key_url {
                    let r = ui.link(tl!("Get a key"));
                    elems.push((format!("connections.{}.getKey", service.id), r.clone(), format!("Get a {} key", service.label)));
                    if r.clicked() {
                        actions.push(Action::GetKey(url));
                    }
                }
            });
        }

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(tl!("Recent requests")).strong().color(t.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let r = ui.button(tl!("Clear"));
                elems.push(("connections.log.clear".into(), r.clone(), "Clear request log".into()));
                if r.clicked() {
                    actions.push(Action::ClearLog);
                }
            });
        });
        let top = ui.cursor().min;
        if requests.is_empty() {
            ui.label(RichText::new(tl!("No requests yet.")).color(t.text_faint));
        }
        for e in requests.iter().rev() {
            let result = match (e.outcome, e.status) {
                (Outcome::Offline, _) => tl!("blocked (offline)").to_string(),
                (Outcome::Cached, _) => tl!("from cache").to_string(),
                (_, Some(s)) => s.to_string(),
                (_, None) => e.error.clone().unwrap_or_else(|| tl!("failed").to_string()),
            };
            let color = if matches!(e.outcome, Outcome::Ok | Outcome::Cached) { t.text_dim } else { WARN };
            ui.label(RichText::new(format!("{} {} · {} · {} ms", e.method, e.url, result, e.millis)).size(11.0).color(color));
        }
        let area = Rect::from_min_max(top, ui.cursor().min + vec2(ui.available_width(), 0.0));
        let r = ui.interact(area, egui::Id::new("connections-log"), Sense::hover());
        elems.push(("connections.log".into(), r, format!("{} requests", requests.len())));
    });
    for (id, r, label) in &elems {
        app.auto.add_response(id, r, label);
    }
    let ctx = ui.ctx().clone();
    for a in actions {
        match a {
            Action::Offline(on) => {
                if let Err(e) = app.session.execute("net.setOffline", json!({"offline": on})) {
                    app.ui.status = e.to_string();
                }
            }
            Action::Save(service, value) => match service.secret.map(|name| net::save_secret(name, &value)) {
                Some(Ok(())) => {
                    ctx.data_mut(|d| d.remove::<String>(key_id(service.id)));
                    ctx.data_mut(|d| d.remove::<Arc<Cache>>(cache_id()));
                    app.session.log.push(Level::Info, "network", format!("Saved the key for {}.", service.label));
                    app.ui.status = tlf!("Saved the key for {name}.", name = service.label);
                }
                Some(Err(e)) => {
                    app.session.log.push(Level::Warning, "network", format!("Could not save the key for {}: {e}", service.label));
                    app.ui.status = tlf!("Could not save the key: {why}", why = e);
                }
                None => {}
            },
            Action::Remove(service) => {
                if let Some(name) = service.secret {
                    match app.session.execute("secrets.delete", json!({"name": name})) {
                        Ok(_) => ctx.data_mut(|d| d.remove::<Arc<Cache>>(cache_id())),
                        Err(e) => app.ui.status = e.to_string(),
                    }
                }
            }
            Action::Connect(service) => match app.session.execute("connections.connect", json!({"service": service.id})) {
                Ok(v) => {
                    if let Some(url) = v["url"].as_str() {
                        crate::links::open(app, &ctx, url);
                    }
                    app.ui.status = tlf!("Waiting for {name} in the browser…", name = service.label);
                }
                Err(e) => {
                    app.session.log.push(Level::Warning, "network", format!("Could not start signing in to {}: {e}", service.label));
                    app.ui.status = tlf!("Could not start the sign-in: {why}", why = e);
                }
            },
            Action::Probe(service) => {
                if let Err(e) = app.session.execute("connections.probe", json!({"service": service.id})) {
                    app.ui.status = e.to_string();
                }
            }
            Action::CancelConnect(job) => {
                let _ = app.session.execute("jobs.cancel", json!({"job": job}));
            }
            Action::Reopen(url) => crate::links::open(app, &ctx, &url),
            Action::Disconnect(service) => match app.session.execute("connections.disconnect", json!({"service": service.id})) {
                Ok(_) => {
                    ctx.data_mut(|d| d.remove::<Arc<Cache>>(cache_id()));
                    app.ui.status = tlf!("Disconnected {name}.", name = service.label);
                }
                Err(e) => app.ui.status = e.to_string(),
            },
            Action::GetKey(url) => crate::links::open(app, &ctx, url),
            Action::ClearLog => {
                let _ = app.session.execute("net.log", json!({"clear": true}));
            }
        }
    }
    // A sign-in that was waiting and is now over with a key stored: say so once.
    let waiting: Vec<&'static str> =
        cache.statuses.iter().filter(|(sv, _)| matches!(connect::attempt(sv.id), Some(Attempt::Waiting { .. }))).map(|(sv, _)| sv.id).collect();
    let before: Vec<&'static str> = ctx.data(|d| d.get_temp(egui::Id::new("connections-waiting"))).unwrap_or_default();
    for id in before.iter().filter(|id| !waiting.contains(id)) {
        if let Some((sv, _)) = cache.statuses.iter().find(|(sv, _)| sv.id == *id)
            && net::connected(id) == Some(true)
        {
            app.session.log.push(Level::Info, "network", format!("Connected to {}.", sv.label));
            app.ui.status = tlf!("Connected to {name}.", name = sv.label);
        }
    }
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("connections-waiting"), waiting.clone()));
    let checking = cache.statuses.iter().any(|(sv, _)| probe::state(sv.id) == Some(Probe::Checking));
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(if waiting.is_empty() && !checking { 2000 } else { 400 }));
}

/// The Offline switch at the right of the status bar: dim "Online", or an amber "Offline" while
/// network requests are blocked. Clicking toggles it.
pub fn status_indicator(app: &mut FilmcraftApp, ui: &mut egui::Ui, sb: Rect, t: &Tokens) {
    let offline = net::is_offline();
    let text = if offline { tl!("Offline") } else { tl!("Online") };
    let color = if offline { WARN } else { t.text_faint };
    let right = sb.max.x - 346.0;
    let galley = ui.painter().layout_no_wrap(text.to_string(), Tokens::ui(11.0), color);
    let rect = Rect::from_min_size(egui::pos2(right - galley.size().x - 12.0, sb.min.y), vec2(galley.size().x + 12.0, sb.height()));
    let resp = ui.interact(rect, egui::Id::new("status-offline"), Sense::click());
    if offline || resp.hovered() {
        ui.painter().rect_filled(rect.shrink2(vec2(0.0, 3.0)), 3.0, t.hover);
    }
    ui.painter().text(egui::pos2(rect.center().x, sb.center().y), Align2::CENTER_CENTER, text, Tokens::ui(11.0), color);
    app.auto.add_response("status.offline", &resp, &format!("Offline mode {}", if offline { "on" } else { "off" }));
    let resp =
        resp.on_hover_text(if offline { tl!("Offline: no network requests. Click to go online.") } else { tl!("Online. Click to stop all network requests.") });
    if resp.clicked() {
        let _ = app.session.execute("net.setOffline", json!({"offline": !offline}));
    }
}
