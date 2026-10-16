//! Tests of [`crate::planner`]: creating a plan from a Format Pack, editing it, undo, saving and
//! reopening the project, the pack library, import and export, and hostile input.

use edith_planner::Planner;
use filmcraft_time::TICKS_PER_SECOND;
use serde_json::{Value, json};

use crate::Session;
use crate::planner::{KEY, get};

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("edith-planner-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn plan(s: &Session) -> Planner {
    get(&s.project).expect("a plan")
}

fn mystery(s: &mut Session, minutes: f64) -> Value {
    s.execute("planner.create", json!({"pack": "mystery", "minutes": minutes})).unwrap()
}

fn beat_id(s: &Session, name: &str) -> u64 {
    plan(s).beats().find(|b| b.name == name).map(|b| b.id).unwrap_or_else(|| panic!("no beat {name}"))
}

#[test]
fn the_commands_exist_and_the_reading_ones_are_not_journaled() {
    let all = crate::commands::command_specs();
    for id in [
        "planner.get",
        "planner.packs",
        "planner.create",
        "planner.applyPack",
        "planner.clear",
        "planner.addSection",
        "planner.updateSection",
        "planner.addBeat",
        "planner.updateBeat",
        "planner.addShot",
        "planner.updateShot",
        "planner.move",
        "planner.delete",
        "planner.link",
        "planner.export",
        "planner.import",
    ] {
        let c = all.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("{id}"));
        assert_eq!(c.journal, !matches!(id, "planner.get" | "planner.packs" | "planner.export"), "{id}");
    }
}

#[test]
fn the_shipped_packs_are_listed_your_own_replace_or_add_and_a_broken_file_is_named_not_fatal() {
    let data = scratch("packs");
    let mut s = Session { prefs_path: Some(data.join("preferences.json")), ..Default::default() };
    let r = s.execute("planner.packs", json!({})).unwrap();
    let ids: Vec<&str> = r["packs"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["mystery", "explainer"]);
    assert!(r["packs"][0]["source"] == "shipped" && r["packs"][0]["repeat"]["default"] == 4 && r["packs"][0]["lookPreset"] == "Cinema Look");
    // your own packs
    let dir = data.join("format-packs");
    std::fs::create_dir_all(&dir).unwrap();
    let mine = |id: &str, name: &str| {
        format!(
            "version = 1\nid = \"{id}\"\nname = \"{name}\"\n[[section]]\nname = \"A\"\n[[section.beat]]\nname = \"All\"\nfrom = 0\nto = 100\nintensity = 5\n"
        )
    };
    std::fs::write(dir.join("travel.toml"), mine("travel", "Travel")).unwrap();
    std::fs::write(dir.join("shadow.toml"), mine("mystery", "My Mystery")).unwrap();
    std::fs::write(dir.join("broken.toml"), "version = 1\nid = 5").unwrap();
    std::fs::write(dir.join("huge.toml"), "x".repeat(300_000)).unwrap();
    std::fs::write(dir.join("notes.txt"), "not a pack").unwrap();
    let r = s.execute("planner.packs", json!({})).unwrap();
    let list: Vec<(String, String, String)> = r["packs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["id"].as_str().unwrap().into(), p["name"].as_str().unwrap().into(), p["source"].as_str().unwrap().into()))
        .collect();
    assert!(list.contains(&("travel".into(), "Travel".into(), "custom".into())));
    assert!(list.contains(&("mystery".into(), "My Mystery".into(), "custom".into())), "a custom pack replaces the shipped one: {list:?}");
    assert!(!list.iter().any(|(id, name, _)| id == "mystery" && name == "Mystery"));
    let problems: Vec<&str> = r["problems"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems.iter().any(|p| p.starts_with("broken.toml")) && problems.iter().any(|p| p.starts_with("huge.toml")));
    // a plan can be made from a pack of yours
    assert!(s.execute("planner.create", json!({"pack": "travel", "minutes": 5})).is_ok());
    // with no data folder (tests, web) only the shipped packs
    assert_eq!(Session::default().execute("planner.packs", json!({})).unwrap()["packs"].as_array().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn a_mystery_plan_is_made_exactly_and_creating_is_one_undo_step() {
    let mut s = demo();
    assert_eq!(s.execute("planner.get", json!({})).unwrap(), json!({"exists": false}));
    let undo = s.history.undo.len();
    let r = mystery(&mut s, 12.0);
    assert_eq!(s.history.undo.len(), undo + 1);
    let target = 12 * 60 * TICKS_PER_SECOND;
    assert_eq!(
        (r["summary"]["plannedTicks"].clone(), r["summary"]["targetTicks"].clone(), r["summary"]["differenceTicks"].clone()),
        (json!(target), json!(target), json!(0))
    );
    assert_eq!(r["summary"]["beats"], 12, "nine beats with four clues");
    assert_eq!(r["summary"]["lookPreset"], "Cinema Look");
    assert_eq!(r["plan"]["pack"], "mystery");
    assert_eq!(r["plan"]["title"], "Mystery");
    s.undo().unwrap();
    assert_eq!(s.execute("planner.get", json!({})).unwrap()["exists"], false);
    s.redo().unwrap();
    assert_eq!(plan(&s).beat_count(), 12);
    // a second create needs `replace`
    assert!(s.execute("planner.create", json!({"pack": "explainer", "minutes": 8})).is_err());
    let r = s.execute("planner.create", json!({"pack": "explainer", "minutes": 8, "clues": 4, "title": "How locks work", "replace": true})).unwrap();
    assert_eq!((r["plan"]["title"].clone(), r["summary"]["beats"].clone()), (json!("How locks work"), json!(11)));
    // a blank plan
    s.execute("planner.create", json!({"title": "Blank", "replace": true})).unwrap();
    assert_eq!((plan(&s).beat_count(), plan(&s).target.0), (0, 0));
    s.execute("planner.clear", json!({})).unwrap();
    assert!(s.execute("planner.clear", json!({})).is_err());
}

#[test]
fn beats_sections_and_shots_can_be_added_edited_moved_and_deleted_with_undo() {
    let mut s = demo();
    mystery(&mut s, 10.0);
    let cold = beat_id(&s, "Cold open");
    let scene = beat_id(&s, "The scene");
    // edit a beat
    s.execute("planner.updateBeat", json!({"beat": cold, "purpose": "A drowned bell rings.", "intensity": 8.5, "seconds": 30, "narration": "It rang once.", "status": "drafted", "visualIntent": "A bell in fog", "soundNote": "One low bell"})).unwrap();
    let b = plan(&s).beat_ref(cold).unwrap().clone();
    assert_eq!((b.purpose.as_str(), b.intensity, b.target.0, b.status.id()), ("A drowned bell rings.", 85, 30 * TICKS_PER_SECOND, "drafted"));
    assert_eq!((b.narration.as_str(), b.visual_intent.as_str(), b.sound_note.as_str()), ("It rang once.", "A bell in fog", "One low bell"));
    // the plan now no longer adds up to the target, and says so
    let r = s.execute("planner.get", json!({})).unwrap();
    assert_ne!(r["summary"]["differenceTicks"], 0);
    assert_eq!(r["summary"]["byStatus"]["drafted"], 1);
    s.undo().unwrap();
    assert_eq!(plan(&s).beat_ref(cold).unwrap().status.id(), "idea");
    s.redo().unwrap();
    // sections and beats
    let sec = s.execute("planner.addSection", json!({"name": "Epilogue"})).unwrap()["section"].as_u64().unwrap();
    let nb =
        s.execute("planner.addBeat", json!({"section": sec, "name": "Afterword", "minutes": 1, "intensity": 2, "purpose": "A last word."})).unwrap()["beat"]
            .as_u64()
            .unwrap();
    assert_eq!(plan(&s).sections.last().unwrap().beats[0].id, nb);
    s.execute("planner.updateSection", json!({"section": sec, "name": "After"})).unwrap();
    assert_eq!(plan(&s).sections.last().unwrap().name, "After");
    // shots
    let shot = s
        .execute("planner.addShot", json!({"beat": cold, "intent": "Bell in the fog", "shotType": "wide", "cameraMove": "pushIn", "seconds": 6}))
        .unwrap()["shot"]
        .as_u64()
        .unwrap();
    s.execute("planner.updateShot", json!({"shot": shot, "intent": "Bell, closer", "source": 42})).unwrap();
    let x = plan(&s).beat_ref(cold).unwrap().shots[0].clone();
    assert_eq!((x.intent.as_str(), x.source, x.target.0), ("Bell, closer", Some(42), 6 * TICKS_PER_SECOND));
    s.execute("planner.updateShot", json!({"shot": shot, "source": null})).unwrap();
    assert_eq!(plan(&s).beat_ref(cold).unwrap().shots[0].source, None);
    // moves: a beat to another section at a place, a section, a shot to another beat
    s.execute("planner.move", json!({"beat": nb, "to": plan(&s).sections[0].id, "at": 0})).unwrap();
    assert_eq!(plan(&s).sections[0].beats[0].id, nb);
    s.execute("planner.move", json!({"section": sec, "at": 0})).unwrap();
    assert_eq!(plan(&s).sections[0].id, sec);
    s.execute("planner.move", json!({"shot": shot, "to": scene})).unwrap();
    assert_eq!(plan(&s).beat_ref(scene).unwrap().shots.len(), 1);
    assert!(plan(&s).beat_ref(cold).unwrap().shots.is_empty());
    // deletes
    s.execute("planner.delete", json!({"shot": shot})).unwrap();
    s.execute("planner.delete", json!({"beat": nb})).unwrap();
    s.execute("planner.delete", json!({"section": sec})).unwrap();
    assert!(plan(&s).find_beat(nb).is_none() && plan(&s).section_index(sec).is_none());
    assert_eq!(plan(&s).beat_count(), 12, "the original beats are all still there");
    // ids stay valid and are never reused
    let after = s.execute("planner.addSection", json!({"name": "New"})).unwrap()["section"].as_u64().unwrap();
    assert!(after > nb && after > sec && after > shot);
    // links
    s.execute("planner.link", json!({"beat": cold, "startSeconds": 5, "durationSeconds": 30})).unwrap();
    let l = plan(&s).beat_ref(cold).unwrap().link.unwrap();
    assert_eq!((l.start.0, l.duration.0), (5 * TICKS_PER_SECOND, 30 * TICKS_PER_SECOND));
    s.execute("planner.link", json!({"beat": cold, "clear": true})).unwrap();
    assert!(plan(&s).beat_ref(cold).unwrap().link.is_none());
}

#[test]
fn applying_a_pack_again_can_keep_the_writing() {
    let mut s = demo();
    mystery(&mut s, 10.0);
    let cold = beat_id(&s, "Cold open");
    s.execute("planner.updateBeat", json!({"beat": cold, "narration": "It rang once.", "status": "drafted"})).unwrap();
    s.execute("planner.addShot", json!({"beat": cold, "intent": "A bell"})).unwrap();
    s.execute("planner.applyPack", json!({"pack": "mystery", "minutes": 15, "keepNarration": true, "clues": 3})).unwrap();
    let kept = plan(&s).beats().find(|b| b.name == "Cold open").unwrap().clone();
    assert_eq!((kept.narration.as_str(), kept.status.id(), kept.shots.len()), ("It rang once.", "drafted", 1));
    assert_eq!(plan(&s).target.0, 15 * 60 * TICKS_PER_SECOND);
    assert_eq!(plan(&s).planned(), plan(&s).target);
    assert_eq!(plan(&s).beat_count(), 11);
    // without keepNarration the writing is gone; the length defaults to the plan's own
    s.execute("planner.applyPack", json!({"pack": "explainer"})).unwrap();
    assert!(plan(&s).beats().all(|b| b.narration.is_empty()));
    assert_eq!(plan(&s).planned().0, 15 * 60 * TICKS_PER_SECOND);
}

#[test]
fn the_plan_comes_back_the_same_after_the_project_is_saved_and_opened() {
    let dir = scratch("save");
    let path = dir.join("p.fcproj").to_string_lossy().to_string();
    let mut s = demo();
    mystery(&mut s, 12.0);
    let cold = beat_id(&s, "Cold open");
    s.execute("planner.updateBeat", json!({"beat": cold, "narration": "Line one.\nLine two.", "intensity": 9})).unwrap();
    s.execute("planner.addShot", json!({"beat": cold, "intent": "A bell", "seconds": 4})).unwrap();
    s.execute("planner.link", json!({"beat": cold, "startSeconds": 0, "durationSeconds": 28.8})).unwrap();
    let before = plan(&s);
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    let mut t = Session::default();
    t.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!(plan(&t), before, "the same planner after reopening");
    assert_eq!(t.execute("planner.get", json!({})).unwrap()["summary"], s.execute("planner.get", json!({})).unwrap()["summary"]);
    // and editing after reopening works
    t.execute("planner.updateBeat", json!({"beat": cold, "name": "Cold open (revised)"})).unwrap();
    assert_eq!(plan(&t).beat_ref(cold).unwrap().name, "Cold open (revised)");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn export_then_import_gives_the_same_plan_and_bad_imports_change_nothing() {
    let dir = scratch("io");
    let file = dir.join("plan.json").to_string_lossy().to_string();
    let mut a = demo();
    mystery(&mut a, 9.0);
    a.execute("planner.updateBeat", json!({"beat": beat_id(&a, "Cold open"), "narration": "Hello."})).unwrap();
    let inline = a.execute("planner.export", json!({})).unwrap();
    assert!(inline.get("path").is_none());
    a.execute("planner.export", json!({"path": file})).unwrap();
    let mut b = Session::default();
    b.execute("planner.import", json!({"path": file})).unwrap();
    assert_eq!(plan(&b), plan(&a));
    let mut c = Session::default();
    c.execute("planner.import", json!({"plan": inline["plan"]})).unwrap();
    assert_eq!(plan(&c), plan(&a));
    // bad imports
    let before = (*b.project).clone();
    std::fs::write(dir.join("bad.json"), "{ not json").unwrap();
    std::fs::write(dir.join("huge.json"), vec![b' '; 9 * 1024 * 1024]).unwrap();
    let mut over = inline["plan"].clone();
    over["sections"][0]["beats"][0]["narration"] = json!("x".repeat(30_000));
    let mut dup = inline["plan"].clone();
    dup["sections"][0]["beats"][1]["id"] = dup["sections"][0]["beats"][0]["id"].clone();
    for p in [
        json!({}),
        json!({"path": dir.join("bad.json").to_string_lossy()}),
        json!({"path": dir.join("huge.json").to_string_lossy()}),
        json!({"path": dir.join("missing.json").to_string_lossy()}),
        json!({"path": "x", "plan": {}}),
        json!({"plan": 5}),
        json!({"plan": {"sections": "no"}}),
        json!({"plan": over}),
        json!({"plan": dup}),
    ] {
        assert!(b.execute("planner.import", p.clone()).is_err(), "accepted {p}");
    }
    assert_eq!(*b.project, before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_plan_in_the_project_counts_as_no_plan_and_the_next_create_repairs_it() {
    let mut s = demo();
    std::sync::Arc::make_mut(&mut s.project).ext.insert(KEY.to_string(), json!({"sections": 5, "target": "long"}));
    assert_eq!(s.execute("planner.get", json!({})).unwrap()["exists"], false);
    assert!(s.execute("planner.updateBeat", json!({"beat": 1, "name": "x"})).is_err());
    s.execute("planner.create", json!({"pack": "explainer", "minutes": 6})).unwrap();
    assert!(get(&s.project).is_some());
    // a plan whose numbers are out of range is cut to fit rather than trusted
    std::sync::Arc::make_mut(&mut s.project).ext.insert(
        KEY.to_string(),
        json!({"target": i64::MAX, "nextId": 0, "sections": [{"id": 0, "name": "n", "beats": [{"id": 0, "intensity": 250, "target": i64::MIN}]}]}),
    );
    let p = get(&s.project).unwrap();
    assert!(p.check().is_ok(), "{:?}", p.check());
}

#[test]
fn hostile_parameters_are_refused_and_the_project_is_unchanged() {
    let mut s = demo();
    mystery(&mut s, 10.0);
    let cold = beat_id(&s, "Cold open");
    let sec = plan(&s).sections[0].id;
    let before = (*s.project).clone();
    let long = "x".repeat(100_000);
    let cases: Vec<(&str, Value)> = vec![
        ("planner.create", json!({"pack": "mystery", "minutes": 10})),
        ("planner.create", json!({"pack": "nope", "minutes": 10, "replace": true})),
        ("planner.create", json!({"pack": "mystery", "replace": true})),
        ("planner.create", json!({"pack": "mystery", "minutes": -1, "replace": true})),
        ("planner.create", json!({"pack": "mystery", "minutes": 1e30, "replace": true})),
        ("planner.create", json!({"pack": "mystery", "seconds": "ten", "replace": true})),
        ("planner.create", json!({"pack": 5, "minutes": 5, "replace": true})),
        ("planner.create", json!({"pack": "mystery", "minutes": 5, "clues": -3, "replace": true})),
        ("planner.create", json!({"pack": "mystery", "minutes": 5, "clues": 1.5, "replace": true})),
        ("planner.create", json!({"title": long, "replace": true})),
        ("planner.create", json!({"title": "a\u{7}b", "replace": true})),
        ("planner.create", json!({"replace": "yes"})),
        ("planner.applyPack", json!({})),
        ("planner.applyPack", json!({"pack": "mystery", "keepNarration": "yes"})),
        ("planner.addSection", json!({})),
        ("planner.addSection", json!({"name": "  "})),
        ("planner.addSection", json!({"name": long})),
        ("planner.addSection", json!({"name": "x", "at": -1})),
        ("planner.updateSection", json!({"section": 999_999, "name": "x"})),
        ("planner.updateSection", json!({"section": sec})),
        ("planner.addBeat", json!({"name": "x"})),
        ("planner.addBeat", json!({"section": 999_999, "name": "x"})),
        ("planner.addBeat", json!({"section": sec})),
        ("planner.addBeat", json!({"section": sec, "name": "x", "intensity": 11})),
        ("planner.addBeat", json!({"section": sec, "name": "x", "intensity": "high"})),
        ("planner.addBeat", json!({"section": sec, "name": "x", "status": "finished"})),
        ("planner.addBeat", json!({"section": sec, "name": "x", "narration": "n".repeat(30_000)})),
        ("planner.addBeat", json!({"section": sec, "name": "x", "seconds": f64::MAX})),
        ("planner.addBeat", json!({"section": sec, "name": "x", "seconds": -5})),
        ("planner.addBeat", json!({"section": sec, "name": "x", "ticks": -5})),
        ("planner.addBeat", json!({"section": sec, "name": "x", "ticks": u64::MAX})),
        ("planner.updateBeat", json!({})),
        ("planner.updateBeat", json!({"beat": 0, "name": "x"})),
        ("planner.updateBeat", json!({"beat": -4, "name": "x"})),
        ("planner.updateBeat", json!({"beat": "one", "name": "x"})),
        ("planner.updateBeat", json!({"beat": cold, "name": ""})),
        ("planner.updateBeat", json!({"beat": cold, "name": 5})),
        ("planner.updateBeat", json!({"beat": cold, "purpose": long})),
        ("planner.updateBeat", json!({"beat": cold, "soundNote": "a".repeat(501)})),
        ("planner.updateBeat", json!({"beat": cold, "visualIntent": ["x"]})),
        ("planner.addShot", json!({"beat": 999_999})),
        ("planner.addShot", json!({"beat": cold, "source": "item"})),
        ("planner.addShot", json!({"beat": cold, "intent": long})),
        ("planner.updateShot", json!({"shot": 999_999, "intent": "x"})),
        ("planner.move", json!({})),
        ("planner.move", json!({"beat": cold, "section": sec})),
        ("planner.move", json!({"beat": cold, "to": 999_999})),
        ("planner.move", json!({"shot": 999_999})),
        ("planner.move", json!({"section": 999_999, "at": 0})),
        ("planner.delete", json!({})),
        ("planner.delete", json!({"beat": 999_999})),
        ("planner.delete", json!({"beat": cold, "shot": 1})),
        ("planner.link", json!({"beat": cold})),
        ("planner.link", json!({"beat": cold, "startSeconds": 1})),
        ("planner.link", json!({"beat": cold, "startSeconds": -1, "durationSeconds": 5})),
        ("planner.link", json!({"beat": cold, "startTicks": u64::MAX, "durationTicks": 5})),
        ("planner.link", json!({"beat": 999_999, "startSeconds": 1, "durationSeconds": 5})),
        ("planner.link", json!({"beat": cold, "clear": "yes"})),
        ("planner.export", json!({"path": 5})),
    ];
    for (cmd, p) in cases {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute(cmd, p.clone())));
        assert!(r.is_ok(), "{cmd} panicked on {p}");
        assert!(r.unwrap().is_err(), "{cmd} accepted {}", p.to_string().chars().take(120).collect::<String>());
    }
    assert_eq!(*s.project, before, "nothing changed");
}

#[test]
fn the_plan_cannot_grow_past_its_limits() {
    let mut s = demo();
    s.execute("planner.create", json!({"title": "Big"})).unwrap();
    let mut sections = Vec::new();
    for i in 0..edith_planner::model::MAX_SECTIONS {
        sections.push(s.execute("planner.addSection", json!({"name": format!("S{i}")})).unwrap()["section"].as_u64().unwrap());
    }
    assert!(s.execute("planner.addSection", json!({"name": "one too many"})).is_err());
    // 400 beats fit, the 401st does not
    for i in 0..edith_planner::model::MAX_BEATS {
        s.execute("planner.addBeat", json!({"section": sections[i % sections.len()], "name": format!("B{i}")})).unwrap();
    }
    assert!(s.execute("planner.addBeat", json!({"section": sections[0], "name": "one too many"})).is_err());
    assert_eq!(plan(&s).beat_count(), edith_planner::model::MAX_BEATS);
    // shots per beat
    let b = plan(&s).beats().next().unwrap().id;
    for _ in 0..edith_planner::model::MAX_SHOTS_PER_BEAT {
        s.execute("planner.addShot", json!({"beat": b})).unwrap();
    }
    assert!(s.execute("planner.addShot", json!({"beat": b})).is_err());
}

// ---------------------------------------------------------------------------- the skeleton

fn sequence() -> Session {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"width": 320, "height": 180, "fps": 24})).unwrap();
    s
}

fn markers(s: &Session) -> Vec<(i64, i64, String, String, u64)> {
    s.active_sequence().unwrap().markers.iter().map(|m| (m.start.0, m.duration.0, m.name.clone(), m.comment.clone(), m.id.0)).collect()
}

fn clips(s: &Session) -> Vec<(usize, u64, i64, i64)> {
    s.active_sequence().unwrap().video_tracks.iter().enumerate().flat_map(|(i, t)| t.items.iter().map(move |c| (i, c.id.0, c.start.0, c.duration.0))).collect()
}

#[test]
fn the_skeleton_puts_one_range_marker_per_beat_at_the_exact_ticks_and_building_again_moves_them() {
    let mut s = sequence();
    mystery(&mut s, 10.0);
    let undo = s.history.undo.len();
    let r = s.execute("planner.buildSkeleton", json!({})).unwrap();
    assert_eq!((r["markers"].clone(), r["placeholders"].clone()), (json!(12), json!(0)));
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    // every beat's marker starts where the beats before it end, and lasts its length
    let p = plan(&s);
    let m = markers(&s);
    assert_eq!(m.len(), 12);
    let mut at = 0i64;
    for (b, (start, dur, name, comment, _)) in p.beats().zip(&m) {
        assert_eq!((*start, *dur), (at, b.target.0), "{}", b.name);
        assert_eq!(name, &format!("Beat: {}", b.name));
        assert_eq!(comment, &b.purpose);
        let l = b.link.unwrap();
        assert_eq!((l.start.0, l.duration.0), (at, b.target.0));
        assert!(l.marker.is_some());
        at += b.target.0;
    }
    assert_eq!(at, 10 * 60 * TICKS_PER_SECOND, "the markers cover the whole video, to the tick");
    // building again moves them: the same number of markers, none doubled
    s.execute("planner.buildSkeleton", json!({"startSeconds": 30})).unwrap();
    let m2 = markers(&s);
    assert_eq!(m2.len(), 12);
    assert_eq!(m2[0].0, 30 * TICKS_PER_SECOND);
    assert_eq!(m2[1].0, 30 * TICKS_PER_SECOND + m[0].1);
    // markers you added yourself are not touched
    s.execute("markers.add", json!({"seconds": 2.0, "name": "Mine"})).unwrap();
    s.execute("planner.buildSkeleton", json!({})).unwrap();
    assert_eq!(markers(&s).len(), 13);
    assert!(markers(&s).iter().any(|x| x.2 == "Mine"));
    // undo takes the last build away, redo brings it back
    s.undo().unwrap();
    assert_eq!(markers(&s).len(), 13);
    s.undo().unwrap();
    s.undo().unwrap();
    assert_eq!(markers(&s).len(), 12);
}

#[test]
fn placeholders_are_made_for_shots_without_a_source_once_and_left_alone_after() {
    let mut s = sequence();
    mystery(&mut s, 2.0);
    let cold = beat_id(&s, "Cold open");
    let a = s.execute("planner.addShot", json!({"beat": cold, "intent": "A bell in the fog"})).unwrap()["shot"].as_u64().unwrap();
    let b = s.execute("planner.addShot", json!({"beat": cold, "intent": "A hand on the rope"})).unwrap()["shot"].as_u64().unwrap();
    let sourced = s.execute("planner.addShot", json!({"beat": cold, "intent": "Already have this", "source": 7})).unwrap()["shot"].as_u64().unwrap();
    let r = s.execute("planner.buildSkeleton", json!({})).unwrap();
    assert_eq!(r["placeholders"], 2, "the shot with a source gets none");
    let c = clips(&s);
    assert_eq!(c.len(), 2);
    assert!(c.iter().all(|x| x.0 == 0), "on the first free video track");
    // inside the beat, one after the other
    let beat_len = plan(&s).beat_ref(cold).unwrap().target.0;
    assert!(c.iter().all(|x| x.2 >= 0 && x.2 + x.3 <= beat_len + TICKS_PER_SECOND), "{c:?} within {beat_len}");
    assert!(c[0].2 < c[1].2);
    let names: Vec<String> = s.project.items.values().map(|i| i.name.clone()).collect();
    assert!(names.iter().any(|n| n == "A bell in the fog") && names.iter().any(|n| n == "A hand on the rope"));
    assert!(!names.iter().any(|n| n == "Already have this"));
    let p = plan(&s);
    let (xa, xb) = (p.beat_ref(cold).unwrap().shots[0].clone(), p.beat_ref(cold).unwrap().shots[1].clone());
    assert_eq!((xa.id, xb.id), (a, b));
    assert!(xa.clip.is_some() && xb.clip.is_some() && p.beat_ref(cold).unwrap().shots[2].clip.is_none());
    assert_eq!(p.beat_ref(cold).unwrap().shots[2].id, sourced);
    // building again makes nothing new and leaves the clips where they are
    s.execute("planner.buildSkeleton", json!({"startSeconds": 5})).unwrap();
    assert_eq!(clips(&s), c, "the placeholders were not touched or doubled");
    // a placeholder you deleted comes back
    let gone = xa.clip.unwrap();
    s.execute("edit.clear", json!({"clips": [gone]})).unwrap();
    assert_eq!(clips(&s).len(), 1);
    let r = s.execute("planner.buildSkeleton", json!({})).unwrap();
    assert_eq!(r["placeholders"], 1);
    assert_eq!(clips(&s).len(), 2);
}

#[test]
fn one_build_is_one_undo_step_that_takes_back_markers_placeholders_and_links() {
    let mut s = sequence();
    mystery(&mut s, 2.0);
    s.execute("planner.addShot", json!({"beat": beat_id(&s, "Cold open"), "intent": "A bell"})).unwrap();
    let before = (*s.project).clone();
    let undo = s.history.undo.len();
    s.execute("planner.buildSkeleton", json!({})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1);
    assert!(!clips(&s).is_empty() && !markers(&s).is_empty());
    s.undo().unwrap();
    assert!(clips(&s).is_empty() && markers(&s).is_empty());
    assert_eq!(plan(&s), edith_planner::Planner { ..get(&before).unwrap() }, "the plan has no links again");
    s.redo().unwrap();
    assert!(!clips(&s).is_empty() && !markers(&s).is_empty());
}

#[test]
fn placeholders_never_overwrite_your_clips_and_a_named_busy_or_locked_track_is_refused() {
    let mut s = sequence();
    mystery(&mut s, 2.0);
    s.execute("planner.addShot", json!({"beat": beat_id(&s, "Cold open"), "intent": "A bell"})).unwrap();
    // something of yours on V1 where the placeholder would go
    let item = s.execute("file.newOfflineFile", json!({"seconds": 20, "audio": false, "name": "Mine"})).unwrap()["item"].as_u64().unwrap();
    s.execute("timeline.place", json!({"item": item, "track": "V1", "time": 0, "audio": false})).unwrap();
    let mine = clips(&s);
    assert_eq!(mine.len(), 1);
    let r = s.execute("planner.buildSkeleton", json!({})).unwrap();
    assert_eq!(r["track"], "V2", "a new track, not V1");
    let c = clips(&s);
    assert_eq!(c.len(), 2);
    assert!(c.contains(&mine[0]), "your clip is untouched");
    assert!(c.iter().any(|x| x.0 == 1));
    // asking for the busy track by name is refused
    let mut t = sequence();
    mystery(&mut t, 2.0);
    t.execute("planner.addShot", json!({"beat": beat_id(&t, "Cold open"), "intent": "A bell"})).unwrap();
    let item = t.execute("file.newOfflineFile", json!({"seconds": 20, "audio": false})).unwrap()["item"].as_u64().unwrap();
    t.execute("timeline.place", json!({"item": item, "track": "V1", "time": 0, "audio": false})).unwrap();
    let before = (*t.project).clone();
    assert!(t.execute("planner.buildSkeleton", json!({"track": "V1"})).is_err());
    assert!(t.execute("planner.buildSkeleton", json!({"track": "V9"})).is_err());
    assert!(t.execute("planner.buildSkeleton", json!({"track": 5})).is_err());
    assert_eq!(*t.project, before, "nothing changed");
    // a locked track
    let seq_id = t.state.active_sequence.unwrap();
    std::sync::Arc::make_mut(&mut t.project).sequence_mut(seq_id).unwrap().video_tracks[0].locked = true;
    assert!(t.execute("planner.buildSkeleton", json!({"track": "V1"})).is_err());
}

#[test]
fn the_skeleton_refuses_what_it_cannot_do_and_changes_nothing() {
    let mut empty = Session::default();
    assert!(empty.execute("planner.buildSkeleton", json!({})).is_err(), "no plan");
    let mut s = sequence();
    s.execute("planner.create", json!({"title": "Blank"})).unwrap();
    assert!(s.execute("planner.buildSkeleton", json!({})).is_err(), "no beats");
    mystery_replace(&mut s);
    let before = (*s.project).clone();
    for p in
        [json!({"startSeconds": -1}), json!({"startSeconds": "soon"}), json!({"startTicks": u64::MAX}), json!({"startSeconds": 1e30}), json!({"track": ["V1"]})]
    {
        assert!(s.execute("planner.buildSkeleton", p.clone()).is_err(), "accepted {p}");
    }
    assert_eq!(*s.project, before);
    // no sequence open
    let mut none = demo();
    mystery(&mut none, 5.0);
    none.state.active_sequence = None;
    assert!(none.execute("planner.buildSkeleton", json!({})).is_err());
    // a zero-length beat still gets a marker but no placeholders
    let mut z = sequence();
    z.execute("planner.create", json!({"title": "Z"})).unwrap();
    let sec = z.execute("planner.addSection", json!({"name": "S"})).unwrap()["section"].as_u64().unwrap();
    let b = z.execute("planner.addBeat", json!({"section": sec, "name": "Instant", "seconds": 0})).unwrap()["beat"].as_u64().unwrap();
    z.execute("planner.addShot", json!({"beat": b, "intent": "x"})).unwrap();
    let r = z.execute("planner.buildSkeleton", json!({})).unwrap();
    assert_eq!((r["markers"].clone(), r["placeholders"].clone()), (json!(1), json!(0)));
}

fn mystery_replace(s: &mut Session) {
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 3, "replace": true})).unwrap();
}

#[test]
fn the_title_and_length_can_change_and_the_beats_can_be_fitted_to_a_new_length_exactly() {
    let mut s = demo();
    mystery(&mut s, 10.0);
    s.execute("planner.update", json!({"title": "The Bell"})).unwrap();
    assert_eq!(plan(&s).title, "The Bell");
    // a new length alone leaves the beats, and the summary shows the difference
    s.execute("planner.update", json!({"minutes": 12})).unwrap();
    assert_eq!(plan(&s).target.0, 12 * 60 * TICKS_PER_SECOND);
    assert_eq!(s.execute("planner.get", json!({})).unwrap()["summary"]["differenceTicks"], -(2 * 60 * TICKS_PER_SECOND));
    // fit scales every beat in proportion and the beats add up to the length again, to the tick
    let before: Vec<i64> = plan(&s).beats().map(|b| b.target.0).collect();
    s.execute("planner.update", json!({"seconds": 777.5, "fit": true})).unwrap();
    let after: Vec<i64> = plan(&s).beats().map(|b| b.target.0).collect();
    assert_eq!(after.iter().sum::<i64>(), plan(&s).target.0);
    assert!(before.iter().zip(&after).all(|(b, a)| (*a as f64 / *b as f64 - 777.5 / 600.0).abs() < 1e-6), "in proportion");
    s.undo().unwrap();
    assert_eq!(plan(&s).beats().map(|b| b.target.0).collect::<Vec<_>>(), before);
    // an all-zero plan is shared equally
    let mut z = demo();
    z.execute("planner.create", json!({"title": "Z"})).unwrap();
    let sec = z.execute("planner.addSection", json!({"name": "S"})).unwrap()["section"].as_u64().unwrap();
    for n in ["a", "b", "c"] {
        z.execute("planner.addBeat", json!({"section": sec, "name": n, "seconds": 0})).unwrap();
    }
    z.execute("planner.update", json!({"seconds": 10, "fit": true})).unwrap();
    assert_eq!(plan(&z).planned().0, 10 * TICKS_PER_SECOND);
    // refusals
    let before = (*s.project).clone();
    for p in [
        json!({}),
        json!({"fit": true}),
        json!({"minutes": 1, "fit": "yes"}),
        json!({"minutes": -1}),
        json!({"minutes": 1e30}),
        json!({"title": "x".repeat(300)}),
        json!({"title": 5}),
        json!({"fit": true, "title": "x"}),
    ] {
        assert!(s.execute("planner.update", p.clone()).is_err(), "accepted {p}");
    }
    assert_eq!(*s.project, before);
    assert!(Session::default().execute("planner.update", json!({"title": "x"})).is_err(), "no plan");
}

#[test]
fn starting_over_removes_the_markers_and_placeholders_the_plan_made_and_nothing_else() {
    let mut s = sequence();
    mystery(&mut s, 2.0);
    let cold = beat_id(&s, "Cold open");
    s.execute("planner.addShot", json!({"beat": cold, "intent": "A bell"})).unwrap();
    s.execute("planner.addShot", json!({"beat": cold, "intent": "A rope"})).unwrap();
    // things of yours: a marker, a clip, and a placeholder you have since moved to another place
    s.execute("markers.add", json!({"seconds": 1.0, "name": "Mine"})).unwrap();
    let item = s.execute("file.newOfflineFile", json!({"seconds": 4, "audio": false, "name": "Mine"})).unwrap()["item"].as_u64().unwrap();
    s.execute("timeline.place", json!({"item": item, "track": "V1", "time": 0, "audio": false})).unwrap();
    s.execute("planner.buildSkeleton", json!({})).unwrap();
    let items_before = s.project.items.len();
    assert_eq!((markers(&s).len(), clips(&s).len()), (13, 3));
    let undo = s.history.undo.len();
    let r = s.execute("planner.clear", json!({})).unwrap();
    assert_eq!((r["markersRemoved"].clone(), r["placeholdersRemoved"].clone()), (json!(12), json!(2)));
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    assert_eq!(s.execute("planner.get", json!({})).unwrap()["exists"], false);
    // only your own marker and clip are left, and the two placeholder files are gone from the project
    assert_eq!(markers(&s).iter().map(|m| m.2.as_str()).collect::<Vec<_>>(), ["Mine"]);
    assert_eq!(clips(&s).len(), 1);
    assert_eq!(s.project.items.len(), items_before - 2);
    // undo brings the plan, the markers and the placeholders all back
    s.undo().unwrap();
    assert_eq!((markers(&s).len(), clips(&s).len()), (13, 3));
    assert!(get(&s.project).is_some());
    // keepTimeline leaves the timeline alone
    let r = s.execute("planner.clear", json!({"keepTimeline": true})).unwrap();
    assert_eq!((r["markersRemoved"].clone(), r["placeholdersRemoved"].clone()), (json!(0), json!(0)));
    assert_eq!((markers(&s).len(), clips(&s).len()), (13, 3));
    // a placeholder that has been replaced by real media is not a placeholder any more: it stays
    let mut t = sequence();
    mystery(&mut t, 2.0);
    t.execute("planner.addShot", json!({"beat": beat_id(&t, "Cold open"), "intent": "A bell"})).unwrap();
    t.execute("planner.buildSkeleton", json!({})).unwrap();
    let clip = clips(&t)[0].1;
    let item = t.active_sequence().unwrap().find_item(filmcraft_project::ClipId(clip)).unwrap().1.item;
    std::sync::Arc::make_mut(&mut t.project).item_mut(item).unwrap().as_media_mut().unwrap().offline = false;
    let r = t.execute("planner.clear", json!({})).unwrap();
    assert_eq!(r["placeholdersRemoved"], 0, "a clip that is no longer the offline placeholder is left");
    assert_eq!(clips(&t).len(), 1);
    // bad parameter
    let mut u = sequence();
    mystery(&mut u, 2.0);
    assert!(u.execute("planner.clear", json!({"keepTimeline": "yes"})).is_err());
    assert!(get(&u.project).is_some());
}
