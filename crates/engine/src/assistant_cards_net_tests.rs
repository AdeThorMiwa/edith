//! The download half of the cards (fork roadmap F16.1), against the hub tests' local stock sites: a clip is found,
//! offered, and only downloaded on the person's Yes. Needs the `net` feature, like the hub tests.

use std::time::{Duration, Instant};

use edith_ai::Part;
use serde_json::{Value, json};

use crate::Session;
use crate::assistant_tests::{scripted, text_reply, tool_reply};

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("filmcraft-cards-net-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn cards(s: &mut Session, thread: u64) -> Value {
    s.execute("assistant.cards", json!({"thread": thread})).unwrap()
}

/// The text of every user message the model was sent, in the last request.
fn user_texts(model: &edith_ai::scripted::ScriptedModel) -> Vec<String> {
    let req = model.requests();
    let last = req.last().unwrap();
    last.messages
        .iter()
        .filter(|m| m.role == edith_ai::Role::User)
        .filter_map(|m| m.parts.iter().find_map(|p| if let Part::Text { text, .. } = p { Some(text.clone()) } else { None }))
        .collect()
}

// ------------------------------------------------------------------------------ downloads

fn wait_for(s: &mut Session, what: &str, mut done: impl FnMut(&mut Session) -> bool) {
    let started = Instant::now();
    while !done(s) {
        assert!(started.elapsed() < Duration::from_secs(15), "{what}");
        s.pump_hub();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn project_with_scratch(s: &mut Session, name: &str) -> std::path::PathBuf {
    s.execute("file.newProject", json!({"name": "T"})).unwrap();
    let dir = tmp(name);
    s.execute("file.projectSettings.scratchDisks", json!({"captured": dir.to_string_lossy()})).unwrap();
    dir
}

#[test]
fn a_clip_is_found_offered_and_only_downloaded_when_the_person_says_yes() {
    let (mut s, _rig) = crate::hub_tests::rig(None);
    let dir = project_with_scratch(&mut s, "yes");
    let model = scripted(vec![
        tool_reply(&[("m", "media_search", json!({"text": "pier at dawn", "kind": "image", "count": 3}))]),
        tool_reply(&[("d", "request_download", json!({"candidate": "pexels:photo-1", "why": "It shows the harbour the narration opens on."}))]),
        text_reply("I found one shot I like; tell me if I may get it."),
        text_reply("Thank you. I will use it for the first idea."),
    ]);
    s.assistant.use_model(model.clone());
    let items = s.project.items.len();
    let thread = s.execute("assistant.send", json!({"text": "find the harbour shot", "wait": true})).unwrap()["thread"].as_u64().unwrap();
    // the search results reached the model with licences, and nothing was downloaded
    let found =
        model.requests()[1].messages[2].parts.iter().find_map(|p| if let Part::ToolResult { content, .. } = p { Some(content.clone()) } else { None }).unwrap();
    assert!(
        found.contains("pexels:photo-1") && found.contains("A pier at dawn") && found.contains("licence:") && found.contains("Nothing has been downloaded"),
        "{found}"
    );
    assert!(found.contains("request_download"), "{found}");
    assert_eq!(s.project.items.len(), items);
    assert!(s.hub.acquired("pexels:photo-1").is_none());
    let open = cards(&mut s, thread);
    let card = &open["cards"][0];
    assert_eq!((card["kind"].as_str(), card["id"].as_str()), (Some("download"), Some("c1")));
    assert_eq!((card["candidate"]["title"].as_str(), card["candidate"]["sourceLabel"].as_str()), (Some("A pier at dawn"), Some("Pexels")));
    assert_eq!(card["why"], "It shows the harbour the narration opens on.");
    assert!(card["candidate"]["rightsLine"].as_str().unwrap().contains("commercial use"), "{card}");
    assert_eq!(card["download"]["state"], "none");
    // Yes starts it, as the person's own action
    s.execute("assistant.card", json!({"thread": thread, "card": "c1", "decision": "yes"})).unwrap();
    assert!(s.hub.acquired("pexels:photo-1").is_some());
    // the answers wait for the download
    wait_for(&mut s, "the download", |s| matches!(s.hub.acquired("pexels:photo-1"), Some(crate::hub::Acquired::Done { .. })));
    assert_eq!(s.project.items.len(), items + 1);
    let open = cards(&mut s, thread);
    assert_eq!((open["waiting"].clone(), open["canSend"].clone()), (json!(0), json!(true)));
    // the clip came with its licence recorded, and cannot be changed after Yes
    let item = open["cards"][0]["download"]["item"].as_u64().unwrap();
    assert!(s.execute("provenance.get", json!({"item": item})).unwrap()["record"]["license"]["name"].as_str().is_some_and(|n| !n.is_empty()));
    assert!(s.execute("assistant.card", json!({"thread": thread, "card": "c1", "decision": "no"})).is_err());
    s.execute("assistant.answerCards", json!({"thread": thread, "wait": true})).unwrap();
    let said = user_texts(&model).pop().unwrap();
    assert!(said.contains("Download “A pier at dawn” from Pexels") && said.contains(&format!("yes, it is now in the project as item {item}")), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_and_show_me_others_download_nothing_and_say_so() {
    let (mut s, _rig) = crate::hub_tests::rig(None);
    let dir = project_with_scratch(&mut s, "no");
    let model = scripted(vec![
        tool_reply(&[("m", "media_search", json!({"text": "pier", "kind": "image"}))]),
        tool_reply(&[
            ("a", "request_download", json!({"candidate": "pexels:photo-1", "why": "a"})),
            ("b", "request_download", json!({"candidate": "pixabay:image-9", "why": "b"})),
        ]),
        text_reply("Two to choose from."),
        text_reply("Understood."),
    ]);
    s.assistant.use_model(model.clone());
    let items = s.project.items.len();
    let thread = s.execute("assistant.send", json!({"text": "find shots", "wait": true})).unwrap()["thread"].as_u64().unwrap();
    assert_eq!(cards(&mut s, thread)["cards"].as_array().unwrap().len(), 2);
    s.execute("assistant.card", json!({"thread": thread, "card": "c1", "decision": "no"})).unwrap();
    let others = s.execute("assistant.card", json!({"thread": thread, "card": "c2", "decision": "others"})).unwrap();
    // Find Media is opened with the words Edith searched with, not the clip's title
    assert_eq!((others["findMedia"]["text"].as_str(), others["findMedia"]["kind"].as_str()), (Some("pier"), Some("image")), "{others}");
    s.execute("assistant.answerCards", json!({"thread": thread, "wait": true})).unwrap();
    let said = user_texts(&model).pop().unwrap();
    assert!(said.contains("A pier at dawn") && said.contains(": no\n") && said.contains("look at other options themselves in Find Media"), "{said}");
    assert_eq!(s.project.items.len(), items, "nothing was downloaded");
    assert!(s.hub.acquired("pexels:photo-1").is_none() && s.hub.acquired("pixabay:image-9").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_download_requests_and_searches_are_refused_when_they_should_be() {
    let (mut s, _rig) = crate::hub_tests::rig(None);
    let model = scripted(vec![
        tool_reply(&[
            ("a", "request_download", json!({"candidate": "pexels:never-seen"})),
            ("b", "request_download", json!({})),
            ("c", "request_download", json!({"candidate": 5})),
            ("d", "media_search", json!({})),
            ("e", "media_search", json!({"text": "x", "kind": "hologram"})),
            ("f", "media_search", json!("pier")),
        ]),
        text_reply("none worked"),
    ]);
    s.assistant.use_model(model.clone());
    let thread = s.execute("assistant.send", json!({"text": "try", "wait": true})).unwrap()["thread"].as_u64().unwrap();
    let results: Vec<(String, bool)> = model.requests()[1].messages[2]
        .parts
        .iter()
        .filter_map(|p| if let Part::ToolResult { content, is_error, .. } = p { Some((content.clone(), *is_error)) } else { None })
        .collect();
    assert_eq!(results.len(), 6);
    assert!(results.iter().all(|(_, e)| *e), "{results:?}");
    assert!(results[0].0.contains("search with media_search first"), "{}", results[0].0);
    assert!(cards(&mut s, thread)["cards"].as_array().unwrap().is_empty());
}

#[test]
fn a_search_with_no_answer_from_the_sites_says_what_failed() {
    let (mut s, _rig) = crate::hub_tests::rig(Some("pexels"));
    let model = scripted(vec![tool_reply(&[("m", "media_search", json!({"text": "pier", "kind": "image"}))]), text_reply("ok")]);
    s.assistant.use_model(model.clone());
    s.execute("assistant.send", json!({"text": "find", "wait": true})).unwrap();
    let found =
        model.requests()[1].messages[2].parts.iter().find_map(|p| if let Part::ToolResult { content, .. } = p { Some(content.clone()) } else { None }).unwrap();
    assert!(found.contains("pexels failed"), "{found}");
    assert!(found.contains("pixabay:image-9"), "the other sources still answered: {found}");
}
