//! Tests of [`crate::version_diff`]: what a version changes against main, in plain words, on the edits
//! a person and the assistant actually make.

use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use crate::Session;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn diff(s: &mut Session) -> Value {
    s.execute("version.diff", json!({})).unwrap()
}

fn lines(s: &mut Session) -> Vec<String> {
    diff(s)["lines"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect()
}

fn clip_ids(s: &Session) -> Vec<(u64, String)> {
    s.active_sequence().unwrap().video_tracks[0].items.iter().map(|c| (c.id.0, c.name.clone())).collect()
}

fn seq_name(s: &Session) -> String {
    s.project.item(s.state.active_sequence.unwrap()).unwrap().name.clone()
}

#[test]
fn a_version_that_changes_nothing_has_an_empty_diff_and_no_version_refuses() {
    let mut s = demo();
    assert!(s.execute("version.diff", json!({})).is_err(), "no version open");
    s.execute("version.create", json!({})).unwrap();
    let d = diff(&mut s);
    assert_eq!(d["empty"], true);
    assert!(d["lines"].as_array().unwrap().is_empty());
    // a change and its undo is nothing again
    s.execute("project.setNotes", json!({"text": "x"})).unwrap();
    assert_eq!(diff(&mut s)["empty"], false);
    s.undo();
    assert_eq!(diff(&mut s)["empty"], true);
}

#[test]
fn moved_trimmed_retimed_and_switched_clips_are_said_in_words() {
    let mut s = demo();
    let clips = clip_ids(&s);
    assert!(clips.len() >= 5, "the demo has enough clips");
    let last = clips.len() - 1;
    let (a, b, c) = (clips[last].0, clips[1].0, clips[2].0);
    let name = seq_name(&s);
    s.execute("version.create", json!({"title": "Edit pass"})).unwrap();
    s.edit_sequence("by hand", |q, _, _| {
        for t in &mut q.video_tracks {
            for it in &mut t.items {
                if it.id.0 == a {
                    it.start += Tick(TICKS_PER_SECOND);
                }
                if it.id.0 == b {
                    it.duration = Tick(it.duration.0 / 2);
                }
                if it.id.0 == c {
                    it.speed = 2.0;
                }
            }
        }
        Ok(())
    })
    .unwrap();
    s.execute("clip.enable", json!({"clips": [clips[3].0]})).unwrap();
    let l = lines(&mut s);
    let has = |needle: &str| l.iter().any(|x| x.contains(needle));
    assert!(has(&format!("Moves 1 clip in “{name}”: {}", clips[last].1)), "{l:?}");
    assert!(has(&format!("Trims 1 clip in “{name}”: {}", clips[1].1)), "{l:?}");
    assert!(has(&format!("Changes the speed of 1 clip in “{name}”: {}", clips[2].1)), "{l:?}");
    // switching a clip off also switches off the sound it is linked to, so that is two clips
    assert!(l.iter().any(|x| x.starts_with("Switches 2 clips on or off")), "{l:?}");
    let d = diff(&mut s);
    let kinds: Vec<&str> = d["changes"].as_array().unwrap().iter().map(|c| c["kind"].as_str().unwrap()).collect();
    for k in ["clipsMoved", "clipsTrimmed", "clipsRetimed", "clipsSwitched"] {
        assert!(kinds.contains(&k), "{k}: {kinds:?}");
    }
}

#[test]
fn effects_markers_and_notes_are_named() {
    let mut s = demo();
    let clip = clip_ids(&s)[0].clone();
    s.execute("version.create", json!({})).unwrap();
    s.execute("effects.apply", json!({"clips": [clip.0], "effect": "lumetri"})).unwrap();
    s.execute("markers.add", json!({"target": "program", "name": "Fix this", "time": 5 * TICKS_PER_SECOND})).unwrap();
    s.execute("project.setNotes", json!({"text": "version notes"})).unwrap();
    let l = lines(&mut s);
    assert!(l.iter().any(|x| x.starts_with("Adds effects to 1 clip") && x.contains(&clip.1)), "{l:?}");
    assert!(l.iter().any(|x| x.starts_with("Adds 1 marker") && x.contains("Fix this")), "{l:?}");
    assert!(l.iter().any(|x| x == "Changes the project notes"), "{l:?}");
    // adjusting an effect that is already there is said as adjusting, not adding
    s.execute("version.commit", json!({})).unwrap();
    s.execute("version.create", json!({})).unwrap();
    let before = s.project.clone();
    s.edit_sequence("tweak", |q, _, _| {
        for t in &mut q.video_tracks {
            for it in &mut t.items {
                if it.id.0 == clip.0
                    && let Some(e) = it.effects.iter_mut().find(|e| e.effect == "lumetri")
                {
                    e.enabled = !e.enabled;
                }
            }
        }
        Ok(())
    })
    .unwrap();
    assert!(!std::sync::Arc::ptr_eq(&before, &s.project));
    let l = lines(&mut s);
    assert!(l.iter().any(|x| x.starts_with("Adjusts effects on 1 clip")), "{l:?}");
    assert!(!l.iter().any(|x| x.starts_with("Adds effects")), "{l:?}");
}

#[test]
fn clips_added_and_removed_and_items_in_the_project_are_counted_with_names() {
    let mut s = demo();
    let clips = clip_ids(&s);
    let name = seq_name(&s);
    s.execute("version.create", json!({})).unwrap();
    // remove two clips by hand; add a matte to the project
    s.edit_sequence("remove", |q, _, _| {
        if let Some(t) = q.video_tracks.first_mut() {
            t.items.retain(|c| c.id.0 != clips[0].0 && c.id.0 != clips[1].0);
        }
        Ok(())
    })
    .unwrap();
    s.execute("file.newColorMatte", json!({})).ok();
    let l = lines(&mut s);
    assert!(l.iter().any(|x| x.starts_with(&format!("Removes 2 clips in “{name}”: ")) && x.contains(&clips[0].1) && x.contains(&clips[1].1)), "{l:?}");
    // more than three names are cut with a count
    let (mut t, many) = (demo(), clip_ids(&demo()));
    t.execute("version.create", json!({})).unwrap();
    let ids: Vec<u64> = many.iter().map(|x| x.0).collect();
    t.edit_sequence("remove many", |q, _, _| {
        if let Some(tr) = q.video_tracks.first_mut() {
            tr.items.retain(|c| !ids.contains(&c.id.0));
        }
        Ok(())
    })
    .unwrap();
    let l = lines(&mut t);
    if many.len() > 3 {
        assert!(l.iter().any(|x| x.contains(" and ") && x.contains(" more")), "{l:?}");
    }
}

#[test]
fn the_plan_is_diffed_by_its_beats_and_the_original_is_what_a_peek_shows_too() {
    let mut s = demo();
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 5})).unwrap();
    let beat = crate::planner::get(&s.project).unwrap().beats().next().unwrap().id;
    s.execute("version.create", json!({})).unwrap();
    s.execute("planner.setSound", json!({"beat": beat, "ambience": "wind"})).unwrap();
    let l = lines(&mut s);
    assert!(l.iter().any(|x| x.starts_with("Changes the plan: 1 beat: ")), "{l:?}");
    // the diff is the same while looking at the original
    s.execute("version.peek", json!({})).unwrap();
    assert_eq!(lines(&mut s), l, "looking at the original does not change what the version changes");
    s.execute("version.peek", json!({})).unwrap();
}
