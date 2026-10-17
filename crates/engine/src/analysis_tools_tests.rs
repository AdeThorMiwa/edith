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

#[test]
fn show_hands_the_person_a_view_of_the_real_numbers_and_the_model_only_a_note() {
    let s = edit_of(&[6.0, 0.8, 6.0]);
    let tools = crate::assistant_tools::tools();
    let d = tools.iter().find(|d| d.spec.name == "show").expect("a show tool");
    assert!(d.read_only);
    assert_eq!(
        d.spec.input_schema["properties"]["view"]["enum"],
        json!(["findings", "timeline", "histogram", "pace", "gauge", "palette", "pie", "compare", "rhythm", "look"])
    );

    // findings: a card of what the rules found, by id
    let r = run(&s, "show", json!({"view": "findings", "caption": "What I noticed"}));
    assert!(!r.is_error, "{}", r.content);
    let v = r.view.clone().expect("a view");
    assert_eq!((v["view"].as_str(), v["caption"].as_str(), v["total"].as_u64()), (Some("findings"), Some("What I noticed"), Some(1)));
    let row = &v["findings"][0];
    assert_eq!(
        (row["kind"].as_str(), row["severity"].as_str(), row["start"].as_i64()),
        (Some("shortBetweenLong"), Some("look"), Some(6 * filmcraft_time::TICKS_PER_SECOND))
    );
    assert!(row["title"].as_str().unwrap().contains("0.8 s"));
    // what the model reads is a note, not the data
    assert!(
        r.content.starts_with("Shown to the person: a card with 1 of 1 findings") && !r.content.contains("startTicks") && !r.content.contains("\"findings\""),
        "{}",
        r.content
    );
    let id = row["id"].as_str().unwrap().to_string();
    let by_id = run(&s, "show", json!({"view": "findings", "ids": [id]}));
    assert_eq!(by_id.view.unwrap()["findings"].as_array().unwrap().len(), 1);
    // a wrong id says which ids there are
    let bad = run(&s, "show", json!({"view": "findings", "ids": ["nope@1"]}));
    assert!(bad.is_error && bad.content.contains("shortBetweenLong@"), "{}", bad.content);
    // nothing found: no card, and a note to say so
    let none = run(&edit_of(&[4.0, 5.0, 6.0, 5.0]), "show", json!({"view": "findings"}));
    assert!(!none.is_error && none.view.is_none() && none.content.contains("no rule fired"), "{}", none.content);
}

#[test]
fn the_timeline_view_carries_the_shots_the_marks_and_the_range() {
    let s = edit_of(&[6.0, 0.8, 6.0, 5.0]);
    let r = run(&s, "show", json!({"view": "timeline"}));
    assert!(!r.is_error, "{}", r.content);
    let v = r.view.unwrap();
    assert_eq!(v["shots"].as_array().unwrap().len(), 4);
    assert_eq!((v["from"].as_i64(), v["truncated"].clone()), (Some(0), json!(false)));
    let t = filmcraft_time::TICKS_PER_SECOND;
    assert!(v["to"].as_i64().unwrap() > 17 * t && v["to"].as_i64().unwrap() < 18 * t);
    let marks = v["marks"].as_array().unwrap();
    assert_eq!(marks.len(), 1);
    assert_eq!((marks[0]["severity"].as_str(), marks[0]["start"].as_i64()), (Some("look"), Some(6 * t)));
    // shots are [start, end, clip] and end to end
    let shots = v["shots"].as_array().unwrap();
    assert!(shots.windows(2).all(|w| w[0][1] == w[1][0]) && shots.iter().all(|x| x[2].is_u64()));
    // a stretch: only the shots that overlap it, and only the marks inside it
    let part = run(&s, "show", json!({"view": "timeline", "from": 6.5, "to": 12})).view.unwrap();
    assert_eq!(part["shots"].as_array().unwrap().len(), 2, "the 0.8 s shot ends at 6.8 and the next runs to 12.8");
    assert!(part["marks"].as_array().unwrap().is_empty(), "the finding is at 6 s, before the stretch");
    // highlight: named findings only
    let id = marks[0]["id"].as_str().unwrap().to_string();
    assert_eq!(run(&s, "show", json!({"view": "timeline", "highlight": [id]})).view.unwrap()["marks"].as_array().unwrap().len(), 1);
    assert_eq!(run(&s, "show", json!({"view": "timeline", "highlight": []})).view.unwrap()["marks"].as_array().unwrap().len(), 0);
    // timecodes work too, and a backwards stretch is refused
    assert!(!run(&s, "show", json!({"view": "timeline", "from": "00:00:02:00", "to": "00:00:09:00"})).is_error);
    assert!(run(&s, "show", json!({"view": "timeline", "from": 9, "to": 3})).is_error);
    assert!(run(&s, "show", json!({"view": "timeline", "from": "soon"})).is_error);
    // no open edit
    assert!(run(&Session::default(), "show", json!({"view": "timeline"})).is_error);
}

#[test]
fn a_very_long_edit_is_cut_to_what_one_view_can_draw() {
    let s = edit_of(&vec![1.0; 1300]);
    let v = run(&s, "show", json!({"view": "timeline"})).view.unwrap();
    assert_eq!(v["shots"].as_array().unwrap().len(), 1200);
    assert_eq!(v["truncated"], true);
}

#[test]
fn the_histogram_view_counts_the_shots_by_length_with_the_plans_target() {
    let mut s = edit_of(&[3.0, 4.0, 5.0, 6.0, 12.0]);
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 0.5})).unwrap();
    let r = run(&s, "show", json!({"view": "histogram"}));
    assert!(!r.is_error, "{}", r.content);
    let v = r.view.unwrap();
    let counts: Vec<u64> = v["buckets"].as_array().unwrap().iter().map(|b| b["count"].as_u64().unwrap()).collect();
    assert_eq!(counts, [0, 0, 1, 3, 1, 0]);
    assert_eq!((v["buckets"][2]["label"].as_str(), v["buckets"][5]["label"].as_str()), (Some("2–4 s"), Some("16 s +")));
    assert_eq!(v["targetSeconds"], 7.0);
    assert!(v["meanSeconds"].as_f64().unwrap() > 5.0 && v["shots"] == 5);
    assert!(r.content.contains("the plan's target of about 7.0 s"), "{}", r.content);
    assert!(run(&Session::default(), "show", json!({"view": "histogram"})).is_error);
}

#[test]
fn hostile_show_input_is_refused_in_words() {
    let s = edit_of(&[6.0, 0.8, 6.0]);
    for odd in [
        json!(null),
        json!([]),
        json!("timeline"),
        json!(7),
        json!({}),
        json!({"view": 7}),
        json!({"view": "pie"}),
        json!({"view": "findings", "ids": "x"}),
        json!({"view": "findings", "ids": [1]}),
        json!({"view": "timeline", "highlight": {"a": 1}}),
    ] {
        let r = run(&s, "show", odd.clone());
        assert!(r.is_error && r.view.is_none(), "{odd}: {}", r.content);
    }
    // too many ids, and absurd times, are refused or clamped, never a panic
    let many: Vec<String> = (0..100).map(|i| format!("x@{i}")).collect();
    assert!(run(&s, "show", json!({"view": "findings", "ids": many})).is_error);
    for t in [json!(1e300), json!(-5), json!(f64::MAX), json!("99:99:99:99"), json!(null)] {
        let _ = run(&s, "show", json!({"view": "timeline", "from": t.clone(), "to": t}));
    }
    // a caption is cleaned and cut
    let c = run(&s, "show", json!({"view": "findings", "caption": format!("a\u{7}b{}", "x".repeat(500))})).view.unwrap();
    let cap = c["caption"].as_str().unwrap();
    assert!(cap.chars().count() <= 120 && !cap.contains('\u{7}'), "{cap}");
}

#[test]
fn a_view_reaches_the_conversation_but_never_the_model() {
    let mut s = edit_of(&[6.0, 0.8, 6.0]);
    let model = Arc::new(ScriptedModel::new());
    let (e, r) = tool_reply(&[("v1", "show", json!({"view": "timeline"}))]);
    model.push_reply(e, r);
    let (e, r) = text_reply("There it is.");
    model.push_reply(e, r);
    s.assistant.use_model(model.clone());
    let r = s.execute("assistant.send", json!({"text": "show me", "wait": true})).unwrap();
    assert_eq!(r["finish"]["kind"], "done", "{r}");
    // what the model got back
    let Part::ToolResult { content, .. } = &model.requests()[1].messages[2].parts[0] else { panic!() };
    assert!(content.starts_with("Shown to the person: a timeline of 3 shots") && !content.contains("\"shots\""), "{content}");
    // what the panel gets: the result event carries the view
    let v = s.execute("assistant.thread", json!({"thread": r["thread"]})).unwrap();
    let ev = v["events"].as_array().unwrap().iter().find(|e| e["type"] == "tool_result").expect("a tool result");
    assert_eq!(ev["view"]["view"], "timeline");
    assert_eq!(ev["view"]["shots"].as_array().unwrap().len(), 3);
}

fn item_named(s: &Session, name: &str) -> u64 {
    s.project.items.values().find(|i| i.name == name).map(|i| i.id.0).unwrap_or_else(|| panic!("no item {name}"))
}

/// Gives the edit's tone a narration script of `words` words.
fn narrate(s: &mut Session, words: usize) {
    let tone = item_named(s, "tone");
    let n = filmcraft_project::Narration {
        text: vec!["word"; words].join(" "),
        language: "en-US".into(),
        voice: "test".into(),
        pitch: Default::default(),
        pace: 1.0,
        take: 0,
        speech_duration: filmcraft_time::Tick(30 * filmcraft_time::TICKS_PER_SECOND),
    };
    Arc::make_mut(&mut s.project).narrations.insert(filmcraft_project::ItemId(tone), n);
}

#[test]
fn the_pace_view_bars_the_cuts_a_minute_along_the_edit_with_the_plans_rate() {
    let mut lengths = vec![1.0; 30];
    lengths.extend([20.0, 20.0, 20.0]);
    let mut s = edit_of(&lengths);
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 1.5})).unwrap();
    let r = run(&s, "show", json!({"view": "pace", "caption": "Cuts over time"}));
    assert!(!r.is_error, "{}", r.content);
    let v = r.view.unwrap();
    let bars: Vec<f64> = v["cutsPerMinute"].as_array().unwrap().iter().map(|b| b.as_f64().unwrap()).collect();
    assert!(bars.len() >= 2 && bars[0] > 30.0 && *bars.last().unwrap() < 5.0, "quick at the start, slow at the end: {bars:?}");
    assert!((v["targetPerMinute"].as_f64().unwrap() - 60.0 / 7.0).abs() < 1e-9, "the pack's 7 s a shot is 8.6 cuts a minute");
    assert!(v["overall"].as_f64().unwrap() > 0.0 && v["barSeconds"].as_f64().unwrap() >= 30.0);
    assert!(r.content.contains("Shown to the person: cuts a minute") && r.content.contains("plan's rate"), "{}", r.content);
    // a long edit is averaged into at most 120 bars
    let long = edit_of(&vec![3.0; 1500]);
    let v = run(&long, "show", json!({"view": "pace"})).view.unwrap();
    assert!(v["cutsPerMinute"].as_array().unwrap().len() <= 120, "{}", v["cutsPerMinute"].as_array().unwrap().len());
    assert!(run(&edit_of(&[5.0]), "show", json!({"view": "pace"})).is_error, "one shot has no pace");
}

#[test]
fn the_gauge_view_sets_one_number_against_its_target_and_says_when_it_cannot() {
    let mut s = edit_of(&[6.0, 6.0, 6.0, 6.0, 6.0]);
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 1})).unwrap();
    narrate(&mut s, 70);
    let g = |s: &Session, m: &str| run(s, "show", json!({"view": "gauge", "metric": m}));
    let shot = g(&s, "averageShot");
    let v = shot.view.clone().unwrap();
    assert_eq!((v["metric"].as_str(), v["unit"].as_str(), v["label"].as_str()), (Some("averageShot"), Some("s"), Some("Average shot")));
    assert!((v["value"].as_f64().unwrap() - 6.0).abs() < 0.05 && v["target"] == 7.0, "{v}");
    assert!(v["low"].as_f64().unwrap() < 6.0 && v["high"].as_f64().unwrap() > 6.0, "6 s is inside the band around 7 s");
    assert!(v["scaleMin"] == 0.0 && v["scaleMax"].as_f64().unwrap() > 6.0);
    let pace = g(&s, "narrationPace").view.unwrap();
    assert!((pace["value"].as_f64().unwrap() - 140.0).abs() < 1.0 && pace["target"] == 140.0, "{pace}");
    assert!(g(&s, "cutsPerMinute").view.is_some());
    // loudness needs the mix; before it is measured the tool says what to do
    let early = g(&s, "loudness");
    assert!(early.is_error && early.content.contains("analysis_run with mix true"), "{}", early.content);
    s.execute("analysis.run", json!({"mix": true, "wait": true})).unwrap();
    let lou = g(&s, "loudness").view.unwrap();
    assert!(
        (lou["scaleMin"].as_f64().unwrap(), lou["scaleMax"].as_f64().unwrap()) == (-40.0, 0.0)
            && lou["target"] == -14.0
            && lou["value"].as_f64().unwrap() < -14.0,
        "{lou}"
    );
    // no narration, no such metric, nothing named
    assert!(g(&edit_of(&[6.0, 6.0]), "narrationPace").is_error);
    for odd in [json!({"view": "gauge"}), json!({"view": "gauge", "metric": "mood"}), json!({"view": "gauge", "metric": 7})] {
        assert!(run(&s, "show", odd).is_error);
    }
    // a gauge with no plan has no target or band, and is still drawn
    let bare = g(&edit_of(&[6.0, 6.0, 6.0]), "averageShot").view.unwrap();
    assert!(bare["target"].is_null() && bare["low"].is_null() && bare["high"].is_null());
}

#[test]
fn the_palette_view_needs_the_item_to_have_been_measured() {
    let mut s = edit_of(&[6.0, 6.0]);
    let shot = item_named(&s, "shot0");
    let early = run(&s, "show", json!({"view": "palette", "item": shot}));
    assert!(early.is_error && early.content.contains(&format!("analysis_run with item {shot}")), "{}", early.content);
    s.execute("analysis.run", json!({"item": shot, "samples": 3, "wait": true})).unwrap();
    let r = run(&s, "show", json!({"view": "palette", "item": shot}));
    assert!(!r.is_error, "{}", r.content);
    let v = r.view.unwrap();
    let colours = v["colors"].as_array().unwrap();
    assert!(!colours.is_empty() && colours.len() <= 5 && colours[0]["rgb"].as_array().unwrap().len() == 3, "{v}");
    assert_eq!(v["name"], "shot0");
    // the tone has no pictures; a missing id and a wrong type are refused
    let tone = item_named(&s, "tone");
    s.execute("analysis.run", json!({"item": tone, "wait": true})).unwrap();
    assert!(run(&s, "show", json!({"view": "palette", "item": tone})).is_error);
    assert!(run(&s, "show", json!({"view": "palette"})).is_error && run(&s, "show", json!({"view": "palette", "item": "x"})).is_error);
}

#[test]
fn the_pie_view_divides_screen_time_narration_or_silence() {
    let mut s = edit_of(&[1.0, 1.0, 5.0, 20.0]);
    let v = run(&s, "show", json!({"view": "pie", "of": "screenTime"})).view.unwrap();
    let slices = v["slices"].as_array().unwrap();
    let share: f64 = slices.iter().map(|x| x["share"].as_f64().unwrap()).sum();
    assert!((share - 1.0).abs() < 1e-9, "{slices:?}");
    let by = |label: &str| slices.iter().find(|x| x["label"] == label).unwrap_or_else(|| panic!("no {label}: {slices:?}"));
    assert_eq!(by("under 2 s")["detail"], "2 shots");
    assert!((by("15 s and more")["share"].as_f64().unwrap() - 20.0 / 27.0).abs() < 0.01, "the long shot is most of the running time");
    assert!(slices.iter().all(|x| x["label"] != "6–15 s"), "a band with no shots is left out");
    // narration, and silence once the mix is measured
    assert!(run(&s, "show", json!({"view": "pie", "of": "narration"})).is_error, "no narration yet");
    narrate(&mut s, 60);
    let n = run(&s, "show", json!({"view": "pie", "of": "narration"})).view.unwrap();
    assert_eq!(n["slices"].as_array().unwrap().len(), 2);
    let early = run(&s, "show", json!({"view": "pie", "of": "silence"}));
    assert!(early.is_error && early.content.contains("analysis_run with mix true"));
    s.execute("analysis.run", json!({"mix": true, "wait": true})).unwrap();
    let q = run(&s, "show", json!({"view": "pie", "of": "silence"})).view.unwrap();
    assert!(q["slices"][0]["label"] == "sound" && q["slices"][0]["share"].as_f64().unwrap() > 0.9, "{q}");
    for odd in [json!({"view": "pie"}), json!({"view": "pie", "of": "mood"}), json!({"view": "pie", "of": 3})] {
        assert!(run(&s, "show", odd).is_error);
    }
    assert!(run(&Session::default(), "show", json!({"view": "pie", "of": "screenTime"})).is_error);
}
