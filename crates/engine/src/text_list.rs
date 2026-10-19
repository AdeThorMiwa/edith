//! `graphics.placeTextList` (fork roadmap F30): titles and on-screen text on the timeline in one call, the way
//! `timeline.placeCutList` puts the pictures there and `timeline.placeSoundList` the sound.
//!
//! Each entry is a stretch of the timeline, the words, and a **role**. The role decides how it looks, from the app's
//! own graphics templates, so Edith chooses meaning and the look stays consistent:
//!
//! | role | made with | `text` is | `secondaryText` is |
//! |---|---|---|---|
//! | `title` | Title – Centered | the title | a subtitle |
//! | `chapter` | Title – Centered, smaller | the chapter's name | a line under it |
//! | `lowerThird` | Lower Third – Rule | a name or place | its role or what it is |
//! | `quote` | italic text in a box, centred | the quotation | |
//! | `note` | small text at the bottom | the note | |
//!
//! `template` names another template of the library for one entry (its first two text properties take `text` and
//! `secondaryText`). Everything goes on one video track named "Titles" (made on top when it is first needed), each
//! fading in and out with the default video transition (a short dissolve, and none between two that touch).
//!
//! Everything is checked first; a bad entry refuses the whole call and names the entry. One undo step. Rules: the text
//! must be **readable for as long as it is up** (a second plus a quarter of a second for each word, text and second
//! line together), it may not overlap another on the track, and it may not land on something already on the Titles
//! track.
//!
//! `graphics.placeTextList {entries: [{start, end, text, role, secondaryText?, template?, reason?}]}`: times are
//! seconds, `"1:25"` or a timecode.

use filmcraft_project::{ClipId, Label, Marker, MarkerId, MarkerKind, TrackId};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use crate::assistant_tools::parse_time;
use crate::commands::{CommandSpec, bad, has_seq};
use crate::{EngineError, Result, Session};

/// Entries in one call.
pub const MAX_ENTRIES: usize = 300;
const MAX_TEXT: usize = 200;
const MAX_SECONDARY: usize = 150;
const MAX_REASON: usize = 300;
/// How long a reader needs: this, plus a quarter second a word.
const READ_BASE_SECONDS: f64 = 1.0;
const READ_PER_WORD_SECONDS: f64 = 0.25;
const TRACK_NAME: &str = "Titles";
const CMD: &str = "graphics.placeTextList";

fn secs(t: Tick) -> f64 {
    t.0 as f64 / TICKS_PER_SECOND as f64
}

/// The shortest a card of `words` words may stay up, seconds.
pub(crate) fn reading_seconds(words: usize) -> f64 {
    READ_BASE_SECONDS + READ_PER_WORD_SECONDS * words as f64
}

pub(crate) fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Title,
    Chapter,
    LowerThird,
    Quote,
    Note,
}

impl Role {
    fn parse(s: &str) -> Option<Role> {
        match s.trim().to_ascii_lowercase().replace(['_', '-', ' '], "").as_str() {
            "title" => Some(Role::Title),
            "chapter" | "section" => Some(Role::Chapter),
            "lowerthird" | "name" | "location" => Some(Role::LowerThird),
            "quote" => Some(Role::Quote),
            "note" | "caption" => Some(Role::Note),
            _ => None,
        }
    }

    /// How long the dissolve at each end is, seconds.
    fn fade(self) -> f64 {
        match self {
            Role::Title | Role::Chapter => 0.5,
            Role::Quote => 0.5,
            Role::LowerThird | Role::Note => 0.3,
        }
    }
}

struct Card {
    n: usize,
    start: Tick,
    end: Tick,
    text: String,
    secondary: String,
    role: Role,
    template: Option<String>,
    reason: String,
}

fn clean(v: &Value, max: usize, multiline: bool) -> Option<String> {
    let t: String = v.as_str()?.chars().map(|c| if c.is_control() && !(multiline && c == '\n') { ' ' } else { c }).collect();
    let t = if multiline {
        t.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n")
    } else {
        t.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    let t = t.trim().to_string();
    (t.chars().count() <= max).then_some(t)
}

fn read_cards(s: &Session, p: &Value) -> Result<Vec<Card>> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rate = q.settings.frame_rate;
    let list = p.get("entries").and_then(Value::as_array).ok_or_else(|| bad(CMD, "`entries` must be a list of titles"))?;
    if list.is_empty() || list.len() > MAX_ENTRIES {
        return Err(bad(CMD, format!("`entries` needs 1 to {MAX_ENTRIES} titles")));
    }
    let frame = rate.frame_duration();
    let mut cards = Vec::with_capacity(list.len());
    for (i, e) in list.iter().enumerate() {
        let n = i + 1;
        let no = |why: String| bad(CMD, format!("entry {n}: {why}"));
        if !e.is_object() {
            return Err(no("is not an object with start, end, text and role".into()));
        }
        let time = |key: &str| -> Result<Tick> {
            let v = e.get(key).filter(|v| !v.is_null()).ok_or_else(|| no(format!("`{key}` is required")))?;
            parse_time(v, q, key).map(|t| rate.snap_nearest(t)).map_err(no)
        };
        let (start, end) = (time("start")?, time("end")?);
        if start < Tick::ZERO {
            return Err(no("starts before the beginning of the timeline".into()));
        }
        if end - start < frame {
            return Err(no(format!("runs {:.3} s to {:.3} s, which is less than one frame; `end` must be after `start`", secs(start), secs(end))));
        }
        let role =
            e.get("role").and_then(Value::as_str).and_then(Role::parse).ok_or_else(|| no("`role` must be title, chapter, lowerThird, quote or note".into()))?;
        let text = e
            .get("text")
            .and_then(|v| clean(v, MAX_TEXT, role == Role::Quote))
            .filter(|t| !t.is_empty())
            .ok_or_else(|| no(format!("`text` is required: the words to show, up to {MAX_TEXT} characters")))?;
        let secondary = match e.get("secondaryText").filter(|v| !v.is_null()) {
            None => String::new(),
            Some(v) => clean(v, MAX_SECONDARY, false).ok_or_else(|| no(format!("`secondaryText` must be text of up to {MAX_SECONDARY} characters")))?,
        };
        let template = match e.get("template").filter(|v| !v.is_null()) {
            None => None,
            Some(v) => {
                Some(clean(v, 100, false).filter(|t| !t.is_empty()).ok_or_else(|| no("`template` must be the id or name of a graphics template".into()))?)
            }
        };
        // long enough to read
        let needed = reading_seconds(word_count(&text) + word_count(&secondary));
        let up = secs(end - start);
        if up + 0.05 < needed {
            return Err(no(format!(
                "“{text}” would be up for {up:.1} s, but {} word{} need about {needed:.1} s to read (a second, and a quarter of a second a word); make it longer or shorten the words",
                word_count(&text) + word_count(&secondary),
                if word_count(&text) + word_count(&secondary) == 1 { "" } else { "s" }
            )));
        }
        let reason: String = match e.get("reason") {
            None | Some(Value::Null) => String::new(),
            Some(r) => clean(r, MAX_REASON, false).ok_or_else(|| no(format!("`reason` must be text of up to {MAX_REASON} characters")))?,
        };
        cards.push(Card { n, start, end, text, secondary, role, template, reason });
    }
    Ok(cards)
}

fn check_overlaps(cards: &[Card]) -> Result<()> {
    let mut order: Vec<&Card> = cards.iter().collect();
    order.sort_by_key(|c| (c.start, c.end));
    for w in order.windows(2) {
        if w[1].start < w[0].end {
            return Err(bad(
                CMD,
                format!(
                    "entries {} and {} overlap: {:.2}–{:.2} s and {:.2}–{:.2} s; one title ends where the next begins",
                    w[0].n,
                    w[1].n,
                    secs(w[0].start),
                    secs(w[0].end),
                    secs(w[1].start),
                    secs(w[1].end)
                ),
            ));
        }
    }
    Ok(())
}

/// The Titles track, by name.
pub(crate) fn titles_track(s: &Session) -> Option<TrackId> {
    s.active_sequence()?.video_tracks.iter().find(|t| t.name.trim().eq_ignore_ascii_case(TRACK_NAME)).map(|t| t.id)
}

/// The Titles track, made on top when there is none.
fn ensure_track(s: &mut Session, made: &mut bool) -> Result<TrackId> {
    if let Some(t) = titles_track(s) {
        return Ok(t);
    }
    let r = s.execute("sequence.addTracks", json!({"video": 1, "audio": 0}))?;
    let id = r["video"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(Value::as_u64)
        .map(TrackId)
        .ok_or_else(|| bad(CMD, "a track for the titles could not be made"))?;
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    s.edit("Name Track", |p, _| {
        let seq = p.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        if let Some(t) = seq.video_tracks.iter_mut().find(|t| t.id == id) {
            t.name = TRACK_NAME.to_string();
        }
        Ok(())
    })?;
    *made = true;
    Ok(id)
}

fn track_index(s: &Session, id: TrackId) -> Result<usize> {
    s.active_sequence().ok_or(EngineError::NoSequence)?.video_tracks.iter().position(|t| t.id == id).ok_or_else(|| bad(CMD, "the titles track went missing"))
}

/// A soft shadow behind every text layer (words have to read over a bright sky) and, when asked, the text centred.
fn style_text(s: &mut Session, clip: ClipId, centre: bool, n: usize) -> Result<()> {
    let list = s.execute("graphics.list", json!({"clip": clip.0}))?;
    for l in list["layers"].as_array().into_iter().flatten().filter(|l| l["kind"] == "text") {
        let Some(layer) = l["layer"].as_u64() else { continue };
        let mut props = json!({"shadow": true, "shadow_distance": 3, "shadow_blur": 12, "shadow_opacity": 85});
        if centre {
            props["align"] = json!("center");
        }
        s.execute("graphics.set", json!({"clip": clip.0, "layer": layer, "props": props}))
            .map_err(|e| bad(CMD, format!("entry {n}: the text could not be styled: {e}")))?;
    }
    Ok(())
}

/// Puts one card on the timeline at `track` and returns its clip.
fn place_card(s: &mut Session, c: &Card, track: usize) -> Result<ClipId> {
    let at = c.start.0;
    let seconds = secs(c.end - c.start);
    let fail = |e: EngineError| bad(CMD, format!("entry {}: {e}", c.n));
    let r = match (&c.template, c.role) {
        (Some(t), _) => {
            let mut r = s.execute("graphics.template.apply", json!({"template": t, "time": at, "track": track})).map_err(fail)?;
            // the first two text properties take the words
            let clip = r["clip"].as_u64().unwrap_or(0);
            let texts: Vec<String> =
                r["controls"].as_array().into_iter().flatten().filter(|k| k["kind"] == "text").filter_map(|k| k["id"].as_str().map(str::to_string)).collect();
            let mut values = serde_json::Map::new();
            if let Some(id) = texts.first() {
                values.insert(id.clone(), json!(c.text));
            }
            if let (Some(id), false) = (texts.get(1), c.secondary.is_empty()) {
                values.insert(id.clone(), json!(c.secondary));
            }
            if !values.is_empty() {
                s.execute("graphics.template.set", json!({"clip": clip, "values": values})).map_err(fail)?;
            }
            r["clip"] = json!(clip);
            r
        }
        (None, Role::Title | Role::Chapter) => {
            let mut values = json!({"title": c.text, "subtitle": c.secondary});
            if c.role == Role::Chapter {
                values["title_size"] = json!(76);
            }
            s.execute("graphics.template.apply", json!({"template": "builtin:title-centered", "values": values, "time": at, "track": track})).map_err(fail)?
        }
        (None, Role::LowerThird) => {
            // no second line: its text is left empty, which draws nothing
            let values = json!({"name": c.text, "role": c.secondary});
            s.execute("graphics.template.apply", json!({"template": "builtin:lower-third-rule", "values": values, "time": at, "track": track})).map_err(fail)?
        }
        (None, Role::Quote) => s
            .execute(
                "graphics.newText",
                json!({"text": c.text, "size": 64, "fontStyle": "Italic", "box": [1400, 500], "position": [260, 380], "seconds": seconds, "time": at, "track": track}),
            )
            .map_err(fail)?,
        (None, Role::Note) => s
            .execute("graphics.newText", json!({"text": c.text, "size": 40, "box": [1500, 120], "position": [210, 880], "seconds": seconds, "time": at, "track": track}))
            .map_err(fail)?,
    };
    let clip = ClipId(r["clip"].as_u64().ok_or_else(|| bad(CMD, format!("entry {}: nothing was placed", c.n)))?);
    style_text(s, clip, c.role == Role::Quote, c.n)?;
    // a template has a length of its own: make it the stretch asked for
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let (duration, reason) = (c.end - c.start, c.reason.clone());
    s.edit("Title Details", |pr, _| {
        let marker = MarkerId(pr.alloc_id());
        let seq = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        let (_, it) = seq.find_item_mut(clip).ok_or_else(|| bad(CMD, "the title was removed while it was being placed"))?;
        it.duration = duration;
        if !reason.is_empty() {
            it.markers.push(Marker {
                id: marker,
                start: Tick::ZERO,
                duration: Tick::ZERO,
                name: "Why this text".into(),
                comment: reason,
                kind: MarkerKind::Comment,
                color: Label::Green,
            });
        }
        Ok(())
    })?;
    Ok(clip)
}

fn place_text_list(s: &mut Session, p: &Value) -> Result<Value> {
    let cards = read_cards(s, p)?;
    check_overlaps(&cards)?;
    // nothing already on the Titles track is covered
    if let Some(q) = s.active_sequence()
        && let Some(track) = titles_track(s).and_then(|id| q.video_tracks.iter().find(|t| t.id == id))
    {
        for c in &cards {
            if let Some(other) = track.items.iter().find(|i| i.start < c.end && i.end() > c.start) {
                return Err(bad(
                    CMD,
                    format!(
                        "entry {}: the {TRACK_NAME} track already has “{}” from {:.1} to {:.1} s; choose another stretch, or delete that title first",
                        c.n,
                        other.name,
                        secs(other.start),
                        secs(other.end())
                    ),
                ));
            }
        }
    }
    let label = format!("Place Text List ({} title{})", cards.len(), if cards.len() == 1 { "" } else { "s" });
    let mut order: Vec<&Card> = cards.iter().collect();
    order.sort_by_key(|c| c.start);
    let (clips, made) = s.edit_group(&label, |s| {
        let mut made = false;
        let id = ensure_track(s, &mut made)?;
        let index = track_index(s, id)?;
        let mut placed: Vec<(usize, ClipId)> = Vec::with_capacity(order.len());
        for c in &order {
            placed.push((c.n, place_card(s, c, index)?));
        }
        // fades: in at the start and out at the end, except where two touch (one dissolve between them)
        let frame = s.sequence_rate().frame_duration();
        for (k, c) in order.iter().enumerate() {
            let Some((_, clip)) = placed.get(k) else { continue };
            let frames = {
                let want = Tick::from_seconds_f64(c.role.fade());
                let third = Tick((c.end - c.start).0 / 3);
                (want.min(third).0 / frame.0.max(1)).max(1)
            };
            let touches_next = order.get(k + 1).is_some_and(|n| n.start == c.end);
            let fail = |e: EngineError| bad(CMD, format!("entry {}: the fade could not be made: {e}", c.n));
            s.execute("sequence.applyVideoTransition", json!({"clip": clip.0, "edge": "in", "frames": frames})).map_err(fail)?;
            if !touches_next {
                s.execute("sequence.applyVideoTransition", json!({"clip": clip.0, "edge": "out", "frames": frames})).map_err(fail)?;
            }
        }
        Ok((placed.into_iter().map(|(_, c)| c).collect::<Vec<_>>(), made))
    })?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let track = titles_track(s).and_then(|id| q.video_tracks.iter().position(|t| t.id == id)).map_or_else(String::new, |i| format!("V{}", i + 1));
    Ok(json!({"placed": cards.len(), "clips": clips.iter().map(|c| c.0).collect::<Vec<_>>(), "track": track, "trackMade": made}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "graphics.placeTextList",
        label: "Place a Text List",
        menu: &[],
        shortcut: None,
        params: r#"{"entries":[{"start":time,"end":time,"text":str,"role":"title|chapter|lowerThird|quote|note","secondaryText":str?,"template":str?,"reason":str?}]}"#,
        enabled: has_seq,
        run: place_text_list,
        journal: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_needs_a_second_and_a_quarter_of_a_second_a_word() {
        assert_eq!(reading_seconds(0), 1.0);
        assert_eq!(reading_seconds(4), 2.0);
        assert_eq!(word_count("  The   long way  home "), 4);
        assert_eq!(word_count(""), 0);
    }

    #[test]
    fn roles_are_read_in_their_common_spellings() {
        assert_eq!(Role::parse("lowerThird"), Some(Role::LowerThird));
        assert_eq!(Role::parse("Lower third"), Some(Role::LowerThird));
        assert_eq!(Role::parse("lower_third"), Some(Role::LowerThird));
        assert_eq!(Role::parse("Chapter"), Some(Role::Chapter));
        assert_eq!(Role::parse("headline"), None);
    }

    #[test]
    fn text_is_cleaned_and_bounded() {
        assert_eq!(clean(&json!("  A\u{7}  B\nC "), 20, false).as_deref(), Some("A B C"));
        assert_eq!(clean(&json!("Line one\n\nline  two"), 40, true).as_deref(), Some("Line one\n\nline two"));
        assert!(clean(&json!("x".repeat(30)), 20, false).is_none());
        assert!(clean(&json!(5), 20, false).is_none());
    }
}
