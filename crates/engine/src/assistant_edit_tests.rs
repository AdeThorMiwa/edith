//! Tests of [`crate::assistant_edit`]: Edit mode and the tools that work inside a version. The gate (only
//! in Edit mode), what the assistant may run, the person's steps being theirs, and the whole thing with a
//! scripted model.

use serde_json::{Value, json};

use super::assistant_tests::{scripted, text_reply, tool_reply};
use crate::Session;
use crate::assistant_edit::{may_run, run_edit};

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn edit_mode(s: &mut Session) {
    s.execute("assistant.settings", json!({"mode": "edit"})).unwrap();
}

fn call(s: &mut Session, tool: &str, input: Value) -> edith_ai::ToolOutput {
    run_edit(s, tool, &input).unwrap_or_else(|| s.run_assistant_tool(tool, &input))
}

fn notes(s: &Session) -> String {
    s.project.notes.clone()
}

#[test]
fn the_tools_exist_and_only_the_three_that_read_are_read_only() {
    let tools = crate::assistant_tools::tools();
    for (name, read_only) in [
        ("version_status", true),
        ("command_list", true),
        ("planner_get", true),
        ("begin_version", false),
        ("edit", false),
        ("undo_steps", false),
        ("redo_step", false),
    ] {
        let t = tools.iter().find(|t| t.spec.name == name).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(t.read_only, read_only, "{name}");
    }
}

#[test]
fn edit_mode_is_a_mode_and_the_other_modes_cannot_change_anything() {
    let mut s = demo();
    s.execute("assistant.settings", json!({"mode": "edit"})).unwrap();
    assert_eq!(s.assistant.settings.mode, "edit");
    let modes = s.execute("assistant.status", json!({})).unwrap()["modes"].clone();
    assert!(modes.as_array().unwrap().iter().any(|m| m["id"] == "edit"), "{modes}");
    assert!(s.execute("assistant.settings", json!({"mode": "direct"})).is_err());
    for mode in ["ask", "coach"] {
        s.execute("assistant.settings", json!({"mode": mode})).unwrap();
        for tool in ["begin_version", "edit", "undo_steps", "redo_step"] {
            let out = call(&mut s, tool, json!({"title": "x", "command": "project.setNotes", "params": {"text": "x"}}));
            assert!(out.is_error && out.content.contains("not in Edit mode"), "{mode} {tool}: {}", out.content);
        }
        assert!(!s.versions.is_open(), "{mode}: nothing was opened");
    }
    // the agent offers the edit tools only in Edit mode, and the engine refuses them to the other profiles too
    let all = crate::assistant_tools::tools();
    assert!(!edith_ai::Profile::Coach.allows(all.iter().find(|t| t.spec.name == "edit").unwrap().read_only));
    assert!(edith_ai::Profile::Propose.allows(all.iter().find(|t| t.spec.name == "edit").unwrap().read_only));
    assert!(s.run_assistant_tool("edit", &json!({})).is_error);
}

#[test]
fn nothing_changes_until_a_version_is_open_and_then_everything_goes_into_it() {
    let mut s = demo();
    edit_mode(&mut s);
    let main_before = s.project.clone();
    let main_undo = s.history.undo.len();
    let out = call(&mut s, "edit", json!({"command": "project.setNotes", "params": {"text": "x"}}));
    assert!(out.is_error && out.content.contains("begin_version"), "{}", out.content);
    assert!(std::sync::Arc::ptr_eq(&main_before, &s.project));
    let out = call(&mut s, "begin_version", json!({"title": "Tighten act two"}));
    assert!(!out.is_error && out.content.contains("Tighten act two") && out.content.contains("You cannot commit"), "{}", out.content);
    assert!(s.versions.is_open());
    // asking again does not open another: it works in the one that is open
    let again = call(&mut s, "begin_version", json!({"title": "Another"}));
    assert!(!again.is_error && again.content.contains("already open") && again.content.contains("Tighten act two"), "{}", again.content);
    // the edit is the assistant's and main is untouched
    let out = call(&mut s, "edit", json!({"command": "project.setNotes", "params": {"text": "changed by the assistant"}}));
    assert!(!out.is_error && out.content.starts_with("Done: project.setNotes") && out.content.contains("1 step"), "{}", out.content);
    assert_eq!(notes(&s), "changed by the assistant");
    let status = call(&mut s, "version_status", json!({}));
    assert!(status.content.contains("by assistant") && status.content.contains("Changes the project notes"), "{}", status.content);
    s.execute("version.peek", json!({})).unwrap();
    assert_eq!(notes(&s), main_before.notes, "main is what it was");
    // an edit while the person looks at the original still goes into the version
    let out = call(&mut s, "edit", json!({"command": "project.setNotes", "params": {"text": "second"}}));
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(notes(&s), main_before.notes, "still looking at the original");
    s.execute("version.peek", json!({})).unwrap();
    assert_eq!(notes(&s), "second");
    // the assistant cannot commit or discard
    for (cmd, p) in [("version.commit", json!({})), ("version.discard", json!({})), ("version.create", json!({}))] {
        let out = call(&mut s, "edit", json!({"command": cmd, "params": p}));
        assert!(out.is_error && out.content.contains("not one the assistant may run"), "{cmd}: {}", out.content);
    }
    assert!(s.versions.is_open());
    // the person commits: one undo step
    s.execute("version.commit", json!({})).unwrap();
    assert_eq!(s.history.undo.len(), main_undo + 1, "one undo step in main, however many the version took");
    assert!(s.history.undo.last().unwrap().0.starts_with("Version: Tighten"));
}

#[test]
fn the_assistant_takes_back_its_own_steps_and_never_the_persons() {
    let mut s = demo();
    edit_mode(&mut s);
    call(&mut s, "begin_version", json!({"title": "T"}));
    for t in ["a1", "a2"] {
        call(&mut s, "edit", json!({"command": "project.setNotes", "params": {"text": t}}));
    }
    // the person makes a step of their own
    s.execute("project.setNotes", json!({"text": "person"})).unwrap();
    let out = call(&mut s, "undo_steps", json!({"steps": 3}));
    assert!(out.is_error && out.content.contains("person's"), "it stops at their step: {}", out.content);
    assert_eq!(notes(&s), "person", "their step was not touched");
    // a step of its own on top, and then it can take that back, but not past the person's
    call(&mut s, "edit", json!({"command": "project.setNotes", "params": {"text": "a3"}}));
    let out = call(&mut s, "undo_steps", json!({"steps": 5}));
    assert!(!out.is_error && out.content.starts_with("Took back 1") && out.content.contains("person's"), "{}", out.content);
    assert_eq!(notes(&s), "person");
    // redo puts its step back
    let out = call(&mut s, "redo_step", json!({}));
    assert!(!out.is_error && out.content.starts_with("Put back"), "{}", out.content);
    assert_eq!(notes(&s), "a3");
    assert!(call(&mut s, "redo_step", json!({})).is_error, "nothing more to put back");
    // refusals
    for bad in [json!({"steps": 0}), json!({"steps": 21}), json!({"steps": "x"}), json!({"steps": -2})] {
        assert!(call(&mut s, "undo_steps", bad.clone()).is_error, "{bad}");
    }
    // steps kept from before a file was saved are a record, and not the assistant's to undo
    let mut t = demo();
    edit_mode(&mut t);
    assert!(call(&mut t, "undo_steps", json!({})).is_error, "no version open");
    call(&mut t, "begin_version", json!({}));
    assert!(call(&mut t, "undo_steps", json!({})).is_error, "nothing to take back");
}

#[test]
fn what_the_assistant_may_run_is_the_edits_to_the_project_and_nothing_that_reaches_outside() {
    for id in [
        "markers.add",
        "effects.apply",
        "clip.enable",
        "timeline.setTrack",
        "trim.edit",
        "planner.addCue",
        "planner.setSound",
        "sound.placeCues",
        "lumetri.applyMatch",
        "look.match",
        "essentialSound.setType",
        "project.setNotes",
        "sequence.addEdit",
        "edit.deselectAll",
    ] {
        assert!(may_run(id).is_ok(), "{id}: {:?}", may_run(id));
    }
    for id in [
        "file.save",
        "file.saveAs",
        "file.open",
        "file.import",
        "file.newProject",
        "export.quick",
        "export.queue.start",
        "assistant.settings",
        "assistant.send",
        "secrets.set",
        "connections.add",
        "net.fetch",
        "prefs.set",
        "hub.search",
        "tts.generate",
        "reference.add",
        "analysis.run",
        "provenance.add",
        "window.floatPanel",
        "shortcuts.set",
        "source.setIn",
        "playback.play",
        "media.relink",
        "version.commit",
        "version.discard",
        "version.create",
        "version.run",
        "edit.undo",
        "edit.redo",
        "sound.sheet",
        "planner.export",
        "planner.import",
        "look.addStill",
        "look.create",
        "look.remove",
        "look.set",
        "no.such.command",
        "x",
        "",
    ] {
        assert!(may_run(id).is_err(), "{id} must not be allowed");
    }
    // a word that says what it would do is enough to refuse it, in any family
    for id in [
        "timeline.exportFrame",
        "clip.saveAs",
        "sequence.renderSelection",
        "markers.importFrom",
        "effects.downloadPreset",
        "mixer.recordTake",
        "project.revealInFinder",
        "graphics.exportTemplate",
    ] {
        assert!(may_run(id).is_err(), "{id}");
    }
    // across every registered command: allowed ones never name a risky word or sit in a refused family
    let all = crate::commands::command_specs();
    let allowed: Vec<&str> = all.iter().map(|c| c.id).filter(|id| may_run(id).is_ok()).collect();
    assert!(allowed.len() > 150, "the assistant can run the edits: {}", allowed.len());
    for id in &allowed {
        let l = id.to_ascii_lowercase();
        for w in ["export", "save", "import", "download", "record", "capture", "render", "relink"] {
            assert!(!l.contains(w), "{id} names {w}");
        }
        for p in ["file.", "export.", "assistant.", "secrets.", "connections.", "net.", "prefs.", "hub.", "tts.", "version."] {
            assert!(!id.starts_with(p), "{id}");
        }
    }
}

#[test]
fn a_command_that_names_a_file_or_an_address_is_refused_wherever_the_name_is() {
    let mut s = demo();
    edit_mode(&mut s);
    call(&mut s, "begin_version", json!({}));
    for p in [
        json!({"path": "/tmp/x"}),
        json!({"url": "http://example.com"}),
        json!({"text": "x", "file": "a"}),
        json!({"nested": {"deeper": [{"folder": "/"}]}}),
        json!({"paths": ["a", "b"]}),
    ] {
        let out = call(&mut s, "edit", json!({"command": "project.setNotes", "params": p.clone()}));
        assert!(out.is_error && out.content.contains("names a file or an address"), "{p}: {}", out.content);
    }
    assert_eq!(notes(&s), "", "none of them ran");
    for bad in [
        json!({}),
        json!({"command": 5}),
        json!({"command": ""}),
        json!({"command": "x".repeat(200)}),
        json!({"command": "project.setNotes", "params": 5}),
        json!({"command": "project.setNotes", "params": [1]}),
    ] {
        assert!(call(&mut s, "edit", bad.clone()).is_error, "{bad}");
    }
    // a command that fails says why and leaves the version as it was
    let before = s.version_info().unwrap().steps.len();
    let out = call(&mut s, "edit", json!({"command": "timeline.setTrack", "params": {"track": "V99", "muted": true}}));
    assert!(out.is_error && out.content.contains("failed"), "{}", out.content);
    assert_eq!(s.version_info().unwrap().steps.len(), before);
    // and the edit tool does not take a run through the session's other doors
    assert!(call(&mut s, "edit", json!({"command": "file.save"})).is_error);
}

#[test]
fn command_list_names_what_the_assistant_may_run_and_nothing_else() {
    let s = demo();
    let overview = crate::assistant_tools::tools().into_iter().find(|t| t.spec.name == "command_list").map(|t| (t.run)(&s, &json!({}))).unwrap();
    assert!(overview.content.contains("families") && overview.content.contains("timeline (") && overview.content.contains("markers ("), "{}", overview.content);
    assert!(!overview.content.contains("file (") && !overview.content.contains("export ("), "{}", overview.content);
    let run = |q: &str| (crate::assistant_tools::tools().into_iter().find(|t| t.spec.name == "command_list").unwrap().run)(&s, &json!({"query": q}));
    let m = run("marker");
    assert!(m.content.contains("- markers.add — Add Marker") && m.content.contains("durationFrames"), "{}", m.content);
    assert!(!run("save").content.contains("file.save"));
    assert!(run("zzzznothing").content.contains("No command you may run matches"));
    assert!(run("").content.contains("families"));
    let many = run("e");
    assert!(many.content.lines().count() <= 42, "capped");
    assert!(many.content.contains("more match") || many.content.lines().count() < 41);
}

#[test]
fn planner_get_reads_the_plan_in_words_with_the_ids_the_planner_commands_take() {
    let mut s = demo();
    let none = s.run_assistant_tool("planner_get", &json!({}));
    assert!(!none.is_error && none.content.contains("no plan yet"), "{}", none.content);
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 10})).unwrap();
    let beat = crate::planner::get(&s.project).unwrap().beats().next().unwrap().id;
    s.execute("planner.updateBeat", json!({"beat": beat, "narration": "One two three. Four five six."})).unwrap();
    s.execute("planner.setSound", json!({"beat": beat, "ambience": "wind over the shore", "music": "low strings"})).unwrap();
    let out = s.run_assistant_tool("planner_get", &json!({}));
    assert!(!out.is_error, "{}", out.content);
    for want in [
        "Plan “",
        "from the mystery pack",
        "Title style of the pack: Short, plain and a little cold",
        "sound plan: bed: wind over the shore; music: low strings",
        &format!("- beat {beat} “Cold open”"),
        "intensity",
        "narration (6 words): One two three. Four five six.",
        "purpose:",
    ] {
        assert!(out.content.contains(want), "{want:?} is not in\n{}", out.content);
    }
    // and in Ask and Coach too: it only reads
    s.execute("assistant.settings", json!({"mode": "ask"})).unwrap();
    assert!(!s.run_assistant_tool("planner_get", &json!({})).is_error);
}

#[test]
fn version_status_is_there_in_every_mode_and_says_so_when_nothing_is_open() {
    let mut s = demo();
    for mode in ["ask", "coach", "edit"] {
        s.execute("assistant.settings", json!({"mode": mode})).unwrap();
        let out = s.run_assistant_tool("version_status", &json!({}));
        assert!(!out.is_error && out.content.contains("No version is open"), "{mode}: {}", out.content);
    }
    s.execute("version.create", json!({"title": "Mine"})).unwrap();
    s.execute("project.setNotes", json!({"text": "by hand"})).unwrap();
    let out = s.run_assistant_tool("version_status", &json!({}));
    assert!(out.content.contains("“Mine” is open (started by you)") && out.content.contains("(by you)"), "{}", out.content);
    s.execute("version.peek", json!({})).unwrap();
    assert!(s.run_assistant_tool("version_status", &json!({})).content.contains("looking at the original"));
}

#[test]
fn a_scripted_model_in_edit_mode_opens_a_version_edits_in_it_and_leaves_the_commit_to_the_person() {
    let mut s = demo();
    s.execute("project.setNotes", json!({"text": "main notes"})).unwrap();
    edit_mode(&mut s);
    let model = scripted(vec![
        tool_reply(&[("t1", "begin_version", json!({"title": "Notes pass"}))]),
        tool_reply(&[("t2", "edit", json!({"command": "project.setNotes", "params": {"text": "tighter notes"}}))]),
        text_reply("I opened a version and changed the notes in it. Look at it and commit it if you like it."),
    ]);
    s.assistant.use_model(model.clone());
    s.execute("assistant.send", json!({"text": "tighten the notes", "wait": true})).unwrap();
    assert!(s.versions.is_open(), "the assistant opened a version");
    assert_eq!(s.versions.title(), Some("Notes pass"));
    assert_eq!(notes(&s), "tighter notes");
    let steps = s.version_info().unwrap().steps;
    assert_eq!((steps.len(), steps[0].by.id()), (1, "assistant"));
    // main is what it was
    s.execute("version.peek", json!({})).unwrap();
    assert_eq!(notes(&s), "main notes");
    s.execute("version.peek", json!({})).unwrap();
    // the request carried the edit tools and a note of where the version stood
    let first = &model.requests()[0];
    assert!(first.tools.iter().any(|t| t.name == "begin_version") && first.tools.iter().any(|t| t.name == "edit"));
    assert!(first.messages[0].text().contains("[edit]") && first.messages[0].text().contains("No version is open"), "{}", first.messages[0].text());
    // in Ask mode the same assistant is not even offered the edit tools
    s.execute("assistant.settings", json!({"mode": "ask"})).unwrap();
    model.push_reply(text_reply("ok").0, text_reply("ok").1);
    s.execute("assistant.send", json!({"text": "what changed?", "wait": true})).unwrap();
    let last = model.requests().last().cloned().unwrap();
    assert!(!last.tools.iter().any(|t| t.name == "edit" || t.name == "begin_version"));
    assert!(last.tools.iter().any(|t| t.name == "version_status"), "but it can read the version");
}
