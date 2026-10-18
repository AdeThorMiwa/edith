//! Tests of the voice-it card (fork roadmap F16.7): Edith offers to make a narration from pasted words, and only
//! the person's Yes makes it. The built-in voice is named in every request, so the tests do not depend on whether the
//! natural voices are downloaded on this computer.

use std::time::{Duration, Instant};

use edith_ai::Part;
use serde_json::{Value, json};

use crate::Session;
use crate::assistant_tests::{scripted, text_reply, tool_reply};

const SCRIPT: &str =
    "The harbour was quiet that morning. [pause 1s] Nobody saw the small boat leave. By noon the fog had lifted, and the water lay flat and bright.";

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn offer(s: &mut Session, input: Value, replies: usize) -> (u64, std::sync::Arc<edith_ai::scripted::ScriptedModel>) {
    let mut list = vec![tool_reply(&[("n", "request_narration", input)]), text_reply("Shall I make it?")];
    for _ in 0..replies {
        list.push(text_reply("Thank you."));
    }
    let model = scripted(list);
    s.assistant.use_model(model.clone());
    let r = s.execute("assistant.send", json!({"text": "here is my script", "wait": true})).unwrap();
    (r["thread"].as_u64().unwrap(), model)
}

fn cards(s: &mut Session, thread: u64) -> Value {
    s.execute("assistant.cards", json!({"thread": thread})).unwrap()
}

fn wait_made(s: &mut Session, thread: u64) -> Value {
    let started = Instant::now();
    loop {
        let c = cards(s, thread);
        if c["cards"][0]["narration"]["state"] != "rendering" {
            return c;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "the voice was never made");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_card_shows_the_words_the_voice_and_how_long_and_makes_nothing() {
    let mut s = demo();
    let before = (s.project.narrations.len(), s.active_sequence().unwrap().audio_tracks.iter().map(|t| t.items.len()).sum::<usize>());
    let (thread, model) = offer(&mut s, json!({"text": SCRIPT, "voice": "basic-female", "why": "Your script reads as narration."}), 0);
    let told =
        model.requests()[1].messages[2].parts.iter().find_map(|p| if let Part::ToolResult { content, .. } = p { Some(content.clone()) } else { None }).unwrap();
    assert!(told.contains("26 words") || told.contains("words, about"), "{told}");
    assert!(told.contains("Nothing is made unless they press Yes") && told.contains("Stop here"), "{told}");
    let c = cards(&mut s, thread);
    let card = &c["cards"][0];
    assert_eq!((card["kind"].as_str(), card["id"].as_str(), card["voice"]["id"].as_str()), (Some("narration"), Some("c1"), Some("basic-female")));
    assert_eq!(card["words"], 25, "the [pause] marker is not a word");
    assert!(card["estimateSeconds"].as_f64().unwrap() > 5.0 && card["estimateSeconds"].as_f64().unwrap() < 20.0, "{card}");
    assert!(card["preview"].as_str().unwrap().starts_with("The harbour was quiet") && card["why"] == "Your script reads as narration.");
    assert_eq!(card["narration"]["state"], "none");
    assert_eq!((c["answered"].clone(), c["canSend"].clone()), (json!(0), json!(true)));
    let after = (s.project.narrations.len(), s.active_sequence().unwrap().audio_tracks.iter().map(|t| t.items.len()).sum::<usize>());
    assert_eq!(before, after, "an offer makes nothing");
}

#[test]
fn yes_makes_the_narration_on_the_timeline_and_the_answers_wait_for_it() {
    let mut s = demo();
    let (thread, model) = offer(&mut s, json!({"text": SCRIPT, "voice": "basic-female"}), 1);
    s.execute("assistant.card", json!({"thread": thread, "card": "c1", "decision": "yes"})).unwrap();
    // it is being made: the answers cannot be sent yet
    let early = s.execute("assistant.cards", json!({"thread": thread})).unwrap();
    if early["cards"][0]["narration"]["state"] == "rendering" {
        assert_eq!((early["waiting"].clone(), early["canSend"].clone()), (json!(1), json!(false)));
        assert!(s.execute("assistant.answerCards", json!({"thread": thread})).unwrap_err().to_string().contains("still being made"));
    }
    let done = wait_made(&mut s, thread);
    let n = &done["cards"][0]["narration"];
    assert_eq!(n["state"], "done", "{done}");
    assert!(n["seconds"].as_f64().unwrap() > 5.0 && n["clip"].as_u64().is_some(), "{n}");
    assert_eq!(done["canSend"], true);
    // a narration clip is on the timeline at the start, with its word times kept
    let clip = filmcraft_project::ClipId(n["clip"].as_u64().unwrap());
    let (_, it) = s.active_sequence().unwrap().find_item(clip).unwrap();
    assert_eq!(it.start, filmcraft_time::Tick::ZERO);
    assert_eq!(s.project.narrations[&it.item].words.len(), 25);
    assert!(s.run_assistant_tool("narration_timings", &json!({})).content.contains("S3:"));
    // the answer says where it is, and the next turn is told
    s.execute("assistant.answerCards", json!({"thread": thread, "wait": true})).unwrap();
    let said = model
        .requests()
        .last()
        .unwrap()
        .messages
        .iter()
        .rev()
        .find_map(|m| m.parts.iter().find_map(|p| if let Part::Text { text, .. } = p { Some(text.clone()) } else { None }))
        .unwrap();
    assert!(
        said.contains("Make the narration (25 words") && said.contains("yes, the narration is on the timeline now (clip") && said.contains("narration_timings"),
        "{said}"
    );
    // it cannot be pressed twice
    assert!(s.execute("assistant.card", json!({"thread": thread, "card": "c1", "decision": "no"})).is_err(), "the card is closed once answered");
}

#[test]
fn a_new_project_with_no_sequence_gets_one_when_the_narration_is_made() {
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name": "Script only"})).unwrap();
    assert!(s.active_sequence().is_none());
    let (thread, _) = offer(&mut s, json!({"text": SCRIPT, "voice": "basic-female"}), 0);
    s.execute("assistant.card", json!({"thread": thread, "card": "c1", "decision": "yes"})).unwrap();
    let done = wait_made(&mut s, thread);
    assert_eq!(done["cards"][0]["narration"]["state"], "done", "{done}");
    assert!(s.active_sequence().is_some(), "a sequence was made to hold it");
    assert_eq!(s.project.narrations.len(), 1);
}

#[test]
fn no_and_choosing_the_voice_yourself_make_nothing_and_say_so() {
    for (decision, want) in [("no", "no"), ("others", "make the narration themselves in Text to Speech")] {
        let mut s = demo();
        let (thread, model) = offer(&mut s, json!({"text": SCRIPT, "voice": "basic-female"}), 1);
        let r = s.execute("assistant.card", json!({"thread": thread, "card": "c1", "decision": decision})).unwrap();
        if decision == "others" {
            assert_eq!(r["textToSpeech"]["text"], SCRIPT, "the panel opens with the words typed in: {r}");
            assert_eq!(r["textToSpeech"]["voice"], "basic-female");
        } else {
            assert!(r.get("textToSpeech").is_none());
        }
        s.execute("assistant.answerCards", json!({"thread": thread, "wait": true})).unwrap();
        let said = model
            .requests()
            .last()
            .unwrap()
            .messages
            .iter()
            .rev()
            .find_map(|m| m.parts.iter().find_map(|p| if let Part::Text { text, .. } = p { Some(text.clone()) } else { None }))
            .unwrap();
        assert!(said.contains(want), "{said}");
        assert!(s.project.narrations.is_empty(), "nothing was made");
    }
}

#[test]
fn a_request_is_refused_with_the_reason_when_it_cannot_be_offered() {
    let long = "word ".repeat(14_000);
    let cases: Vec<(Value, &str)> = vec![
        (json!({}), "`text` is required"),
        (json!({"text": ""}), "`text` is required"),
        (json!({"text": "   \u{7}  "}), "`text` is required"),
        (json!({"text": "... --- !!!"}), "`text` is required"),
        (json!({"text": 5}), "`text` is required"),
        (json!({"text": long}), "at most 64 KB"),
        (json!({"text": SCRIPT, "voice": "no-such-voice"}), "there is no voice"),
        (json!({"text": SCRIPT, "voice": 5}), "voice"),
    ];
    for (input, want) in cases {
        let mut s = demo();
        s.assistant.use_model(scripted(vec![tool_reply(&[("n", "request_narration", input.clone())]), text_reply("ok")]));
        let model_out = {
            let m = scripted(vec![tool_reply(&[("n", "request_narration", input.clone())]), text_reply("ok")]);
            s.assistant.use_model(m.clone());
            let thread = s.execute("assistant.send", json!({"text": "x", "wait": true})).unwrap()["thread"].as_u64().unwrap();
            assert!(cards(&mut s, thread)["cards"].as_array().unwrap().is_empty(), "{input}");
            m.requests()[1].messages[2]
                .parts
                .iter()
                .find_map(|p| if let Part::ToolResult { content, is_error, .. } = p { Some((content.clone(), *is_error)) } else { None })
                .unwrap()
        };
        assert!(model_out.1 && model_out.0.contains(want), "{want:?} not in {:?} for {input}", model_out.0);
    }
    // a narration already there: only one is made this way
    let mut s = demo();
    s.execute("tts.create", json!({"text": "Hello there.", "voice": "basic-female", "seconds": 0})).unwrap();
    let m = scripted(vec![tool_reply(&[("n", "request_narration", json!({"text": SCRIPT, "voice": "basic-female"}))]), text_reply("ok")]);
    s.assistant.use_model(m.clone());
    s.execute("assistant.send", json!({"text": "x", "wait": true})).unwrap();
    let told =
        m.requests()[1].messages[2].parts.iter().find_map(|p| if let Part::ToolResult { content, .. } = p { Some(content.clone()) } else { None }).unwrap();
    assert!(told.contains("already a narration on the timeline"), "{told}");
}

#[test]
fn the_tool_is_offered_in_every_mode_and_cannot_make_anything_itself() {
    let t = crate::assistant_tools::tools().into_iter().find(|t| t.spec.name == "request_narration").expect("the tool");
    assert!(t.read_only);
    for id in ["tts.create", "tts.render", "tts.edit", "tts.preview"] {
        assert!(crate::assistant_edit::may_run(id).is_err(), "{id} must not be allowed to the assistant");
    }
}
