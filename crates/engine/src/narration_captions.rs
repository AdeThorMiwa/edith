//! `transcript.fromNarration` and `captions.fromNarration` (fork roadmap F30): the narration's words, which the voice
//! knows to the frame, become a transcript, and the transcript becomes captions.
//!
//! A recording has to be listened to by a speech recogniser before it has a transcript. A narration made in the app
//! does not: the voice said each word, so it knows when ([`filmcraft_project::SpokenWord`]). `transcript.fromNarration`
//! writes those words as the narration's transcript (source `narration`), so the Text panel, text-based editing,
//! search and `transcript.createCaptions` work on it like on anything transcribed. `captions.fromNarration` does that
//! and then groups the words into captions the usual way (at most 42 characters a line and two lines, a new caption
//! after a pause, none shorter than a second), and says how many of them go by faster than a person reads.
//!
//! Both are one undo step. A narration that already has a transcript (a recording, or one written before) keeps it
//! unless `overwrite` is true. A narration with no word times (an older file, a voice that cannot say) is skipped and
//! named.
//!
//! `transcript.fromNarration {overwrite?}`; `captions.fromNarration {maxChars?, lines?, minSeconds?, maxSeconds?, format?, name?, overwrite?}`.

use std::sync::Arc;

use filmcraft_project::{ItemId, Transcript, Word};
use filmcraft_time::TICKS_PER_SECOND;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p, has_seq};
use crate::{EngineError, Result, Session};

/// Characters a second a reader keeps up with; above this a caption is "fast".
const READABLE_CPS: f64 = 21.0;

/// The narrations used in the active sequence, with whether they have word times and a transcript.
fn narrations_in_use(s: &Session) -> Vec<(ItemId, bool, bool)> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    let mut seen: Vec<ItemId> = Vec::new();
    let mut out = Vec::new();
    for it in q.audio_tracks.iter().flat_map(|t| t.items.iter()) {
        let Some(n) = s.project.narrations.get(&it.item) else { continue };
        if seen.contains(&it.item) {
            continue;
        }
        seen.push(it.item);
        out.push((it.item, !n.words.is_empty(), s.project.transcripts.contains_key(&it.item)));
    }
    out
}

/// Writes a transcript for each narration in use that has word times and no transcript (or all, with `overwrite`).
/// Returns the items written and the names of those skipped for having no word times.
fn write_transcripts(s: &mut Session, overwrite: bool) -> Result<(Vec<ItemId>, Vec<String>)> {
    let found = narrations_in_use(s);
    let mut skipped = Vec::new();
    let mut todo: Vec<ItemId> = Vec::new();
    for (item, has_words, has_transcript) in found {
        if !has_words {
            skipped.push(s.project.item(item).map(|i| i.name.clone()).unwrap_or_default());
        } else if overwrite || !has_transcript {
            todo.push(item);
        }
    }
    if todo.is_empty() {
        return Ok((todo, skipped));
    }
    s.edit("Transcript From Narration", |p, _| {
        for item in &todo {
            let Some(n) = p.narrations.get(item) else { continue };
            let mut t = Transcript {
                language: n.language.split(['-', '_']).next().unwrap_or("en").to_ascii_lowercase(),
                source: "narration".into(),
                speakers: Vec::new(),
                words: n.words.iter().map(|w| Word { text: w.text.clone(), start: w.start, end: w.end, speaker: None, confidence: 1.0 }).collect(),
            };
            t.normalize();
            p.transcripts.insert(*item, Arc::new(t));
        }
        Ok(())
    })?;
    Ok((todo, skipped))
}

fn from_narration(s: &mut Session, p: &Value) -> Result<Value> {
    let (written, skipped) = write_transcripts(s, bool_p(p, "overwrite").unwrap_or(false))?;
    let have = narrations_in_use(s);
    if have.is_empty() {
        return Err(bad("transcript.fromNarration", "there is no narration made in the app on this timeline"));
    }
    Ok(
        json!({"written": written.iter().map(|i| i.0).collect::<Vec<_>>(), "skippedNoWords": skipped, "alreadyHadOne": have.iter().filter(|(_, w, t)| *w && *t).count().saturating_sub(written.len())}),
    )
}

fn captions_from_narration(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "captions.fromNarration";
    if narrations_in_use(s).is_empty() {
        return Err(bad(cmd, "there is no narration made in the app on this timeline (transcript.generate listens to a recording)"));
    }
    let mut q = p.as_object().cloned().unwrap_or_default();
    q.remove("overwrite");
    q.entry("name").or_insert(json!("Narration"));
    let (made, skipped) = s.edit_group("Captions From Narration", |s| {
        let (_, skipped) = write_transcripts(s, bool_p(p, "overwrite").unwrap_or(false))?;
        let made = s.execute("transcript.createCaptions", Value::Object(q.clone()))?;
        Ok((made, skipped))
    })?;
    // how many go by faster than a person reads
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let fast = made["track"]
        .as_u64()
        .and_then(|id| seq.caption_tracks.iter().find(|t| t.id.0 == id))
        .map(|t| {
            t.captions
                .iter()
                .filter(|c| {
                    let secs = c.duration.0 as f64 / TICKS_PER_SECOND as f64;
                    secs > 0.0 && c.text.chars().filter(|ch| !ch.is_whitespace()).count() as f64 / secs > READABLE_CPS
                })
                .count()
        })
        .unwrap_or(0);
    let mut notes: Vec<String> = Vec::new();
    if fast > 0 {
        notes.push(format!(
            "{fast} caption{} {} faster than about {READABLE_CPS:.0} characters a second, which is quicker than most people read",
            if fast == 1 { "" } else { "s" },
            if fast == 1 { "goes by" } else { "go by" }
        ));
    }
    if !skipped.is_empty() {
        notes.push(format!("no word times for {}: it has no captions", skipped.join(", ")));
    }
    Ok(json!({"track": made["track"], "captions": made["captions"], "fast": fast, "notes": notes}))
}

fn enabled(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    if narrations_in_use(s).is_empty() { Err("there is no narration made in the app on this timeline".into()) } else { Ok(()) }
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "transcript.fromNarration",
            label: "Transcript From Narration",
            menu: &[],
            shortcut: None,
            params: r#"{"overwrite":bool?}"#,
            enabled,
            run: from_narration,
            journal: true,
        },
        CommandSpec {
            id: "captions.fromNarration",
            label: "Captions From Narration",
            menu: &[],
            shortcut: None,
            params: r#"{"maxChars":n?,"lines":n?,"minSeconds":f64?,"maxSeconds":f64?,"format":"srt|vtt|…"?,"name":str?,"overwrite":bool?}"#,
            enabled,
            run: captions_from_narration,
            journal: true,
        },
    ]
}
