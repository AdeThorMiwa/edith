//! Tests of conversations belonging to projects (fork roadmap F16.6): a new project starts with none of another
//! project's conversations, the identity travels in the project file, undo cannot take it away, and conversations
//! from before projects can be moved into one.

use serde_json::{Value, json};

use crate::Session;
use crate::assistant_tests::{TEST_KEY, scripted, set_project_key, text_reply};

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("filmcraft-scope-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn ask(s: &mut Session, text: &str) -> u64 {
    s.assistant.use_model(scripted(vec![text_reply("ok")]));
    s.execute("assistant.send", json!({"text": text, "wait": true})).unwrap()["thread"].as_u64().unwrap()
}

fn titles(s: &mut Session) -> Vec<String> {
    s.execute("assistant.threads", json!({})).unwrap()["threads"].as_array().unwrap().iter().map(|t| t["title"].as_str().unwrap().to_string()).collect()
}

fn older(s: &mut Session) -> Vec<u64> {
    s.execute("assistant.threads", json!({})).unwrap()["older"].as_array().unwrap().iter().map(|t| t["id"].as_u64().unwrap()).collect()
}

#[test]
fn a_new_project_does_not_show_the_conversations_of_another() {
    let dir = tmp("two");
    let mut s = Session::default();
    s.assistant.set_dir(&dir);
    s.execute("file.openDemoProject", json!({})).unwrap();
    let first = s.execute("assistant.threads", json!({})).unwrap();
    assert_eq!(
        (first["threads"].as_array().unwrap().len(), first["scope"]["project"].clone()),
        (0, Value::Null),
        "nothing yet, and no identity until it is used"
    );
    ask(&mut s, "what is in the first project?");
    assert_eq!(titles(&mut s), ["what is in the first project?"]);
    let key_a = s.execute("assistant.status", json!({})).unwrap()["scope"]["project"].as_str().unwrap().to_string();
    assert!(key_a.len() >= 32, "{key_a}");
    let project_a = s.project.clone();

    // a new project: the panel's lists are empty and the first project's conversation is not in them
    s.execute("file.newProject", json!({"name": "Second"})).unwrap();
    let st = s.execute("assistant.status", json!({})).unwrap();
    assert!(st["threads"].as_array().unwrap().is_empty() && st["scope"]["project"].is_null(), "{st}");
    assert!(st["older"].as_array().unwrap().is_empty(), "a conversation of another project is not an older one");
    let t = ask(&mut s, "and the second?");
    assert_eq!(titles(&mut s), ["and the second?"]);
    let key_b = s.execute("assistant.status", json!({})).unwrap()["scope"]["project"].as_str().unwrap().to_string();
    assert_ne!(key_a, key_b);

    // the conversation of another project cannot be read, continued, renamed or deleted from this one
    let old = s.assistant.threads.iter().find(|x| x.id != t).map(|x| x.id).unwrap();
    for (cmd, p) in [
        ("assistant.thread", json!({"thread": old})),
        ("assistant.send", json!({"thread": old, "text": "hi"})),
        ("assistant.renameThread", json!({"thread": old, "title": "x"})),
        ("assistant.deleteThread", json!({"thread": old})),
        ("assistant.trace", json!({"thread": old})),
        ("assistant.cancel", json!({"thread": old})),
        ("assistant.cards", json!({"thread": old})),
        ("assistant.adoptThread", json!({"thread": old})),
    ] {
        let e = s.execute(cmd, p).expect_err(cmd).to_string();
        assert!(e.contains("another project") || e.contains("already belongs to a project"), "{cmd}: {e}");
    }
    assert!(s.assistant.threads.iter().any(|x| x.id == old), "nothing was deleted");

    // back in the first project, its own conversation is there and the second's is not
    s.project = project_a;
    assert_eq!(titles(&mut s), ["what is in the first project?"]);
    let resumed = s.execute("assistant.cards", json!({}));
    assert!(resumed.is_err() || resumed.unwrap()["cards"].as_array().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_identity_is_saved_in_the_project_file_and_the_conversations_follow_the_file() {
    let dir = tmp("file");
    let mut s = Session::default();
    s.assistant.set_dir(&dir);
    s.execute("file.openDemoProject", json!({})).unwrap();
    assert!(!s.is_dirty());
    ask(&mut s, "remember me");
    assert!(s.is_dirty(), "the project now remembers its conversations, so it asks to be saved");
    // saved and opened again later (a different session, a fresh load of the same file)
    let bytes = filmcraft_format::encode(&s.project, false);
    let loaded = filmcraft_format::decode(&bytes).unwrap();
    let mut later = Session::default();
    later.assistant.set_dir(&dir);
    later.project = std::sync::Arc::new(loaded.project);
    assert_eq!(titles(&mut later), ["remember me"]);
    // and a copy of the file carries on the same conversations
    assert_eq!(titles(&mut later), titles(&mut s));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn undo_and_redo_cannot_take_the_identity_away() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("markers.add", json!({"time": 5 * filmcraft_time::TICKS_PER_SECOND, "name": "first"})).unwrap();
    ask(&mut s, "a question after the edit");
    let key = crate::assistant::project_key(&s.project).expect("an identity");
    s.execute("markers.add", json!({"time": 8 * filmcraft_time::TICKS_PER_SECOND, "name": "second"})).unwrap();
    // undo past the point where the assistant was first used: that older snapshot never had the identity
    s.undo().unwrap();
    s.undo().unwrap();
    assert_eq!(crate::assistant::project_key(&s.project).as_deref(), Some(key.as_str()), "kept through undo");
    assert_eq!(titles(&mut s), ["a question after the edit"]);
    s.redo().unwrap();
    s.redo().unwrap();
    assert_eq!(crate::assistant::project_key(&s.project).as_deref(), Some(key.as_str()), "kept through redo");
    // and a revert to an earlier mark
    let mark = s.undo_mark();
    s.execute("markers.add", json!({"time": 9 * filmcraft_time::TICKS_PER_SECOND, "name": "third"})).unwrap();
    s.revert_to(mark);
    assert_eq!(crate::assistant::project_key(&s.project).as_deref(), Some(key.as_str()));
}

#[test]
fn conversations_from_before_projects_are_listed_apart_and_adopted_one_at_a_time() {
    let dir = tmp("older");
    let folder = dir.join("edith-threads");
    std::fs::create_dir_all(&folder).unwrap();
    let thread = json!({"messages": [{"role": "user", "parts": [{"type": "text", "text": "q"}]}, {"role": "assistant", "parts": [{"type": "text", "text": "a"}]}], "usage": {"input_tokens": 0, "output_tokens": 0, "cache_read_tokens": 0, "cache_write_tokens": 0}});
    let old = |id: u64, title: &str| json!({"version": 1, "id": id, "title": title, "thread": thread, "events": [], "dropped": 0, "finish": {"kind": "done"}, "usage": {"input_tokens": 1, "output_tokens": 1, "cache_read_tokens": 0, "cache_write_tokens": 0}, "meta": {"prompt_version": "2", "model": "m", "profile": "read_only", "started_unix": 0}, "replies": []});
    std::fs::write(folder.join("1.json"), old(1, "from long ago").to_string()).unwrap();
    std::fs::write(folder.join("2.json"), old(2, "also old").to_string()).unwrap();
    // one claims a project that is not this one, and one has an identity that is nonsense
    let mut other = old(3, "someone else's");
    other["project"] = json!(TEST_KEY);
    std::fs::write(folder.join("3.json"), other.to_string()).unwrap();
    let mut junk = old(4, "junk identity");
    junk["project"] = json!("../../etc/passwd");
    std::fs::write(folder.join("4.json"), junk.to_string()).unwrap();

    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.assistant.set_dir(&dir);
    assert!(titles(&mut s).is_empty(), "none of them belong to this project");
    assert_eq!(older(&mut s), [1, 2, 4], "the ones with no project (or a nonsense one) are the older ones");
    // reading an older one is not possible until it is adopted
    assert!(s.execute("assistant.thread", json!({"thread": 1})).is_err());
    let r = s.execute("assistant.adoptThread", json!({"thread": 1})).unwrap();
    assert_eq!((r["thread"].clone(), r["saved"].clone()), (json!(1), json!(true)));
    assert_eq!(titles(&mut s), ["from long ago"]);
    assert_eq!(older(&mut s), [2, 4]);
    assert!(s.execute("assistant.thread", json!({"thread": 1})).is_ok());
    assert!(s.execute("assistant.adoptThread", json!({"thread": 1})).unwrap_err().to_string().contains("already belongs"));
    assert!(s.execute("assistant.adoptThread", json!({"thread": 3})).is_err(), "another project's conversation is not up for adoption");
    for bad in [json!({}), json!({"thread": "one"}), json!({"thread": 999}), json!({"thread": -1})] {
        assert!(s.execute("assistant.adoptThread", bad.clone()).is_err(), "{bad}");
    }
    // the adoption is on disk: another run of the app finds it, in the same project
    let mut later = Session { project: s.project.clone(), ..Session::default() };
    later.assistant.set_dir(&dir);
    assert_eq!(titles(&mut later), ["from long ago"]);
    // and a different project does not
    let mut elsewhere = Session::default();
    elsewhere.assistant.set_dir(&dir);
    assert!(titles(&mut elsewhere).is_empty());
    assert_eq!(older(&mut elsewhere), [2, 4]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_identity_in_a_project_is_no_identity_and_never_a_crash() {
    for bad in [
        json!(5),
        json!({"id": 5}),
        json!({"id": ""}),
        json!({"id": "short"}),
        json!({"id": "x".repeat(200)}),
        json!({"id": "has spaces in it okay"}),
        json!({"id": "../../etc"}),
        json!(null),
        json!([]),
    ] {
        let mut s = Session::default();
        s.edit("odd", |p, _| {
            p.ext.insert(crate::assistant::PROJECT_KEY.to_string(), bad.clone());
            Ok(())
        })
        .unwrap();
        assert!(crate::assistant::project_key(&s.project).is_none(), "{bad}");
        let st = s.execute("assistant.status", json!({})).unwrap();
        assert!(st["scope"]["project"].is_null() && st["threads"].as_array().unwrap().is_empty(), "{bad}");
        // using the assistant gives it a good one, replacing the damaged one
        ask(&mut s, "hello");
        assert!(crate::assistant::project_key(&s.project).is_some(), "{bad}");
        assert_eq!(titles(&mut s), ["hello"]);
    }
    // a project with a good identity keeps it
    let mut s = Session::default();
    set_project_key(&mut s, "a-perfectly-good-key-1234");
    ask(&mut s, "one");
    assert_eq!(crate::assistant::project_key(&s.project).as_deref(), Some("a-perfectly-good-key-1234"));
}
