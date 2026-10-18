//! The voice-it card (fork roadmap F16.7): when the person has pasted words that a voice should read, Edith offers
//! to make the narration, and the person's **Yes** makes it.
//!
//! Like a download, making a narration is the person's action, never the assistant's: it takes minutes, writes a
//! sound file and puts a clip on the timeline. `request_narration` only shows a card (a preview of the words, the
//! voice, how many words and about how long) with **Yes**, **No** and **I'll choose the voice myself** (which opens
//! Text to Speech with the words typed in). Yes starts the voice in the background (`tts.render`), and when it has
//! finished places it at the start of the timeline (`tts.create`); the group's **Send answers** waits for that, so the
//! next turn already has the narration and its word times.
//!
//! What the text *is* (narration to be read, words someone says on camera, text for the screen, an outline) is for
//! Edith to work out from its shape, and to ask about only when she cannot tell; the card is for the first kind only.

use std::collections::HashMap;

use edith_ai::ToolOutput;
use serde_json::{Value, json};

use crate::Session;
use crate::assistant_cards::{next_card_id, room_check};
use crate::assistant_tools::{ToolDef, def};
use crate::commands::bad;
use crate::{EngineError, Result};

/// Characters of the script shown in the card.
const PREVIEW_CHARS: usize = 420;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum State {
    Rendering,
    Done { clip: u64, seconds: f64, voice: String },
    Failed(String),
}

/// A voice being made for a card the person said Yes to.
pub(crate) struct Voicing {
    job: Option<u64>,
    since: web_time::Instant,
    state: State,
    text: String,
    voice: String,
}

pub(crate) type VoicingMap = HashMap<(u64, String), Voicing>;

/// The words as the voice will read them: no control characters except line breaks.
fn clean_script(v: &Value) -> Option<String> {
    let t: String = v.as_str()?.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).collect();
    let t = t.trim().to_string();
    (t.chars().any(char::is_alphanumeric)).then_some(t)
}

/// The words a voice will say: markers in square brackets (`[pause 1s]`, `[pace 0.9]`) are directions, not words.
fn spoken_words(text: &str) -> usize {
    let mut plain = String::with_capacity(text.len());
    let mut inside = false;
    for c in text.chars() {
        match c {
            '[' => inside = true,
            ']' if inside => {
                inside = false;
                plain.push(' ');
            }
            _ if !inside => plain.push(c),
            _ => {}
        }
    }
    plain.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)).count()
}

/// (id, name, natural, installed-natural) of the voice asked for, or the best one installed.
fn resolve_voice(requested: Option<&str>) -> std::result::Result<(String, String, bool, bool), String> {
    let models = crate::transcript::models_dir();
    let voices = filmcraft_tts::voices_in(models.as_deref());
    let natural_installed = voices.iter().any(|v| v.engine == "neural" && v.installed);
    let pick = match requested {
        Some(id) => voices.iter().find(|v| v.id == id).ok_or_else(|| {
            let ok: Vec<&str> = voices.iter().filter(|v| v.installed).map(|v| v.id).collect();
            format!("there is no voice “{id}”; the ones you can use are {}", ok.join(", "))
        })?,
        None => voices
            .iter()
            .find(|v| v.id == "kokoro-heart" && v.installed)
            .or_else(|| voices.iter().find(|v| v.installed && v.engine == "neural"))
            .or_else(|| voices.iter().find(|v| v.installed))
            .ok_or("no voice is available")?,
    };
    if !pick.installed {
        return Err(format!("the voice “{}” is not downloaded; the natural voices are in Text to Speech ▸ Download natural voices", pick.id));
    }
    Ok((pick.id.to_string(), pick.name.to_string(), pick.engine == "neural", natural_installed))
}

pub(crate) fn request(s: &Session, thread: u64, input: &Value) -> ToolOutput {
    let so_far = match room_check(s, thread) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let Some(text) = input.get("text").and_then(clean_script) else {
        return ToolOutput::error(
            "`text` is required: exactly the words the voice should read (no stage directions, no speaker names), with [pause 1s] where a beat belongs",
        );
    };
    if text.len() > filmcraft_tts::MAX_TEXT_BYTES {
        return ToolOutput::error(format!(
            "that is {} KB of words and a narration holds at most {} KB: make it in parts (one section at a time), or ask the person to use Text to Speech",
            text.len() / 1024,
            filmcraft_tts::MAX_TEXT_BYTES / 1024
        ));
    }
    let words = spoken_words(&text);
    if words == 0 {
        return ToolOutput::error("there are no words in that text to read");
    }
    if let Some(q) = s.active_sequence()
        && let Some(c) = q.audio_tracks.iter().flat_map(|t| t.items.iter()).find(|c| s.project.narrations.contains_key(&c.item))
    {
        return ToolOutput::error(format!(
            "there is already a narration on the timeline (clip {}). Only one is made this way: to change its words or voice, the person selects it and uses Text to Speech",
            c.id.0
        ));
    }
    if input.get("voice").is_some_and(|v| !v.is_null() && !v.is_string()) {
        return ToolOutput::error("`voice` must be a voice id such as kokoro-heart or basic-female (or left out)");
    }
    let (voice, voice_name, natural, natural_installed) = match resolve_voice(input.get("voice").and_then(Value::as_str)) {
        Ok(v) => v,
        Err(e) => return ToolOutput::error(e),
    };
    let why = input
        .get("why")
        .and_then(|w| w.as_str())
        .map(|w| w.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|w| w.chars().count() <= 300)
        .unwrap_or_default();
    let seconds = edith_planner::speech_ticks(&text, 150).0 as f64 / filmcraft_time::TICKS_PER_SECOND as f64;
    let id = next_card_id(s, thread);
    let preview: String = text.chars().take(PREVIEW_CHARS).collect();
    let view = json!({
        "kind": "narration", "id": id, "text": text, "preview": preview, "truncated": text.chars().count() > PREVIEW_CHARS,
        "words": words, "estimateSeconds": seconds, "why": why,
        "voice": {"id": voice, "name": voice_name, "natural": natural}, "naturalInstalled": natural_installed,
    });
    let mut said = format!(
        "Narration card {id} is now shown to the person: {words} words, about {}, in the voice {voice_name}. That makes {} card{} this turn (at most 6). Nothing is made unless they press Yes: then the voice is made and placed at the start of the timeline, and their answer says so. Stop here: their answers to every card arrive together as their next message.",
        clock(seconds),
        so_far + 1,
        if so_far == 0 { "" } else { "s" }
    );
    if !natural {
        said.push_str(" The natural voices are not downloaded, so this is the basic voice, which sounds robotic and whose word times are only estimates; tell the person the natural voices are in Text to Speech ▸ Download natural voices.");
    }
    ToolOutput::ok(said).with_view(view)
}

fn clock(seconds: f64) -> String {
    let n = seconds.max(0.0).round() as u64;
    if n >= 60 { format!("{}:{:02}", n / 60, n % 60) } else { format!("{n} seconds") }
}

/// The person pressed Yes: start making the voice.
pub(crate) fn start(s: &mut Session, thread: u64, card: &Value) -> Result<()> {
    let cmd = "assistant.card";
    let id = card["id"].as_str().unwrap_or_default().to_string();
    if s.assistant.cards.voicing.contains_key(&(thread, id.clone())) {
        return Err(bad(cmd, "this narration has been started"));
    }
    let text = card["text"].as_str().filter(|t| !t.is_empty()).ok_or_else(|| bad(cmd, "the card has no words"))?.to_string();
    let voice = card["voice"]["id"].as_str().unwrap_or("basic-female").to_string();
    let rendered = s.execute("tts.render", json!({"text": text, "voice": voice}))?;
    let job = rendered["job"].as_u64();
    s.assistant.cards.voicing.insert((thread, id), Voicing { job, since: web_time::Instant::now(), state: State::Rendering, text, voice });
    // already made (the same words and voice were rendered before): place it at once
    pump(s);
    Ok(())
}

/// Places the narrations whose voices have finished. Cheap when nothing is being made.
pub(crate) fn pump(s: &mut Session) {
    let waiting: Vec<(u64, String)> = s.assistant.cards.voicing.iter().filter(|(_, v)| v.state == State::Rendering).map(|(k, _)| k.clone()).collect();
    for key in waiting {
        let Some(v) = s.assistant.cards.voicing.get(&key) else { continue };
        let (job, text, voice) = (v.job, v.text.clone(), v.voice.clone());
        let outcome: Option<std::result::Result<(), String>> = match job {
            None => Some(Ok(())),
            Some(id) => s.jobs.iter().find(|j| j.id == id).and_then(|j| {
                if !j.progress.finished.load(std::sync::atomic::Ordering::Relaxed) {
                    return None;
                }
                Some(match j.result.lock().unwrap_or_else(|e| e.into_inner()).clone() {
                    Some(Ok(_)) => Ok(()),
                    Some(Err(e)) => Err(e),
                    None => Err("the voice stopped without an answer".to_string()),
                })
            }),
        };
        let Some(outcome) = outcome else { continue };
        let state = match outcome {
            Err(e) => State::Failed(e),
            Ok(()) => match make_clip(s, &text, &voice) {
                Ok((clip, seconds)) => State::Done { clip, seconds, voice: voice_name(&voice) },
                Err(e) => State::Failed(e.to_string()),
            },
        };
        if let Some(v) = s.assistant.cards.voicing.get_mut(&key) {
            v.state = state;
        }
    }
}

fn voice_name(id: &str) -> String {
    filmcraft_tts::voices_in(crate::transcript::models_dir().as_deref()).iter().find(|v| v.id == id).map_or_else(|| id.to_string(), |v| v.name.to_string())
}

/// Puts the finished narration at the start of the timeline, making a sequence first when there is none.
fn make_clip(s: &mut Session, text: &str, voice: &str) -> Result<(u64, f64)> {
    if s.active_sequence().is_none() {
        s.execute("file.newSequence", json!({"name": "Sequence 1"}))?;
    }
    let r = s.execute("tts.create", json!({"text": text, "voice": voice, "seconds": 0}))?;
    // tts.create selects the new clip, and a selected narration brings Text to Speech forward over the conversation:
    // the person is answering cards, so leave nothing selected
    let _ = s.execute("edit.deselectAll", json!({}));
    let clip = r["clip"].as_u64().ok_or_else(|| EngineError::Other("the narration was made but its clip is unknown".into()))?;
    let seconds = r["duration"].as_i64().map_or(0.0, |t| t as f64 / filmcraft_time::TICKS_PER_SECOND as f64);
    Ok((clip, seconds))
}

pub(crate) fn state_json(s: &Session, thread: u64, card: &Value) -> Option<Value> {
    if card["kind"] != "narration" {
        return None;
    }
    let id = card["id"].as_str()?.to_string();
    Some(match s.assistant.cards.voicing.get(&(thread, id)) {
        None => json!({"state": "none"}),
        Some(v) => match &v.state {
            State::Rendering => json!({"state": "rendering", "seconds": v.since.elapsed().as_secs_f64()}),
            State::Done { clip, seconds, voice } => json!({"state": "done", "clip": clip, "seconds": seconds, "voice": voice}),
            State::Failed(m) => json!({"state": "failed", "message": m}),
        },
    })
}

/// What the answer says about this card, in the message to the assistant.
pub(crate) fn answer_yes(s: &Session, thread: u64, card: &Value) -> String {
    match state_json(s, thread, card).as_ref().map(|v| (v["state"].as_str().unwrap_or(""), v)) {
        Some(("done", v)) => format!(
            "yes, the narration is on the timeline now (clip {}, {}, voice {}); narration_timings has its word times",
            v["clip"],
            clock(v["seconds"].as_f64().unwrap_or(0.0)),
            v["voice"].as_str().unwrap_or("")
        ),
        Some(("failed", v)) => format!("yes, but making the narration failed: {}", v["message"].as_str().unwrap_or("")),
        _ => "yes (still being made)".to_string(),
    }
}

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![def(
        "request_narration",
        "Offer to make the narration from words the person pasted, as a card with a preview, the voice, how many words and about how long, and Yes, No and \"I'll choose the voice myself\"; then END YOUR TURN as with ask_user. You cannot make it yourself: if they press Yes the voice is made and placed at the start of the timeline, and their answer says so. Use it only for words a voice is meant to READ ALOUD, which you work out from the text's shape (flowing prose in paragraphs, a script of what the voice says); do not offer it for dialogue between named speakers, on-screen text, an outline or a shot list, and ask the person only when you cannot tell what the text is. Pass exactly the words to be read: no stage directions or speaker names, and `[pause 1s]` where a beat belongs between ideas. Counts toward the 6 cards of a turn.",
        json!({
            "text": {"type": "string", "description": "Exactly what the voice reads, up to 64 KB."},
            "voice": {"type": "string", "description": "A voice id such as kokoro-heart (natural) or basic-female. Leave out for the best one installed."},
            "why": {"type": "string", "description": "One sentence for the person: why this is the narration."},
        }),
        |_, _| ToolOutput::error("this tool works inside a conversation only"),
    )]
}
