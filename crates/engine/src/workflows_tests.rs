//! Tests of workflows in the engine (fork roadmap F16.4): the library with the person's own files, where the
//! project is in one, saved with the project and with the version, and the three tools.

use serde_json::{Value, json};

use crate::Session;
use crate::assistant_edit::run_edit;

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("filmcraft-workflows-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

const MINE: &str = "---\nname: my-montage\ndescription: Cut a montage to a score, on the beat.\n---\nKeep it simple.\n\n## Stage 1: pick the score\nFind the music.\n\n## Stage 2: cut\nCut on the beat.\n";

fn with_folder(s: &mut Session, name: &str) -> std::path::PathBuf {
    let d = tmp(name);
    s.prefs_path = Some(d.join("preferences.json"));
    let w = d.join("workflows");
    std::fs::create_dir_all(&w).unwrap();
    w
}

fn names(v: &Value) -> Vec<String> {
    v["workflows"].as_array().unwrap().iter().map(|w| w["name"].as_str().unwrap().to_string()).collect()
}

#[test]
fn the_shipped_workflow_is_listed_and_the_persons_own_files_join_it_with_the_bad_ones_named() {
    let mut s = demo();
    let list = s.execute("workflow.list", json!({})).unwrap();
    assert_eq!(names(&list), ["rough-cut-from-narration"]);
    assert_eq!(list["workflows"][0]["yours"], false);
    assert!(list["workflows"][0]["stages"].as_array().unwrap().len() >= 5);
    let dir = with_folder(&mut s, "own");
    std::fs::write(dir.join("my-montage.md"), MINE).unwrap();
    std::fs::write(dir.join("broken.md"), "---\nname: Broken Name\ndescription: x\n---\n## Stage 1: a\nx").unwrap();
    std::fs::write(dir.join("empty.md"), "").unwrap();
    std::fs::write(dir.join("huge.md"), "x".repeat(40_000)).unwrap();
    std::fs::write(dir.join("notes.txt"), MINE).unwrap();
    // one of the person's files replaces a shipped workflow of the same name
    std::fs::write(dir.join("rough.md"), MINE.replace("my-montage", "rough-cut-from-narration").replace("a montage", "my own rough cut")).unwrap();
    let list = s.execute("workflow.list", json!({})).unwrap();
    let mut got = names(&list);
    got.sort();
    assert_eq!(got, ["my-montage", "rough-cut-from-narration"]);
    let yours: Vec<bool> = list["workflows"].as_array().unwrap().iter().map(|w| w["yours"].as_bool().unwrap()).collect();
    assert!(yours.iter().all(|y| *y), "both are the person's now: {list}");
    let problems: Vec<&str> = list["problems"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
    assert_eq!(problems.len(), 3, "{problems:?}");
    assert!(problems.iter().any(|p| p.starts_with("broken.md: ") && p.contains("lowercase letters")), "{problems:?}");
    assert!(
        problems.iter().any(|p| p.starts_with("empty.md: ")) && problems.iter().any(|p| p.starts_with("huge.md: ") && p.contains("longer than 32 KB")),
        "{problems:?}"
    );
    // a folder that is not there, or a pile of files, is fine
    let mut t = demo();
    t.prefs_path = Some(std::env::temp_dir().join("filmcraft-no-such-folder").join("preferences.json"));
    assert_eq!(names(&t.execute("workflow.list", json!({})).unwrap()), ["rough-cut-from-narration"]);
    let lots = with_folder(&mut t, "lots");
    for k in 0..60 {
        std::fs::write(lots.join(format!("w{k:02}.md")), MINE.replace("my-montage", &format!("flow-{k:02}"))).unwrap();
    }
    assert!(names(&t.execute("workflow.list", json!({})).unwrap()).len() <= 51, "only the first 50 files are read");
}

#[test]
fn progress_is_recorded_by_number_or_name_and_each_record_is_one_undo_step() {
    let mut s = demo();
    assert_eq!(s.execute("workflow.state", json!({})).unwrap()["active"], false);
    let undo = s.history.undo.len();
    let r = s.execute("workflow.progress", json!({"workflow": "rough-cut-from-narration", "stage": 1, "note": "Asked about the tone.\n\nWaiting."})).unwrap();
    assert_eq!(
        (r["active"].clone(), r["stage"].clone(), r["stageName"].clone(), r["status"].clone()),
        (json!(true), json!(1), json!("understand"), json!("working"))
    );
    assert_eq!(r["note"], "Asked about the tone. Waiting.");
    assert_eq!(s.history.undo.len(), undo + 1);
    // the workflow need not be named again, and a stage can be named
    let r = s.execute("workflow.progress", json!({"stage": "Find the Ideas", "status": "waiting"})).unwrap();
    assert_eq!((r["stage"].clone(), r["status"].clone()), (json!(2), json!("waiting")));
    s.execute("workflow.progress", json!({"stage": 1, "status": "done"})).unwrap();
    s.execute("workflow.progress", json!({"stage": 2, "status": "done"})).unwrap();
    let r = s.execute("workflow.state", json!({})).unwrap();
    assert_eq!((r["stage"].clone(), r["done"].clone()), (json!(2), json!([1, 2])));
    assert_eq!(r["stages"].as_u64().unwrap() as usize, s.execute("workflow.list", json!({})).unwrap()["workflows"][0]["stages"].as_array().unwrap().len());
    // undo steps back through them, one at a time
    s.undo().unwrap();
    assert_eq!(s.execute("workflow.state", json!({})).unwrap()["done"], json!([1]));
    // clearing forgets it, and can be taken back
    s.execute("workflow.clear", json!({})).unwrap();
    assert_eq!(s.execute("workflow.state", json!({})).unwrap()["active"], false);
    s.undo().unwrap();
    assert_eq!(s.execute("workflow.state", json!({})).unwrap()["active"], true);
    assert!(s.execute("workflow.clear", json!({})).is_ok(), "clearing twice is fine");
}

#[test]
fn the_progress_is_saved_with_the_project_and_odd_values_are_refused() {
    let mut s = demo();
    s.execute("workflow.progress", json!({"workflow": "rough-cut-from-narration", "stage": 3, "status": "waiting", "note": "Asking about the harbour clip."}))
        .unwrap();
    let bytes = filmcraft_format::encode(&s.project, false);
    let back = filmcraft_format::decode(&bytes).unwrap();
    let st = crate::workflows::state(&back.project).expect("the progress came back");
    assert_eq!(
        (st.name.as_str(), st.stage, st.status.as_str(), st.note.as_str()),
        ("rough-cut-from-narration", 3, "waiting", "Asking about the harbour clip.")
    );
    let before = (s.revision, s.history.undo.len());
    for (p, want) in [
        (json!({"stage": 1, "workflow": "no-such"}), "there is no workflow"),
        (json!({"workflow": "rough-cut-from-narration"}), "`stage` is required"),
        (json!({"workflow": "rough-cut-from-narration", "stage": 99}), "has no stage"),
        (json!({"workflow": "rough-cut-from-narration", "stage": "nope"}), "has no stage"),
        (json!({"workflow": "rough-cut-from-narration", "stage": true}), "`stage` is required"),
        (json!({"workflow": "rough-cut-from-narration", "stage": 1, "status": "flying"}), "`status` must be"),
        (json!({"workflow": "rough-cut-from-narration", "stage": 1, "note": 5}), "`note` must be text"),
        (json!({"workflow": "rough-cut-from-narration", "stage": 1, "note": "x".repeat(301)}), "longer than 300"),
    ] {
        let e = s.execute("workflow.progress", p.clone()).expect_err(&p.to_string()).to_string();
        assert!(e.contains(want), "{want:?} not in {e:?}");
    }
    assert_eq!((s.revision, s.history.undo.len()), before, "refused input changes nothing");
    let mut fresh = demo();
    assert!(fresh.execute("workflow.progress", json!({"stage": 1})).unwrap_err().to_string().contains("`workflow` is required"));
    // a hand-edited project with nonsense in the slot reads as no progress, never a crash
    let mut t = demo();
    t.edit("x", |p, _| {
        p.ext.insert(crate::workflows::KEY.to_string(), json!({"name": 5, "stage": "x"}));
        Ok(())
    })
    .unwrap();
    assert_eq!(t.execute("workflow.state", json!({})).unwrap()["active"], false);
    t.edit("y", |p, _| {
        p.ext.insert(crate::workflows::KEY.to_string(), json!({"name": "gone-away", "stage": 2, "status": "teleporting", "note": "n\u{7}ote", "done": "no"}));
        Ok(())
    })
    .unwrap();
    let r = t.execute("workflow.state", json!({})).unwrap();
    assert_eq!((r["active"].clone(), r["known"].clone(), r["status"].clone(), r["note"].clone()), (json!(true), json!(false), json!("working"), json!("note")));
    assert!(crate::workflows::context_line(&t).is_none(), "a workflow that is no longer there gives no line");
}

#[test]
fn the_list_tool_gives_descriptions_only_and_the_load_tool_gives_the_whole_text() {
    let mut s = demo();
    let list = s.run_assistant_tool("workflow_list", &json!({}));
    let load = s.run_assistant_tool("workflow_load", &json!({"name": "rough-cut-from-narration"}));
    assert!(!list.is_error && !load.is_error);
    assert!(list.content.contains("rough-cut-from-narration") && list.content.contains("Turn a narration into a first cut"), "{}", list.content);
    assert!(!list.content.contains("look at the project before asking anything"), "the text of a stage is not in the list");
    assert!(load.content.contains("look at the project before asking anything") && load.content.contains("Stages: 1 understand"), "{}", load.content);
    assert!(list.content.len() * 4 < load.content.len(), "listing is far smaller than loading: {} vs {}", list.content.len(), load.content.len());
    assert!(list.content.len() < 1500);
    // a clumsy name still finds it, a wrong one says what there is
    assert!(!s.run_assistant_tool("workflow_load", &json!({"name": "  Rough-Cut-From-Narration "})).is_error);
    let bad = s.run_assistant_tool("workflow_load", &json!({"name": "no-such"}));
    assert!(bad.is_error && bad.content.contains("rough-cut-from-narration"), "{}", bad.content);
    for odd in [json!({}), json!({"name": 5}), json!({"name": ""}), json!({"name": "x".repeat(200)}), json!(null), json!([1])] {
        assert!(s.run_assistant_tool("workflow_load", &odd).is_error, "{odd}");
    }
    // where the project stands is said in both
    s.execute("workflow.progress", json!({"workflow": "rough-cut-from-narration", "stage": 2, "note": "Six ideas, shown."})).unwrap();
    let list = s.run_assistant_tool("workflow_list", &json!({}));
    let load = s.run_assistant_tool("workflow_load", &json!({"name": "rough-cut-from-narration"}));
    for t in [&list.content, &load.content] {
        assert!(t.contains("at stage 2 of") && t.contains("Six ideas, shown."), "{t}");
    }
    for name in ["workflow_list", "workflow_load"] {
        assert!(crate::assistant_tools::tools().into_iter().find(|t| t.spec.name == name).unwrap().read_only, "{name}");
    }
    assert!(!crate::assistant_tools::tools().into_iter().find(|t| t.spec.name == "workflow_progress").unwrap().read_only);
    // the person's files that failed are mentioned
    let dir = with_folder(&mut s, "badfile");
    std::fs::write(dir.join("oops.md"), "nothing useful").unwrap();
    assert!(s.run_assistant_tool("workflow_list", &json!({})).content.contains("oops.md"));
}

#[test]
fn the_assistant_records_progress_only_in_edit_mode_and_inside_its_version() {
    let mut s = demo();
    let input = json!({"workflow": "rough-cut-from-narration", "stage": 1, "status": "working", "note": "Looking at the project."});
    // not in Edit mode
    for mode in ["ask", "coach"] {
        s.execute("assistant.settings", json!({"mode": mode})).unwrap();
        let out = run_edit(&mut s, "workflow_progress", &input).unwrap();
        assert!(out.is_error && out.content.contains("not in Edit mode"), "{mode}: {}", out.content);
    }
    s.execute("assistant.settings", json!({"mode": "edit"})).unwrap();
    // no version open yet
    let out = run_edit(&mut s, "workflow_progress", &input).unwrap();
    assert!(out.is_error && out.content.contains("begin_version"), "{}", out.content);
    assert_eq!(s.execute("workflow.state", json!({})).unwrap()["active"], false);
    run_edit(&mut s, "begin_version", &json!({"title": "Rough cut"})).unwrap();
    let out = run_edit(&mut s, "workflow_progress", &input).unwrap();
    assert!(!out.is_error && out.content.contains("stage 1 of") && out.content.contains("understand"), "{}", out.content);
    // it is a step of the version, by the assistant, and the context now says where it is
    let info = s.version_info().unwrap();
    assert_eq!(info.steps.len(), 1);
    assert!(info.steps[0].label.contains("Workflow: understand (working)") && info.steps[0].by == crate::version::Actor::Assistant);
    assert!(
        crate::assistant_edit::context(&s).contains("Workflow in progress: “rough-cut-from-narration”, at stage 1 of"),
        "{}",
        crate::assistant_edit::context(&s)
    );
    // a bad stage is an error the assistant can read
    let out = run_edit(&mut s, "workflow_progress", &json!({"stage": 40})).unwrap();
    assert!(out.is_error && out.content.contains("has no stage"), "{}", out.content);
    // discarding the version takes the progress with it; committing keeps it
    s.execute("version.discard", json!({})).unwrap();
    assert_eq!(s.execute("workflow.state", json!({})).unwrap()["active"], false);
    run_edit(&mut s, "begin_version", &json!({"title": "Again"})).unwrap();
    run_edit(&mut s, "workflow_progress", &input).unwrap();
    s.execute("version.commit", json!({})).unwrap();
    assert_eq!(s.execute("workflow.state", json!({})).unwrap()["active"], true);
}

#[test]
fn the_edit_tool_cannot_reach_the_workflow_commands_and_the_progress_survives_a_saved_version() {
    // the generic edit tool is for the edit; workflow progress has a tool of its own
    assert!(crate::assistant_edit::may_run("workflow.progress").is_err());
    assert!(crate::assistant_edit::may_run("workflow.clear").is_err());
    let mut s = demo();
    s.execute("assistant.settings", json!({"mode": "edit"})).unwrap();
    run_edit(&mut s, "begin_version", &json!({"title": "Saved one"})).unwrap();
    run_edit(&mut s, "workflow_progress", &json!({"workflow": "rough-cut-from-narration", "stage": 4, "status": "waiting", "note": "Cut idea one."})).unwrap();
    let saved = s.version_save().expect("a version to save");
    let mut t = demo();
    t.version_restore(&saved.to_ext()).unwrap();
    let r = t.execute("workflow.state", json!({})).unwrap();
    assert_eq!((r["stage"].clone(), r["status"].clone(), r["note"].clone()), (json!(4), json!("waiting"), json!("Cut idea one.")), "{r}");
}
