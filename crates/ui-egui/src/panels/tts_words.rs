//! The Pronunciation section of the Text to Speech panel (fork roadmap F06.1): teach the narrator
//! how to say names and places. **Check script** lists the words the voice would only guess at as
//! chips; pressing a chip puts the word in the editor row, where you write how it sounds in plain
//! words ("koo brick"), press **Play** to hear just that word, and save it **for this project** or
//! **for your channel**. Saved words are listed below with a Remove button. Every action is a
//! `tts.*` command (`tts.checkScript`, `tts.setPronunciation`, `tts.removePronunciation`).
//!
//! Saving never re-renders a narration that already exists; when one is loaded in the panel, a
//! **Regenerate narration** button runs Save for it.
//!
//! Automation ids: `tts.words` (twirl), `tts.words.check`, `tts.words.chip.<word>`,
//! `tts.words.word`, `tts.words.say`, `tts.words.play`, `tts.words.save.project`,
//! `tts.words.save.channel`, `tts.words.remove.<project|channel>.<word>`, `tts.words.regenerate`.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Rect, RichText};
use filmcraft_engine::Session;
use serde_json::{Value, json};

use crate::automation::Registry;
use crate::theme::Tokens;

/// Most flagged words shown as chips.
const MAX_CHIPS: usize = 40;

#[derive(Default)]
struct State {
    open: bool,
    /// The last check: the script it was made on, and what it found.
    checked: Option<Checked>,
    word: String,
    say: String,
    /// Something was saved since the narration was last regenerated.
    saved: bool,
    /// Press a chip: put the cursor in the "say it like" field on the next frame.
    focus_say: bool,
}

struct Checked {
    text: String,
    available: bool,
    reason: String,
    words: Vec<(String, u64)>,
}

/// What the section asks the panel to do (it cannot run a preview itself: that is a job).
pub enum Event {
    /// Say `word` the way `say` describes, without saving.
    Audition { word: String, say: String },
    /// Run Save on the loaded narration so it is rendered again.
    Regenerate,
    /// A message for the status line.
    Status(String),
}

fn visible(ui: &egui::Ui, r: Rect) -> Rect {
    r.intersect(ui.clip_rect())
}

fn rows(v: &Value, key: &str) -> Vec<(String, String)> {
    v[key].as_array().map(|a| a.iter().filter_map(|r| Some((r["word"].as_str()?.to_string(), r["say"].as_str()?.to_string()))).collect()).unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
pub fn show(ui: &mut egui::Ui, t: &Tokens, auto: &mut Registry, session: &mut Session, text: &str, editing: bool, w: f32) -> Vec<Event> {
    let shared: Arc<Mutex<State>> =
        ui.data_mut(|d| d.get_temp_mut_or_insert_with(egui::Id::new("tts-words-state"), || Arc::new(Mutex::new(State::default()))).clone());
    let mut st = shared.lock().unwrap_or_else(PoisonError::into_inner);
    let mut events = Vec::new();

    let (hr, open) = crate::widgets::section_header(ui, egui::Id::new("tts-words"), tl!("Pronunciation"), st.open, t, true);
    auto.add("tts.words", visible(ui, hr.rect), "Pronunciation");
    st.open = open;
    if !open {
        return events;
    }
    ui.add_space(4.0);
    ui.add(
        egui::Label::new(
            RichText::new(tl!("Teach the narrator names and places it would otherwise guess at. Spell the sound in plain words, for example \"koo brick\"."))
                .size(11.0)
                .color(t.text_faint),
        )
        .wrap(),
    );
    ui.add_space(6.0);

    // ---- Check script, and the words it finds ----
    let can_check = !text.trim().is_empty();
    let check = ui.add_enabled(can_check, egui::Button::new(tl!("Check script")));
    auto.add("tts.words.check", visible(ui, check.rect), "Check script for words the voice would guess");
    if check.clicked() {
        match session.execute("tts.checkScript", json!({"text": text})) {
            Ok(r) => {
                st.checked = Some(Checked {
                    text: text.to_string(),
                    available: r["available"].as_bool().unwrap_or(false),
                    reason: r["reason"].as_str().unwrap_or("").to_string(),
                    words: r["words"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|x| Some((x["word"].as_str()?.to_string(), x["count"].as_u64().unwrap_or(1)))).take(MAX_CHIPS).collect())
                        .unwrap_or_default(),
                });
            }
            Err(e) => events.push(Event::Status(e.to_string())),
        }
    }
    let mut chosen: Option<String> = None;
    if let Some(c) = &st.checked {
        ui.add_space(4.0);
        if c.text != text {
            ui.add(egui::Label::new(RichText::new(tl!("The script changed since the check. Check again.")).size(11.5).color(t.text_dim)).wrap());
        } else if !c.available {
            ui.add(egui::Label::new(RichText::new(&c.reason).size(11.5).color(t.text_dim)).wrap());
        } else if c.words.is_empty() {
            ui.label(RichText::new(tl!("Every word is known. Nothing to fix.")).size(11.5).color(t.text_dim));
        } else {
            ui.label(RichText::new(tl!("The voice would guess at these. Press one to fix it.")).size(11.5).color(t.text_dim));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (word, count) in &c.words {
                    let label = if *count > 1 { format!("{word} ×{count}") } else { word.clone() };
                    let r = crate::panels::assistant_widgets::pill(ui, &label, st.word.eq_ignore_ascii_case(word), true, t);
                    auto.add(&format!("tts.words.chip.{}", word.to_lowercase()), visible(ui, r.rect), &format!("Fix the word {word}"));
                    if r.clicked() {
                        chosen = Some(word.clone());
                    }
                }
            });
        }
    }
    if let Some(word) = chosen {
        st.word = word;
        st.focus_say = true;
    }

    // ---- the editor row ----
    ui.add_space(8.0);
    let half = ((w - 8.0) / 2.0).max(60.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let r = ui.add(
            egui::TextEdit::singleline(&mut st.word)
                .id(egui::Id::new("tts-words-word"))
                .desired_width(half)
                .hint_text(RichText::new(tl!("Word")).color(t.text_faint)),
        );
        auto.add("tts.words.word", visible(ui, r.rect), "Word to fix");
        let focus = std::mem::take(&mut st.focus_say);
        let r = ui.add(
            egui::TextEdit::singleline(&mut st.say)
                .id(egui::Id::new("tts-words-say"))
                .desired_width(half)
                .hint_text(RichText::new(tl!("Say it like… (koo brick)")).color(t.text_faint)),
        );
        if focus {
            r.request_focus();
        }
        auto.add("tts.words.say", visible(ui, r.rect), "How to say the word");
    });
    let ready = !st.word.trim().is_empty() && !st.say.trim().is_empty();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let play = ui.add_enabled(ready, egui::Button::new(tl!("Play")));
        auto.add("tts.words.play", visible(ui, play.rect), "Hear the word");
        if play.clicked() {
            events.push(Event::Audition { word: st.word.trim().to_string(), say: st.say.trim().to_string() });
        }
        for (scope, label, id) in
            [("project", tl!("Save for this project"), "tts.words.save.project"), ("channel", tl!("Save for my channel"), "tts.words.save.channel")]
        {
            let b = ui.add_enabled(ready, egui::Button::new(label));
            auto.add(id, visible(ui, b.rect), label);
            if b.clicked() {
                match session.execute("tts.setPronunciation", json!({"word": st.word, "say": st.say, "scope": scope})) {
                    Ok(_) => {
                        let done = st.word.trim().to_lowercase();
                        if let Some(c) = st.checked.as_mut() {
                            c.words.retain(|(w, _)| w.to_lowercase() != done);
                        }
                        st.word.clear();
                        st.say.clear();
                        st.saved = true;
                    }
                    Err(e) => events.push(Event::Status(e.to_string())),
                }
            }
        }
    });

    // ---- what is saved ----
    let listed = session.execute("tts.pronunciations", json!({})).unwrap_or(Value::Null);
    let mut remove: Option<(&'static str, String)> = None;
    for (scope, title) in [("project", tl!("This project")), ("channel", tl!("My channel"))] {
        let list = rows(&listed, scope);
        if list.is_empty() {
            continue;
        }
        ui.add_space(8.0);
        ui.label(RichText::new(title).size(11.0).color(t.text_faint));
        for (word, say) in &list {
            ui.horizontal(|ui| {
                let x = ui.small_button(tl!("Remove")).on_hover_text(tl!("Remove this word"));
                auto.add(&format!("tts.words.remove.{scope}.{word}"), visible(ui, x.rect), &format!("Remove {word} from {scope}"));
                if x.clicked() {
                    remove = Some((scope, word.clone()));
                }
                ui.label(RichText::new(word).size(12.0).color(t.text).strong());
                ui.add(egui::Label::new(RichText::new(format!("→ {say}")).size(12.0).color(t.text_dim)).truncate());
            });
        }
    }
    if let Some((scope, word)) = remove {
        match session.execute("tts.removePronunciation", json!({"word": word, "scope": scope})) {
            Ok(_) => st.saved = true,
            Err(e) => events.push(Event::Status(e.to_string())),
        }
    }

    // ---- after a change ----
    if st.saved {
        ui.add_space(8.0);
        if editing {
            ui.add(
                egui::Label::new(
                    RichText::new(tl!("Saved. The narration on the timeline keeps its old sound until you regenerate it.")).size(11.5).color(t.text_dim),
                )
                .wrap(),
            );
            let b = ui.button(tl!("Regenerate narration"));
            auto.add("tts.words.regenerate", visible(ui, b.rect), "Regenerate the narration with these words");
            if b.clicked() {
                st.saved = false;
                events.push(Event::Regenerate);
            }
        } else {
            ui.add(egui::Label::new(RichText::new(tl!("Saved. New narrations will use it.")).size(11.5).color(t.text_dim)).wrap());
        }
    }
    events
}
