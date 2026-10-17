//! Tests of the assistant's analysis and coach tools: the text a model reads, the numbers in it
//! coming from the measurements, `analysis_run` waiting for its job inside a turn, and hostile input.

use edith_ai::scripted::ScriptedModel;
use edith_ai::{Message, Part, Response, Role, StopReason, StreamEvent, ToolOutput, Usage};
use serde_json::{Value, json};
use std::sync::Arc;

use crate::Session;
use crate::analysis_tests::{demo, edit_of, footage};

fn run(s: &Session, name: &str, input: Value) -> ToolOutput {
    s.run_assistant_tool(name, &input)
}

fn text_reply(t: &str) -> (Vec<StreamEvent>, Response) {
    (
        vec![StreamEvent::Text(t.into())],
        Response { message: Message::assistant_text(t), stop_reason: StopReason::EndTurn, usage: Usage::default(), model: "claude-haiku-5-5".into() },
    )
}

fn tool_reply(calls: &[(&str, &str, Value)]) -> (Vec<StreamEvent>, Response) {
    let parts =
        calls.iter().map(|(id, name, input)| Part::ToolUse { id: (*id).into(), name: (*name).into(), input: input.clone(), input_error: None }).collect();
    (
        vec![],
        Response {
            message: Message { role: Role::Assistant, parts },
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
            model: "claude-haiku-5-5".into(),
        },
    )
}

#[test]
fn the_four_tools_exist_and_leave_the_project_alone() {
    let tools = crate::assistant_tools::tools();
    for name in ["coach_findings", "analysis_get", "analysis_list", "analysis_run"] {
        let d = tools.iter().find(|d| d.spec.name == name).unwrap_or_else(|| panic!("no {name}"));
        assert!(d.read_only, "{name}");
        assert!(!d.spec.description.is_empty() && d.spec.input_schema["type"] == "object");
    }
    let mut s = demo();
    let rev = s.revision;
    let item = footage(&s);
    s.execute("analysis.run", json!({"item": item, "samples": 2, "wait": true})).unwrap();
    for (name, input) in [("coach_findings", json!({})), ("analysis_get", json!({"item": item})), ("analysis_list", json!({}))] {
        assert!(!run(&s, name, input).is_error, "{name}");
    }
    assert_eq!(s.revision, rev);
}

#[test]
fn the_coach_tool_hands_the_model_the_findings_with_their_ids_places_and_numbers() {
    let s = edit_of(&[6.0, 0.8, 6.0]);
    let r = run(&s, "coach_findings", json!({}));
    assert!(!r.is_error, "{}", r.content);
    let t = &r.content;
    assert!(t.contains("3 shots, 1 finding"), "{t}");
    assert!(t.contains("[look]") && t.contains("shortBetweenLong@") && t.contains("at 0:06"), "{t}");
    assert!(t.contains("0.8 s") && t.contains("jolt") && t.contains("idea: rhythm") && t.contains("before 6.00"), "{t}");
    assert!(t.contains("The mix has not been measured"), "{t}");
    assert!(t.contains("Targets: none"), "{t}");
    let ok = run(&edit_of(&[4.0, 5.0, 6.0, 5.0]), "coach_findings", json!({}));
    assert!(ok.content.contains("0 findings") && ok.content.contains("Nothing stands out"), "{}", ok.content);
    // no sequence: an error the model can read, not a panic
    let none = run(&Session::default(), "coach_findings", json!({}));
    assert!(none.is_error);
    for odd in [json!(null), json!([1]), json!("x"), json!(7), json!({"sequence": "x"}), json!({"sequence": -4}), json!({"sequence": 1e30})] {
        let _ = run(&s, "coach_findings", odd);
    }
}

#[test]
fn analysis_get_answers_from_the_timeline_at_once_and_adds_the_shots_once_they_are_measured() {
    let mut s = edit_of(&[3.0, 4.0, 5.0, 6.0]);
    let r = run(&s, "analysis_get", json!({}));
    assert!(!r.is_error, "{}", r.content);
    let t = &r.content;
    assert!(t.contains("4 shots") && t.contains("Shot length: mean 4.5 s") && t.contains("Cuts: 3"), "{t}");
    assert!(t.contains("Shot lengths by size:") && t.contains("2–4 s: 1"), "{t}");
    assert!(t.contains("Track use: V1 100%, A1 100%"), "{t}");
    assert!(t.contains("have not been looked at") && t.contains("analysis_run with sequence true"), "{t}");
    // measured: the same numbers and a look at each shot
    s.execute("analysis.run", json!({"sequence": true, "wait": true})).unwrap();
    let t = run(&s, "analysis_get", json!({"sequence": true})).content;
    assert!(t.contains("A look at 4 of 4 shots") && t.contains("brightness") && t.contains("clip #"), "{t}");
    assert!(!t.contains("have not been looked at"));
    // the mix, before and after
    assert!(run(&s, "analysis_get", json!({"mix": true})).content.contains("has not been measured"));
    s.execute("analysis.run", json!({"mix": true, "wait": true})).unwrap();
    let m = run(&s, "analysis_get", json!({"mix": true})).content;
    assert!(m.contains("Integrated loudness") && m.contains("LUFS") && m.contains("Silence: 0%"), "{m}");
}

#[test]
fn analysis_get_for_an_item_says_what_to_run_and_then_reports_it() {
    let mut s = demo();
    let item = footage(&s);
    let before = run(&s, "analysis_get", json!({"item": item})).content;
    assert!(before.contains("has not been measured") && before.contains(&format!("analysis_run with item {item}")), "{before}");
    s.execute("analysis.run", json!({"item": item, "samples": 4, "wait": true})).unwrap();
    let after = run(&s, "analysis_get", json!({"item": item})).content;
    assert!(after.contains("Pictures (4 measured") && after.contains("Camera:") && after.contains("Palette: #"), "{after}");
    assert!(run(&s, "analysis_list", json!({})).content.contains("current"));
    let empty = run(&Session::default(), "analysis_list", json!({}));
    assert_eq!(empty.content, "Nothing has been measured yet.");
}

#[test]
fn hostile_tool_input_gives_an_error_or_text_never_a_panic() {
    let s = demo();
    for name in ["analysis_get", "analysis_list", "coach_findings"] {
        for odd in [
            json!(null),
            json!([]),
            json!("x"),
            json!(7),
            json!({"item": "x"}),
            json!({"item": -1}),
            json!({"item": 1e30}),
            json!({"item": 999_999}),
            json!({"sequence": 5}),
            json!({"mix": "yes"}),
            json!({"item": null, "sequence": false, "mix": false}),
        ] {
            let _ = run(&s, name, odd);
        }
    }
    assert!(run(&s, "analysis_get", json!({"item": 999_999})).is_error);
    assert!(run(&s, "analysis_get", json!({"item": "x"})).is_error);
    // the sequence's own id is not footage
    let seq = s.state.active_sequence.unwrap().0;
    assert!(run(&s, "analysis_get", json!({"item": seq})).is_error);
}

#[test]
fn analysis_run_inside_a_turn_waits_for_its_job_and_hands_the_model_the_result() {
    let mut s = demo();
    let item = footage(&s);
    let model = Arc::new(ScriptedModel::new());
    let (e, r) = tool_reply(&[("t1", "analysis_run", json!({"item": item, "samples": 3}))]);
    model.push_reply(e, r);
    let (e, r) = text_reply("Your footage is bright.");
    model.push_reply(e, r);
    s.assistant.use_model(model.clone());
    let r = s.execute("assistant.send", json!({"text": "measure it", "wait": true})).unwrap();
    assert_eq!(r["finish"]["kind"], "done", "{r}");
    let second = &model.requests()[1];
    let Part::ToolResult { content, is_error, .. } = &second.messages[2].parts[0] else { panic!("no tool result in {:?}", second.messages[2]) };
    assert!(!*is_error && content.contains("Pictures (3 measured"), "{content}");
    assert!(s.assistant.pending_tools_len() == 0, "nothing left waiting");
    // already measured: answered at once without a second job
    let jobs = s.jobs.len();
    let model = Arc::new(ScriptedModel::new());
    let (e, r) = tool_reply(&[("t2", "analysis_run", json!({"item": item, "samples": 3}))]);
    model.push_reply(e, r);
    let (e, r) = text_reply("Same again.");
    model.push_reply(e, r);
    s.assistant.use_model(model.clone());
    s.execute("assistant.send", json!({"text": "again", "wait": true})).unwrap();
    let Part::ToolResult { content, .. } = &model.requests()[1].messages[2].parts[0] else { panic!() };
    assert!(content.contains("Pictures (3 measured"), "{content}");
    assert_eq!(s.jobs.iter().filter(|j| j.label.starts_with("Analyse")).count(), 1, "the second call was served from the cache");
    let _ = jobs;
}

#[test]
fn analysis_run_with_bad_input_answers_with_the_error_in_the_turn() {
    let mut s = demo();
    let model = Arc::new(ScriptedModel::new());
    let (e, r) = tool_reply(&[("t1", "analysis_run", json!({"item": 424_242})), ("t2", "analysis_run", json!("junk")), ("t3", "analysis_run", json!({}))]);
    model.push_reply(e, r);
    let (e, r) = text_reply("I could not.");
    model.push_reply(e, r);
    s.assistant.use_model(model.clone());
    let r = s.execute("assistant.send", json!({"text": "go", "wait": true})).unwrap();
    assert_eq!(r["finish"]["kind"], "done");
    let results = &model.requests()[1].messages[2].parts;
    assert_eq!(results.len(), 3);
    for p in results {
        let Part::ToolResult { is_error, .. } = p else { panic!("{p:?}") };
        assert!(*is_error);
    }
    assert!(s.jobs.iter().all(|j| !j.label.starts_with("Analyse")), "no job was started");
}

#[test]
fn the_mode_is_a_setting_that_only_accepts_ask_or_coach() {
    let mut s = Session::default();
    assert_eq!(s.execute("assistant.status", json!({})).unwrap()["settings"]["mode"], "ask", "Ask is the starting mode");
    let modes = s.execute("assistant.status", json!({})).unwrap()["modes"].clone();
    assert_eq!(modes, json!([{"id": "ask", "label": "Ask"}, {"id": "coach", "label": "Coach"}]));
    s.execute("assistant.settings", json!({"mode": "coach"})).unwrap();
    assert_eq!(s.execute("assistant.status", json!({})).unwrap()["settings"]["mode"], "coach");
    for bad in [json!("propose"), json!("Coach"), json!(""), json!(7), json!(null), json!(["coach"]), json!("coach; drop")] {
        assert!(s.execute("assistant.settings", json!({"mode": bad.clone()})).is_err(), "{bad}");
    }
    assert_eq!(s.execute("assistant.status", json!({})).unwrap()["settings"]["mode"], "coach", "a refused change changes nothing");
}

#[test]
fn a_thread_follows_the_mode_chosen_for_each_message_and_coach_gets_its_prompt_and_starting_numbers() {
    let mut s = edit_of(&[6.0, 0.8, 6.0]);
    let model = Arc::new(ScriptedModel::new());
    for t in ["Ask answer.", "Coach answer.", "Ask again."] {
        let (e, r) = text_reply(t);
        model.push_reply(e, r);
    }
    s.assistant.use_model(model.clone());
    let r = s.execute("assistant.send", json!({"text": "how is my pacing?", "wait": true})).unwrap();
    let thread = r["thread"].as_u64().unwrap();
    s.execute("assistant.settings", json!({"mode": "coach"})).unwrap();
    s.execute("assistant.send", json!({"text": "coach me", "thread": thread, "wait": true})).unwrap();
    s.execute("assistant.settings", json!({"mode": "ask"})).unwrap();
    s.execute("assistant.send", json!({"text": "and now?", "thread": thread, "wait": true})).unwrap();
    let reqs = model.requests();
    let system = |i: usize| reqs[i].system.iter().map(|p| p.text.clone()).collect::<String>();
    let user_text = |i: usize| reqs[i].messages.iter().rev().find(|m| m.role == Role::User).map(|m| m.text()).unwrap_or_default();
    // Ask: the shared prompt, no coach section, no starting numbers
    assert!(!system(0).contains("## coach mode") && !user_text(0).contains("[coach]"), "{}", user_text(0));
    // Coach: its section, and the edit's numbers and findings ahead of the question
    assert!(system(1).contains("## coach mode") && system(1).contains("Coach mode and read-only"));
    let u = user_text(1);
    assert!(
        u.contains("[coach]")
            && u.contains("3 shots")
            && u.contains("shortBetweenLong@")
            && u.contains("The mix has not been measured")
            && u.contains("coach me"),
        "{u}"
    );
    // the thread goes back to Ask on the next message
    assert!(!system(2).contains("## coach mode") && !user_text(2).contains("[coach]"));
    // the same tools in both: Coach is not given an editing tool
    let names = |i: usize| reqs[i].tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(0), names(1));
    assert!(names(1).contains(&"coach_findings".to_string()) && names(1).contains(&"analysis_get".to_string()));
}

#[test]
fn coach_mode_with_nothing_open_asks_without_a_starting_block() {
    let mut s = Session::default();
    s.execute("assistant.settings", json!({"mode": "coach"})).unwrap();
    let model = Arc::new(ScriptedModel::new());
    let (e, r) = text_reply("Open something first.");
    model.push_reply(e, r);
    s.assistant.use_model(model.clone());
    let r = s.execute("assistant.send", json!({"text": "help", "wait": true})).unwrap();
    assert_eq!(r["finish"]["kind"], "done", "{r}");
    let u = model.requests()[0].messages[0].text();
    assert!(!u.contains("[coach]") && u.contains("help"), "{u}");
}
