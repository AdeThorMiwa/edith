//! Tests of [`crate::soundplan`]: the spotting sheet as structured data and as text, its round trip
//! through a saved project, and the cues as markers on the timeline.

use filmcraft_time::TICKS_PER_SECOND;
use serde_json::json;

use crate::Session;
use crate::planner::get;

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("edith-soundplan-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn demo_with_plan() -> (Session, u64) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 10})).unwrap();
    let beat = get(&s.project).unwrap().beats().next().unwrap().id;
    s.execute(
        "planner.setSound",
        json!({"beat": beat, "ambience": "wind over a shore", "music": "uneasy strings", "musicInSeconds": 2, "musicOutSeconds": 20}),
    )
    .unwrap();
    s.execute("planner.addCue", json!({"beat": beat, "text": "a bell, far off", "atSeconds": 5})).unwrap();
    s.execute("planner.addCue", json!({"beat": beat, "kind": "silence", "text": "after the name", "atSeconds": 12, "lengthSeconds": 3})).unwrap();
    (s, beat)
}

fn markers(s: &Session) -> Vec<(String, i64, i64, String)> {
    s.active_sequence().unwrap().markers.iter().map(|m| (m.name.clone(), m.start.0, m.duration.0, m.comment.clone())).collect()
}

fn ours(s: &Session) -> Vec<(String, i64, i64, String)> {
    markers(s).into_iter().filter(|m| m.0.starts_with("Sound · ")).collect()
}

#[test]
fn the_commands_exist_and_the_sheet_is_not_an_edit() {
    let all = crate::commands::command_specs();
    for id in ["sound.sheet", "sound.placeCues"] {
        assert!(all.iter().any(|c| c.id == id), "{id}");
    }
    let (mut s, _) = demo_with_plan();
    let before = s.project.clone();
    s.execute("sound.sheet", json!({})).unwrap();
    assert!(std::sync::Arc::ptr_eq(&before, &s.project));
}

#[test]
fn the_sheet_lists_each_beat_in_order_with_its_bed_music_effects_and_silences() {
    let (mut s, beat) = demo_with_plan();
    let r = s.execute("sound.sheet", json!({})).unwrap();
    let text = r["text"].as_str().unwrap();
    for want in [
        "SPOTTING SHEET",
        "Times are from the start of the video.",
        "Cold open  0:00-",
        "  Ambience: wind over a shore",
        "  Music: uneasy strings, in at 0:02, out at 0:20",
        "  0:05  EFFECT   a bell, far off",
        "  0:12  SILENCE  3 s (after the name)",
        "nothing planned yet",
        "Nothing planned yet for:",
    ] {
        assert!(text.contains(want), "{want:?} is not in\n{text}");
    }
    let order: Vec<usize> = ["Ambience:", "Music:", "EFFECT", "SILENCE"].iter().map(|k| text.find(k).unwrap()).collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "bed, music, then the cues as they happen\n{text}");
    let beats = r["beats"].as_array().unwrap();
    assert_eq!(beats[0]["id"], beat);
    assert_eq!(beats[0]["planned"], true);
    assert_eq!(beats[0]["music"]["enters"], 2 * TICKS_PER_SECOND);
    assert_eq!(beats[0]["cues"][1]["kind"], "silence");
    assert_eq!(beats[0]["cues"][1]["start"], 12 * TICKS_PER_SECOND);
    assert!(beats.iter().skip(1).all(|b| b["planned"] == false), "a pack's sound note is not a sound plan");
    assert!(text.contains("  Note: "), "but the note is on the sheet: {text}");
    assert_eq!(r["planned"].as_u64().unwrap() as usize, beats.iter().filter(|b| b["planned"] == true).count());
    // one beat
    let one = s.execute("sound.sheet", json!({"beat": beat})).unwrap();
    assert_eq!(one["beats"].as_array().unwrap().len(), 1);
    assert!(!one["text"].as_str().unwrap().contains("Nothing planned yet for:"), "that one beat is planned");
    assert_eq!(one["planned"], 1);
}

#[test]
fn a_beat_that_is_placed_on_the_timeline_shifts_its_times() {
    let (mut s, beat) = demo_with_plan();
    s.execute("planner.link", json!({"beat": beat, "startSeconds": 60, "durationSeconds": 30})).unwrap();
    let r = s.execute("sound.sheet", json!({"beat": beat})).unwrap();
    let text = r["text"].as_str().unwrap();
    assert!(text.contains("Cold open  1:00-1:30"), "{text}");
    assert!(text.contains("1:05  EFFECT") && text.contains("1:12  SILENCE") && text.contains("in at 1:02, out at 1:20"), "{text}");
    assert_eq!(r["beats"][0]["cues"][0]["start"], 65 * TICKS_PER_SECOND);
    assert_eq!(r["beats"][0]["cues"][0]["inBeat"], 5 * TICKS_PER_SECOND, "the plan keeps times from the start of the beat");
}

#[test]
fn the_sheet_round_trips_through_the_project_and_exports_as_text() {
    let dir = scratch("sheet");
    let proj = dir.join("p.fcproj").to_string_lossy().to_string();
    let txt = dir.join("sheet.txt").to_string_lossy().to_string();
    let (mut s, _) = demo_with_plan();
    let r = s.execute("sound.sheet", json!({"path": txt})).unwrap();
    assert_eq!(r["path"], json!(txt));
    assert_eq!(std::fs::read_to_string(&txt).unwrap(), r["text"].as_str().unwrap(), "the file is the text");
    s.execute("file.saveAs", json!({"path": proj})).unwrap();
    let mut t = Session::default();
    t.execute("file.open", json!({"path": proj})).unwrap();
    assert_eq!(t.execute("sound.sheet", json!({})).unwrap()["text"], r["text"], "the same sheet after reopening");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_sheet_refuses_what_it_cannot_do() {
    let mut none = Session::default();
    assert!(none.execute("sound.sheet", json!({})).is_err(), "no plan");
    let (mut s, _) = demo_with_plan();
    for bad in [json!({"beat": 999_999}), json!({"path": ""}), json!({"path": 5}), json!({"path": "a\u{0}b"})] {
        assert!(s.execute("sound.sheet", bad.clone()).is_err(), "{bad}");
    }
    assert!(s.execute("sound.sheet", json!({"path": "/nonexistent-dir/x/sheet.txt"})).is_err());
}

#[test]
fn cues_become_markers_in_their_own_colours_and_asked_again_they_are_replaced_not_doubled() {
    let (mut s, beat) = demo_with_plan();
    let r = s.execute("sound.placeCues", json!({})).unwrap();
    assert_eq!(r["placed"], 4, "bed, music, one effect, one silence");
    let m = ours(&s);
    assert_eq!(m.len(), 4);
    let find = |k: &str| m.iter().find(|x| x.0.starts_with(&format!("Sound · {k}"))).cloned().unwrap_or_else(|| panic!("{k} in {m:?}"));
    assert_eq!(find("Music: uneasy strings").1, 2 * TICKS_PER_SECOND);
    assert_eq!(find("Music: uneasy strings").2, 18 * TICKS_PER_SECOND, "from in to out");
    assert_eq!((find("Effect: a bell, far off").1, find("Effect: a bell, far off").2), (5 * TICKS_PER_SECOND, 0), "a point");
    assert_eq!((find("Silence: after the name").1, find("Silence: after the name").2), (12 * TICKS_PER_SECOND, 3 * TICKS_PER_SECOND));
    assert!(find("Ambience: wind over a shore").3.starts_with(&format!("From the sound plan [beat {beat}]")));
    let colours: Vec<_> = s
        .active_sequence()
        .unwrap()
        .markers
        .iter()
        .filter(|x| x.name.starts_with("Sound · ") && x.comment.starts_with("From"))
        .map(|x| format!("{:?}", x.color))
        .collect();
    let mut sorted = colours.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 4, "each kind has its own colour: {colours:?}");
    // again: replaced, not doubled
    let n = markers(&s).len();
    s.execute("sound.placeCues", json!({})).unwrap();
    assert_eq!(markers(&s).len(), n);
    // a change to the plan, then place again: the marker moves
    let cue = get(&s.project).unwrap().beats().next().unwrap().sound.cues[0].id;
    s.execute("planner.updateCue", json!({"cue": cue, "atSeconds": 7})).unwrap();
    s.execute("sound.placeCues", json!({})).unwrap();
    assert_eq!(ours(&s).iter().find(|x| x.0.starts_with("Sound · Effect")).unwrap().1, 7 * TICKS_PER_SECOND);
    assert_eq!(markers(&s).len(), n);
    // one undo step takes the whole placing back
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(ours(&s).iter().find(|x| x.0.starts_with("Sound · Effect")).unwrap().1, 5 * TICKS_PER_SECOND);
}

#[test]
fn markers_that_are_not_ours_are_never_touched_and_clear_takes_only_ours() {
    let (mut s, beat) = demo_with_plan();
    // a marker of the person's own that merely looks like ours: no note from the plan, so it stays
    s.edit_sequence("own markers", |q, ctx, _| {
        for (name, comment) in [("Sound · mine", "my own note"), ("Beat: Cold open", "")] {
            q.markers.push(filmcraft_project::Marker {
                id: filmcraft_project::MarkerId(ctx.alloc()),
                start: filmcraft_time::Tick(TICKS_PER_SECOND),
                duration: filmcraft_time::Tick::ZERO,
                name: name.into(),
                comment: comment.into(),
                kind: filmcraft_project::MarkerKind::Comment,
                color: filmcraft_project::Label::Green,
            });
        }
        Ok(())
    })
    .unwrap();
    let before = markers(&s).len();
    s.execute("sound.placeCues", json!({})).unwrap();
    assert_eq!(markers(&s).len(), before + 4);
    let r = s.execute("sound.placeCues", json!({"clear": true})).unwrap();
    assert_eq!((r["placed"].as_u64(), r["removed"].as_u64()), (Some(0), Some(4)));
    assert_eq!(markers(&s).len(), before, "only the four we made went");
    assert!(markers(&s).iter().any(|m| m.0 == "Sound · mine") && markers(&s).iter().any(|m| m.0 == "Beat: Cold open"));
    // one beat only
    s.execute("sound.placeCues", json!({"beat": beat})).unwrap();
    assert_eq!(markers(&s).len(), before + 4);
}

#[test]
fn placing_cues_refuses_without_a_plan_a_sequence_or_anything_to_place() {
    let mut none = Session::default();
    assert!(none.execute("sound.placeCues", json!({})).is_err());
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    assert!(s.execute("sound.placeCues", json!({})).is_err(), "no plan");
    s.execute("planner.create", json!({"minutes": 5})).unwrap();
    let blank = s.execute("sound.placeCues", json!({}));
    // a plan from a pack carries sound notes, not a sound plan: nothing to place
    assert!(blank.is_err(), "{blank:?}");
    let before = s.active_sequence().unwrap().markers.len();
    for bad in [json!({"beat": 999_999}), json!({"beat": "x"}), json!({"clear": "yes"})] {
        let _ = s.execute("sound.placeCues", bad);
    }
    assert_eq!(s.active_sequence().unwrap().markers.len(), before);
}
