//! Tests of [`crate::assistant`] and [`crate::assistant_tools`]: the tools describe the real demo
//! edit; a turn's tool calls cross from the worker to the session thread and back; threads can be
//! polled, stopped and replayed; and every command refuses hostile input without changing anything.

use super::*;
use crate::assistant::MAX_TEXT;
use edith_ai::scripted::ScriptedModel;
use edith_ai::{AgentEvent, Cancel, Capabilities, ChatModel, Message, ModelInfo, Part, Response, Role, StopReason, StreamEvent, ToolOutput, Usage};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn text_reply(t: &str) -> (Vec<StreamEvent>, Response) {
    (
        vec![StreamEvent::Text(t.into())],
        Response {
            message: Message::assistant_text(t),
            stop_reason: StopReason::EndTurn,
            usage: Usage { input_tokens: 200, output_tokens: 30, ..Default::default() },
            model: "claude-haiku-5-5".into(),
        },
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
            usage: Usage { input_tokens: 100, output_tokens: 20, ..Default::default() },
            model: "claude-haiku-5-5".into(),
        },
    )
}

fn scripted(replies: Vec<(Vec<StreamEvent>, Response)>) -> Arc<ScriptedModel> {
    let m = Arc::new(ScriptedModel::new());
    for (e, r) in replies {
        m.push_reply(e, r);
    }
    m
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("filmcraft-assistant-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn run_tool(s: &Session, name: &str, input: Value) -> ToolOutput {
    s.run_assistant_tool(name, &input)
}

fn wait_until_idle(s: &mut Session, thread: u64) -> Value {
    let started = Instant::now();
    loop {
        let v = s.execute("assistant.thread", json!({"thread": thread})).unwrap();
        if v["running"] == false {
            return v;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "the turn never finished");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_overview_names_the_project_the_open_sequence_and_the_media() {
    let s = demo();
    let o = run_tool(&s, "project_overview", json!({}));
    assert!(!o.is_error, "{}", o.content);
    for want in ["open sequence: \"Main Edit\"", "playhead", "selection: none", "sequences:", "1920x1080", "media:"] {
        assert!(o.content.contains(want), "{want:?} missing from:\n{}", o.content);
    }
    let empty = run_tool(&Session::default(), "project_overview", json!({}));
    assert!(empty.content.contains("no sequence is open"));
}

#[test]
fn the_sequence_view_is_a_compact_edit_list_with_clip_ids_transitions_and_markers() {
    let s = demo();
    let v = run_tool(&s, "sequence_view", json!({}));
    assert!(!v.is_error, "{}", v.content);
    let c = &v.content;
    assert!(c.starts_with("sequence \"Main Edit\""), "{c}");
    assert!(c.contains("1920x1080 23.976fps") && c.contains("length 00:00:26:18"), "{c}");
    assert!(c.contains("V1 #") && c.contains("Ocean_Sunset.mp4") && c.contains("A1 #"), "{c}");
    let clips = c.lines().filter(|l| l.starts_with('V') || l.starts_with('A')).filter(|l| l.contains(" #")).count();
    assert!(clips >= 10, "{clips} clips in:\n{c}");
    assert!(c.contains("transitions:") && c.contains("markers:") && c.contains("Music hit"), "{c}");
    assert!(c.len() < 6000, "the whole demo edit is {} bytes; compactness is the point", c.len());
}

#[test]
fn a_range_shows_only_the_clips_in_it_and_accepts_seconds_or_timecode() {
    let s = demo();
    let all = run_tool(&s, "sequence_view", json!({})).content;
    let part = run_tool(&s, "sequence_view", json!({"from": "00:00:10:00", "to": 14.0})).content;
    let count = |t: &str| t.lines().filter(|l| l.contains(" #")).count();
    assert!(count(&part) > 0 && count(&part) < count(&all), "{} of {}", count(&part), count(&all));
    assert!(!part.contains("Ocean_Sunset.mp4") && part.contains("City_Night_Drive.mp4"), "{part}");
    let same = run_tool(&s, "sequence_view", json!({"from": "10", "to": "14"})).content;
    assert_eq!(count(&same), count(&part));
    assert!(run_tool(&s, "sequence_view", json!({"from": 500})).content.contains("no clips in this range"));
}

#[test]
fn clip_detail_gives_everything_about_one_clip() {
    let s = demo();
    let list = run_tool(&s, "sequence_view", json!({"to": 4})).content;
    let id: u64 =
        list.lines().find(|l| l.starts_with("V1 #")).and_then(|l| l.split('#').nth(1)).and_then(|r| r.split(' ').next()).and_then(|n| n.parse().ok()).unwrap();
    let d = run_tool(&s, "clip_detail", json!({"clip": id}));
    assert!(!d.is_error, "{}", d.content);
    let heading = format!("clip #{id}");
    for want in [heading.as_str(), "on the timeline:", "source range:", "speed 100%", "effects:", "motion:", "opacity:"] {
        assert!(d.content.contains(want), "{want:?} missing from:\n{}", d.content);
    }
    assert!(!d.content.contains('/'), "no file paths reach the model: {}", d.content);
}

#[test]
fn the_transcript_tool_maps_media_words_onto_the_timeline() {
    let mut s = demo();
    let (item, start, source_in) = {
        let q = s.active_sequence().unwrap();
        let i = &q.video_tracks[0].items[0];
        (i.item, i.start, i.source_in)
    };
    assert!(run_tool(&s, "transcript", json!({"item": item.0})).content.contains("no transcript"));
    let words = ["The", "lighthouse", "keeper", "had", "not", "slept."].iter().enumerate().map(|(n, w)| {
        let a = Tick(source_in.0 + n as i64 * TICKS_PER_SECOND / 2);
        filmcraft_project::Word::new(*w, a, Tick(a.0 + TICKS_PER_SECOND / 3))
    });
    let tr = filmcraft_project::Transcript { language: "en".into(), source: "manual".into(), speakers: vec![], words: words.collect() };
    Arc::make_mut(&mut s.project).transcripts.insert(item, Arc::new(tr));
    let by_clip = {
        let id = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
        run_tool(&s, "transcript", json!({"clip": id}))
    };
    assert!(!by_clip.is_error, "{}", by_clip.content);
    assert!(by_clip.content.contains("timeline time") && by_clip.content.contains("The lighthouse keeper had not slept."), "{}", by_clip.content);
    assert!(
        by_clip.content.contains(&format!(
            "[{}]",
            filmcraft_time::format_timecode_frames(
                s.active_sequence().unwrap().settings.frame_rate.frame_at(start),
                s.active_sequence().unwrap().settings.frame_rate,
                false
            )
        )),
        "{}",
        by_clip.content
    );
    let narrow = run_tool(&s, "transcript", json!({"item": item.0, "from": 1.0, "to": 2.0}));
    assert!(narrow.content.contains("keeper") && !narrow.content.contains("lighthouse") || narrow.content.contains("lighthouse"), "{}", narrow.content);
}

#[test]
fn tools_refuse_hostile_arguments_with_an_error_not_a_panic() {
    let s = demo();
    let cases: Vec<(&str, Value)> = vec![
        ("clip_detail", json!({})),
        ("clip_detail", json!({"clip": "x"})),
        ("clip_detail", json!({"clip": -1})),
        ("clip_detail", json!({"clip": 1.5})),
        ("clip_detail", json!({"clip": 99999999999u64})),
        ("clip_detail", json!({"clip": 1, "sequence": 99999})),
        ("sequence_view", json!({"sequence": "main"})),
        ("sequence_view", json!({"sequence": 424242})),
        ("sequence_view", json!({"from": "banana"})),
        ("sequence_view", json!({"from": "1:2:3:4:5"})),
        ("sequence_view", json!({"from": "00:xx:00:00"})),
        ("sequence_view", json!({"from": -5})),
        ("sequence_view", json!({"from": 1e300})),
        ("sequence_view", json!({"from": true})),
        ("sequence_view", json!({"from": 10, "to": 5})),
        ("transcript", json!({})),
        ("transcript", json!({"clip": 99999999})),
        ("transcript", json!({"item": "x"})),
        ("transcript", json!({"item": 1, "from": "zzz"})),
    ];
    for (name, input) in cases {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_tool(&s, name, input.clone())));
        assert!(r.is_ok(), "{name} panicked on {input}");
        assert!(r.unwrap().is_error, "{name} accepted {input}");
    }
    // not-an-object inputs are treated as no arguments
    for odd in [json!(null), json!([1, 2]), json!("text"), json!(7)] {
        assert!(!run_tool(&s, "sequence_view", odd.clone()).is_error, "{odd}");
        assert!(!run_tool(&s, "project_overview", odd).is_error);
    }
}

#[test]
fn every_tool_is_read_only_and_the_engine_refuses_anything_else() {
    assert!(crate::assistant_tools::tools().iter().all(|d| d.read_only));
    let s = demo();
    let r = run_tool(&s, "delete_clip", json!({"clip": 1}));
    assert!(r.is_error && r.content.contains("unknown tool"), "{}", r.content);
}

#[test]
fn a_turn_with_a_tool_call_runs_the_tool_on_the_session_thread_and_answers() {
    let mut s = demo();
    let model = scripted(vec![tool_reply(&[("t1", "sequence_view", json!({"to": 6}))]), text_reply("Your first shot is Ocean_Sunset.mp4.")]);
    s.assistant.use_model(model.clone());
    let r = s.execute("assistant.send", json!({"text": "what is my first shot?", "wait": true})).unwrap();
    assert_eq!(r["running"], false);
    assert_eq!(r["finish"]["kind"], "done");
    assert_eq!(r["text"], "Your first shot is Ocean_Sunset.mp4.");
    assert!(r["usageText"].as_str().unwrap().contains("tokens"), "{r}");
    // the second request carried the real tool result from the real project
    let second = &model.requests()[1];
    let Part::ToolResult { content, is_error, .. } = &second.messages[2].parts[0] else { panic!("no tool result in {:?}", second.messages[2]) };
    assert!(!*is_error && content.contains("Ocean_Sunset.mp4") && content.contains("sequence \"Main Edit\""), "{content}");
    // the panel's stream
    let v = s.execute("assistant.thread", json!({"thread": r["thread"]})).unwrap();
    let kinds: Vec<&str> = v["events"].as_array().unwrap().iter().filter_map(|e| e["type"].as_str()).collect();
    for want in ["step_started", "tool_call", "tool_result", "text", "usage", "finished"] {
        assert!(kinds.contains(&want), "{want} missing from {kinds:?}");
    }
    assert_eq!(v["next"], v["events"].as_array().unwrap().len());
    // it ran as a job, and the editor is unchanged
    assert!(s.jobs.iter().any(|j| j.label == "Assistant" && j.progress.finished.load(Ordering::Relaxed)));
    assert!(!s.is_dirty());
}

#[test]
fn a_turn_that_is_not_waited_for_is_served_by_polling_and_the_stream_pages() {
    let mut s = demo();
    s.assistant.use_model(scripted(vec![tool_reply(&[("a", "project_overview", json!({})), ("b", "sequence_view", json!({}))]), text_reply("ok")]));
    let r = s.execute("assistant.send", json!({"text": "look"})).unwrap();
    assert_eq!(r["running"], true);
    let id = r["thread"].as_u64().unwrap();
    let done = wait_until_idle(&mut s, id);
    assert_eq!(done["finish"]["kind"], "done");
    // reading in pieces from an index returns the rest, with nothing repeated
    let all = s.execute("assistant.thread", json!({"thread": id})).unwrap();
    let total = all["next"].as_u64().unwrap();
    let tail = s.execute("assistant.thread", json!({"thread": id, "since": total - 2})).unwrap();
    assert_eq!(tail["events"].as_array().unwrap().len(), 2);
    assert_eq!(tail["next"], total);
    assert!(s.execute("assistant.thread", json!({"thread": id, "since": total + 100})).unwrap()["events"].as_array().unwrap().is_empty());
}

struct WaitsForStop;
impl ChatModel for WaitsForStop {
    fn provider(&self) -> &str {
        "anthropic"
    }
    fn models(&self) -> std::result::Result<Vec<ModelInfo>, edith_ai::LlmError> {
        Ok(vec![])
    }
    fn stream(&self, _r: &edith_ai::Request, cancel: &Cancel, sink: &mut dyn FnMut(StreamEvent)) -> std::result::Result<Response, edith_ai::LlmError> {
        sink(StreamEvent::Text("thinking about it".into()));
        while !cancel.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(edith_ai::LlmError::Cancelled)
    }
}

#[test]
fn stop_ends_a_turn_that_is_waiting_on_the_model() {
    let mut s = demo();
    s.assistant.use_model(Arc::new(WaitsForStop));
    let id = s.execute("assistant.send", json!({"text": "hello"})).unwrap()["thread"].as_u64().unwrap();
    // a second message to a busy thread is refused
    assert!(s.execute("assistant.send", json!({"text": "again", "thread": id})).is_err());
    assert!(s.execute("assistant.deleteThread", json!({"thread": id})).is_err(), "a running thread cannot be deleted");
    assert_eq!(s.execute("assistant.cancel", json!({"thread": id})).unwrap()["stopping"], true);
    let done = wait_until_idle(&mut s, id);
    assert_eq!(done["finish"]["kind"], "stopped");
    assert_eq!(s.execute("assistant.cancel", json!({"thread": id})).unwrap()["stopping"], false);
    // the thread can be used again afterwards
    s.assistant.use_model(scripted(vec![text_reply("back")]));
    let r = s.execute("assistant.send", json!({"text": "again", "thread": id, "wait": true})).unwrap();
    assert_eq!(r["text"], "back");
}

#[test]
fn the_jobs_panels_cancel_reaches_the_assistant() {
    let mut s = demo();
    s.assistant.use_model(Arc::new(WaitsForStop));
    let id = s.execute("assistant.send", json!({"text": "hello"})).unwrap()["thread"].as_u64().unwrap();
    let job = s.jobs.iter().find(|j| j.label == "Assistant").unwrap().id;
    s.execute("jobs.cancel", json!({"job": job})).unwrap();
    assert_eq!(wait_until_idle(&mut s, id)["finish"]["kind"], "stopped");
}

#[test]
fn context_blocks_are_sent_in_the_user_message() {
    let mut s = demo();
    let model = scripted(vec![text_reply("ok")]);
    s.assistant.use_model(model.clone());
    s.execute("assistant.send", json!({"text": "why?", "wait": true, "context": [{"label": "selection", "text": "clip #29 at 00:00:04:23"}]})).unwrap();
    let req = &model.requests()[0];
    assert!(req.messages[0].text().contains("[selection]") && req.messages[0].text().contains("#29"));
    assert!(!req.system[0].text.contains("#29"), "context never goes in the cached system prompt");
    assert_eq!(req.tools.len(), 4);
    assert_eq!(req.model, "claude-haiku-5-5");
}

#[test]
fn settings_are_validated_saved_and_loaded_again() {
    let dir = tmp("settings");
    let mut s = Session::default();
    s.assistant.set_dir(&dir);
    assert_eq!(s.execute("assistant.settings", json!({})).unwrap()["settings"]["model"], "claude-haiku-5-5");
    let r = s
        .execute("assistant.settings", json!({"model": "claude-sonnet-5-5", "effort": "high", "showThinking": false, "maxSteps": 6, "maxCostUsd": 0.25}))
        .unwrap();
    assert_eq!((r["saved"].clone(), r["settings"]["model"].clone(), r["settings"]["effort"].clone()), (json!(true), json!("claude-sonnet-5-5"), json!("high")));
    let mut again = Session::default();
    again.assistant.set_dir(&dir);
    let loaded = again.execute("assistant.settings", json!({})).unwrap();
    assert_eq!(
        (
            loaded["settings"]["model"].clone(),
            loaded["settings"]["maxSteps"].clone(),
            loaded["settings"]["maxCostUsd"].clone(),
            loaded["settings"]["showThinking"].clone()
        ),
        (json!("claude-sonnet-5-5"), json!(6), json!(0.25), json!(false))
    );
    // clearing the effort and the cost limit
    let r = s.execute("assistant.settings", json!({"effort": null, "maxCostUsd": null})).unwrap();
    assert!(r["settings"]["effort"].is_null() && r["settings"]["maxCostUsd"].is_null());
    // hostile values change nothing
    let before = s.execute("assistant.settings", json!({})).unwrap();
    for bad in [
        json!({"model": "bad model!"}),
        json!({"model": ""}),
        json!({"model": 5}),
        json!({"service": "x".repeat(200)}),
        json!({"effort": "ultra"}),
        json!({"effort": 5}),
        json!({"showThinking": "yes"}),
        json!({"maxSteps": 0}),
        json!({"maxSteps": 41}),
        json!({"maxSteps": "x"}),
        json!({"maxOutputTokens": 10}),
        json!({"maxCostUsd": -1}),
        json!({"maxCostUsd": 0}),
        json!({"maxCostUsd": "x"}),
        json!({"model": "ok", "maxSteps": 0}),
    ] {
        assert!(s.execute("assistant.settings", bad.clone()).is_err(), "{bad}");
        assert_eq!(s.execute("assistant.settings", json!({})).unwrap(), before, "{bad} changed something");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_settings_file_means_defaults_and_a_note() {
    let dir = tmp("damaged");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(edith_ai::SETTINGS_FILE), "{ not json").unwrap();
    let mut s = Session::default();
    s.assistant.set_dir(&dir);
    let st = s.execute("assistant.status", json!({})).unwrap();
    assert_eq!(st["settings"]["model"], "claude-haiku-5-5");
    assert!(st["settingsNote"].as_str().unwrap().contains("damaged"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_model_list_is_fetched_in_the_background_with_badges() {
    let mut s = Session::default();
    let caps = Capabilities {
        tools: true,
        parallel_tools: true,
        images: true,
        thinking: true,
        effort: true,
        caching: true,
        context_tokens: 1_000_000,
        max_output_tokens: 128_000,
    };
    let info = |id: &str| ModelInfo { id: id.into(), display_name: id.to_uppercase(), capabilities: caps.clone(), price: None };
    s.assistant.use_model(Arc::new(ScriptedModel::new().with_models(vec![info("claude-sonnet-5-5"), info("claude-haiku-5-5")])));
    let first = s.execute("assistant.models", json!({})).unwrap();
    assert!(first["updating"] == true || first["models"].as_array().unwrap().len() == 2);
    let started = Instant::now();
    let done = loop {
        let v = s.execute("assistant.models", json!({})).unwrap();
        if v["updating"] == false {
            break v;
        }
        assert!(started.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(done["models"].as_array().unwrap().len(), 2);
    assert!(done["models"][0]["badges"].as_array().unwrap().iter().any(|b| b == "1M context"));
    assert_eq!((done["selected"].clone(), done["resolved"].clone()), (json!("claude-haiku-5-5"), json!("claude-haiku-5-5")));
    assert!(s.execute("assistant.models", json!({"refresh": "yes"})).is_err());
    s.execute("assistant.settings", json!({"model": "claude-gone"})).unwrap();
    assert_eq!(s.execute("assistant.models", json!({})).unwrap()["resolved"], "claude-haiku-5-5", "a missing preferred model falls back to a usual one");
}

#[cfg(not(feature = "net"))]
#[test]
fn without_a_provider_the_assistant_says_so_plainly() {
    let mut s = demo();
    let st = s.execute("assistant.status", json!({})).unwrap();
    assert_eq!(st["available"], false);
    let e = s.execute("assistant.send", json!({"text": "hi"})).unwrap_err().to_string();
    assert!(e.contains("not available in this build"), "{e}");
    assert!(
        s.assistant.threads.is_empty()
            || s.execute("assistant.threads", json!({})).unwrap()["threads"].as_array().unwrap().iter().all(|t| t["running"] == false)
    );
}

#[test]
fn threads_are_created_listed_and_deleted() {
    let mut s = demo();
    let a = s.execute("assistant.newThread", json!({"title": "Pacing"})).unwrap()["thread"].as_u64().unwrap();
    let b = s.execute("assistant.newThread", json!({})).unwrap()["thread"].as_u64().unwrap();
    assert_ne!(a, b);
    let list = s.execute("assistant.threads", json!({})).unwrap();
    assert_eq!(list["threads"].as_array().unwrap().len(), 2);
    assert_eq!(list["threads"][0]["title"], "Pacing");
    s.execute("assistant.deleteThread", json!({"thread": a})).unwrap();
    assert_eq!(s.execute("assistant.threads", json!({})).unwrap()["threads"].as_array().unwrap().len(), 1);
    for _ in 0..60 {
        let _ = s.execute("assistant.newThread", json!({}));
    }
    assert!(s.execute("assistant.newThread", json!({})).is_err(), "there is a limit on threads");
}

#[test]
fn a_thread_trace_collects_every_turn_and_replays_to_the_same_answers() {
    let mut s = demo();
    s.assistant.use_model(scripted(vec![tool_reply(&[("t1", "project_overview", json!({}))]), text_reply("one"), text_reply("two")]));
    let id = s.execute("assistant.send", json!({"text": "first", "wait": true})).unwrap()["thread"].as_u64().unwrap();
    s.execute("assistant.send", json!({"text": "second", "thread": id, "wait": true})).unwrap();
    let jsonl = s.execute("assistant.trace", json!({"thread": id})).unwrap()["jsonl"].as_str().unwrap().to_string();
    let trace = edith_ai::Trace::from_jsonl(&jsonl).unwrap();
    assert_eq!(trace.replies.len(), 3, "both turns' replies are in one trace");
    assert!(trace.events.iter().any(|e| matches!(e, AgentEvent::ToolResult { .. })));
    assert_eq!(trace.meta.prompt_version, "2");
    // a new session fed the recorded replies gives the same answers, with its own tools run on its own project
    let mut replay = demo();
    replay.assistant.use_model(Arc::new(trace.replay_model()));
    assert_eq!(replay.execute("assistant.send", json!({"text": "first", "wait": true})).unwrap()["text"], "one");
    let id2 = replay.assistant.threads[0].id;
    assert_eq!(replay.execute("assistant.send", json!({"text": "second", "thread": id2, "wait": true})).unwrap()["text"], "two");
}

#[test]
fn every_assistant_command_refuses_hostile_parameters_without_changes() {
    let mut s = demo();
    s.assistant.use_model(scripted(vec![]));
    let snap = (*s.project).clone();
    let undo0 = s.history.undo.len();
    let big = "x".repeat(MAX_TEXT + 1);
    let many: Vec<Value> = (0..11).map(|i| json!({"label": format!("l{i}"), "text": "t"})).collect();
    let cases: Vec<(&str, Value)> = vec![
        ("assistant.send", json!({})),
        ("assistant.send", json!({"text": ""})),
        ("assistant.send", json!({"text": "   "})),
        ("assistant.send", json!({"text": 5})),
        ("assistant.send", json!({"text": big})),
        ("assistant.send", json!({"text": "hi", "wait": "yes"})),
        ("assistant.send", json!({"text": "hi", "thread": 999})),
        ("assistant.send", json!({"text": "hi", "thread": "x"})),
        ("assistant.send", json!({"text": "hi", "context": "no"})),
        ("assistant.send", json!({"text": "hi", "context": [{"label": "", "text": "t"}]})),
        ("assistant.send", json!({"text": "hi", "context": [{"label": "l"}]})),
        ("assistant.send", json!({"text": "hi", "context": many})),
        ("assistant.thread", json!({})),
        ("assistant.thread", json!({"thread": 999})),
        ("assistant.thread", json!({"thread": -1})),
        ("assistant.thread", json!({"thread": 1, "since": "x"})),
        ("assistant.cancel", json!({})),
        ("assistant.cancel", json!({"thread": 999})),
        ("assistant.deleteThread", json!({"thread": 999})),
        ("assistant.trace", json!({"thread": "x"})),
        ("assistant.newThread", json!({"title": ""})),
        ("assistant.newThread", json!({"title": 5})),
        ("assistant.newThread", json!({"title": "x".repeat(201)})),
    ];
    for (id, p) in cases {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute(id, p.clone())));
        assert!(r.is_ok(), "{id} panicked on {p}");
        assert!(r.unwrap().is_err(), "{id} accepted {p}");
        assert_eq!(*s.project, snap, "{id} changed the project on {p}");
        assert_eq!(s.history.undo.len(), undo0);
    }
    assert!(s.assistant.threads.is_empty(), "refused sends must not leave threads behind");
}

#[test]
fn the_assistant_commands_are_not_journaled_and_have_no_way_to_edit() {
    for c in crate::commands::command_specs().iter().filter(|c| c.id.starts_with("assistant.")) {
        assert!(!c.journal, "{} must not be journaled (it would put your questions in the session log)", c.id);
    }
    assert!(crate::commands::command_specs().iter().filter(|c| c.id.starts_with("assistant.")).count() >= 10);
}

/// Asks the real model about the demo project, with the key stored in the keychain: a handful of
/// tool calls and a short answer, a few cents at most. It checks the whole chain against the live
/// service for the first time, tool use included.
/// `cargo test -p filmcraft-engine --lib --features net -- --ignored real_assistant`
#[cfg(feature = "net")]
#[test]
#[ignore = "uses the real network and the stored Anthropic key"]
fn real_assistant_answers_about_the_demo_project_using_its_tools() {
    let mut s = demo();
    let started = Instant::now();
    let r = s
        .execute("assistant.send", json!({"text": "Use your tools to look at my edit. Name the first clip on the main video track and tell me how long the whole edit is. Be brief.", "wait": true}))
        .unwrap();
    eprintln!("finished in {:.1}s: {}", started.elapsed().as_secs_f64(), r["finish"]);
    eprintln!("answer: {}", r["text"]);
    eprintln!("usage: {}", r["usageText"]);
    let thread = r["thread"].as_u64().unwrap();
    let v = s.execute("assistant.thread", json!({"thread": thread})).unwrap();
    let calls: Vec<String> =
        v["events"].as_array().unwrap().iter().filter(|e| e["type"] == "tool_call").map(|e| e["name"].as_str().unwrap_or("?").to_string()).collect();
    eprintln!("tools it called: {calls:?}");
    // How long each request to the provider took (the network layer logs every one), so a slow turn
    // can be told apart: many requests, or a few slow ones.
    for e in edith_net::shared().entries(None) {
        eprintln!("  request {} {} -> {:?} in {} ms ({} bytes)", e.method, e.url, e.status, e.millis, e.bytes);
    }
    eprintln!("keychain reads are counted by the secret store's cache: one per half hour, not one per request");
    assert_eq!(r["finish"]["kind"], "done", "{r}");
    assert!(!calls.is_empty(), "the model never used a tool");
    let text = r["text"].as_str().unwrap().to_lowercase();
    assert!(text.contains("ocean") && text.contains("26"), "the answer should name the first clip and the length: {text}");
}

#[test]
fn slash_commands_are_listed_and_the_model_gets_the_expansion_while_the_person_sees_what_they_typed() {
    let mut s = demo();
    let list = s.execute("assistant.commands", json!({})).unwrap();
    let names: Vec<&str> = list["commands"].as_array().unwrap().iter().filter_map(|c| c["name"].as_str()).collect();
    assert_eq!(names, ["explain-cut", "pacing", "summarize", "glossary"]);
    assert_eq!(list["commands"][3]["takesText"], true);
    assert_eq!(list["commands"][1]["takesText"], false);
    let model = scripted(vec![text_reply("The pacing is steady."), text_reply("A J-cut is...")]);
    s.assistant.use_model(model.clone());
    let r = s.execute("assistant.send", json!({"text": "/pacing", "wait": true})).unwrap();
    assert!(model.requests()[0].messages[0].text().contains("Review the pacing of the whole edit"), "the model is asked the expansion");
    let events = s.execute("assistant.thread", json!({"thread": r["thread"]})).unwrap()["events"].clone();
    assert_eq!(events[0], json!({"type": "user_message", "text": "/pacing"}), "the conversation shows what was typed");
    // a command with text
    s.execute("assistant.send", json!({"text": "/glossary J-cut", "thread": r["thread"], "wait": true})).unwrap();
    assert!(model.requests()[1].messages.last().unwrap().text().contains("\"J-cut\""));
}

#[test]
fn a_bad_slash_command_is_refused_and_leaves_no_thread_but_a_path_is_just_text() {
    let mut s = demo();
    let model = scripted(vec![text_reply("ok")]);
    s.assistant.use_model(model.clone());
    for bad in ["/nope", "/glossary", "/pacingx"] {
        let e = s.execute("assistant.send", json!({"text": bad})).unwrap_err().to_string();
        assert!(e.contains("/pacing") || e.contains("for example"), "{bad}: {e}");
    }
    assert!(s.assistant.threads.is_empty(), "refused commands leave no thread behind");
    assert!(model.requests().is_empty());
    // a message that merely starts with a slash is an ordinary message
    let r = s.execute("assistant.send", json!({"text": "/Users/me/clip.mp4 will not play, why?", "wait": true})).unwrap();
    assert_eq!(r["finish"]["kind"], "done");
    assert!(model.requests()[0].messages[0].text().contains("/Users/me/clip.mp4"));
}

fn saved_file(dir: &std::path::Path, id: u64) -> std::path::PathBuf {
    dir.join("edith-threads").join(format!("{id}.json"))
}

#[test]
fn a_finished_turn_is_saved_and_a_new_session_restores_and_continues_the_conversation() {
    let dir = tmp("threads-roundtrip");
    let mut a = demo();
    a.assistant.set_dir(&dir);
    a.assistant.use_model(scripted(vec![tool_reply(&[("t1", "project_overview", json!({}))]), text_reply("It is a 26 second edit.")]));
    let r = a.execute("assistant.send", json!({"text": "how long is it?", "wait": true})).unwrap();
    let id = r["thread"].as_u64().unwrap();
    assert!(saved_file(&dir, id).exists(), "the thread is written when the turn ends");
    assert!(!dir.join("edith-threads").join(format!("{id}.json.tmp")).exists(), "no temporary file is left behind");

    // a later run of the app
    let mut b = demo();
    b.assistant.set_dir(&dir);
    let list = b.execute("assistant.threads", json!({})).unwrap();
    assert_eq!(list["threads"].as_array().unwrap().len(), 1);
    assert_eq!((list["threads"][0]["id"].as_u64(), list["threads"][0]["title"].as_str()), (Some(id), Some("how long is it?")));
    assert_eq!(list["threads"][0]["running"], false);
    assert_eq!(list["threads"][0]["finish"]["kind"], "done");
    let view = b.execute("assistant.thread", json!({"thread": id})).unwrap();
    let kinds: Vec<&str> = view["events"].as_array().unwrap().iter().filter_map(|e| e["type"].as_str()).collect();
    assert_eq!(kinds.first(), Some(&"user_message"));
    assert!(kinds.contains(&"tool_result") && kinds.contains(&"text"), "{kinds:?}");
    assert!(view["usageText"].as_str().unwrap().contains("tokens"));
    // the restored conversation can be continued: the model is sent everything said before
    let model = scripted(vec![text_reply("About 27 seconds, to be precise.")]);
    b.assistant.use_model(model.clone());
    b.execute("assistant.send", json!({"text": "and exactly?", "thread": id, "wait": true})).unwrap();
    assert_eq!(model.requests()[0].messages.len(), 5, "user, call, results, answer, and the new question");
    // new threads do not reuse an old id
    let fresh = b.execute("assistant.newThread", json!({})).unwrap()["thread"].as_u64().unwrap();
    assert!(fresh > id);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn deleting_a_thread_removes_its_file() {
    let dir = tmp("threads-delete");
    let mut s = demo();
    s.assistant.set_dir(&dir);
    s.assistant.use_model(scripted(vec![text_reply("hi")]));
    let id = s.execute("assistant.send", json!({"text": "hello", "wait": true})).unwrap()["thread"].as_u64().unwrap();
    assert!(saved_file(&dir, id).exists());
    s.execute("assistant.deleteThread", json!({"thread": id})).unwrap();
    assert!(!saved_file(&dir, id).exists());
    let mut again = demo();
    again.assistant.set_dir(&dir);
    assert!(again.execute("assistant.threads", json!({})).unwrap()["threads"].as_array().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

fn record(id: u64, thread: Value) -> Value {
    json!({"version": 1, "id": id, "title": "saved", "thread": thread, "events": [], "dropped": 0, "finish": {"kind": "done"}, "usage": {"input_tokens": 1, "output_tokens": 1, "cache_read_tokens": 0, "cache_write_tokens": 0}, "meta": {"prompt_version": "2", "model": "m", "profile": "read_only", "started_unix": 0}, "replies": []})
}

#[test]
fn damaged_oversize_mislabelled_and_broken_files_are_skipped_or_repaired_never_fatal() {
    let dir = tmp("threads-hostile");
    let folder = dir.join("edith-threads");
    std::fs::create_dir_all(&folder).unwrap();
    let valid = json!({"messages": [{"role": "user", "parts": [{"type": "text", "text": "q"}]}, {"role": "assistant", "parts": [{"type": "text", "text": "a"}]}], "usage": {"input_tokens": 0, "output_tokens": 0, "cache_read_tokens": 0, "cache_write_tokens": 0}});
    std::fs::write(folder.join("1.json"), record(1, valid.clone()).to_string()).unwrap();
    // a conversation ending in a tool call nobody answered: the provider would reject it, so it is cut back
    let dangling = json!({"messages": [{"role": "user", "parts": [{"type": "text", "text": "q"}]}, {"role": "assistant", "parts": [{"type": "text", "text": "a"}]}, {"role": "user", "parts": [{"type": "text", "text": "q2"}]}, {"role": "assistant", "parts": [{"type": "tool_use", "id": "x", "name": "n", "input": {}}]}], "usage": {"input_tokens": 0, "output_tokens": 0, "cache_read_tokens": 0, "cache_write_tokens": 0}});
    std::fs::write(folder.join("2.json"), record(2, dangling).to_string()).unwrap();
    // skipped: not JSON, a file named for a different id, a wrong version, an unreadable shape, an oversize file, a stray file
    std::fs::write(folder.join("3.json"), "not json at all").unwrap();
    std::fs::write(folder.join("4.json"), record(99, valid.clone()).to_string()).unwrap();
    let mut wrong_version = record(5, valid.clone());
    wrong_version["version"] = json!(99);
    std::fs::write(folder.join("5.json"), wrong_version.to_string()).unwrap();
    std::fs::write(folder.join("6.json"), r#"{"id": 6}"#).unwrap();
    std::fs::write(folder.join("7.json"), vec![b' '; 21 * 1024 * 1024]).unwrap();
    std::fs::write(folder.join("notes.txt"), "stray").unwrap();
    std::fs::write(folder.join("x.json"), "{}").unwrap();
    // a title with control characters and a huge length is cleaned
    let mut messy = record(8, valid);
    messy["title"] = json!(format!("bad\u{7} title {}", "x".repeat(500)));
    std::fs::write(folder.join("8.json"), messy.to_string()).unwrap();

    let mut s = demo();
    s.assistant.set_dir(&dir);
    let st = s.execute("assistant.status", json!({})).unwrap();
    assert_eq!(st["restoreNote"], "5 saved conversations could not be restored.", "{st}");
    let list = s.execute("assistant.threads", json!({})).unwrap();
    let ids: Vec<u64> = list["threads"].as_array().unwrap().iter().filter_map(|t| t["id"].as_u64()).collect();
    assert_eq!(ids, [1, 2, 8]);
    let title8 = list["threads"][2]["title"].as_str().unwrap();
    assert!(title8.chars().count() <= 80 && !title8.chars().any(char::is_control), "{title8}");
    // the dangling one still works: ask something new and the model is sent a valid conversation
    let model = scripted(vec![text_reply("fine")]);
    s.assistant.use_model(model.clone());
    s.execute("assistant.send", json!({"text": "next", "thread": 2, "wait": true})).unwrap();
    let sent = &model.requests()[0].messages;
    assert_eq!(sent.len(), 4, "q, a, q2 (the unanswered call was cut), and the new question");
    assert!(sent.iter().all(|m| !m.tool_uses().any(|_| true)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_the_newest_threads_are_restored() {
    let dir = tmp("threads-limit");
    let folder = dir.join("edith-threads");
    std::fs::create_dir_all(&folder).unwrap();
    let thread = json!({"messages": [], "usage": {"input_tokens": 0, "output_tokens": 0, "cache_read_tokens": 0, "cache_write_tokens": 0}});
    for id in 1..=60u64 {
        std::fs::write(folder.join(format!("{id}.json")), record(id, thread.clone()).to_string()).unwrap();
    }
    let mut s = Session::default();
    s.assistant.set_dir(&dir);
    let ids: Vec<u64> = s.execute("assistant.threads", json!({})).unwrap()["threads"].as_array().unwrap().iter().filter_map(|t| t["id"].as_u64()).collect();
    assert_eq!((ids.len(), ids.first().copied(), ids.last().copied()), (50, Some(11), Some(60)));
    assert!(s.execute("assistant.newThread", json!({})).is_err(), "the limit still applies");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_save_that_fails_is_reported_in_the_conversation_and_never_breaks_the_turn() {
    let dir = tmp("threads-unwritable");
    std::fs::create_dir_all(&dir).unwrap();
    // a plain file where the folder should be, so nothing can be written there
    std::fs::write(dir.join("edith-threads"), "in the way").unwrap();
    let mut s = demo();
    s.assistant.set_dir(&dir);
    s.assistant.use_model(scripted(vec![text_reply("answer")]));
    let r = s.execute("assistant.send", json!({"text": "hello", "wait": true})).unwrap();
    assert_eq!(r["finish"]["kind"], "done");
    assert_eq!(r["text"], "answer");
    let view = s.execute("assistant.thread", json!({"thread": r["thread"]})).unwrap();
    let notices: Vec<&str> = view["events"].as_array().unwrap().iter().filter(|e| e["type"] == "notice").filter_map(|e| e["text"].as_str()).collect();
    assert!(notices.iter().any(|n| n.contains("could not be saved")), "{notices:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
