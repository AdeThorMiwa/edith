//! The Narrator voice section of the Text to Speech panel (fork roadmap F06.4): pick a saved way
//! your narration sounds (voice, pace, vocal pitch and the effects on its track), use it, save the
//! current one under a name, delete one of yours, and, with a narration loaded, hear the plain voice
//! without its effects to judge what they do. Every action is a `tts.*` command.
//!
//! Saved voices are kept for every project (`edith-narrator-voices.json`); using one copies it, so
//! a project does not depend on the file.
//!
//! Automation ids: `tts.voice.preset` (the picker), `tts.voice.use`, `tts.voice.delete`,
//! `tts.voice.name`, `tts.voice.save`, `tts.voice.plain`.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Rect, RichText};
use filmcraft_engine::Session;
use filmcraft_project::VocalPitch;
use serde_json::{Value, json};

use crate::automation::Registry;
use crate::theme::Tokens;

#[derive(Default)]
struct State {
    open: bool,
    selected: String,
    name: String,
}

/// What the section asks the panel to do (it holds no draft of its own).
pub enum Event {
    /// Load these settings into the draft, remember the voice's name, and when a narration is loaded
    /// apply the voice to it (rendering it again).
    Use { name: String, voice: Option<String>, pace: Option<f64>, pitch: Option<VocalPitch> },
    /// A message for the status line.
    Status(String),
}

fn visible(ui: &egui::Ui, r: Rect) -> Rect {
    r.intersect(ui.clip_rect())
}

/// `editing`: the narration clip loaded in the panel, if any.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut egui::Ui,
    t: &Tokens,
    auto: &mut Registry,
    session: &mut Session,
    voice: &str,
    pace: f64,
    pitch: VocalPitch,
    editing: Option<u64>,
    w: f32,
) -> Vec<Event> {
    let shared: Arc<Mutex<State>> =
        ui.data_mut(|d| d.get_temp_mut_or_insert_with(egui::Id::new("tts-voice-state"), || Arc::new(Mutex::new(State::default()))).clone());
    let mut st = shared.lock().unwrap_or_else(PoisonError::into_inner);
    let mut events = Vec::new();

    let (hr, open) = crate::widgets::section_header(ui, egui::Id::new("tts-voice-section"), tl!("Narrator voice"), st.open, t, true);
    auto.add("tts.voice.section", visible(ui, hr.rect), "Narrator voice");
    st.open = open;
    if !open {
        return events;
    }
    ui.add_space(4.0);
    ui.add(
        egui::Label::new(
            RichText::new(tl!(
                "Keep the sound of your narration: the voice, pace and pitch, and the effects on its track. Saved voices work in every project."
            ))
            .size(11.0)
            .color(t.text_faint),
        )
        .wrap(),
    );
    ui.add_space(6.0);

    let listed = session.execute("tts.voicePresets", json!({"clip": editing})).unwrap_or(Value::Null);
    let voices: Vec<Value> = listed["voices"].as_array().cloned().unwrap_or_default();
    if !voices.iter().any(|v| v["name"].as_str() == Some(st.selected.as_str())) {
        st.selected = voices.first().and_then(|v| v["name"].as_str()).unwrap_or("").to_string();
    }
    let current = voices.iter().find(|v| v["name"].as_str() == Some(st.selected.as_str())).cloned().unwrap_or(Value::Null);
    let built_in = current["builtIn"].as_bool().unwrap_or(true);

    // ---- pick, use, delete ----
    let selected = st.selected.clone();
    let cb = egui::ComboBox::from_id_salt("tts-voice-preset").selected_text(&selected).width(w).show_ui(ui, |ui| {
        for v in &voices {
            let name = v["name"].as_str().unwrap_or("");
            let label = if v["builtIn"].as_bool() == Some(true) { format!("{name} ({})", tl!("built in")) } else { name.to_string() };
            if ui.selectable_label(name == selected, label).clicked() {
                st.selected = name.to_string();
            }
        }
    });
    auto.add("tts.voice.preset", visible(ui, cb.response.rect), "Narrator voice");
    if let Some(effects) = current["inserts"].as_array().filter(|a| !a.is_empty()) {
        let names: Vec<&str> = effects.iter().filter_map(|e| e["name"].as_str()).collect();
        ui.add(egui::Label::new(RichText::new(names.join(" · ")).size(11.0).color(t.text_dim)).wrap());
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let hover =
            if editing.is_some() { tl!("Use it on the narration in the panel and render it again.") } else { tl!("Use it for the next narration you add.") };
        let b = ui.add_enabled(!selected.is_empty(), egui::Button::new(tl!("Use"))).on_hover_text(hover);
        auto.add("tts.voice.use", visible(ui, b.rect), "Use this narrator voice");
        if b.clicked() {
            events.push(Event::Use {
                name: selected.clone(),
                voice: current["voice"].as_str().map(str::to_string),
                pace: current["pace"].as_f64(),
                pitch: current["pitch"].as_str().and_then(VocalPitch::from_id),
            });
        }
        let d = ui
            .add_enabled(!built_in, egui::Button::new(tl!("Delete")))
            .on_hover_text(tl!("Delete this saved narrator voice. Narrations that used it keep their sound."));
        auto.add("tts.voice.delete", visible(ui, d.rect), "Delete this narrator voice");
        if d.clicked() {
            match session.execute("tts.deleteVoicePreset", json!({"name": selected})) {
                Ok(_) => st.selected.clear(),
                Err(e) => events.push(Event::Status(e.to_string())),
            }
        }
    });

    // ---- save the current sound ----
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let r = ui.add(
            egui::TextEdit::singleline(&mut st.name)
                .id(egui::Id::new("tts-voice-name"))
                .desired_width((w - 130.0).max(80.0))
                .hint_text(RichText::new(tl!("Name this narrator voice")).color(t.text_faint)),
        );
        auto.add("tts.voice.name", visible(ui, r.rect), "Name for the narrator voice");
        let ready = !st.name.trim().is_empty();
        let b = ui
            .add_enabled(ready, egui::Button::new(tl!("Save current")))
            .on_hover_text(tl!("Saves the voice, pace and pitch in the panel and, with a narration loaded, the effects on its track."));
        auto.add("tts.voice.save", visible(ui, b.rect), "Save the current sound as a narrator voice");
        if b.clicked() {
            let mut p = json!({"name": st.name, "voice": voice, "pace": pace, "pitch": pitch.id()});
            if let Some(c) = editing {
                p["clip"] = json!(c);
            }
            match session.execute("tts.saveVoicePreset", p) {
                Ok(r) => {
                    st.selected = r["saved"].as_str().unwrap_or("").to_string();
                    st.name.clear();
                }
                Err(e) => events.push(Event::Status(e.to_string())),
            }
        }
    });

    // ---- plain voice, to compare ----
    if editing.is_some()
        && let Some(track) = listed["track"].as_object().filter(|t| t["inserts"].as_u64().unwrap_or(0) > 0)
    {
        let plain = track["plain"].as_bool().unwrap_or(false);
        ui.add_space(8.0);
        ui.label(RichText::new(tl!("On this narration's track")).size(11.0).color(t.text_faint));
        let effects = track["effects"].as_array().cloned().unwrap_or_default();
        let row = ui.horizontal_wrapped(|ui| {
            for e in &effects {
                let on = e["enabled"].as_bool().unwrap_or(true);
                let text = RichText::new(e["name"].as_str().unwrap_or("")).size(11.5).color(if on { t.text } else { t.text_faint });
                ui.label(if on { text } else { text.strikethrough() });
            }
        });
        auto.add("tts.voice.chain", visible(ui, row.response.rect), "Effects on this narration's track");
        ui.add_space(4.0);
        let b = ui
            .button(if plain { tl!("Hear with effects") } else { tl!("Hear plain voice") })
            .on_hover_text(tl!("Switches the effects on this narration's track off or on, so you can hear what they do. Play the timeline as usual."));
        auto.add("tts.voice.plain", visible(ui, b.rect), "Hear the narration without its effects, or with them again");
        if b.clicked()
            && let Err(e) = session.execute("tts.compareVoice", json!({"clip": editing, "plain": !plain}))
        {
            events.push(Event::Status(e.to_string()));
        }
    }
    events
}
