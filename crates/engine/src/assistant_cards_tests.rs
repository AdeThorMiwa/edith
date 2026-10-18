//! Tests of the question and download cards (fork roadmap F16.1): a question ends the turn, several cards
//! travel together and come back as one message, nothing downloads without the person's Yes, and hostile
//! input is refused.

use edith_ai::Part;
use serde_json::{Value, json};

use crate::Session;
use crate::assistant_tests::{scripted, text_reply, tool_reply};

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("filmcraft-cards-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn ask(id: &str, q: &str, options: Value) -> (String, String, Value) {
    (id.to_string(), "ask_user".to_string(), json!({"question": q, "options": options}))
}

/// A scripted reply with several tool calls.
fn calls(list: Vec<(String, String, Value)>) -> (Vec<edith_ai::StreamEvent>, edith_ai::Response) {
    let refs: Vec<(&str, &str, Value)> = list.iter().map(|(a, b, c)| (a.as_str(), b.as_str(), c.clone())).collect();
    tool_reply(&refs)
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

fn events(s: &mut Session, thread: u64) -> Vec<Value> {
    s.execute("assistant.thread", json!({"thread": thread})).unwrap()["events"].as_array().unwrap().clone()
}

#[test]
fn several_questions_in_one_turn_end_it_and_come_back_as_one_message() {
    let mut s = demo();
    let model = scripted(vec![
        calls(vec![
            ask("a", "What tone should the film have?", json!(["Slow and contemplative", {"label": "Tense", "description": "short shots, low drone"}, "Warm"])),
            ask("b", "Is the harbour footage yours?", json!(["Yes", "No"])),
        ]),
        text_reply("I have two questions for you; answer them together."),
        text_reply("Slow it is. I will begin with the first idea."),
    ]);
    s.assistant.use_model(model.clone());
    let r = s.execute("assistant.send", json!({"text": "cut my narration", "wait": true})).unwrap();
    let thread = r["thread"].as_u64().unwrap();
    assert_eq!(r["finish"]["kind"], "done");
    // the model was told to stop and wait, and both cards are open
    let result = &model.requests()[1].messages[2];
    let texts: Vec<String> = result.parts.iter().filter_map(|p| if let Part::ToolResult { content, .. } = p { Some(content.clone()) } else { None }).collect();
    assert!(texts.len() == 2 && texts.iter().all(|t| t.contains("Stop here")), "{texts:?}");
    let open = cards(&mut s, thread);
    assert_eq!(open["cards"].as_array().unwrap().len(), 2, "{open}");
    assert_eq!((open["cards"][0]["id"].as_str(), open["cards"][1]["id"].as_str()), (Some("c1"), Some("c2")));
    assert_eq!(open["cards"][0]["options"][1]["detail"], "short shots, low drone");
    assert_eq!((open["answered"].clone(), open["canSend"].clone()), (json!(0), json!(true)));
    // choosing is not sending
    let n = events(&mut s, thread).len();
    s.execute("assistant.card", json!({"thread": thread, "card": "c1", "option": 0})).unwrap();
    s.execute("assistant.card", json!({"thread": thread, "card": "c2", "other": "Some of it, the rest is from a stock site"})).unwrap();
    assert_eq!(events(&mut s, thread).len(), n, "nothing is sent by choosing");
    let open = cards(&mut s, thread);
    assert_eq!(open["answered"], 2);
    assert_eq!(open["cards"][0]["answer"], "Slow and contemplative");
    // a choice can be changed until it is sent
    s.execute("assistant.card", json!({"thread": thread, "card": "c1", "option": 2})).unwrap();
    assert_eq!(cards(&mut s, thread)["cards"][0]["draft"]["label"], "Warm");
    // one message, in plain words, and the next turn starts from it
    let sent = s.execute("assistant.answerCards", json!({"thread": thread, "wait": true})).unwrap();
    assert_eq!(sent["finish"]["kind"], "done");
    let last = user_texts(&model).pop().unwrap();
    assert_eq!(
        last,
        "Here are my answers to your questions:\n1. What tone should the film have?: Warm\n2. Is the harbour footage yours?: (in their own words) Some of it, the rest is from a stock site"
    );
    // the conversation records the answers for showing the cards as answered
    let answered = events(&mut s, thread).into_iter().find(|e| e["type"] == "cards_answered").expect("an answered event");
    assert_eq!(answered["answers"][0], json!({"card": "c1", "answer": "Warm"}));
    // nothing is open any more, and answering again is refused
    assert!(cards(&mut s, thread)["cards"].as_array().unwrap().is_empty());
    assert!(s.execute("assistant.answerCards", json!({"thread": thread})).is_err());
    assert!(s.execute("assistant.card", json!({"thread": thread, "card": "c1", "option": 0})).is_err());
}

#[test]
fn an_unanswered_card_goes_as_no_answer_and_typing_instead_leaves_the_cards_closed() {
    let mut s = demo();
    let model = scripted(vec![
        calls(vec![ask("a", "Which?", json!(["One", "Two"])), ask("b", "And?", json!(["Three", "Four"]))]),
        text_reply("Questions."),
        text_reply("Fine."),
        text_reply("Ok."),
    ]);
    s.assistant.use_model(model.clone());
    let thread = s.execute("assistant.send", json!({"text": "go", "wait": true})).unwrap()["thread"].as_u64().unwrap();
    s.execute("assistant.card", json!({"thread": thread, "card": "c2", "option": 1})).unwrap();
    s.execute("assistant.answerCards", json!({"thread": thread, "wait": true})).unwrap();
    assert!(user_texts(&model).pop().unwrap().ends_with("1. Which?: no answer\n2. And?: Four"));
    // a fresh batch, and this time the person types instead
    let model = scripted(vec![calls(vec![ask("a", "Again?", json!(["Yes", "No"]))]), text_reply("Q."), text_reply("Understood.")]);
    s.assistant.use_model(model);
    s.execute("assistant.send", json!({"text": "next", "thread": thread, "wait": true})).unwrap();
    assert_eq!(cards(&mut s, thread)["cards"].as_array().unwrap().len(), 1);
    s.execute("assistant.card", json!({"thread": thread, "card": "c3", "option": 0})).unwrap();
    s.execute("assistant.send", json!({"text": "actually, just do it your way", "thread": thread, "wait": true})).unwrap();
    assert!(cards(&mut s, thread)["cards"].as_array().unwrap().is_empty(), "the card is not open any more");
    assert!(s.execute("assistant.card", json!({"thread": thread, "card": "c3", "option": 1})).is_err());
    // card ids go on counting
    let model = scripted(vec![calls(vec![ask("a", "More?", json!(["Yes", "No"]))]), text_reply("Q.")]);
    s.assistant.use_model(model);
    s.execute("assistant.send", json!({"text": "again", "thread": thread, "wait": true})).unwrap();
    assert_eq!(cards(&mut s, thread)["cards"][0]["id"], "c4");
}

#[test]
fn a_card_is_only_what_it_should_be_and_there_are_at_most_six_a_turn() {
    let mut s = demo();
    let seven: Vec<_> = (0..7).map(|k| ask(&format!("q{k}"), &format!("Question {k}?"), json!(["Yes", "No"]))).collect();
    let model = scripted(vec![calls(seven), text_reply("Done asking.")]);
    s.assistant.use_model(model.clone());
    let thread = s.execute("assistant.send", json!({"text": "ask lots", "wait": true})).unwrap()["thread"].as_u64().unwrap();
    assert_eq!(cards(&mut s, thread)["cards"].as_array().unwrap().len(), 6, "the seventh was refused");
    let results: Vec<(String, bool)> = model.requests()[1].messages[2]
        .parts
        .iter()
        .filter_map(|p| if let Part::ToolResult { content, is_error, .. } = p { Some((content.clone(), *is_error)) } else { None })
        .collect();
    assert_eq!(results.iter().filter(|(_, e)| *e).count(), 1);
    assert!(results[6].0.contains("6 cards are already shown"), "{results:?}");

    // bad cards are refused with the reason, and nothing is shown
    let bad = vec![
        json!({}),
        json!({"question": "", "options": ["a", "b"]}),
        json!({"question": "Q?", "options": ["only one"]}),
        json!({"question": "Q?", "options": ["a", "b", "c", "d", "e", "f", "g"]}),
        json!({"question": "Q?", "options": ["a", "A"]}),
        json!({"question": "Q?", "options": ["a", ""]}),
        json!({"question": "Q?", "options": ["a", 5]}),
        json!({"question": "Q?", "options": [{"label": "x".repeat(81)}, "b"]}),
        json!({"question": "x".repeat(301), "options": ["a", "b"]}),
        json!({"question": "Q?", "options": "yes or no"}),
        json!({"question": 5, "options": ["a", "b"]}),
    ];
    for (k, input) in bad.into_iter().enumerate() {
        let mut t = demo();
        t.assistant.use_model(scripted(vec![tool_reply(&[("a", "ask_user", input.clone())]), text_reply("ok")]));
        let id = t.execute("assistant.send", json!({"text": "x", "wait": true})).unwrap()["thread"].as_u64().unwrap();
        assert!(cards(&mut t, id)["cards"].as_array().unwrap().is_empty(), "{k}: {input}");
    }
    // control characters and long runs of space are tidied
    let mut t = demo();
    t.assistant.use_model(scripted(vec![
        tool_reply(&[("a", "ask_user", json!({"question": "Which\n\n   one\u{7}?", "options": ["a\tb", "c"], "allowOther": false}))]),
        text_reply("ok"),
    ]));
    let id = t.execute("assistant.send", json!({"text": "x", "wait": true})).unwrap()["thread"].as_u64().unwrap();
    let c = cards(&mut t, id);
    assert_eq!((c["cards"][0]["question"].as_str(), c["cards"][0]["options"][0]["label"].as_str()), (Some("Which one ?"), Some("a b")));
    // a question that takes its choices only does not take other words
    assert!(t.execute("assistant.card", json!({"thread": id, "card": "c1", "other": "my own"})).is_err());
    // hostile card answers
    for p in [
        json!({"card": "c1"}),
        json!({"card": "c1", "option": 9}),
        json!({"card": "c1", "option": -1}),
        json!({"card": "c1", "option": "a"}),
        json!({"card": "c1", "option": 0, "other": "x"}),
        json!({"card": "c9", "option": 0}),
        json!({"card": 5, "option": 0}),
        json!({"option": 0}),
        json!({"card": "c1", "decision": "yes"}),
        json!({"card": "c1", "option": 0, "thread": 999}),
    ] {
        let mut q = p.clone();
        if q.get("thread").is_none() {
            q["thread"] = json!(id);
        }
        assert!(t.execute("assistant.card", q).is_err(), "{p}");
    }
    assert!(cards(&mut t, id)["answered"] == 0, "nothing was chosen by those");
    assert!(Session::default().execute("assistant.cards", json!({})).is_err(), "no conversation yet");
}

#[test]
fn the_cards_are_saved_with_the_conversation_and_open_again_in_a_later_run() {
    let dir = tmp("saved");
    let mut a = demo();
    a.assistant.set_dir(&dir);
    a.assistant.use_model(scripted(vec![calls(vec![ask("a", "Which tone?", json!(["Slow", "Fast"]))]), text_reply("Over to you.")]));
    let thread = a.execute("assistant.send", json!({"text": "start", "wait": true})).unwrap()["thread"].as_u64().unwrap();
    a.execute("assistant.card", json!({"thread": thread, "card": "c1", "option": 1})).unwrap();
    let mut b = demo();
    b.assistant.set_dir(&dir);
    let open = cards(&mut b, thread);
    assert_eq!(open["cards"].as_array().unwrap().len(), 1, "{open}");
    assert_eq!(
        (open["cards"][0]["question"].as_str(), open["answered"].clone()),
        (Some("Which tone?"), json!(0)),
        "the unsent choice is not kept, the question is"
    );
    // and it can be answered there
    let model = scripted(vec![text_reply("Slow.")]);
    b.assistant.use_model(model.clone());
    b.execute("assistant.card", json!({"thread": thread, "card": "c1", "option": 0})).unwrap();
    b.execute("assistant.answerCards", json!({"thread": thread, "wait": true})).unwrap();
    assert!(user_texts(&model).pop().unwrap().ends_with("1. Which tone?: Slow"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_assistant_cannot_answer_a_card_or_download_with_the_editing_tool() {
    for id in ["assistant.card", "assistant.cards", "assistant.answerCards", "assistant.send", "hub.acquire", "hub.search"] {
        assert!(crate::assistant_edit::may_run(id).is_err(), "{id} must not be allowed to the assistant");
    }
}

#[test]
fn a_licence_that_is_unknown_or_forbids_commercial_use_is_not_offered() {
    let c = |name: &str, commercial: Option<bool>| edith_sources::Candidate {
        id: "pexels:1".into(),
        source: "pexels".into(),
        kind: edith_sources::Kind::Video,
        title: "Pier".into(),
        author: "Sam".into(),
        author_url: String::new(),
        source_url: String::new(),
        thumb: String::new(),
        width: None,
        height: None,
        seconds: None,
        tags: vec![],
        rights: edith_sources::Rights { name: name.into(), url: String::new(), commercial, attribution_required: None },
        download: None,
    };
    assert!(crate::assistant_cards::licence_problem(&c("", None)).unwrap().contains("states no licence"));
    assert!(crate::assistant_cards::licence_problem(&c("CC BY-NC", Some(false))).unwrap().contains("not licensed for commercial use"));
    assert!(crate::assistant_cards::licence_problem(&c("Pexels License", Some(true))).is_none());
    assert!(crate::assistant_cards::licence_problem(&c("CC BY 4.0", None)).is_none(), "named, commercial use not contradicted");
}

#[test]
fn the_new_tools_are_listed_for_every_mode() {
    let names: Vec<String> = crate::assistant_tools::tools().into_iter().map(|t| t.spec.name).collect();
    for n in ["ask_user", "request_download", "media_search"] {
        assert!(names.iter().any(|x| x == n), "{n}");
    }
    let t = crate::assistant_tools::tools().into_iter().find(|t| t.spec.name == "request_download").unwrap();
    assert!(t.read_only, "it changes nothing in the project: only the person's Yes downloads");
}
