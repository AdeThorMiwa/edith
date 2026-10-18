//! `narration_timings` (fork roadmap F16.2): when each word of the narration is spoken, on the timeline.
//!
//! The assistant needs real times to cut on words and pauses and to know where each idea starts and
//! ends. They come, best first, from: the natural voice itself (it knows how long every sound lasts,
//! kept with the narration when it was made); the transcript of a recorded voice (Whisper's word
//! times, found by `transcript.run`); and, for a narration saved before times were kept, a plain
//! estimate from the length of the words. Every answer says which it is.
//!
//! Read-only, in every mode. Times are seconds on the timeline, taking the clip's start, trim and speed
//! into account; words trimmed off the clip are left out.

use edith_ai::ToolOutput;
use filmcraft_project::{ClipId, TrackItem};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use crate::Session;
use crate::assistant_tools::{ToolDef, def, parse_time};

/// Words in one answer.
const PAGE_WORDS: usize = 150;
/// A gap between words at least this long is shown as a pause, seconds.
const PAUSE_SECONDS: f64 = 0.30;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Basis {
    /// The natural voice's own times.
    Voice,
    /// The basic voice's: the stretch between pauses is exact, the words inside it are spread by length.
    VoiceEstimate,
    /// A transcript of a recording.
    Transcript,
    /// Spread over the clip by the length of the words: for a narration that kept no times.
    Estimate,
}

impl Basis {
    fn describe(self) -> &'static str {
        match self {
            Basis::Voice => "exact: the voice itself timed every word as it spoke it",
            Basis::VoiceEstimate => {
                "estimated: the basic voice times each stretch between pauses, and the words inside it are spread by their length, so a word can be off by a few tenths of a second"
            }
            Basis::Transcript => "from the transcript (speech recognition): usually within a tenth of a second or two, but it can mishear or split a word",
            Basis::Estimate => {
                "a rough estimate: this narration kept no times, so the words are spread over the clip by their length and pauses written in the script are not known; make the narration again (tts.edit) for exact times"
            }
        }
    }

    fn word(self) -> &'static str {
        match self {
            Basis::Voice => "voice-exact",
            Basis::VoiceEstimate => "voice-estimated",
            Basis::Transcript => "transcript",
            Basis::Estimate => "estimated",
        }
    }
}

struct Timed {
    text: String,
    start: Tick,
    end: Tick,
    sentence: u32,
}

pub(crate) fn secs(t: Tick) -> f64 {
    t.0 as f64 / TICKS_PER_SECOND as f64
}

pub(crate) fn ticks(t: f64) -> Tick {
    Tick(if t.is_finite() { t.round().clamp(-1.0e18, 1.0e18) as i64 } else { 0 })
}

/// Media time to timeline time for a clip, as the sequence transcript maps it.
pub(crate) fn to_timeline(it: &TrackItem, media: Tick) -> Tick {
    it.start + ticks((media - it.source_in).0 as f64 / it.speed)
}

/// Whether the audio clip `it` has something to time.
fn timed_source(s: &Session, it: &TrackItem) -> bool {
    s.project.narrations.contains_key(&it.item) || s.project.transcripts.get(&it.item).is_some_and(|t| !t.words.is_empty())
}

/// The words of `it` on the timeline, and where they came from.
fn words_of(s: &Session, it: &TrackItem) -> Option<(Vec<Timed>, Basis)> {
    let narration = s.project.narrations.get(&it.item);
    let mut basis = Basis::Voice;
    let mut media: Vec<(String, Tick, Tick, u32)> = Vec::new();
    if let Some(n) = narration.filter(|n| !n.words.is_empty()) {
        if n.words.iter().any(|w| !w.exact) {
            basis = Basis::VoiceEstimate;
        }
        media = n.words.iter().map(|w| (w.text.clone(), w.start, w.end, w.sentence)).collect();
    } else if let Some(t) = s.project.transcripts.get(&it.item).filter(|t| !t.words.is_empty()) {
        basis = Basis::Transcript;
        let mut sentence = 0u32;
        for w in &t.words {
            media.push((w.text.clone(), w.start, w.end.max(w.start), sentence));
            if w.text.trim_end_matches(['"', '\'', ')', '”', '’']).ends_with(['.', '!', '?']) {
                sentence = sentence.saturating_add(1);
            }
        }
    } else {
        // no kept times: spread the words of the script over the speech
        let n = narration?;
        basis = Basis::Estimate;
        let plain = strip_markers(&n.text);
        let rate = 24_000usize;
        let total = usize::try_from(n.speech_duration.to_units_floor(rate as i64)).unwrap_or(0);
        let mut spread = Vec::new();
        filmcraft_tts::estimate_words(&plain, 0, total, 0, &mut spread);
        media = spread
            .into_iter()
            .map(|w| (w.text, Tick::from_units(w.start as i64, rate as i64), Tick::from_units(w.end as i64, rate as i64), w.sentence))
            .collect();
    }
    if it.reverse || it.frame_hold.is_some() || !(it.speed.is_finite() && it.speed > 0.0) {
        return None;
    }
    let words = media
        .into_iter()
        .filter_map(|(text, a, b, sentence)| {
            let (a, b) = (to_timeline(it, a), to_timeline(it, b.max(a)));
            let mid = Tick(a.0 + (b.0 - a.0) / 2);
            // trimmed off the clip: not heard
            (mid >= it.start && mid < it.end()).then(|| Timed { text, start: a.max(it.start), end: b.min(it.end()).max(a.max(it.start)), sentence })
        })
        .collect();
    Some((words, basis))
}

/// A script without its `[pause 1s]`, `[pace 0.9]`, `[pitch -2]` and `[normal]` markers.
fn strip_markers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = false;
    for c in text.chars() {
        match c {
            '[' => depth = true,
            ']' if depth => {
                depth = false;
                out.push(' ');
            }
            _ if !depth => out.push(c),
            _ => {}
        }
    }
    out
}

fn clip_label(s: &Session, it: &TrackItem) -> String {
    let name = s.project.items.get(&it.item).map_or(it.name.as_str(), |i| i.name.as_str());
    format!("clip {} “{}”", it.id.0, name)
}

fn narration_timings(s: &Session, input: &Value) -> ToolOutput {
    let Some(q) = s.active_sequence() else { return ToolOutput::error("no sequence is open") };
    let candidates: Vec<&TrackItem> = q.audio_tracks.iter().flat_map(|t| t.items.iter()).filter(|it| timed_source(s, it)).collect();
    let wanted = match input.get("clip") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_u64() {
            Some(c) => Some(ClipId(c)),
            None => return ToolOutput::error("`clip` must be a clip id (from sequence_view)"),
        },
    };
    let it = match wanted {
        Some(c) => match q.find_item(c) {
            Some((_, it)) if timed_source(s, it) => it,
            Some(_) => {
                return ToolOutput::error(format!(
                    "clip {} has no narration or transcript to take times from; transcribe it first (analysis_run or transcript.run)",
                    c.0
                ));
            }
            None => return ToolOutput::error(format!("no clip {} in this sequence", c.0)),
        },
        None => match candidates.as_slice() {
            [one] => one,
            [] => {
                return ToolOutput::error(
                    "no narration or transcribed speech on the timeline: make a narration (tts.create) or transcribe a recording (transcript.run) first",
                );
            }
            many => {
                let list: Vec<String> = many.iter().take(12).map(|it| format!("{} from {:.1} s", clip_label(s, it), secs(it.start))).collect();
                return ToolOutput::error(format!("{} clips have speech to time: {}. Pass `clip`.", many.len(), list.join("; ")));
            }
        },
    };
    let Some((all, basis)) = words_of(s, it) else {
        return ToolOutput::error(format!("{} is reversed, frozen or at a speed of zero, so its words cannot be placed on the timeline", clip_label(s, it)));
    };
    let from = match input.get("from").filter(|v| !v.is_null()).map(|v| parse_time(v, q, "from")) {
        Some(Err(e)) => return ToolOutput::error(e),
        Some(Ok(t)) => Some(t),
        None => None,
    };
    let to = match input.get("to").filter(|v| !v.is_null()).map(|v| parse_time(v, q, "to")) {
        Some(Err(e)) => return ToolOutput::error(e),
        Some(Ok(t)) => Some(t),
        None => None,
    };
    let page = input.get("page").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
    let inside: Vec<&Timed> = all.iter().filter(|w| from.is_none_or(|f| w.end > f) && to.is_none_or(|t| w.start < t)).collect();
    let pages = inside.len().div_ceil(PAGE_WORDS).max(1);
    if all.is_empty() {
        return ToolOutput::ok(format!("{} has no words on the timeline (they may all be trimmed off).", clip_label(s, it)));
    }
    let skip = page.saturating_sub(1).saturating_mul(PAGE_WORDS);
    let shown: Vec<&Timed> = inside.iter().copied().skip(skip).take(PAGE_WORDS).collect();
    if shown.is_empty() {
        return ToolOutput::error(format!(
            "page {page} is past the end: there {} {pages} page{}",
            if pages == 1 { "is" } else { "are" },
            if pages == 1 { "" } else { "s" }
        ));
    }
    let sentences = {
        let mut ids: Vec<u32> = all.iter().map(|w| w.sentence).collect();
        ids.dedup();
        ids.len()
    };
    let first = all.first().map_or(Tick::ZERO, |w| w.start);
    let last = all.last().map_or(Tick::ZERO, |w| w.end);
    let mut out = format!(
        "{}: {} words in {} sentences, {:.2} s to {:.2} s on the timeline. Timings {} [{}].\n",
        clip_label(s, it),
        all.len(),
        sentences,
        secs(first),
        secs(last),
        basis.describe(),
        basis.word()
    );
    out.push_str(&format!("Page {page} of {pages} (words {}–{} of {}). Each word is text@start-end in seconds on the timeline; ‹pause› marks a gap of {PAUSE_SECONDS} s or more. A cut should land in a gap, or at the start or end of a word, never inside one.\n", skip + 1, skip + shown.len(), inside.len()));
    let mut current: Option<u32> = None;
    let mut prev_end: Option<Tick> = None;
    for w in &shown {
        if current != Some(w.sentence) {
            out.push_str(&format!("\nS{}: ", w.sentence + 1));
            current = Some(w.sentence);
        } else {
            out.push(' ');
        }
        if let Some(e) = prev_end {
            let gap = secs(w.start) - secs(e);
            if gap >= PAUSE_SECONDS {
                out.push_str(&format!("‹pause {gap:.2}› "));
            }
        }
        out.push_str(&format!("{}@{:.2}-{:.2}", w.text, secs(w.start), secs(w.end)));
        prev_end = Some(w.end);
    }
    if page < pages {
        out.push_str(&format!("\n\nMore: call again with page {}.", page + 1));
    }
    ToolOutput::ok(out)
}

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![def(
        "narration_timings",
        "When each word of the narration (or of any recorded speech that has a transcript) is spoken, in seconds on the timeline, grouped into sentences, with the pauses between them. It says how reliable the times are: the natural voice's own times are exact, a transcript's are close, anything else is an estimate. Use it to put cuts on word and pause boundaries and to know where each idea starts and ends. Paged (150 words a page); narrow with `from` and `to`. With one narration or transcribed clip `clip` can be left out.",
        json!({
            "clip": {"type": "integer", "description": "The clip id (from sequence_view). Optional when only one clip has speech to time."},
            "from": {"type": "string", "description": "Only words after this timeline time (seconds, \"1:25\", or a timecode with frames)."},
            "to": {"type": "string", "description": "Only words before this timeline time."},
            "page": {"type": "integer", "description": "Page of 150 words, from 1."},
        }),
        narration_timings,
    )]
}
