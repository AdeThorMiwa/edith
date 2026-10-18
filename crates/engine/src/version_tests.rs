//! Tests of [`crate::version`]: a version is a branch of the edit that is checked out while it is open,
//! main stays untouched until it is committed (one undo step) or discarded, you can look at the
//! original read-only, both of you edit inside it, and what reaches outside the edit is refused.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::Session;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn notes(s: &Session) -> String {
    s.project.notes.clone()
}

fn set_notes(s: &mut Session, text: &str) {
    s.execute("project.setNotes", json!({"text": text})).unwrap();
}

fn undo_len(s: &Session) -> usize {
    s.history.undo.len()
}

fn list(s: &mut Session) -> Value {
    s.execute("version.list", json!({})).unwrap()
}

fn a_clip(s: &Session) -> u64 {
    s.active_sequence().unwrap().video_tracks[0].items[0].id.0
}

#[test]
fn the_commands_exist_and_with_no_version_most_of_them_refuse() {
    let all = crate::commands::command_specs();
    for id in ["version.create", "version.list", "version.peek", "version.commit", "version.discard", "version.rename", "version.rewind", "version.run"] {
        assert!(all.iter().any(|c| c.id == id), "{id}");
    }
    let mut s = demo();
    assert_eq!(list(&mut s), json!({"open": false}));
    for (cmd, p) in [
        ("version.peek", json!({})),
        ("version.commit", json!({})),
        ("version.discard", json!({})),
        ("version.rename", json!({"title": "x"})),
        ("version.rewind", json!({"steps": 1})),
        ("version.run", json!({"command": "project.setNotes", "params": {"text": "x"}})),
    ] {
        assert!(s.execute(cmd, p.clone()).is_err(), "{cmd} {p}");
    }
}

#[test]
fn while_a_version_is_open_everything_goes_into_it_and_main_is_untouched() {
    let mut s = demo();
    set_notes(&mut s, "main as it was");
    let main_undo = undo_len(&s);
    let main_project = s.project.clone();
    let v = s.execute("version.create", json!({"title": "Tighten act two"})).unwrap();
    assert_eq!(
        (v["open"].clone(), v["title"].as_str(), v["startedBy"].as_str(), v["peeking"].clone()),
        (json!(true), Some("Tighten act two"), Some("you"), json!(false))
    );
    // the version starts as main is, with no steps and nothing to undo
    assert!(Arc::ptr_eq(&s.project, &main_project));
    assert_eq!(undo_len(&s), 0, "the version has its own history, empty at first");
    assert_eq!(v["stepCount"], 0);
    set_notes(&mut s, "changed in the version");
    assert_eq!(notes(&s), "changed in the version");
    assert_eq!(undo_len(&s), 1);
    // undo works inside it and stops at where it started: it never reaches main's history
    s.execute("edit.undo", json!({})).ok();
    assert_eq!(notes(&s), "main as it was");
    assert!(s.undo().is_none(), "nothing further back: main's own steps are not the version's");
    assert!(Arc::ptr_eq(&s.project, &main_project));
    // only one at a time
    assert!(s.execute("version.create", json!({})).is_err());
    // discarding gives main back exactly, with its history
    set_notes(&mut s, "something else");
    s.execute("version.discard", json!({})).unwrap();
    assert_eq!(notes(&s), "main as it was");
    assert!(Arc::ptr_eq(&s.project, &main_project), "the very same project, not a copy");
    assert_eq!(undo_len(&s), main_undo);
    assert_eq!(list(&mut s), json!({"open": false}));
}

#[test]
fn committing_is_one_undo_step_and_undo_gives_back_the_original() {
    let mut s = demo();
    set_notes(&mut s, "main as it was");
    let before = undo_len(&s);
    let original = s.project.clone();
    s.execute("version.create", json!({"title": "Notes pass"})).unwrap();
    set_notes(&mut s, "one");
    set_notes(&mut s, "two");
    s.execute("effects.apply", json!({"clips": [a_clip(&s)], "effect": "lumetri"})).unwrap();
    assert_eq!(undo_len(&s), 3);
    let r = s.execute("version.commit", json!({})).unwrap();
    assert_eq!((r["committed"].clone(), r["title"].as_str(), r["steps"].clone()), (json!(true), Some("Notes pass"), json!(3)));
    assert_eq!(notes(&s), "two", "main now has everything");
    assert_eq!(undo_len(&s), before + 1, "however many steps it took, one undo step");
    assert_eq!(s.history.undo.last().unwrap().0, "Version: Notes pass");
    assert_eq!(list(&mut s), json!({"open": false}));
    assert_eq!(s.undo().as_deref(), Some("Version: Notes pass"));
    assert!(Arc::ptr_eq(&s.project, &original), "one undo and main is exactly what it was");
    s.redo();
    assert_eq!(notes(&s), "two");
    // a version that changed nothing leaves no undo step
    let n = undo_len(&s);
    s.execute("version.create", json!({})).unwrap();
    let r = s.execute("version.commit", json!({})).unwrap();
    assert_eq!(r["committed"], false);
    assert_eq!(undo_len(&s), n);
    // one that changed things and changed them back is the same
    s.execute("version.create", json!({})).unwrap();
    set_notes(&mut s, "a");
    s.undo();
    assert_eq!(s.execute("version.commit", json!({})).unwrap()["committed"], false);
    assert_eq!(undo_len(&s), n);
}

#[test]
fn the_original_can_be_looked_at_read_only_and_you_come_back_to_the_version() {
    let mut s = demo();
    set_notes(&mut s, "original");
    s.execute("version.create", json!({})).unwrap();
    set_notes(&mut s, "in the version");
    let p = s.execute("version.peek", json!({})).unwrap();
    assert_eq!(p["peeking"], true);
    assert_eq!(notes(&s), "original", "what is shown is main");
    // nothing can be changed while you look
    assert!(s.execute("project.setNotes", json!({"text": "sneaky"})).is_err());
    assert!(s.undo().is_none() && s.redo().is_none());
    assert_eq!(notes(&s), "original");
    assert_eq!(list(&mut s)["stepCount"], 1, "the version's steps are still there");
    // the assistant's edit goes to the version, not to what is shown
    s.execute("version.run", json!({"command": "project.setNotes", "params": {"text": "from the assistant"}})).unwrap();
    assert_eq!(notes(&s), "original", "still looking at the original");
    assert!(list(&mut s)["peeking"].as_bool().unwrap());
    s.execute("version.peek", json!({})).unwrap();
    assert_eq!(notes(&s), "from the assistant", "back in the version, with the assistant's edit");
    // a toggle: no argument flips it; asking for what already is changes nothing
    assert_eq!(s.execute("version.peek", json!({"on": false})).unwrap()["peeking"], false);
    assert_eq!(s.execute("version.peek", json!({"on": true})).unwrap()["peeking"], true);
    assert_eq!(s.execute("version.peek", json!({"on": true})).unwrap()["peeking"], true);
    // committing while looking at the original commits the version
    s.execute("version.commit", json!({})).unwrap();
    assert_eq!(notes(&s), "from the assistant");
    assert!(!s.versions.is_open() && !s.versions.is_peeking());
    // discarding while looking at the original returns to main
    s.execute("version.create", json!({})).unwrap();
    set_notes(&mut s, "never kept");
    s.execute("version.peek", json!({})).unwrap();
    s.execute("version.discard", json!({})).unwrap();
    assert_eq!(notes(&s), "from the assistant");
    assert!(s.execute("project.setNotes", json!({"text": "editing main again"})).is_ok());
}

#[test]
fn each_step_says_who_made_it_and_the_two_of_you_can_edit_the_same_version() {
    let mut s = demo();
    s.execute("version.create", json!({"title": "Together", "by": "assistant"})).unwrap();
    assert_eq!(list(&mut s)["startedBy"], "assistant");
    s.execute("version.run", json!({"command": "project.setNotes", "params": {"text": "assistant 1"}})).unwrap();
    set_notes(&mut s, "your edit on top");
    s.execute("version.run", json!({"command": "project.setNotes", "params": {"text": "assistant 2"}, "by": "assistant"})).unwrap();
    let steps = list(&mut s)["steps"].clone();
    let who: Vec<&str> = steps.as_array().unwrap().iter().map(|x| x["by"].as_str().unwrap()).collect();
    assert_eq!(who, ["assistant", "you", "assistant"], "{steps}");
    assert!(steps[0]["label"].as_str().is_some_and(|l| !l.is_empty()));
    // undo and redo keep who made what
    s.undo();
    s.redo();
    let again: Vec<String> = list(&mut s)["steps"].as_array().unwrap().iter().map(|x| x["by"].as_str().unwrap().to_string()).collect();
    assert_eq!(again, ["assistant", "you", "assistant"]);
    let r = s.execute("version.commit", json!({})).unwrap();
    assert!(r["byYou"].as_u64().unwrap() >= 1 && r["byAssistant"].as_u64().unwrap() >= 1, "{r}");
    // `by` is checked
    s.execute("version.create", json!({})).unwrap();
    assert!(s.execute("version.run", json!({"command": "project.setNotes", "params": {"text": "x"}, "by": "robot"})).is_err());
    assert!(s.execute("version.create", json!({"by": 5})).is_err());
}

#[test]
fn rewinding_undoes_the_versions_last_steps_and_never_goes_before_its_start() {
    let mut s = demo();
    set_notes(&mut s, "main");
    s.execute("version.create", json!({})).unwrap();
    for t in ["a", "b", "c"] {
        set_notes(&mut s, t);
    }
    let r = s.execute("version.rewind", json!({"steps": 2})).unwrap();
    assert_eq!(r["undone"], 2);
    assert_eq!(notes(&s), "a");
    let r = s.execute("version.rewind", json!({"steps": 50})).unwrap();
    assert_eq!(r["undone"], 1, "only one step was left");
    assert_eq!(notes(&s), "main", "back at where the version started, and no further");
    for bad in [json!({"steps": 0}), json!({"steps": -1}), json!({"steps": "x"}), json!({})] {
        assert!(s.execute("version.rewind", bad.clone()).is_err(), "{bad}");
    }
    // while looking at the original, rewinding still rewinds the version
    set_notes(&mut s, "z");
    s.execute("version.peek", json!({})).unwrap();
    s.execute("version.rewind", json!({"steps": 1})).unwrap();
    assert!(s.versions.is_peeking());
    s.execute("version.peek", json!({})).unwrap();
    assert_eq!(notes(&s), "main");
}

#[test]
fn what_reaches_outside_the_edit_is_refused_while_a_version_is_open_and_allowed_after() {
    let mut s = demo();
    s.execute("version.create", json!({"title": "Open"})).unwrap();
    for (cmd, p) in [
        ("file.save", json!({})),
        ("file.saveAs", json!({"path": "/tmp/never.fcproj"})),
        ("file.saveCopy", json!({"path": "/tmp/never.fcproj"})),
        ("file.newProject", json!({})),
        ("file.open", json!({"path": "/tmp/never.fcproj"})),
        ("file.openDemoProject", json!({})),
        ("file.revert", json!({})),
        ("file.exportEdl", json!({"path": "/tmp/never.edl"})),
        ("export.quick", json!({})),
        ("export.queue.add", json!({})),
    ] {
        let e = s.execute(cmd, p.clone()).expect_err(cmd).to_string();
        assert!(e.contains("Commit it or discard it first") && e.contains("Open"), "{cmd}: {e}");
    }
    assert!(!std::path::Path::new("/tmp/never.fcproj").exists());
    // a version cannot be run inside a version, and its parameters are checked
    for p in
        [json!({"command": "version.discard"}), json!({"command": ""}), json!({}), json!({"command": 5}), json!({"command": "project.setNotes", "params": 5})]
    {
        assert!(s.execute("version.run", p.clone()).is_err(), "{p}");
    }
    assert!(s.execute("version.run", json!({"command": "no.such.command"})).is_err());
    s.execute("version.discard", json!({})).unwrap();
    assert!(s.execute("file.openDemoProject", json!({})).is_ok(), "allowed again");
}

#[test]
fn the_title_is_cleaned_and_can_be_renamed() {
    let mut s = demo();
    s.execute("version.create", json!({"title": "  \u{7}   "})).unwrap();
    assert_eq!(list(&mut s)["title"], "New version", "a blank title is the default");
    let long = "t".repeat(500);
    s.execute("version.rename", json!({"title": long})).unwrap();
    assert_eq!(list(&mut s)["title"].as_str().unwrap().chars().count(), 80);
    s.execute("version.rename", json!({"title": "Colour pass"})).unwrap();
    assert_eq!(list(&mut s)["title"], "Colour pass");
    assert!(s.execute("version.rename", json!({})).is_err());
    assert!(s.execute("version.rename", json!({"title": 5})).is_err());
    set_notes(&mut s, "x");
    s.execute("version.commit", json!({})).unwrap();
    assert_eq!(s.history.undo.last().unwrap().0, "Version: Colour pass");
    assert!(s.execute("version.create", json!({"title": 5})).is_err());
}

#[test]
fn a_discard_leaves_a_clean_project_clean_and_edits_in_a_version_make_it_dirty() {
    let mut s = demo();
    s.bump_view();
    s.saved_revision = s.revision;
    assert!(!s.is_dirty());
    s.execute("version.create", json!({})).unwrap();
    assert!(!s.is_dirty(), "opening a version is not a change");
    set_notes(&mut s, "x");
    assert!(s.is_dirty(), "work in a version is unsaved work");
    s.execute("version.discard", json!({})).unwrap();
    assert!(!s.is_dirty(), "main was clean and is exactly as it was");
    // committing is a change
    s.execute("version.create", json!({})).unwrap();
    set_notes(&mut s, "y");
    s.execute("version.commit", json!({})).unwrap();
    assert!(s.is_dirty());
}

#[test]
fn selection_that_points_into_the_version_does_not_dangle_when_you_go_back() {
    let mut s = demo();
    let seq = s.state.active_sequence.unwrap();
    s.execute("version.create", json!({})).unwrap();
    // something that exists only in the version
    s.execute("file.newColorMatte", json!({})).ok();
    let n_items = s.project.items.len();
    s.execute("version.discard", json!({})).unwrap();
    assert!(s.project.items.len() <= n_items);
    assert_eq!(s.state.active_sequence, Some(seq));
    assert!(s.active_sequence().is_some());
    // the same going back and forth by looking at the original
    s.execute("version.create", json!({})).unwrap();
    s.execute("version.peek", json!({})).unwrap();
    assert!(s.active_sequence().is_some());
    s.execute("version.peek", json!({})).unwrap();
    assert!(s.active_sequence().is_some());
}
