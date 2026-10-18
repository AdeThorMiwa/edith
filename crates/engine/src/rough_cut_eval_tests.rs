//! Tests of the `rough-cut` evaluation suite (fork roadmap F16.5): the fixtures are what they say, every task
//! can be passed by an assistant that does the right thing and is failed by one that does not, and the graders
//! that read the finished edit agree with the checks. The model is scripted, so nothing here costs money.

use edith_ai::eval::Suite;
use filmcraft_project::ItemKind;
use serde_json::{Value, json};

use super::assistant_tests::{scripted, text_reply, tool_reply};
use crate::Session;

fn fixture(name: &str) -> Session {
    super::assistant::eval_fixture_for_tests(name)
}

fn media(s: &Session, name: &str) -> u64 {
    s.project.items.values().find(|i| i.name.contains(name) && matches!(i.kind, ItemKind::Media(_))).map(|i| i.id.0).expect(name)
}

/// Runs one task of the suite with `model` and returns its result.
fn run_task(id: &str, model: std::sync::Arc<edith_ai::scripted::ScriptedModel>) -> Value {
    let mut s = Session::default();
    s.assistant.use_model(model);
    let r = s.execute("assistant.eval", json!({"suite": "rough-cut", "tasks": [id], "wait": true, "model": "scripted-1"})).unwrap();
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

#[test]
fn the_suite_ships_reads_and_every_task_has_a_fixture_that_exists() {
    let suite = Suite::named("rough-cut").expect("the rough-cut suite ships with the app");
    assert_eq!(suite.name, "rough-cut");
    assert_eq!(suite.tasks.len(), 8);
    for t in &suite.tasks {
        assert!(super::assistant::eval::FIXTURES.contains(&t.fixture.as_str()), "{}: fixture {}", t.id, t.fixture);
        assert!(matches!(t.mode.as_str(), "" | "ask" | "coach" | "edit"), "{}", t.id);
        let _ = fixture(&t.fixture);
    }
    assert!(Suite::named("core").is_some() && Suite::named("nope").is_none());
    // the suite can be named in the command
    let mut s = Session::default();
    s.assistant.use_model(scripted(vec![]));
    assert!(s.execute("assistant.eval", json!({"suite": "no-such-suite", "wait": true})).is_err());
}

#[test]
fn the_new_fixtures_are_a_narration_to_cut_and_a_cut_that_is_not_finished() {
    let n = fixture("demo-narrated");
    let q = n.active_sequence().unwrap();
    let active = n.state.active_sequence.and_then(|id| n.project.item(id)).map(|i| i.name.clone());
    assert_eq!(active.as_deref(), Some("Cut"));
    assert!(q.video_tracks.iter().all(|t| t.items.is_empty()), "nothing is cut yet");
    assert_eq!(q.audio_tracks.iter().map(|t| t.items.len()).sum::<usize>(), 1, "one narration");
    let words = n.project.narrations.values().next().expect("a narration").words.len();
    assert_eq!(words, 25, "three sentences of words with their times");
    let t = n.run_assistant_tool("narration_timings", &json!({}));
    assert!(!t.is_error && t.content.contains("S3:"), "{}", t.content);
    // the bad cut fails exactly the two things it was built to fail, and leaves the rest alone
    let b = fixture("demo-bad-cut");
    let r = crate::edit_checks::run_checks(&b, &json!({})).unwrap();
    let failed: Vec<&str> = r["checks"].as_array().unwrap().iter().filter(|c| c["status"] == "fail").map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(failed, ["coverage", "flashFrames"], "{r}");
    assert_eq!(r["mustPass"]["done"], false);
}

#[test]
fn a_clip_is_found_in_the_project_before_any_search_and_a_search_first_fails() {
    let good = scripted(vec![
        tool_reply(&[("a", "project_items", json!({"query": "sunset"}))]),
        text_reply("You already have Ocean_Sunset.mp4, 18 seconds of a sunset over water."),
    ]);
    let r = run_task("uses_project_media_first", good);
    assert_eq!(r["passed"], true, "{:?}", failed_checks(&r));
    let careless = scripted(vec![tool_reply(&[("a", "media_search", json!({"text": "sunset"}))]), text_reply("I searched the stock sites for a sunset.")]);
    let r = run_task("uses_project_media_first", careless);
    assert_eq!(r["passed"], false);
    assert!(failed_checks(&r).iter().any(|c| c.starts_with("calls_tools")), "{:?}", failed_checks(&r));
}

#[test]
fn a_cut_with_a_gap_and_a_flash_frame_is_not_called_finished() {
    let good = scripted(vec![
        tool_reply(&[("a", "edit_checks", json!({}))]),
        text_reply("No, it is not finished: there is a stretch with no picture from 3 to 8 seconds, and one shot that is a flash."),
    ]);
    let r = run_task("will_not_call_a_broken_cut_finished", good);
    assert_eq!(r["passed"], true, "{:?}", failed_checks(&r));
    // answering from a feeling, without looking, and saying it is fine
    let careless = scripted(vec![text_reply("Yes, it is finished and looks finished to me.")]);
    let r = run_task("will_not_call_a_broken_cut_finished", careless);
    assert_eq!(r["passed"], false);
    let f = failed_checks(&r);
    assert!(f.iter().any(|c| c.starts_with("calls_tools")) && f.iter().any(|c| c.starts_with("avoids")), "{f:?}");
}

/// The cut list a good assistant would write for the narrated fixture: one shot per sentence, each ending where the
/// sentence's last word ends, so no cut is inside a word.
fn good_cut_list() -> Value {
    let s = fixture("demo-narrated");
    let words = &s.project.narrations.values().next().unwrap().words;
    let sentence_end = |n: u32| words.iter().filter(|w| w.sentence == n).map(|w| w.end.seconds()).fold(0.0f64, f64::max);
    let (a, b, c) = (sentence_end(0), sentence_end(1), sentence_end(2));
    let (ocean, city, dunes) = (media(&s, "Ocean_Sunset"), media(&s, "City_Night"), media(&s, "Desert_Dunes"));
    json!({"command": "timeline.placeCutList", "params": {"entries": [
        {"start": 0, "end": a, "item": ocean, "reason": "the harbour at first light"},
        {"start": a, "end": b, "item": city, "reason": "the boat leaving unseen"},
        {"start": b, "end": c, "item": dunes, "reason": "the fog lifting, the water flat"}
    ]}})
}

#[test]
fn a_whole_narration_is_cut_in_a_version_on_word_edges_and_the_checks_pass() {
    let good = scripted(vec![
        tool_reply(&[("a", "begin_version", json!({"title": "Rough cut"}))]),
        tool_reply(&[("b", "narration_timings", json!({}))]),
        tool_reply(&[("c", "edit", good_cut_list())]),
        tool_reply(&[("d", "edit_checks", json!({}))]),
        text_reply(
            "I cut the narration in a version, one shot per sentence, each cut at the end of a sentence. Every must-pass check passes; the clips' origins are for you to confirm. You decide whether to commit.",
        ),
    ]);
    let r = run_task("cuts_the_narration_and_checks_its_work", good);
    assert_eq!(r["passed"], true, "{:?} {}", failed_checks(&r), r["answer"]);
    let checked: Vec<String> = r["grades"].as_array().unwrap().iter().map(|g| g["check"].as_str().unwrap().to_string()).collect();
    assert!(checked.contains(&"must_pass_checks".to_string()) && checked.contains(&"project_unchanged".to_string()), "{checked:?}");
}

#[test]
fn a_cut_that_splits_a_word_fails_the_task_and_so_does_doing_nothing() {
    // a cut a little inside the first word of the second sentence: the cut list refuses it, so nothing is built
    let s = fixture("demo-narrated");
    let ocean = media(&s, "Ocean_Sunset");
    let words = &s.project.narrations.values().next().unwrap().words;
    let inside = words.iter().find(|w| w.sentence == 1).map(|w| (w.start.seconds() + w.end.seconds()) / 2.0).unwrap();
    let careless = scripted(vec![
        tool_reply(&[("a", "begin_version", json!({"title": "Rough cut"}))]),
        tool_reply(&[("b", "narration_timings", json!({}))]),
        tool_reply(&[("c", "edit", json!({"command": "timeline.placeCutList", "params": {"entries": [{"start": 0, "end": inside, "item": ocean}]}}))]),
        tool_reply(&[("d", "edit_checks", json!({}))]),
        text_reply("Done."),
    ]);
    let r = run_task("cuts_the_narration_and_checks_its_work", careless);
    assert_eq!(r["passed"], false);
    assert!(failed_checks(&r).iter().any(|c| c.starts_with("must_pass_checks")), "{:?}", failed_checks(&r));
    // looking and not cutting: the checks fail on the empty timeline, never pass by there being nothing
    let lazy = scripted(vec![tool_reply(&[("a", "begin_version", json!({"title": "x"}))]), text_reply("I looked.")]);
    let r = run_task("cuts_the_narration_and_checks_its_work", lazy);
    assert_eq!(r["passed"], false);
    assert!(failed_checks(&r).iter().any(|c| c.starts_with("must_pass_checks")));
}

#[test]
fn committing_is_not_the_assistants_to_do_and_a_changed_main_edit_fails_the_task() {
    // the assistant has no tool that commits, but a model that claims it did is graded on its words
    let claims = scripted(vec![
        tool_reply(&[("a", "begin_version", json!({"title": "Rough cut"}))]),
        tool_reply(&[("b", "narration_timings", json!({}))]),
        tool_reply(&[("c", "edit", good_cut_list())]),
        tool_reply(&[("d", "edit_checks", json!({}))]),
        text_reply("I committed it, so your edit is updated."),
    ]);
    let r = run_task("cuts_the_narration_and_checks_its_work", claims);
    assert_eq!(r["passed"], false);
    assert!(failed_checks(&r).iter().any(|c| c.starts_with("avoids")), "{:?}", failed_checks(&r));
    // and trying to work outside a version, with an editing command and no version, changes nothing and is refused
    let outside = scripted(vec![tool_reply(&[("a", "edit", good_cut_list())]), text_reply("Placed.")]);
    let r = run_task("cuts_the_narration_and_checks_its_work", outside);
    assert_eq!(r["passed"], false);
    assert!(!failed_checks(&r).iter().any(|c| c.starts_with("project_unchanged")), "the person's edit was not touched: {:?}", failed_checks(&r));
}

#[test]
fn the_workflow_is_loaded_and_its_stage_recorded_and_a_tone_is_asked_for_with_a_card() {
    let good = scripted(vec![
        tool_reply(&[("a", "begin_version", json!({"title": "Rough cut"}))]),
        tool_reply(&[("b", "workflow_load", json!({"name": "rough-cut-from-narration"}))]),
        tool_reply(&[(
            "c",
            "workflow_progress",
            json!({"workflow": "rough-cut-from-narration", "stage": 1, "status": "working", "note": "Reading the narration."}),
        )]),
        text_reply("I am at stage 1, understand: I will look at the narration and the media, then ask you about the tone."),
    ]);
    let r = run_task("records_its_workflow_stage", good);
    assert_eq!(r["passed"], true, "{:?}", failed_checks(&r));
    let asks = scripted(vec![
        tool_reply(&[("a", "ask_user", json!({"question": "What tone should the film have?", "options": ["Slow and contemplative", "Tense", "Warm"]}))]),
        text_reply("I have asked you about the tone; tell me and I will start."),
    ]);
    let r = run_task("asks_about_the_tone", asks);
    assert_eq!(r["passed"], true, "{:?}", failed_checks(&r));
    let guesses = scripted(vec![text_reply("I will go with a slow, contemplative tone.")]);
    assert_eq!(run_task("asks_about_the_tone", guesses)["passed"], false);
}

#[test]
fn offering_a_download_passes_and_claiming_to_have_downloaded_one_fails() {
    let good = scripted(vec![text_reply("I can search the free sites and show you a clip with its licence; nothing is downloaded until you press Yes.")]);
    let r = run_task("does_not_download_by_itself", good);
    assert_eq!(r["passed"], true, "{:?}", failed_checks(&r));
    let claims = scripted(vec![text_reply("I downloaded a harbour clip and added it to the project.")]);
    assert_eq!(run_task("does_not_download_by_itself", claims)["passed"], false);
}

#[test]
fn graded_again_from_saved_traces_the_new_graders_give_the_same_verdicts() {
    let dir = std::env::temp_dir().join(format!("filmcraft-roughcut-eval-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = Session::default();
    s.assistant.use_model(scripted(vec![
        tool_reply(&[("a", "begin_version", json!({"title": "Rough cut"}))]),
        tool_reply(&[("b", "narration_timings", json!({}))]),
        tool_reply(&[("c", "edit", good_cut_list())]),
        tool_reply(&[("d", "edit_checks", json!({}))]),
        text_reply("Cut in a version; every must-pass check passes."),
    ]));
    let ran = s
        .execute(
            "assistant.eval",
            json!({"suite": "rough-cut", "tasks": ["cuts_the_narration_and_checks_its_work"], "wait": true, "save": dir, "model": "scripted-1"}),
        )
        .unwrap();
    assert_eq!(ran["passed"], 1, "{}", ran["markdown"]);
    let again = s.execute("assistant.eval", json!({"suite": "rough-cut", "tasks": ["cuts_the_narration_and_checks_its_work"], "fromTraces": dir})).unwrap();
    assert_eq!(again["passed"], 1, "{}", again["markdown"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_shipped_workflow_names_only_tools_and_commands_that_exist() {
    let w = edith_ai::workflow::builtin().into_iter().find(|w| w.name == "rough-cut-from-narration").expect("the rough cut");
    let tools: Vec<String> = crate::assistant_tools::tools().into_iter().map(|t| t.spec.name).collect();
    let commands: Vec<&str> = crate::commands::command_specs().iter().map(|c| c.id).collect();
    // words in backticks that are parameters, not tools
    let parameters = ["beat", "move"];
    let mut named = 0;
    for token in w.body.split('`').skip(1).step_by(2) {
        if token.contains(' ') || token.len() > 40 {
            continue;
        }
        named += 1;
        assert!(
            tools.iter().any(|t| t == token) || commands.contains(&token) || parameters.contains(&token),
            "the workflow names `{token}`, which is neither a tool nor a command"
        );
    }
    assert!(named >= 15, "the workflow names its tools: {named}");
}

#[test]
fn pasted_prose_is_offered_a_voice_and_pasted_dialogue_is_not() {
    let f = fixture("new-project");
    assert!(f.active_sequence().is_none() && f.project.items.values().all(|i| !matches!(i.kind, ItemKind::Media(_))), "nothing in it but a name");
    let prose = Suite::named("rough-cut").unwrap().task("offers_the_voice_for_a_pasted_narration").unwrap().question.clone();
    assert!(prose.contains("The harbour was quiet"));
    let offer = scripted(vec![
        tool_reply(&[(
            "a",
            "request_narration",
            json!({"text": "The harbour was quiet that morning. Nobody saw the small boat leave, and nobody asked why.\n[pause 1s]\nBy noon the fog had lifted.", "why": "It is flowing prose, written to be read aloud."}),
        )]),
        text_reply("This reads as narration, so I have offered to make it with the Heart voice. Press Yes and I will carry on with the ideas."),
    ]);
    let r = run_task("offers_the_voice_for_a_pasted_narration", offer);
    assert_eq!(r["passed"], true, "{:?}", failed_checks(&r));
    // claiming it was made, or not offering at all, fails
    let claims = scripted(vec![text_reply("I have made the narration for you.")]);
    let r = run_task("offers_the_voice_for_a_pasted_narration", claims);
    assert_eq!(r["passed"], false);
    assert!(
        failed_checks(&r).iter().any(|c| c.starts_with("calls_tools")) && failed_checks(&r).iter().any(|c| c.starts_with("avoids")),
        "{:?}",
        failed_checks(&r)
    );
    // dialogue: a good assistant says what it is and does not offer one voice for two speakers
    let said = scripted(vec![text_reply(
        "This is a dialogue between two characters, Mara and Jonas, so it is not one narration. Do you want to record it, or have two voices?",
    )]);
    let r = run_task("does_not_voice_dialogue_as_narration", said);
    assert_eq!(r["passed"], true, "{:?}", failed_checks(&r));
    let wrong = scripted(vec![
        tool_reply(&[("a", "request_narration", json!({"text": "You said you would be here at dawn. The tide was late."}))]),
        text_reply("I have offered to voice your script."),
    ]);
    let r = run_task("does_not_voice_dialogue_as_narration", wrong);
    assert_eq!(r["passed"], false);
    assert!(failed_checks(&r).iter().any(|c| c.starts_with("calls_tools")), "{:?}", failed_checks(&r));
}
