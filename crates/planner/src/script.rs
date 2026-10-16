//! Narration as sentences, and how long it takes to say (roadmap F09).
//!
//! A beat's narration is one piece of text; the A/V script shows it a sentence at a time and edits
//! it a sentence at a time. [`split`] cuts text at the end of each sentence and keeps every other
//! character exactly where it was, so [`Split::join`] gives back the original text.
//!
//! Rules: a sentence ends at `.`, `!` or `?` (with any closing quotes or brackets after it) when
//! whitespace or the end of the text follows, and at every line break. A full stop after an
//! abbreviation (`Dr.`, `etc.`) or an initial (`J. Smith`), or inside a number (`3.5`), does not
//! end one. Anything in square brackets is one piece, so `[pause 1.5s]`, `[pace 0.8]` and `[pitch -2]`
//! never split, and a marker before a sentence stays with it.

use filmcraft_time::{TICKS_PER_SECOND, Tick};

/// Abbreviations whose full stop does not end a sentence (lower-case, without the dot).
const ABBREVIATIONS: [&str; 24] = [
    "dr", "mr", "mrs", "ms", "prof", "st", "vs", "etc", "e.g", "i.e", "no", "inc", "ltd", "jr", "sr", "mt", "fig", "approx", "dept", "est", "gen", "col",
    "sgt", "capt",
];

/// A sentence and the whitespace that followed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sentence {
    /// The sentence itself, without the whitespace around it.
    pub text: String,
    /// The whitespace after it (a space, a line break, …).
    pub gap: String,
}

/// Text cut into sentences.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Split {
    /// Whitespace before the first sentence.
    pub lead: String,
    pub parts: Vec<Sentence>,
}

impl Split {
    /// The text again, exactly as it was split.
    pub fn join(&self) -> String {
        let mut out = self.lead.clone();
        for p in &self.parts {
            out.push_str(&p.text);
            out.push_str(&p.gap);
        }
        out
    }
}

/// Whether the word ending just before byte `end` of `chars` makes the full stop there an
/// abbreviation or an initial.
fn is_abbreviation(chars: &[char], end: usize) -> bool {
    let mut start = end;
    while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '.') {
        start -= 1;
    }
    let word: String = chars[start..end].iter().collect::<String>().to_lowercase();
    if word.is_empty() {
        return false;
    }
    if ABBREVIATIONS.contains(&word.as_str()) {
        return true;
    }
    // an initial: one capital letter ("J. Smith")
    let initial = chars[start..end].len() == 1 && chars[start].is_uppercase();
    initial && chars.get(end + 1).is_some_and(|c| c.is_whitespace()) && chars.get(end + 2).is_some_and(|c| c.is_uppercase())
}

/// Cut `text` into sentences.
pub fn split(text: &str) -> Split {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = Split::default();
    let mut i = 0;
    // leading whitespace
    while i < n && chars[i].is_whitespace() {
        out.lead.push(chars[i]);
        i += 1;
    }
    let mut cur = String::new();
    let mut depth = 0usize;
    while i < n {
        let c = chars[i];
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        cur.push(c);
        i += 1;
        let ends = depth == 0 && matches!(c, '.' | '!' | '?') && {
            // swallow more terminators and closing marks: `?!`, `."`, `.)`
            while i < n && matches!(chars[i], '.' | '!' | '?' | '"' | '\'' | ')' | ']' | '\u{201d}' | '\u{2019}') {
                cur.push(chars[i]);
                i += 1;
            }
            let boundary = i >= n || chars[i].is_whitespace();
            boundary && !(c == '.' && cur.ends_with('.') && is_abbreviation(&chars, i - 1))
        };
        let newline = depth == 0 && i < n && chars[i] == '\n';
        if ends || newline || i >= n {
            let mut gap = String::new();
            while i < n && chars[i].is_whitespace() {
                gap.push(chars[i]);
                i += 1;
            }
            let trimmed = cur.trim_end().to_string();
            let extra: String = cur[trimmed.len()..].to_string();
            if trimmed.is_empty() {
                // only whitespace: it belongs to the gap of the sentence before, or to the lead
                match out.parts.last_mut() {
                    Some(last) => last.gap.push_str(&format!("{extra}{gap}")),
                    None => out.lead.push_str(&format!("{extra}{gap}")),
                }
            } else {
                out.parts.push(Sentence { text: trimmed, gap: format!("{extra}{gap}") });
            }
            cur.clear();
        }
    }
    out
}

/// The text as sentences only (no whitespace).
pub fn sentences(text: &str) -> Vec<String> {
    split(text).parts.into_iter().map(|p| p.text).collect()
}

/// `[pause 1.5s]` / `[pause 500ms]` as ticks (at most ten seconds), `None` for anything else.
fn pause_ticks(marker: &str) -> Option<i64> {
    let inner = marker.strip_prefix('[')?.strip_suffix(']')?.trim().to_ascii_lowercase();
    let arg = inner.strip_prefix("pause")?.trim();
    let (num, per_second) = match arg.strip_suffix("ms") {
        Some(n) => (n.trim(), 1000.0),
        None => (arg.strip_suffix('s')?.trim(), 1.0),
    };
    if num.is_empty() || num.len() > 10 || !num.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let seconds: f64 = num.parse().ok()?;
    let seconds = (seconds / per_second).clamp(0.0, 10.0);
    Some((seconds * TICKS_PER_SECOND as f64).round() as i64)
}

/// Words in `text` that are spoken (markers in square brackets are not), and the pauses they ask for.
pub fn words_and_pauses(text: &str) -> (u64, Tick) {
    let mut words = 0u64;
    let mut pauses = 0i64;
    let mut rest = text;
    let mut spoken = String::new();
    while let Some(open) = rest.find('[') {
        spoken.push_str(&rest[..open]);
        spoken.push(' ');
        let from = &rest[open..];
        match from.find(']') {
            Some(close) => {
                if let Some(t) = from.get(..=close).and_then(pause_ticks) {
                    pauses = pauses.saturating_add(t);
                }
                rest = from.get(close + 1..).unwrap_or("");
            }
            None => {
                rest = &from[1..];
            }
        }
    }
    spoken.push_str(rest);
    for w in spoken.split_whitespace() {
        if w.chars().any(char::is_alphanumeric) {
            words += 1;
        }
    }
    (words, Tick(pauses))
}

/// How long `text` takes to say at `words_per_minute` (a figure outside 40 to 400 is replaced by
/// 150): its words at that pace plus the pauses written in it. Integer arithmetic throughout.
pub fn speech_ticks(text: &str, words_per_minute: u32) -> Tick {
    let wpm = if (40..=400).contains(&words_per_minute) { u128::from(words_per_minute) } else { 150 };
    let (words, pauses) = words_and_pauses(text);
    let talk = u128::from(words) * 60 * TICKS_PER_SECOND as u128 / wpm;
    Tick(i64::try_from(talk).unwrap_or(i64::MAX).saturating_add(pauses.0))
}

/// Where a beat sits on the timeline if the plan is laid out from `start`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeatRange {
    pub beat: u64,
    pub section: u64,
    pub start: Tick,
    pub end: Tick,
}

/// Every beat's planned range, end to end from `start`, in plan order.
pub fn ranges(plan: &crate::Planner, start: Tick) -> Vec<BeatRange> {
    let mut at = start.0;
    let mut out = Vec::new();
    for s in &plan.sections {
        for b in &s.beats {
            let end = at.saturating_add(b.target.0.max(0));
            out.push(BeatRange { beat: b.id, section: s.id, start: Tick(at), end: Tick(end) });
            at = end;
        }
    }
    out
}
