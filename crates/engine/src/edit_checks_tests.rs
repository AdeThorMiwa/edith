//! Tests of `edit.checks` and the `edit_checks` tool (fork roadmap F16.3): every check has a fixture that passes
//! and one that fails, the scope narrows them, and odd input is refused.

use std::sync::Arc;

use filmcraft_project::{ItemKind, SpokenWord};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use crate::Session;

fn tmp(name: &str) -> String {
    // tests run side by side: every call gets a folder of its own
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("filmcraft-checks-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d.to_string_lossy().into_owned()
}

fn media(s: &Session, name: &str) -> u64 {
    s.project.items.values().find(|i| i.name.contains(name) && matches!(i.kind, ItemKind::Media(_))).map(|i| i.id.0).expect(name)
}

fn edit() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name": "Cut", "audio": 2, "video": 2})).unwrap();
    s
}

fn cut(s: &mut Session, entries: Value) {
    s.execute("timeline.placeCutList", json!({"entries": entries})).unwrap();
}

fn checks(s: &mut Session, p: Value) -> Value {
    s.execute("edit.checks", p).unwrap()
}

fn get<'a>(r: &'a Value, id: &str) -> &'a Value {
    r["checks"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("no check {id} in {r}"))
}

fn status(r: &Value, id: &str) -> String {
    get(r, id)["status"].as_str().unwrap().to_string()
}

fn recorded(s: &mut Session, item: u64, commercial: bool) {
    s.execute(
        "provenance.set",
        json!({"item": item, "source": "own", "license": {"name": "Own work", "commercial": commercial, "attributionRequired": false}}),
    )
    .unwrap();
}

#[test]
fn an_empty_edit_has_nothing_to_check_and_the_command_changes_nothing() {
    let mut s = edit();
    assert!(s.execute("edit.checks", json!({})).unwrap_err().to_string().contains("edit is empty"));
    let ocean = media(&s, "Ocean_Sunset");
    cut(&mut s, json!([{"start": 0, "end": 5, "item": ocean}]));
    let (rev, undo) = (s.revision, s.history.undo.len());
    let r = checks(&mut s, json!({}));
    assert_eq!((s.revision, s.history.undo.len()), (rev, undo), "checking is not an edit");
    assert!(crate::commands::command_specs().iter().find(|c| c.id == "edit.checks").is_some_and(|c| !c.journal));
    assert_eq!(r["checks"].as_array().unwrap().len(), 16);
    assert_eq!(r["scope"], "the whole edit");
}

#[test]
fn coverage_finds_a_stretch_with_no_picture() {
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    cut(&mut s, json!([{"start": 0, "end": 3, "item": ocean}, {"start": 3, "end": 6, "item": ocean, "sourceIn": 4}]));
    assert_eq!(status(&checks(&mut s, json!({})), "coverage"), "pass");
    cut(&mut s, json!([{"start": 8, "end": 11, "item": ocean, "sourceIn": 8}]));
    let r = checks(&mut s, json!({}));
    let c = get(&r, "coverage");
    assert_eq!(c["status"], "fail", "{c}");
    assert!(c["issues"][0]["text"].as_str().unwrap().contains("no picture from 0:06.0 to 0:08.0"), "{c}");
    assert_eq!(r["mustPass"]["done"], false);
    // looking only at the part that is fine
    assert_eq!(status(&checks(&mut s, json!({"from": 0, "to": 6})), "coverage"), "pass");
    assert_eq!(checks(&mut s, json!({"from": 0, "to": 6}))["scope"], "0:00.0 to 0:06.0");
}

#[test]
fn a_shot_of_a_frame_or_two_is_a_flash_frame() {
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    cut(
        &mut s,
        json!([{"start": 0, "end": 3, "item": ocean}, {"start": 3, "end": "00:00:03:02", "item": ocean, "sourceIn": 5}, {"start": "00:00:03:02", "end": 6, "item": ocean, "sourceIn": 8}]),
    );
    let r = checks(&mut s, json!({}));
    let c = get(&r, "flashFrames");
    assert_eq!(c["status"], "fail", "{c}");
    assert!(c["issues"][0]["text"].as_str().unwrap().contains("on screen for only 0.08 s"), "{c}");
    let mut t = edit();
    let ocean = media(&t, "Ocean_Sunset");
    cut(&mut t, json!([{"start": 0, "end": 3, "item": ocean}]));
    assert_eq!(status(&checks(&mut t, json!({})), "flashFrames"), "pass");
}

/// A narration whose words are exact: "One two." (0.1–0.9 s) and "Three four." (1.2–1.9 s).
fn narrated(s: &mut Session) {
    let r = s.execute("tts.create", json!({"text": "One two. Three four.", "seconds": 0.0, "dir": tmp("words")})).unwrap();
    let item = filmcraft_project::ItemId(r["item"].as_u64().unwrap());
    let w = |t: &str, a: f64, b: f64, n: u32| SpokenWord {
        text: t.into(),
        start: Tick((a * TICKS_PER_SECOND as f64) as i64),
        end: Tick((b * TICKS_PER_SECOND as f64) as i64),
        sentence: n,
        exact: true,
    };
    s.edit("test", |pr, _| {
        pr.narrations.get_mut(&item).unwrap().words = vec![w("One", 0.1, 0.4, 0), w("two", 0.5, 0.9, 0), w("Three", 1.2, 1.5, 1), w("four", 1.6, 1.9, 1)];
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_cut_inside_a_spoken_word_fails_and_one_in_a_pause_passes() {
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    assert_eq!(
        status(
            &{
                cut(&mut s, json!([{"start": 0, "end": 3, "item": ocean}]));
                checks(&mut s, json!({}))
            },
            "cutsOnWords"
        ),
        "na",
        "nothing speaks"
    );
    narrated(&mut s);
    // 0.75 s is inside "two"; the cut list refuses it, so this one is made by hand
    s.execute("timeline.place", json!({"item": ocean, "track": "V2", "audio": false, "seconds": 0.75, "sourceIn": 0, "duration": 2 * TICKS_PER_SECOND}))
        .unwrap();
    let r = checks(&mut s, json!({}));
    let c = get(&r, "cutsOnWords");
    assert_eq!(c["status"], "fail", "{c}");
    assert!(c["issues"][0]["text"].as_str().unwrap().contains("inside the word “two”"), "{c}");
    let mut t = edit();
    let ocean = media(&t, "Ocean_Sunset");
    narrated(&mut t);
    cut(&mut t, json!([{"start": 0, "end": 1.0, "item": ocean}, {"start": 1.0, "end": 3, "item": ocean, "sourceIn": 4}]));
    assert_eq!(status(&checks(&mut t, json!({})), "cutsOnWords"), "pass");
}

fn plan_with_two_ideas(s: &mut Session) -> (u64, u64) {
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 1})).unwrap();
    let beats: Vec<u64> = crate::planner::get(&s.project).unwrap().beats().map(|b| b.id).collect();
    s.execute("planner.link", json!({"beat": beats[0], "startSeconds": 0, "durationSeconds": 4})).unwrap();
    s.execute("planner.link", json!({"beat": beats[1], "startSeconds": 4, "durationSeconds": 4})).unwrap();
    (beats[0], beats[1])
}

#[test]
fn a_shot_may_not_run_across_the_boundary_between_two_ideas() {
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    assert_eq!(
        status(
            &{
                cut(&mut s, json!([{"start": 0, "end": 8, "item": ocean, "speed": 0.5}]));
                checks(&mut s, json!({}))
            },
            "ideaBoundaries"
        ),
        "na",
        "no plan placed"
    );
    let (first, second) = plan_with_two_ideas(&mut s);
    let r = checks(&mut s, json!({}));
    let c = get(&r, "ideaBoundaries");
    assert_eq!(c["status"], "fail", "{c}");
    assert!(c["issues"][0]["text"].as_str().unwrap().contains("runs across the start of"), "{c}");
    // cut at the boundary and it passes
    let mut t = edit();
    let ocean = media(&t, "Ocean_Sunset");
    let (a, b) = plan_with_two_ideas(&mut t);
    cut(&mut t, json!([{"start": 0, "end": 4, "item": ocean}, {"start": 4, "end": 8, "item": ocean, "sourceIn": 6}]));
    assert_eq!(status(&checks(&mut t, json!({})), "ideaBoundaries"), "pass");
    // one idea at a time
    let one = checks(&mut t, json!({"beat": a}));
    assert!(one["scope"].as_str().unwrap().starts_with("the idea “"), "{}", one["scope"]);
    assert!((one["to"].as_i64().unwrap() - 4 * TICKS_PER_SECOND).abs() < 10);
    assert!(t.execute("edit.checks", json!({"beat": 999_999})).is_err());
    let _ = (first, second, b);
    let mut u = edit();
    u.execute("planner.create", json!({"pack": "mystery", "minutes": 1})).unwrap();
    let ocean = media(&u, "Ocean_Sunset");
    cut(&mut u, json!([{"start": 0, "end": 3, "item": ocean}]));
    let unplaced = crate::planner::get(&u.project).unwrap().beats().next().unwrap().id;
    assert!(u.execute("edit.checks", json!({"beat": unplaced})).unwrap_err().to_string().contains("not placed on the timeline"));
}

#[test]
fn shot_lengths_are_held_to_the_plans_range() {
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    cut(&mut s, json!([{"start": 0, "end": 7, "item": ocean}]));
    assert_eq!(status(&checks(&mut s, json!({})), "shotLengths"), "na", "no plan, no range");
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 1})).unwrap();
    let r = checks(&mut s, json!({}));
    assert_eq!(status(&r, "shotLengths"), "pass", "{}", get(&r, "shotLengths"));
    assert!(get(&r, "shotLengths")["summary"].as_str().unwrap().contains("the plan averages 7 s"));
    // half a second is short for a plan that averages seven, and thirty is long
    cut(&mut s, json!([{"start": 7, "end": 7.5, "item": ocean, "sourceIn": 1}, {"start": 7.5, "end": 37.5, "item": ocean, "speed": 0.5}]));
    let r = checks(&mut s, json!({}));
    let c = get(&r, "shotLengths");
    assert_eq!(c["status"], "fail", "{c}");
    let text: Vec<&str> = c["issues"].as_array().unwrap().iter().map(|i| i["text"].as_str().unwrap()).collect();
    assert!(text.iter().any(|t| t.contains("shorter than")) && text.iter().any(|t| t.contains("longer than")), "{text:?}");
    // and the pace note follows
    assert!(get(&r, "pace")["summary"].as_str().unwrap().contains("the plan aims for about 7 s"));
}

#[test]
fn rights_are_known_unknown_or_forbidding() {
    let mut s = edit();
    let (ocean, city) = (media(&s, "Ocean_Sunset"), media(&s, "City_Night"));
    cut(&mut s, json!([{"start": 0, "end": 3, "item": ocean}, {"start": 3, "end": 6, "item": city}]));
    let r = checks(&mut s, json!({}));
    let c = get(&r, "rights");
    assert_eq!(c["status"], "needsYou", "an origin nobody recorded is for the person to confirm: {c}");
    assert_eq!(r["mustPass"]["done"], true, "…and it does not block done");
    assert_eq!(r["mustPass"]["needsYou"], 1);
    recorded(&mut s, ocean, true);
    recorded(&mut s, city, true);
    assert_eq!(status(&checks(&mut s, json!({})), "rights"), "pass");
    recorded(&mut s, city, false);
    let r = checks(&mut s, json!({}));
    assert_eq!(status(&r, "rights"), "fail", "{}", get(&r, "rights"));
    assert_eq!(r["mustPass"]["done"], false);
    // only the stretch looked at
    assert_eq!(status(&checks(&mut s, json!({"from": 0, "to": 3})), "rights"), "pass");
}

fn tone(seconds: f64) -> Vec<f32> {
    (0..(seconds * 48_000.0) as usize).flat_map(|i| [(0.1 * (i as f32 * 0.058).sin()), (0.1 * (i as f32 * 0.058).sin())]).collect()
}

fn sound(s: &mut Session, name: &str, track: usize, at: f64, seconds: f64, tag: &str) {
    let bytes: Arc<[u8]> = crate::previews::write_wav_f32(&tone(seconds), 48_000).into();
    let item = crate::commands::import_bytes(s, &format!("/{name}.wav"), bytes, None).unwrap();
    let r = s.execute("timeline.place", json!({"item": item.0, "audioTrack": format!("A{track}"), "seconds": at})).unwrap();
    s.execute("essentialSound.setType", json!({"clips": [r["clips"][0].as_u64().unwrap()], "type": tag})).unwrap();
}

#[test]
fn nothing_for_four_seconds_in_the_sound_is_dead_air_unless_planned() {
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    cut(&mut s, json!([{"start": 0, "end": 30, "item": ocean, "speed": 0.5}]));
    for (name, track, at, len, tag) in
        [("d1", 1, 0.0, 10.0, "dialogue"), ("d2", 1, 14.0, 16.0, "dialogue"), ("b1", 2, 0.0, 10.0, "ambience"), ("b2", 2, 14.0, 16.0, "ambience")]
    {
        sound(&mut s, name, track, at, len, tag);
    }
    let r = checks(&mut s, json!({}));
    let c = get(&r, "deadAir");
    assert_eq!(c["status"], "fail", "{c}");
    assert!(c["issues"][0]["text"].as_str().unwrap().contains("0:10"), "{c}");
    // the plan asks for that silence
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 1})).unwrap();
    let beat = crate::planner::get(&s.project).unwrap().beats().next().unwrap().id;
    s.execute("planner.addCue", json!({"beat": beat, "kind": "silence", "text": "after the name", "atSeconds": 10, "lengthSeconds": 4})).unwrap();
    assert_eq!(status(&checks(&mut s, json!({})), "deadAir"), "pass");
    // looking only at a stretch with sound in it
    assert_eq!(status(&checks(&mut s, json!({"from": 0, "to": 5})), "deadAir"), "pass");
}

fn png(w: u32, h: u32) -> Arc<[u8]> {
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(w, h, image::Rgba([90, 120, 160, 255])))
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out.into()
}

#[test]
fn footage_that_comes_back_too_soon_and_a_still_that_sits_are_worth_a_look() {
    let mut s = edit();
    let (ocean, city) = (media(&s, "Ocean_Sunset"), media(&s, "City_Night"));
    cut(&mut s, json!([{"start": 0, "end": 3, "item": ocean}, {"start": 3, "end": 6, "item": city}, {"start": 6, "end": 9, "item": ocean, "sourceIn": 6}]));
    let r = checks(&mut s, json!({}));
    let c = get(&r, "variety");
    assert_eq!(c["status"], "note", "{c}");
    assert!(c["issues"][0]["text"].as_str().unwrap().contains("comes back at 0:06.0"), "{c}");
    assert_eq!(r["mustPass"]["done"], true, "a note never blocks done");
    // a still held for four seconds with no move, then with one
    let still = crate::commands::import_bytes(&mut s, "/photo.png", png(64, 48), None).unwrap().0;
    cut(&mut s, json!([{"start": 9, "end": 13, "item": still}]));
    let c = checks(&mut s, json!({}));
    assert_eq!(status(&c, "stillsMove"), "note", "{}", get(&c, "stillsMove"));
    s.undo().unwrap();
    cut(&mut s, json!([{"start": 9, "end": 13, "item": still, "move": "pushIn"}]));
    assert_eq!(status(&checks(&mut s, json!({})), "stillsMove"), "pass");
}

#[test]
fn the_tool_says_the_verdict_first_and_refuses_to_call_a_failing_cut_done() {
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    cut(&mut s, json!([{"start": 0, "end": 3, "item": ocean}, {"start": 5, "end": 8, "item": ocean, "sourceIn": 7}]));
    let out = s.run_assistant_tool("edit_checks", &json!({}));
    assert!(!out.is_error, "{}", out.content);
    let first = out.content.lines().next().unwrap();
    assert!(first.contains("NOT DONE: 1 must-pass check fails"), "{first}");
    for want in [
        "[FAIL] Every moment has picture",
        "[PASS] No flash frames",
        "[n/a] No cut inside a word",
        "[NEEDS THE PERSON] Every clip's rights are known",
        "Should (measured, not blocking)",
        "Taste (only the person decides",
        "Do not say the cut is done while a must-pass check fails",
    ] {
        assert!(out.content.contains(want), "{want:?} not in\n{}", out.content);
    }
    // when it is all fine, it says so
    let mut t = edit();
    let ocean = media(&t, "Ocean_Sunset");
    cut(&mut t, json!([{"start": 0, "end": 3, "item": ocean}]));
    recorded(&mut t, ocean, true);
    let ok = t.run_assistant_tool("edit_checks", &json!({}));
    assert!(
        ok.content.lines().next().unwrap().contains("every must-pass check passes") && !ok.content.contains("Do not say the cut is done"),
        "{}",
        ok.content
    );
    // hostile input
    for bad in [
        json!(null),
        json!([1]),
        json!("x"),
        json!({"beat": "one"}),
        json!({"from": "soon"}),
        json!({"from": 5, "to": 2}),
        json!({"to": -1}),
        json!({"from": 1e30}),
    ] {
        let r = t.run_assistant_tool("edit_checks", &bad);
        assert!(r.is_error || r.content.contains("Checks for"), "{bad}: {}", r.content);
    }
    assert!(Session::default().run_assistant_tool("edit_checks", &json!({})).is_error);
    let tool = crate::assistant_tools::tools().into_iter().find(|t| t.spec.name == "edit_checks").unwrap();
    assert!(tool.read_only);
}

// ------------------------------------------------------------------------------------------------ sound (F17.3)

#[test]
fn a_voice_with_nothing_under_it_is_worth_a_note_and_planned_sound_must_be_there() {
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    narrated(&mut s);
    cut(&mut s, json!([{"start": 0, "end": 8, "item": ocean, "speed": 0.5}]));
    let r = checks(&mut s, json!({}));
    let c = get(&r, "soundUnderIdeas");
    assert_eq!(c["status"], "note", "{c}");
    assert!(c["summary"].as_str().unwrap().contains("nothing sounds but the voice"), "{c}");
    // the plan asks for a bed and a score under the first idea (0 to 4 s) and none under the second
    let (first, _) = plan_with_two_ideas(&mut s);
    s.execute("planner.setSound", json!({"beat": first, "ambience": "wind over the shore", "music": "low strings"})).unwrap();
    let r = checks(&mut s, json!({}));
    let c = get(&r, "soundUnderIdeas");
    assert_eq!(c["status"], "note", "{c}");
    assert_eq!(c["total"], 2, "the bed and the music are both missing: {c}");
    assert!(c["issues"][0]["text"].as_str().unwrap().contains("plans a bed (wind over the shore)"), "{c}");
    assert!(c["issues"][1]["text"].as_str().unwrap().contains("plans music (low strings)"), "{c}");
    // a bed under the whole idea, but still no music
    sound(&mut s, "bed", 2, 0.0, 4.0, "ambience");
    let c = get(&checks(&mut s, json!({})), "soundUnderIdeas").clone();
    assert_eq!(c["total"], 1, "{c}");
    s.execute("sequence.addTracks", json!({"video": 0, "audio": 1, "audioAfter": "last"})).unwrap();
    sound(&mut s, "score", 3, 0.0, 4.0, "music");
    let c = get(&checks(&mut s, json!({})), "soundUnderIdeas").clone();
    assert_eq!(c["status"], "pass", "{c}");
}

#[test]
fn music_under_the_voice_at_full_level_is_flagged_until_the_mix_ducks_it() {
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    assert_eq!(
        status(
            &{
                cut(&mut s, json!([{"start": 0, "end": 8, "item": ocean, "speed": 0.5}]));
                checks(&mut s, json!({}))
            },
            "musicDucked"
        ),
        "na"
    );
    narrated(&mut s);
    assert_eq!(status(&checks(&mut s, json!({})), "musicDucked"), "na", "there is a voice but no music or ambience yet");
    sound(&mut s, "score", 2, 0.0, 8.0, "music");
    let r = checks(&mut s, json!({}));
    let c = get(&r, "musicDucked");
    assert_eq!(c["status"], "note", "{c}");
    assert!(c["issues"][0]["text"].as_str().unwrap().contains("sound.autoMix"), "{c}");
    s.execute("sound.autoMix", json!({})).unwrap();
    let c = get(&checks(&mut s, json!({})), "musicDucked").clone();
    assert_eq!(c["status"], "pass", "{c}");
}

#[test]
fn sound_from_a_stock_site_has_its_rights_checked_like_a_picture() {
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    cut(&mut s, json!([{"start": 0, "end": 8, "item": ocean, "speed": 0.5}]));
    recorded(&mut s, ocean, true);
    sound(&mut s, "score", 2, 0.0, 8.0, "music");
    let score = s.project.items.values().find(|i| i.name.contains("score")).map(|i| i.id.0).unwrap();
    let r = checks(&mut s, json!({}));
    let c = get(&r, "rights");
    assert_eq!(c["status"], "needsYou", "{c}");
    assert!(c["issues"][0]["text"].as_str().unwrap().contains("score"), "{c}");
    recorded(&mut s, score, true);
    assert_eq!(status(&checks(&mut s, json!({})), "rights"), "pass");
    recorded(&mut s, score, false);
    let r = checks(&mut s, json!({}));
    assert_eq!(status(&r, "rights"), "fail", "music that does not allow this use blocks done: {}", get(&r, "rights"));
    assert_eq!(r["mustPass"]["done"], false);
}

#[test]
fn the_loudness_of_the_mix_is_measured_only_when_asked_and_held_to_the_target() {
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    cut(&mut s, json!([{"start": 0, "end": 4, "item": ocean}]));
    sound(&mut s, "score", 2, 0.0, 4.0, "music");
    let r = checks(&mut s, json!({}));
    assert_eq!(status(&r, "mixLoudness"), "na");
    assert!(get(&r, "mixLoudness")["summary"].as_str().unwrap().contains("measure: true"));
    // the test tone is quiet: a long way under -16
    let r = checks(&mut s, json!({"measure": true}));
    let c = get(&r, "mixLoudness");
    assert_eq!(c["status"], "note", "{c}");
    assert!(c["summary"].as_str().unwrap().contains("too quiet") && c["summary"].as_str().unwrap().contains("LUFS"), "{c}");
    // against a target it is near, it passes
    let measured: f64 = c["summary"].as_str().unwrap().split("measures ").nth(1).unwrap().split(' ').next().unwrap().parse().unwrap();
    let r = checks(&mut s, json!({"measure": true, "targetLufs": measured.round()}));
    assert_eq!(status(&r, "mixLoudness"), "pass", "{}", get(&r, "mixLoudness"));
    assert!(s.execute("edit.checks", json!({"measure": true, "targetLufs": 5})).unwrap_err().to_string().contains("-60 to 0"));
}

// ------------------------------------------------------------------------------------------------ text (F30)

fn titled(s: &mut Session, entries: Value) {
    s.execute("graphics.placeTextList", json!({"entries": entries})).unwrap();
}

#[test]
fn an_opening_title_is_looked_for_in_the_first_fifteen_seconds() {
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    cut(&mut s, json!([{"start": 0, "end": 18, "item": ocean}]));
    let r = checks(&mut s, json!({}));
    let c = get(&r, "openingTitle");
    assert_eq!(c["status"], "note", "{c}");
    assert!(c["summary"].as_str().unwrap().contains("cold open"), "{c}");
    assert_eq!(status(&r, "textReadable"), "na");
    titled(&mut s, json!([{"start": 20, "end": 24, "text": "Too late", "role": "title"}]));
    assert_eq!(status(&checks(&mut s, json!({})), "openingTitle"), "note", "a title after fifteen seconds is not an opening");
    titled(&mut s, json!([{"start": 2, "end": 6, "text": "The Long Way Home", "role": "title"}]));
    let r = checks(&mut s, json!({}));
    assert_eq!(status(&r, "openingTitle"), "pass");
    assert!(get(&r, "openingTitle")["summary"].as_str().unwrap().contains("The Long Way Home"));
    assert_eq!(status(&r, "textReadable"), "pass");
    // looking at a stretch from the middle says nothing about an opening
    assert_eq!(status(&checks(&mut s, json!({"from": 5, "to": 15})), "openingTitle"), "na");
}

#[test]
fn text_shortened_after_it_was_placed_is_flagged_as_too_brief_to_read() {
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    cut(&mut s, json!([{"start": 0, "end": 18, "item": ocean}]));
    titled(&mut s, json!([{"start": 2, "end": 6, "text": "Four words in all", "role": "lowerThird"}]));
    let clip = s.active_sequence().unwrap().video_tracks.iter().flat_map(|t| t.items.iter()).find(|i| i.graphic.is_some()).unwrap().id;
    // the person trims it to a second
    s.edit_sequence("trim", |q, _, _| {
        q.find_item_mut(clip).unwrap().1.duration = Tick(TICKS_PER_SECOND);
        Ok(())
    })
    .unwrap();
    let r = checks(&mut s, json!({}));
    let c = get(&r, "textReadable");
    assert_eq!(c["status"], "note", "{c}");
    assert!(c["issues"][0]["text"].as_str().unwrap().contains("Four words in all"), "{c}");
    assert_eq!(r["mustPass"]["done"], true, "a note never blocks done");
}

#[test]
fn each_section_after_the_first_wants_a_title_where_it_begins() {
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    cut(&mut s, json!([{"start": 0, "end": 18, "item": ocean}]));
    assert_eq!(status(&checks(&mut s, json!({})), "chapterTitles"), "na", "no plan");
    // the plan's first section ("Opening") has its ideas at 0 and 4 s; the second ("Setup") begins at 8 s
    plan_with_two_ideas(&mut s);
    let setup = crate::planner::get(&s.project).unwrap().sections[1].beats[0].id;
    s.execute("planner.link", json!({"beat": setup, "startSeconds": 8, "durationSeconds": 4})).unwrap();
    let r = checks(&mut s, json!({}));
    let c = get(&r, "chapterTitles");
    assert_eq!(c["status"], "note", "{c}");
    assert!(c["issues"][0]["text"].as_str().unwrap().contains("begins at 0:08.0"), "{c}");
    titled(&mut s, json!([{"start": 8, "end": 12, "text": "Part two", "role": "chapter"}]));
    assert_eq!(status(&checks(&mut s, json!({})), "chapterTitles"), "pass");
}
