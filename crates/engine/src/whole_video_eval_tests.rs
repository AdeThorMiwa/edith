//! Tests of the `whole-video` evaluation suite (fork roadmap F32): the fixture is what it says, every task can be
//! passed by an assistant that does the right thing and failed by one that does not, and the graders that read the
//! finished edit agree with the checks. The model is scripted, so nothing here costs money.

use edith_ai::eval::{GradeContext, Suite, grade};
use edith_ai::{AgentEvent, Finish, Profile, Trace, TraceMeta};
use filmcraft_project::ItemKind;
use serde_json::{Value, json};

use super::assistant_tests::{scripted, text_reply, tool_reply};
use crate::Session;

fn fixture(name: &str) -> Session {
    super::assistant::eval_fixture_for_tests(name)
}

fn item(s: &Session, name: &str) -> u64 {
    s.project.items.values().find(|i| i.name.contains(name) && matches!(i.kind, ItemKind::Media(_))).map(|i| i.id.0).expect(name)
}

fn run_task(id: &str, model: std::sync::Arc<edith_ai::scripted::ScriptedModel>) -> Value {
    let mut s = Session::default();
    s.assistant.use_model(model);
    let r = s.execute("assistant.eval", json!({"suite": "whole-video", "tasks": [id], "wait": true, "model": "scripted-1"})).unwrap();
    r["results"][0].clone()
}

fn failed_checks(r: &Value) -> Vec<String> {
    r["grades"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|g| g["passed"] == false)
        .map(|g| format!("{}: {}", g["check"].as_str().unwrap_or(""), g["detail"].as_str().unwrap_or("")))
        .collect()
}

/// How long the fixture's film is: where the narration's last word ends.
fn film_seconds() -> f64 {
    let s = fixture("demo-cut");
    s.project.narrations.values().next().unwrap().words.iter().map(|w| w.end.seconds()).fold(0.0f64, f64::max)
}

fn edit_call(command: &str, params: Value) -> Value {
    json!({"command": command, "params": params})
}

#[test]
fn the_suite_ships_and_every_task_has_a_fixture_that_exists() {
    let suite = Suite::named("whole-video").expect("the whole-video suite ships with the app");
    assert_eq!(suite.tasks.len(), 5);
    for t in &suite.tasks {
        assert!(super::assistant::eval::FIXTURES.contains(&t.fixture.as_str()), "{}: fixture {}", t.id, t.fixture);
        let _ = fixture(&t.fixture);
    }
}

#[test]
fn the_cut_fixture_is_a_narration_cut_on_sentence_ends_with_a_score_and_a_bed_to_use() {
    let s = fixture("demo-cut");
    let q = s.active_sequence().unwrap();
    assert_eq!(q.video_tracks[0].items.len(), 3, "a shot to each sentence");
    assert_eq!(q.audio_tracks.iter().map(|t| t.items.len()).sum::<usize>(), 1, "the narration and nothing else");
    for name in ["Harbour score", "Harbour bed"] {
        let it = s.project.items.values().find(|i| i.name.contains(name)).unwrap_or_else(|| panic!("{name}"));
        assert!(crate::provenance::get(&s.project, it.id).is_some(), "{name}'s origin is recorded");
    }
    // nothing about the cut itself fails; what it lacks is sound, a title and polish, which are should-checks
    let r = crate::edit_checks::run_checks(&s, &json!({})).unwrap();
    assert_eq!(r["mustPass"]["done"], true, "{r}");
    let status = |id: &str| r["checks"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap()["status"].as_str().unwrap().to_string();
    assert_eq!((status("soundUnderIdeas").as_str(), status("openingTitle").as_str()), ("note", "note"));
    assert_eq!(status("transitionRestraint"), "pass");
}

/// A run that called these tools in this order and said `text`, graded with no measurements.
fn trace_of(tools: &[&str], text: &str) -> Trace {
    let mut t = Trace::new(TraceMeta { prompt_version: "18".into(), model: "m".into(), profile: Profile::ReadOnly, started_unix: 0 });
    t.record(&AgentEvent::UserMessage { text: "q".into(), attached: vec![] });
    t.record(&AgentEvent::StepStarted { step: 1 });
    for (i, name) in tools.iter().enumerate() {
        t.record(&AgentEvent::ToolCall { id: format!("t{i}"), name: (*name).into(), input: json!({}) });
    }
    t.record(&AgentEvent::Text { text: text.into() });
    t.record(&AgentEvent::Finished { finish: Finish::Done });
    t
}

fn graded(task: &str, trace: &Trace) -> Vec<String> {
    let suite = Suite::named("whole-video").unwrap();
    let t = suite.task(task).unwrap();
    let cx = GradeContext { read_only: &|_| Some(true), price: None, project_unchanged: Some(true), must_fail: Some(0), checks: None, seconds: 0.0 };
    grade(t, trace, &cx).grades.iter().filter(|g| !g.passed).map(|g| g.check.clone()).collect()
}

#[test]
fn a_clip_is_looked_at_before_it_is_offered_and_a_sound_is_measured_before_it_is_offered() {
    // looking first, then offering: nothing fails
    let good = trace_of(&["media_search", "media_look", "request_download"], "I looked at three results and offered the one with the lowest fog.");
    assert!(graded("looks_before_it_offers_a_clip", &good).is_empty(), "{:?}", graded("looks_before_it_offers_a_clip", &good));
    // offering without looking, or looking after, fails the order
    for tools in [&["media_search", "request_download"][..], &["media_search", "request_download", "media_look"]] {
        let bad = trace_of(tools, "I offered one.");
        assert!(graded("looks_before_it_offers_a_clip", &bad).iter().any(|c| c == "calls_tools" || c == "calls_in_order"), "{tools:?}");
    }
    // claiming a download is graded on the words
    let claims = trace_of(&["media_search", "media_look", "request_download"], "I downloaded it and added it to the project.");
    assert_eq!(graded("looks_before_it_offers_a_clip", &claims), ["avoids"]);
    // the same for sound, and she may not say she heard it
    let good = trace_of(&["media_search", "media_listen", "request_download"], "Measured, not heard: it is a steady, quiet piece around 90 BPM.");
    assert!(graded("listens_before_it_offers_music", &good).is_empty());
    let heard = trace_of(&["media_search", "media_listen", "request_download"], "I listened to it and it sounds beautiful.");
    assert_eq!(graded("listens_before_it_offers_music", &heard), ["avoids"]);
    let unmeasured = trace_of(&["media_search", "request_download"], "It is calm.");
    assert!(!graded("listens_before_it_offers_music", &unmeasured).is_empty());
}

#[test]
fn a_score_and_a_bed_are_placed_and_mixed_in_a_version_and_the_checks_agree() {
    let s = fixture("demo-cut");
    let (score, bed) = (item(&s, "Harbour score"), item(&s, "Harbour bed"));
    let end = film_seconds();
    let place = edit_call(
        "timeline.placeSoundList",
        json!({"entries": [
            {"start": 0, "end": end, "item": score, "lane": "music", "reason": "a slow score under the whole film"},
            {"start": 0, "end": end / 2.0, "item": bed, "lane": "ambience", "reason": "the harbour, for the first half"}
        ]}),
    );
    let good = scripted(vec![
        tool_reply(&[("a", "begin_version", json!({"title": "Sound"}))]),
        tool_reply(&[("b", "project_items", json!({}))]),
        tool_reply(&[("c", "edit", place.clone())]),
        tool_reply(&[("d", "edit", edit_call("sound.autoMix", json!({})))]),
        tool_reply(&[("e", "edit_checks", json!({}))]),
        text_reply(
            "I put the score under the whole film and the harbour bed under the first half, then mixed it: the voice leads and both dip under it. Every must-pass check passes; you decide whether to commit.",
        ),
    ]);
    let r = run_task("places_and_mixes_the_sound", good);
    assert_eq!(r["passed"], true, "{:?} {}", failed_checks(&r), r["answer"]);
    // placing without mixing leaves the music at full level under the voice
    let unmixed = scripted(vec![
        tool_reply(&[("a", "begin_version", json!({"title": "Sound"}))]),
        tool_reply(&[("b", "project_items", json!({}))]),
        tool_reply(&[("c", "edit", place)]),
        tool_reply(&[("e", "edit_checks", json!({}))]),
        text_reply("Placed."),
    ]);
    let r = run_task("places_and_mixes_the_sound", unmixed);
    assert_eq!(r["passed"], false);
    assert!(failed_checks(&r).iter().any(|c| c.starts_with("checks_pass") && c.contains("musicDucked")), "{:?}", failed_checks(&r));
    // doing nothing leaves a voice with nothing under it
    let nothing = scripted(vec![
        tool_reply(&[("a", "begin_version", json!({"title": "Sound"}))]),
        tool_reply(&[("b", "project_items", json!({}))]),
        tool_reply(&[("e", "edit_checks", json!({}))]),
        text_reply("It sounds fine."),
    ]);
    let r = run_task("places_and_mixes_the_sound", nothing);
    assert_eq!(r["passed"], false);
    assert!(failed_checks(&r).iter().any(|c| c.starts_with("checks_pass") && c.contains("soundUnderIdeas")), "{:?}", failed_checks(&r));
}

#[test]
fn an_opening_title_that_can_be_read_passes_and_none_fails() {
    let title =
        |start: f64, end: f64| edit_call("graphics.placeTextList", json!({"entries": [{"start": start, "end": end, "text": "The Harbour", "role": "title"}]}));
    let script = |t: Option<Value>| {
        let mut replies = vec![tool_reply(&[("a", "begin_version", json!({"title": "Titles"}))])];
        if let Some(t) = t {
            replies.push(tool_reply(&[("b", "edit", t)]));
        }
        replies.push(tool_reply(&[("c", "edit_checks", json!({}))]));
        replies.push(text_reply("Done in the version; you decide whether to commit."));
        scripted(replies)
    };
    let r = run_task("gives_titles_that_can_be_read", script(Some(title(0.5, 4.5))));
    assert_eq!(r["passed"], true, "{:?}", failed_checks(&r));
    let r = run_task("gives_titles_that_can_be_read", script(None));
    assert_eq!(r["passed"], false);
    assert!(failed_checks(&r).iter().any(|c| c.contains("openingTitle")), "{:?}", failed_checks(&r));
}

#[test]
fn a_polish_with_fades_and_hard_cuts_passes_and_a_dissolve_on_every_cut_fails() {
    let s = fixture("demo-cut");
    let q = s.active_sequence().unwrap();
    let shots: Vec<(f64, f64, u64)> =
        q.video_tracks[0].items.iter().map(|i| (i.start.seconds(), i.end().seconds(), s.project.item(i.item).map(|p| p.id.0).unwrap())).collect();
    let good = scripted(vec![
        tool_reply(&[("a", "begin_version", json!({"title": "Polish"}))]),
        tool_reply(&[("b", "edit", edit_call("edit.polish", json!({})))]),
        tool_reply(&[("c", "edit_checks", json!({}))]),
        text_reply("I added a fade in and a fade out and kept both cuts hard: nothing in this film changes time or place there."),
    ]);
    let r = run_task("polishes_with_restraint", good);
    assert_eq!(r["passed"], true, "{:?} {}", failed_checks(&r), r["answer"]);
    // a dissolve on every cut: the film becomes a slideshow of two
    let entries: Vec<Value> = shots
        .iter()
        .enumerate()
        .map(|(k, (a, b, item))| {
            let mut e = json!({"start": a, "end": b, "item": item, "sourceIn": 3});
            if k > 0 {
                e["transition"] = json!({"kind": "dissolve", "seconds": 0.5});
            }
            e
        })
        .collect();
    let busy = scripted(vec![
        tool_reply(&[("a", "begin_version", json!({"title": "Polish"}))]),
        tool_reply(&[("b", "edit", edit_call("timeline.placeCutList", json!({"entries": entries})))]),
        tool_reply(&[("c", "edit_checks", json!({}))]),
        text_reply("I added a fade to every cut, and kept the hard cut out of it."),
    ]);
    let r = run_task("polishes_with_restraint", busy);
    assert_eq!(r["passed"], false, "{}", r["answer"]);
    assert!(failed_checks(&r).iter().any(|c| c.contains("transitionRestraint")), "{:?}", failed_checks(&r));
}
