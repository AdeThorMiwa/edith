//! Tests of `graphics.placeTextList` (fork roadmap F30): titles and on-screen text in one call and one undo step, each
//! role looking as its template does, long enough to read, on a track of their own, nothing changed when refused.

use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use crate::Session;

fn edit() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name": "Titles", "audio": 1, "video": 1})).unwrap();
    s
}

fn place(s: &mut Session, entries: Value) -> crate::Result<Value> {
    s.execute("graphics.placeTextList", json!({"entries": entries}))
}

fn secs(t: Tick) -> f64 {
    t.0 as f64 / TICKS_PER_SECOND as f64
}

/// Times land on frames (about 29.97 a second): a frame or so from what was asked.
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.05
}

fn titles(s: &Session) -> &filmcraft_project::Track {
    s.active_sequence().unwrap().video_tracks.iter().find(|t| t.name == "Titles").expect("a Titles track")
}

/// Every text the graphic clip shows, back to front.
fn words(it: &filmcraft_project::TrackItem) -> Vec<String> {
    it.effects
        .iter()
        .filter(|e| filmcraft_project::graphic::is_layer(e))
        .filter_map(|e| e.param("text").and_then(|p| if let filmcraft_project::ParamValue::Text(t) = &p.value { Some(t.clone()) } else { None }))
        .collect()
}

#[test]
fn every_role_is_placed_on_one_titles_track_with_fades_and_the_stretch_asked_for() {
    let mut s = edit();
    let before = s.active_sequence().unwrap().clone();
    let r = place(
        &mut s,
        json!([
            {"start": 1, "end": 5, "text": "The Long Way Home", "role": "title", "secondaryText": "a story in three parts", "reason": "The question the film asks."},
            {"start": 10, "end": 14, "text": "Part one", "role": "chapter", "secondaryText": "The shore"},
            {"start": 20, "end": 24, "text": "Marguerite Dunn", "role": "lowerThird", "secondaryText": "Harbour master"},
            {"start": 30, "end": 36, "text": "Nobody ever asked the sea what it wanted.", "role": "quote"},
            {"start": 40, "end": 44, "text": "Filmed in March", "role": "note"},
        ]),
    )
    .unwrap();
    assert_eq!((r["placed"].clone(), r["trackMade"].clone()), (json!(5), json!(true)), "{r}");
    let t = titles(&s);
    assert_eq!(t.items.len(), 5);
    let top = s.active_sequence().unwrap().video_tracks.len() - 1;
    assert_eq!(s.active_sequence().unwrap().video_tracks[top].name, "Titles", "on top, over the picture");
    for (item, (a, b)) in t.items.iter().zip([(1.0, 5.0), (10.0, 14.0), (20.0, 24.0), (30.0, 36.0), (40.0, 44.0)]) {
        assert!(near(secs(item.start), a) && near(secs(item.end()), b), "{} {}", secs(item.start), secs(item.end()));
    }
    // the words are on the clips, each role in the template that fits it
    assert!(
        words(&t.items[0]).contains(&"The Long Way Home".to_string()) && words(&t.items[0]).contains(&"a story in three parts".to_string()),
        "{:?}",
        words(&t.items[0])
    );
    assert!(words(&t.items[1]).contains(&"Part one".to_string()) && words(&t.items[1]).contains(&"The shore".to_string()), "{:?}", words(&t.items[1]));
    assert!(
        words(&t.items[2]).contains(&"Marguerite Dunn".to_string()) && words(&t.items[2]).contains(&"Harbour master".to_string()),
        "{:?}",
        words(&t.items[2])
    );
    assert!(words(&t.items[3]).iter().any(|w| w.starts_with("Nobody ever asked")), "{:?}", words(&t.items[3]));
    assert_eq!(words(&t.items[4]), ["Filmed in March"]);
    assert!(t.items[0].markers.iter().any(|m| m.name == "Why this text" && m.comment.contains("question")));
    // each fades in and out
    assert_eq!(t.transitions.len(), 10, "{}", t.transitions.len());
    // one undo step takes the track and everything on it away
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(*s.active_sequence().unwrap(), before);
}

#[test]
fn titles_that_touch_share_one_dissolve_and_a_second_call_uses_the_same_track() {
    let mut s = edit();
    place(
        &mut s,
        json!([{"start": 2, "end": 5, "text": "First place", "role": "lowerThird"}, {"start": 5, "end": 8, "text": "Second place", "role": "lowerThird"}]),
    )
    .unwrap();
    assert_eq!(titles(&s).transitions.len(), 3, "in, one between them, out");
    let tracks = s.active_sequence().unwrap().video_tracks.len();
    let again = place(&mut s, json!([{"start": 20, "end": 23, "text": "Third place", "role": "lowerThird"}])).unwrap();
    assert_eq!(again["trackMade"], json!(false));
    assert_eq!(s.active_sequence().unwrap().video_tracks.len(), tracks);
    assert_eq!(titles(&s).items.len(), 3);
}

#[test]
fn a_title_is_refused_when_it_cannot_be_read_or_overlaps_or_is_not_well_formed_and_nothing_changes() {
    let mut s = edit();
    place(&mut s, json!([{"start": 10, "end": 14, "text": "Already here", "role": "title"}])).unwrap();
    let before = s.active_sequence().unwrap().clone();
    let refuse = |s: &mut Session, entries: Value, needle: &str| {
        let e = place(s, entries).unwrap_err().to_string();
        assert!(e.contains(needle), "wanted “{needle}” in: {e}");
    };
    refuse(&mut s, json!([{"start": 20, "end": 21, "text": "This sentence has far too many words to read in a second", "role": "quote"}]), "need about");
    refuse(
        &mut s,
        json!([{"start": 20, "end": 24, "text": "Fine", "role": "note"}, {"start": 22, "end": 26, "text": "Overlaps", "role": "note"}]),
        "entries 1 and 2 overlap",
    );
    refuse(&mut s, json!([{"start": 12, "end": 16, "text": "On top of another", "role": "title"}]), "already has");
    refuse(&mut s, json!([{"start": 20, "end": 24, "text": "No role"}]), "title, chapter, lowerThird, quote or note");
    refuse(&mut s, json!([{"start": 20, "end": 24, "text": "Bad role", "role": "headline"}]), "title, chapter, lowerThird, quote or note");
    refuse(&mut s, json!([{"start": 20, "end": 24, "text": "   ", "role": "title"}]), "`text` is required");
    refuse(&mut s, json!([{"start": 24, "end": 20, "text": "Backwards", "role": "title"}]), "less than one frame");
    refuse(&mut s, json!([{"start": 20, "end": 24, "text": "Odd template", "role": "title", "template": "no-such-template"}]), "entry 1");
    refuse(&mut s, json!([{"start": 20, "end": 24, "text": "x".repeat(300), "role": "title"}]), "`text` is required");
    refuse(&mut s, json!([]), "needs 1 to");
    refuse(&mut s, json!([{"start": 20, "end": 24, "text": "Fine", "role": "note", "reason": 5}]), "`reason`");
    assert_eq!(*s.active_sequence().unwrap(), before, "no refused call left a title or a track");
}

#[test]
fn another_template_can_be_asked_for_by_name() {
    let mut s = edit();
    place(&mut s, json!([{"start": 2, "end": 6, "text": "The harbour", "role": "title", "secondaryText": "March", "template": "builtin:title-boxed"}]))
        .unwrap();
    let t = titles(&s);
    assert!(words(&t.items[0]).iter().any(|w| w.contains("The harbour")), "{:?}", words(&t.items[0]));
}
