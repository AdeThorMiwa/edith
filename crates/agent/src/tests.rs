//! Tests of the loop. The model is scripted and the editor is fake, so nothing here touches a
//! network. `assert_valid` checks the rules every finished thread must keep.

use crate::testing::FakeEnvironment;
use crate::*;
use edith_llm::scripted::ScriptedModel;
use edith_llm::{Cancel, LlmError, Message, Part, Response, Role, StopReason, StreamEvent, Usage};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn usage(input: u64, output: u64) -> Usage {
    Usage { input_tokens: input, output_tokens: output, ..Default::default() }
}

fn reply(parts: Vec<Part>, stop: StopReason) -> Response {
    Response { message: Message { role: Role::Assistant, parts }, stop_reason: stop, usage: usage(10, 5), model: "m".into() }
}

fn text_reply(t: &str) -> Response {
    reply(vec![Part::Text { text: t.into() }], StopReason::EndTurn)
}

fn use_part(id: &str, name: &str, input: Value) -> Part {
    Part::ToolUse { id: id.into(), name: name.into(), input, input_error: None }
}

fn tool_reply(calls: Vec<Part>) -> Response {
    reply(calls, StopReason::ToolUse)
}

fn push(m: &ScriptedModel, r: Response) {
    let events = r.message.parts.iter().filter_map(|p| if let Part::Text { text } = p { Some(StreamEvent::Text(text.clone())) } else { None }).collect();
    m.push_reply(events, r);
}

fn cancel() -> Cancel {
    Arc::new(AtomicBool::new(false))
}

fn agent<'a>(m: &'a ScriptedModel, e: &'a FakeEnvironment, tweak: impl FnOnce(&mut AgentConfig)) -> Agent<'a> {
    let mut config = AgentConfig::new("claude-test");
    config.retry_delays = vec![Duration::ZERO, Duration::ZERO];
    tweak(&mut config);
    Agent { model: m, env: e, pack: PromptPack::builtin(), config }
}

fn run(a: &Agent, thread: &mut Thread, text: &str) -> (TurnReport, Vec<AgentEvent>) {
    run_with(a, thread, &[], text, &cancel())
}

fn run_with(a: &Agent, thread: &mut Thread, ctx: &[ContextBlock], text: &str, c: &Cancel) -> (TurnReport, Vec<AgentEvent>) {
    let mut events = Vec::new();
    let report = a.run_turn(thread, ctx, text, c, &mut |e| events.push(e));
    assert_valid(thread);
    (report, events)
}

/// The rules every thread keeps: each tool call is answered exactly once, in the very next message,
/// which is a user message holding all the answers; and no answer refers to a call that was not made.
fn assert_valid(t: &Thread) {
    let mut seen_uses: Vec<String> = Vec::new();
    for (i, m) in t.messages.iter().enumerate() {
        let ids: Vec<&str> = m.parts.iter().filter_map(|p| if let Part::ToolUse { id, .. } = p { Some(id.as_str()) } else { None }).collect();
        let answered: Vec<&str> =
            m.parts.iter().filter_map(|p| if let Part::ToolResult { tool_use_id, .. } = p { Some(tool_use_id.as_str()) } else { None }).collect();
        for a in &answered {
            assert!(seen_uses.iter().any(|u| u == a), "answer for a call never made: {a}");
        }
        if ids.is_empty() {
            continue;
        }
        assert_eq!(m.role, Role::Assistant);
        let next = t.messages.get(i + 1).unwrap_or_else(|| panic!("tool calls {ids:?} were never answered"));
        assert_eq!(next.role, Role::User);
        let got: Vec<&str> =
            next.parts.iter().filter_map(|p| if let Part::ToolResult { tool_use_id, .. } = p { Some(tool_use_id.as_str()) } else { None }).collect();
        assert_eq!(got, ids, "answers must be in one message, in order");
        seen_uses.extend(ids.iter().map(|s| s.to_string()));
    }
}

fn editor() -> FakeEnvironment {
    FakeEnvironment::new()
        .tool("project_inspect", true, |_| ToolOutput::ok("sequence Main Edit, 12 clips"))
        .tool("transcript", true, |i| ToolOutput::ok(format!("transcript from {}", i["from"])))
        .tool("delete_clip", false, |_| ToolOutput::ok("deleted"))
}

#[test]
fn a_plain_question_is_answered_and_the_thread_grows_by_two() {
    let m = ScriptedModel::new();
    push(&m, text_reply("It jumps because the cut ignores the sentence."));
    let e = editor();
    let a = agent(&m, &e, |_| {});
    let mut t = Thread::new();
    let (r, events) = run(&a, &mut t, "why is the cut abrupt?");
    assert_eq!((r.finish, r.steps), (Finish::Done, 1));
    assert_eq!(t.messages.len(), 2);
    assert_eq!(t.messages[1].text(), "It jumps because the cut ignores the sentence.");
    assert!(matches!(events.first(), Some(AgentEvent::UserMessage { text }) if text == "why is the cut abrupt?"), "the turn opens with what the person said");
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Text { text } if text.contains("jumps"))));
    assert!(matches!(events.last(), Some(AgentEvent::Finished { finish: Finish::Done })));
    assert_eq!(t.usage, usage(10, 5));
}

#[test]
fn the_request_carries_the_stable_cached_prompt_and_only_the_offered_tools() {
    let e = editor();
    let m = ScriptedModel::new();
    push(&m, text_reply("a"));
    run(&agent(&m, &e, |_| {}), &mut Thread::new(), "q");
    let req = &m.requests()[0];
    let names: Vec<_> = req.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["project_inspect", "transcript"], "a read-only thread is never offered an editing tool");
    assert!(req.system.last().is_some_and(|p| p.cache) && req.system[0].text.contains("You are Edith") && req.system[0].text.contains("read-only"));
    assert_eq!(req.model, "claude-test");
    let m = ScriptedModel::new();
    push(&m, text_reply("a"));
    run(&agent(&m, &e, |c| c.profile = Profile::Propose), &mut Thread::new(), "q");
    assert_eq!(m.requests()[0].tools.len(), 3);
    assert!(m.requests()[0].system[0].text.contains("propose"));
}

#[test]
fn tool_calls_run_in_order_and_all_answers_go_back_in_one_message() {
    let m = ScriptedModel::new();
    push(
        &m,
        tool_reply(vec![
            Part::Text { text: "Looking.".into() },
            use_part("t1", "project_inspect", json!({})),
            use_part("t2", "transcript", json!({"from": 42})),
        ]),
    );
    push(&m, text_reply("Done looking."));
    let e = editor();
    let mut t = Thread::new();
    let (r, events) = run(&agent(&m, &e, |_| {}), &mut t, "look at it");
    assert_eq!((r.finish, r.steps), (Finish::Done, 2));
    assert_eq!(t.messages.len(), 4, "user, assistant with calls, one results message, assistant");
    assert_eq!(e.calls().iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["project_inspect", "transcript"]);
    let Part::ToolResult { content, is_error, .. } = &t.messages[2].parts[1] else { panic!("no result") };
    assert_eq!((content.as_str(), *is_error), ("transcript from 42", false));
    let cards = events.iter().filter(|e| matches!(e, AgentEvent::ToolCall { .. } | AgentEvent::ToolResult { .. })).count();
    assert_eq!(cards, 4, "a call card and a result card for each of the two tools");
    // the second request saw the results
    assert_eq!(m.requests()[1].messages.len(), 3);
}

#[test]
fn thinking_blocks_come_back_unchanged_and_first() {
    let thinking = Part::Opaque { provider: "anthropic".into(), value: json!({"type": "thinking", "thinking": "hm", "signature": "SIG"}) };
    let m = ScriptedModel::new();
    push(&m, tool_reply(vec![thinking.clone(), use_part("t1", "project_inspect", json!({}))]));
    push(&m, text_reply("ok"));
    let e = editor();
    let mut t = Thread::new();
    run(&agent(&m, &e, |_| {}), &mut t, "q");
    assert_eq!(m.requests()[1].messages[1].parts[0], thinking, "the reasoning block is sent back exactly as it came");
    assert_eq!(t.messages[1].parts[0], thinking);
}

#[test]
fn a_read_only_thread_refuses_an_editing_tool_even_if_the_model_asks() {
    let m = ScriptedModel::new();
    push(&m, tool_reply(vec![use_part("t1", "delete_clip", json!({"clip": 3}))]));
    push(&m, text_reply("I can't change it."));
    let e = editor();
    let mut t = Thread::new();
    let (r, _) = run(&agent(&m, &e, |_| {}), &mut t, "delete the clip");
    assert_eq!(r.finish, Finish::Done);
    assert!(e.calls().is_empty(), "the editing tool must never run");
    let Part::ToolResult { content, is_error, .. } = &t.messages[2].parts[0] else { panic!() };
    assert!(*is_error && content.contains("read-only"), "{content}");
}

#[test]
fn unknown_tools_and_broken_input_get_error_answers_and_the_turn_continues() {
    let m = ScriptedModel::new();
    push(
        &m,
        tool_reply(vec![
            use_part("t1", "no_such_tool", json!({})),
            Part::ToolUse { id: "t2".into(), name: "transcript".into(), input: json!({}), input_error: Some("the tool input was not valid JSON".into()) },
        ]),
    );
    push(&m, text_reply("sorry"));
    let e = editor();
    let mut t = Thread::new();
    let (r, _) = run(&agent(&m, &e, |_| {}), &mut t, "q");
    assert_eq!(r.finish, Finish::Done);
    assert!(e.calls().is_empty());
    let parts = &t.messages[2].parts;
    let Part::ToolResult { content: c1, is_error: e1, .. } = &parts[0] else { panic!() };
    let Part::ToolResult { content: c2, is_error: e2, .. } = &parts[1] else { panic!() };
    assert!(*e1 && c1.contains("Unknown tool") && c1.contains("project_inspect"), "{c1}");
    assert!(*e2 && c2.contains("INVALID_JSON"), "{c2}");
}

#[test]
fn more_tool_calls_than_allowed_are_answered_not_run() {
    let calls: Vec<Part> = (0..40).map(|i| use_part(&format!("t{i}"), "project_inspect", json!({}))).collect();
    let m = ScriptedModel::new();
    push(&m, tool_reply(calls));
    push(&m, text_reply("ok"));
    let e = editor();
    let mut t = Thread::new();
    run(&agent(&m, &e, |c| c.max_tools_per_reply = 5), &mut t, "q");
    assert_eq!(e.calls().len(), 5);
    assert_eq!(t.messages[2].parts.len(), 40, "every call is answered, even the ones not run");
    assert!(matches!(&t.messages[2].parts[39], Part::ToolResult { is_error: true, content, .. } if content.contains("more than 5")));
}

#[test]
fn a_refusal_runs_nothing_and_still_leaves_a_valid_thread() {
    let m = ScriptedModel::new();
    push(&m, reply(vec![Part::Text { text: "I can't".into() }, use_part("t1", "project_inspect", json!({}))], StopReason::Refusal));
    let e = editor();
    let mut t = Thread::new();
    let (r, _) = run(&agent(&m, &e, |_| {}), &mut t, "q");
    assert_eq!(r.finish, Finish::Refused);
    assert!(e.calls().is_empty());
    assert!(matches!(t.messages.last().and_then(|m| m.parts.first()), Some(Part::ToolResult { is_error: true, .. })));
}

#[test]
fn a_cut_off_reply_with_a_tool_call_does_not_run_it() {
    let m = ScriptedModel::new();
    push(&m, reply(vec![use_part("t1", "transcript", json!({}))], StopReason::MaxTokens));
    let e = editor();
    let mut t = Thread::new();
    let (r, _) = run(&agent(&m, &e, |_| {}), &mut t, "q");
    assert_eq!(r.finish, Finish::CutOff);
    assert!(e.calls().is_empty());
    // a plain cut-off with no tool call is also CutOff and needs no answer
    let m = ScriptedModel::new();
    push(&m, reply(vec![Part::Text { text: "long".into() }], StopReason::MaxTokens));
    let mut t2 = Thread::new();
    assert_eq!(run(&agent(&m, &e, |_| {}), &mut t2, "q").0.finish, Finish::CutOff);
    assert_eq!(t2.messages.len(), 2);
}

#[test]
fn the_step_limit_stops_a_looping_model_with_every_call_answered() {
    let m = ScriptedModel::new();
    for i in 0..10 {
        push(&m, tool_reply(vec![use_part(&format!("t{i}"), "project_inspect", json!({}))]));
    }
    let e = editor();
    let mut t = Thread::new();
    let (r, _) = run(&agent(&m, &e, |c| c.max_steps = 3), &mut t, "q");
    assert_eq!((r.finish, r.steps), (Finish::StepLimit, 3));
    assert_eq!(e.calls().len(), 3);
    assert_eq!(m.remaining(), 7);
    // the thread is still valid, so the person can simply ask again
    let m2 = ScriptedModel::new();
    push(&m2, text_reply("fine"));
    let (r2, _) = run(&agent(&m2, &e, |_| {}), &mut t, "carry on");
    assert_eq!(r2.finish, Finish::Done);
}

#[test]
fn stop_during_tools_answers_the_rest_and_finishes_stopped() {
    let c = cancel();
    let flag = c.clone();
    let e = FakeEnvironment::new()
        .tool("a", true, move |_| {
            flag.store(true, Ordering::SeqCst);
            ToolOutput::ok("a done")
        })
        .tool("b", true, |_| ToolOutput::ok("b done"));
    let m = ScriptedModel::new();
    push(&m, tool_reply(vec![use_part("t1", "a", json!({})), use_part("t2", "b", json!({}))]));
    let mut t = Thread::new();
    let (r, _) = run_with(&agent(&m, &e, |_| {}), &mut t, &[], "q", &c);
    assert_eq!(r.finish, Finish::Stopped);
    assert_eq!(e.calls().len(), 1, "the second tool must not run after Stop");
    assert!(matches!(&t.messages[2].parts[1], Part::ToolResult { is_error: true, content, .. } if content.contains("stopped")));
}

#[test]
fn stop_while_the_model_is_answering_is_stopped_not_failed() {
    let m = ScriptedModel::new();
    m.push_error(LlmError::Cancelled);
    let e = editor();
    let mut t = Thread::new();
    let (r, events) = run(&agent(&m, &e, |_| {}), &mut t, "q");
    assert_eq!(r.finish, Finish::Stopped);
    assert!(matches!(events.last(), Some(AgentEvent::Finished { finish: Finish::Stopped })));
    let c = cancel();
    c.store(true, Ordering::SeqCst);
    let (r, _) = run_with(&agent(&m, &e, |_| {}), &mut Thread::new(), &[], "q", &c);
    assert_eq!((r.finish, r.steps), (Finish::Stopped, 0), "a stop before the first request sends nothing");
}

#[test]
fn transient_failures_before_any_output_are_retried_quietly() {
    let m = ScriptedModel::new();
    m.push_error(LlmError::Overloaded);
    m.push_error(LlmError::RateLimited);
    push(&m, text_reply("finally"));
    let e = editor();
    let mut t = Thread::new();
    let (r, events) = run(&agent(&m, &e, |_| {}), &mut t, "q");
    assert_eq!(r.finish, Finish::Done);
    assert_eq!(m.requests().len(), 3);
    assert_eq!(events.iter().filter(|e| matches!(e, AgentEvent::Notice { .. })).count(), 2);
    assert!(events.contains(&AgentEvent::StepRestarted));
    assert_eq!(t.messages.len(), 2, "retries leave no trace in the thread");
}

#[test]
fn failures_that_should_not_be_retried_end_the_turn_with_a_clear_message() {
    // not transient
    let m = ScriptedModel::new();
    m.push_error(LlmError::Auth("the provider refused the key".into()));
    let e = editor();
    let (r, _) = run(&agent(&m, &e, |_| {}), &mut Thread::new(), "q");
    assert!(matches!(r.finish, Finish::Failed { retryable: false, ref message } if message.contains("refused")));
    assert_eq!(m.requests().len(), 1);
    // transient, but out of retries
    let m = ScriptedModel::new();
    for _ in 0..3 {
        m.push_error(LlmError::Overloaded);
    }
    let (r, _) = run(&agent(&m, &e, |_| {}), &mut Thread::new(), "q");
    assert!(matches!(r.finish, Finish::Failed { retryable: true, .. }));
    assert_eq!(m.requests().len(), 3);
    // transient, but text had already been shown: not asked again (it would duplicate the text)
    let m = ScriptedModel::new();
    m.push_partial(vec![StreamEvent::Text("half an ans".into())], LlmError::Overloaded);
    let (r, _) = run(&agent(&m, &e, |_| {}), &mut Thread::new(), "q");
    assert!(matches!(r.finish, Finish::Failed { retryable: true, .. }));
    assert_eq!(m.requests().len(), 1);
}

#[test]
fn budgets_end_a_turn_after_the_pending_calls_were_answered() {
    let m = ScriptedModel::new();
    for i in 0..5 {
        push(&m, tool_reply(vec![use_part(&format!("t{i}"), "project_inspect", json!({}))]));
    }
    let e = editor();
    let mut t = Thread::new();
    let (r, _) = run(&agent(&m, &e, |c| c.max_total_tokens = Some(40)), &mut t, "q");
    assert_eq!(r.finish, Finish::BudgetReached);
    assert_eq!(r.steps, 3, "15 tokens a reply: 45 >= 40 after the third");
    // a money budget, using the model's price
    let m = ScriptedModel::new();
    for i in 0..5 {
        let mut resp = tool_reply(vec![use_part(&format!("t{i}"), "project_inspect", json!({}))]);
        resp.usage = usage(1_000_000, 0);
        push(&m, resp);
    }
    let price = edith_llm::Price { input_per_mtok: 1.0, output_per_mtok: 1.0, cache_read_per_mtok: None, source: "test".into() };
    let mut t = Thread::new();
    let (r, _) = run(
        &agent(&m, &e, |c| {
            c.max_cost_usd = Some(2.5);
            c.price = Some(price);
        }),
        &mut t,
        "q",
    );
    assert_eq!((r.finish, r.steps), (Finish::BudgetReached, 3));
}

#[test]
fn big_tool_results_are_cut_before_they_reach_the_model() {
    let e = FakeEnvironment::new().tool("huge", true, |_| ToolOutput::ok("x".repeat(100_000)));
    let m = ScriptedModel::new();
    push(&m, tool_reply(vec![use_part("t1", "huge", json!({}))]));
    push(&m, text_reply("ok"));
    let mut t = Thread::new();
    run(&agent(&m, &e, |c| c.max_tool_result_chars = 1000), &mut t, "q");
    let Part::ToolResult { content, .. } = &t.messages[2].parts[0] else { panic!() };
    assert!(content.len() < 1100 && content.contains("99000 more characters"), "{}", content.len());
}

#[test]
fn context_goes_in_the_user_message_so_the_cached_prompt_never_changes() {
    let e = editor();
    let ctx = [ContextBlock { label: "selection".into(), text: "clip A, 0:42 to 0:47".into() }];
    let run_once = |ctx: &[ContextBlock]| {
        let m = ScriptedModel::new();
        push(&m, text_reply("a"));
        run_with(&agent(&m, &e, |_| {}), &mut Thread::new(), ctx, "why?", &cancel());
        m.requests()[0].clone()
    };
    let with = run_once(&ctx);
    let without = run_once(&[]);
    assert_eq!(with.system, without.system, "different context, identical system prompt");
    assert!(with.messages[0].text().contains("[selection]") && with.messages[0].text().contains("0:42"));
    assert!(!with.system[0].text.contains("0:42"));
}

#[test]
fn a_multi_turn_session_stays_valid_after_every_kind_of_ending() {
    let e = editor();
    let m = ScriptedModel::new();
    let mut t = Thread::new();
    // turn 1: tools then an answer
    push(&m, tool_reply(vec![use_part("a1", "project_inspect", json!({}))]));
    push(&m, text_reply("one"));
    run(&agent(&m, &e, |_| {}), &mut t, "first");
    // turn 2: a refusal with a dangling call
    push(&m, reply(vec![use_part("b1", "transcript", json!({}))], StopReason::Refusal));
    run(&agent(&m, &e, |_| {}), &mut t, "second");
    // turn 3: step limit
    push(&m, tool_reply(vec![use_part("c1", "project_inspect", json!({}))]));
    run(&agent(&m, &e, |c| c.max_steps = 1), &mut t, "third");
    // turn 4: a normal answer after all that
    push(&m, text_reply("four"));
    let (r, _) = run(&agent(&m, &e, |_| {}), &mut t, "fourth");
    assert_eq!(r.finish, Finish::Done);
    let last = m.requests().pop().unwrap();
    assert_eq!(last.messages.len(), t.messages.len() - 1, "the whole history is sent each time");
}

#[test]
fn a_trace_round_trips_and_replays_to_the_same_thread() {
    let make_model = || {
        let m = ScriptedModel::new();
        push(
            &m,
            tool_reply(vec![
                Part::Opaque { provider: "anthropic".into(), value: json!({"type": "thinking", "thinking": "t", "signature": "s"}) },
                use_part("t1", "transcript", json!({"from": 7})),
            ]),
        );
        push(&m, text_reply("The cut at 0:07 lands mid-word."));
        m
    };
    let e = editor();
    let m = make_model();
    let a = agent(&m, &e, |_| {});
    let mut t1 = Thread::new();
    let mut trace = Trace::new(TraceMeta { prompt_version: a.pack.version.clone(), model: "claude-test".into(), profile: Profile::ReadOnly, started_unix: 0 });
    let report = a.run_turn(&mut t1, &[], "why abrupt?", &cancel(), &mut |ev| trace.record(&ev));
    trace.finish(&report);
    assert_valid(&t1);

    let text = trace.to_jsonl();
    assert!(text.lines().count() > 5 && text.lines().next().is_some_and(|l| l.contains("\"line\":\"meta\"")));
    let back = Trace::from_jsonl(&text).unwrap();
    assert_eq!(back, trace);

    let e2 = editor();
    let replay = back.replay_model();
    let a2 = agent(&replay, &e2, |_| {});
    let mut t2 = Thread::new();
    let r2 = a2.run_turn(&mut t2, &[], "why abrupt?", &cancel(), &mut |_| {});
    assert_eq!(t2, t1, "replaying the recorded replies reproduces the thread exactly");
    assert_eq!(r2.finish, Finish::Done);
    assert_eq!(e2.calls(), e.calls());
}

#[test]
fn damaged_traces_are_errors_that_never_echo_their_content() {
    assert!(Trace::from_jsonl("").is_err());
    let e = Trace::from_jsonl("{\"line\":\"meta\",\"prompt_version\":\"1\",\"model\":\"m\",\"profile\":\"read_only\",\"started_unix\":0}\nSECRET not json\n")
        .unwrap_err();
    assert!(e.contains("line 2") && !e.contains("SECRET"), "{e}");
}

#[test]
fn repair_keeps_a_valid_thread_whole_and_cuts_a_broken_one_back_to_its_valid_part() {
    // a real thread, ended in several ways, is already valid
    let m = ScriptedModel::new();
    push(&m, tool_reply(vec![use_part("a1", "project_inspect", json!({})), use_part("a2", "transcript", json!({}))]));
    push(&m, text_reply("one"));
    let e = editor();
    let mut t = Thread::new();
    run(&agent(&m, &e, |_| {}), &mut t, "first");
    let before = t.clone();
    assert_eq!(t.repair(), 0);
    assert_eq!(t, before);

    let user = |s: &str| Message::user_text(s);
    let call = |id: &str| Message { role: Role::Assistant, parts: vec![use_part(id, "project_inspect", json!({}))] };
    let answer = |ids: &[&str]| {
        Message::tool_results(ids.iter().map(|i| Part::ToolResult { tool_use_id: (*i).into(), content: "ok".into(), is_error: false }).collect())
    };
    let broken: Vec<(Vec<Message>, usize)> = vec![
        // a call that was never answered: cut back to before it (a trailing question is fine)
        (vec![user("q"), Message::assistant_text("a"), user("q2"), call("x")], 3),
        // answered by the wrong call
        (vec![user("q"), call("x"), answer(&["y"])], 1),
        // a partial answer
        (vec![user("q"), Message { role: Role::Assistant, parts: vec![use_part("x", "n", json!({})), use_part("y", "n", json!({}))] }, answer(&["x"])], 1),
        // an answer with no call at all
        (vec![user("q"), Message::assistant_text("a"), answer(&["ghost"])], 2),
        // answers in the wrong order
        (vec![user("q"), Message { role: Role::Assistant, parts: vec![use_part("x", "n", json!({})), use_part("y", "n", json!({}))] }, answer(&["y", "x"])], 1),
        // a call inside a user message
        (vec![user("q"), Message { role: Role::User, parts: vec![use_part("x", "n", json!({}))] }], 1),
        // an assistant message that is also an answer
        (
            vec![user("q"), Message { role: Role::Assistant, parts: vec![Part::ToolResult { tool_use_id: "x".into(), content: "c".into(), is_error: false }] }],
            1,
        ),
    ];
    for (messages, keep) in broken {
        let mut t = Thread { messages: messages.clone(), usage: Usage::default() };
        let dropped = t.repair();
        assert_eq!(t.messages.len(), keep, "{messages:?}");
        assert_eq!(dropped, messages.len() - keep);
        assert_valid(&t);
        assert_eq!(t.repair(), 0, "repair is idempotent");
    }
    let mut empty = Thread::new();
    assert_eq!(empty.repair(), 0);
}
