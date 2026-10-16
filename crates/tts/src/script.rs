//! Script parsing: speech text and pause markers.
//!
//! A pause marker is `[pause <n>s]` or `[pause <n>ms]` (case-insensitive, `n` a decimal number,
//! spaces optional: `[pause 1.5s]`, `[Pause 500 ms]`). Pauses are capped at [`MAX_PAUSE_MS`]. A
//! bracket that is not a well-formed marker is ordinary text. The Text to Speech panel's
//! *Add pause* button inserts `[pause 1s]`.
//!
//! **Directions** change how the text after them is spoken, until the next direction (they stick):
//! `[pace 0.85]` (also `0.85x`, `85%`) is that many times the narration's pace, `[pitch -2]`
//! (also `+1.5`, `-2st`) that many semitones from its vocal pitch, `[normal]` goes back to the
//! narration's own settings. A direction *sets* the value, it does not stack on the previous one.
//! Place them between sentences. Results are clamped to the voice's limits by the caller.

/// Longest single pause.
pub const MAX_PAUSE_MS: u32 = 10_000;

/// A change of delivery, from the marker's place on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Direction {
    /// Pace as a multiple of the narration's pace.
    Pace(f64),
    /// Pitch in semitones from the narration's vocal pitch.
    Pitch(f64),
    /// The narration's own pace and pitch.
    Normal,
}

/// A piece of a script.
#[derive(Clone, Debug, PartialEq)]
pub enum Segment {
    Speech(String),
    Pause { millis: u32 },
    Direction(Direction),
}

/// The marker *Add pause* inserts.
pub const PAUSE_MARKER: &str = "[pause 1s]";

/// Split `text` into speech and pauses, in order. Empty speech pieces are dropped.
pub fn parse(text: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut speech = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let (before, from) = rest.split_at(open);
        speech.push_str(before);
        let close = from.find(']');
        let marker = close.and_then(|c| from.get(..=c));
        let found = marker.and_then(|m| pause_millis(m).map(|millis| Segment::Pause { millis }).or_else(|| direction(m).map(Segment::Direction)));
        match (close, found) {
            (Some(close), Some(segment)) => {
                flush(&mut out, &mut speech);
                out.push(segment);
                rest = from.get(close + 1..).unwrap_or("");
            }
            _ => {
                speech.push('[');
                rest = from.get(1..).unwrap_or("");
            }
        }
    }
    speech.push_str(rest);
    flush(&mut out, &mut speech);
    out
}

fn flush(out: &mut Vec<Segment>, speech: &mut String) {
    if !speech.trim().is_empty() {
        out.push(Segment::Speech(std::mem::take(speech)));
    }
    speech.clear();
}

/// `[pause 1.5s]` → 1500. None if `marker` is not a pause marker.
fn pause_millis(marker: &str) -> Option<u32> {
    let inner = marker.strip_prefix('[')?.strip_suffix(']')?.trim();
    let lower = inner.to_ascii_lowercase();
    let arg = lower.strip_prefix("pause")?.trim();
    let (num, scale) = if let Some(n) = arg.strip_suffix("ms") { (n.trim(), 1.0) } else { (arg.strip_suffix('s')?.trim(), 1000.0) };
    if num.is_empty() || num.len() > 12 || !num.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let v: f64 = num.parse().ok()?;
    let ms = (v * scale).round();
    if !ms.is_finite() || ms < 0.0 {
        return None;
    }
    Some(ms.min(f64::from(MAX_PAUSE_MS)) as u32)
}

/// `[pace 0.9]`, `[pace 90%]`, `[pitch -2]`, `[normal]`. None if `marker` is not a direction.
fn direction(marker: &str) -> Option<Direction> {
    let lower = marker.strip_prefix('[')?.strip_suffix(']')?.trim().to_ascii_lowercase();
    if lower == "normal" {
        return Some(Direction::Normal);
    }
    if let Some(arg) = lower.strip_prefix("pace") {
        let arg = arg.trim();
        let (num, scale) = match arg.strip_suffix('%') {
            Some(n) => (n.trim(), 0.01),
            None => (arg.strip_suffix('x').unwrap_or(arg).trim(), 1.0),
        };
        let v = plain_number(num)? * scale;
        return (v > 0.0 && v.is_finite()).then_some(Direction::Pace(v));
    }
    let arg = lower.strip_prefix("pitch")?.trim();
    let arg = arg.strip_suffix("st").unwrap_or(arg).trim();
    let (sign, num) = match arg.strip_prefix('-') {
        Some(n) => (-1.0, n),
        None => (1.0, arg.strip_prefix('+').unwrap_or(arg)),
    };
    let v = sign * plain_number(num.trim())?;
    v.is_finite().then_some(Direction::Pitch(v))
}

/// Digits with at most one dot, at most eight characters (no signs, no exponents).
fn plain_number(s: &str) -> Option<f64> {
    if s.is_empty() || s.len() > 8 || !s.chars().all(|c| c.is_ascii_digit() || c == '.') || s.matches('.').count() > 1 {
        return None;
    }
    s.parse().ok()
}

/// A stretch of script spoken with one delivery: `text` (speech and `[pause Nms]` markers, no
/// directions), at `pace` times the narration's pace and `semitones` from its vocal pitch.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub pace: f64,
    pub semitones: f64,
}

/// Whether the script has any direction marker.
pub fn has_directions(text: &str) -> bool {
    parse(text).iter().any(|s| matches!(s, Segment::Direction(_)))
}

/// The script as stretches that share a delivery, in order. Directions set the delivery for what
/// follows (they do not stack); `[normal]` resets it. Stretches with no text are dropped.
pub fn runs(text: &str) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    let mut cur = Run { text: String::new(), pace: 1.0, semitones: 0.0 };
    for seg in parse(text) {
        let (pace, semitones) = match seg {
            Segment::Speech(s) => {
                cur.text.push_str(&s);
                continue;
            }
            Segment::Pause { millis } => {
                cur.text.push_str(&format!("[pause {millis}ms]"));
                continue;
            }
            Segment::Direction(Direction::Pace(p)) => (p, cur.semitones),
            Segment::Direction(Direction::Pitch(st)) => (cur.pace, st),
            Segment::Direction(Direction::Normal) => (1.0, 0.0),
        };
        if pace == cur.pace && semitones == cur.semitones {
            continue; // nothing changes, so the reading is not split here
        }
        if cur.text.is_empty() {
            cur.pace = pace;
            cur.semitones = semitones;
        } else {
            out.push(std::mem::replace(&mut cur, Run { text: String::new(), pace, semitones }));
        }
    }
    if !cur.text.is_empty() {
        out.push(cur);
    }
    out
}

/// The script with its directions taken out (speech and pause markers kept).
pub fn strip_directions(text: &str) -> String {
    runs(text).into_iter().map(|r| r.text).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(s: &str) -> Segment {
        Segment::Speech(s.into())
    }

    #[test]
    fn splits_speech_and_pauses() {
        assert_eq!(parse("Hello. [pause 1s] World"), vec![sp("Hello. "), Segment::Pause { millis: 1000 }, sp(" World")]);
        assert_eq!(parse("[Pause 500 ms]Go"), vec![Segment::Pause { millis: 500 }, sp("Go")]);
        assert_eq!(parse("a[pause 1.5s][pause 2s]b"), vec![sp("a"), Segment::Pause { millis: 1500 }, Segment::Pause { millis: 2000 }, sp("b")]);
        assert_eq!(parse(PAUSE_MARKER), vec![Segment::Pause { millis: 1000 }]);
    }

    #[test]
    fn malformed_markers_are_text() {
        assert_eq!(parse("[pause]"), vec![sp("[pause]")]);
        assert_eq!(parse("[pause 1x] a"), vec![sp("[pause 1x] a")]);
        assert_eq!(parse("[pause 1s"), vec![sp("[pause 1s")]);
        assert_eq!(parse("[note] [pause -1s]"), vec![sp("[note] [pause -1s]")]);
        assert_eq!(parse("[[pause 1s]]"), vec![sp("["), Segment::Pause { millis: 1000 }, sp("]")]);
    }

    #[test]
    fn hostile_inputs_never_panic_and_pauses_are_capped() {
        assert_eq!(parse("[pause 999999999999s]"), vec![Segment::Pause { millis: MAX_PAUSE_MS }]);
        assert_eq!(parse("[pause 1e9s]"), vec![sp("[pause 1e9s]")]);
        for s in ["", "[", "]", "[[[[", "日本語[pause 1s]👍", "\u{301}[", "[pause 1.2.3s]", "\0[pause 0s]"] {
            let _ = parse(s);
        }
        assert_eq!(parse("  "), vec![]);
    }

    fn dir(s: &str) -> Option<Direction> {
        match parse(s).as_slice() {
            [Segment::Direction(d)] => Some(*d),
            _ => None,
        }
    }

    #[test]
    fn directions_in_all_their_spellings() {
        assert_eq!(dir("[pace 0.85]"), Some(Direction::Pace(0.85)));
        assert_eq!(dir("[Pace 1.2x]"), Some(Direction::Pace(1.2)));
        assert_eq!(dir("[pace 90%]"), Some(Direction::Pace(0.9)));
        assert_eq!(dir("[ pace  1 ]"), Some(Direction::Pace(1.0)));
        assert_eq!(dir("[pitch -2]"), Some(Direction::Pitch(-2.0)));
        assert_eq!(dir("[pitch +1.5]"), Some(Direction::Pitch(1.5)));
        assert_eq!(dir("[PITCH 3st]"), Some(Direction::Pitch(3.0)));
        assert_eq!(dir("[pitch -0.5 st]"), Some(Direction::Pitch(-0.5)));
        assert_eq!(dir("[normal]"), Some(Direction::Normal));
        assert_eq!(dir("[ Normal ]"), Some(Direction::Normal));
        assert_eq!(parse("A. [pace 0.9] B."), vec![sp("A. "), Segment::Direction(Direction::Pace(0.9)), sp(" B.")]);
    }

    #[test]
    fn malformed_directions_are_text_like_a_bad_pause() {
        for bad in [
            "[pace]",
            "[pace fast]",
            "[pace 0]",
            "[pace -1]",
            "[pace 1e9]",
            "[pace 1.2.3]",
            "[pitch]",
            "[pitch two]",
            "[pitch --2]",
            "[pitch 123456789]",
            "[normal x]",
            "[normally]",
            "[pace 0.9",
        ] {
            assert_eq!(parse(bad), vec![sp(bad)], "{bad}");
        }
    }

    #[test]
    fn runs_group_the_script_by_delivery_and_directions_set_rather_than_stack() {
        let r = |text: &str, pace: f64, semitones: f64| Run { text: text.into(), pace, semitones };
        // no directions: one run, the text unchanged
        assert_eq!(runs("Hello. World."), vec![r("Hello. World.", 1.0, 0.0)]);
        assert_eq!(runs("Hi. [pause 1.5s] There."), vec![r("Hi. [pause 1500ms] There.", 1.0, 0.0)]);
        // a direction affects what follows; pace and pitch are independent; they stick
        assert_eq!(
            runs("One. [pace 0.8] Two. [pitch -2] Three. [pace 0.8] Four."),
            vec![r("One. ", 1.0, 0.0), r(" Two. ", 0.8, 0.0), r(" Three.  Four.", 0.8, -2.0)],
            "a direction that changes nothing does not split the reading"
        );
        assert_eq!(runs("A. [pace 0.5] [pace 2] B."), vec![r("A. ", 1.0, 0.0), r(" B.", 2.0, 0.0)]);
        assert_eq!(runs("A. [pace 0.5] B. [normal] C."), vec![r("A. ", 1.0, 0.0), r(" B. ", 0.5, 0.0), r(" C.", 1.0, 0.0)]);
        // a direction first, or last, or alone
        assert_eq!(runs("[pitch 2] Hi."), vec![r(" Hi.", 1.0, 2.0)]);
        assert_eq!(runs("Hi. [pitch 2]"), vec![r("Hi. ", 1.0, 0.0)]);
        assert!(runs("[pace 0.5]").is_empty());
        assert!(runs("").is_empty());
        assert!(has_directions("a [normal] b") && !has_directions("a [pause 1s] b [note]"));
        assert_eq!(strip_directions("Hi. [pace 0.5] There. [pause 1s] Bye [pitch 1]."), "Hi.  There. [pause 1000ms] Bye .");
    }

    #[test]
    fn thousands_of_directions_and_hostile_text_never_panic() {
        let many: String = (0..10_000).map(|i| format!("[pace {}]w{i} ", 1 + i % 3)).collect();
        assert!(runs(&many).len() <= 10_000);
        for s in ["[pace 1][pace", "[[pace 1]]", "[pitch +][pitch -]", "\u{0}[normal]\u{301}", "日本[pace 2]語", "[pace 99999999]"] {
            let _ = (parse(s), runs(s), strip_directions(s));
        }
    }
}
