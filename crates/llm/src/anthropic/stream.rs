//! Turning Anthropic's server-sent events into one [`Response`].
//!
//! Feed it the lines of the stream one at a time ([`StreamAssembler::feed_line`]); it tells the
//! caller what arrived through the sink and, when the stream ends, [`StreamAssembler::finish`]
//! returns the whole assistant message. It is plain code with no network: recorded streams replay
//! through it in tests, and it treats the stream as hostile (bounded blocks, bounded tool input,
//! no panics on garbage).

use super::PROVIDER;
use crate::{LlmError, Message, Part, Response, Role, StopReason, StreamEvent, Usage};
use serde_json::{Value, json};

/// Most content blocks in one reply.
const MAX_BLOCKS: usize = 256;
/// Most bytes of one tool call's input.
const MAX_TOOL_INPUT: usize = 4 * 1024 * 1024;
/// Most text in one reply (characters are bytes here; the transport caps the stream too).
const MAX_TEXT: usize = 8 * 1024 * 1024;

enum Block {
    Text(String),
    Thinking {
        text: String,
        signature: String,
    },
    Tool {
        id: String,
        name: String,
        json: String,
    },
    /// A block this version does not interpret (redacted thinking, server tools): kept whole.
    Other(Value),
}

#[derive(Default)]
pub struct StreamAssembler {
    model: String,
    usage: Usage,
    stop_reason: Option<StopReason>,
    blocks: Vec<Option<Block>>,
    parts: Vec<Part>,
    text_bytes: usize,
    started: bool,
    finished: bool,
}

fn stop_reason(s: &str) -> StopReason {
    match s {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        "pause_turn" => StopReason::PauseTurn,
        "refusal" => StopReason::Refusal,
        other => StopReason::Other(other.chars().take(60).collect()),
    }
}

fn u64_of(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

impl StreamAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// True once `message_stop` has been seen.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    fn merge_usage(&mut self, u: &Value) {
        if let Some(n) = u64_of(u, "input_tokens") {
            self.usage.input_tokens = n;
        }
        if let Some(n) = u64_of(u, "output_tokens") {
            self.usage.output_tokens = n;
        }
        if let Some(n) = u64_of(u, "cache_read_input_tokens") {
            self.usage.cache_read_tokens = n;
        }
        if let Some(n) = u64_of(u, "cache_creation_input_tokens") {
            self.usage.cache_write_tokens = n;
        }
    }

    fn index(v: &Value) -> Result<usize, LlmError> {
        let i = v.get("index").and_then(Value::as_u64).ok_or_else(|| LlmError::bad_stream("a block event had no index"))?;
        let i = usize::try_from(i).unwrap_or(usize::MAX);
        if i >= MAX_BLOCKS { Err(LlmError::bad_stream("the reply has too many parts")) } else { Ok(i) }
    }

    /// Handles one line of the stream. Lines that are not `data:` lines (`event:`, comments, blank
    /// lines) are ignored: the `type` inside the data says everything.
    pub fn feed_line(&mut self, line: &str, sink: &mut dyn FnMut(StreamEvent)) -> Result<(), LlmError> {
        let Some(data) = line.strip_prefix("data:") else { return Ok(()) };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return Ok(());
        }
        let v: Value = serde_json::from_str(data).map_err(|_| LlmError::bad_stream("a stream line was not valid JSON"))?;
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "message_start" => {
                self.started = true;
                if let Some(m) = v.get("message") {
                    if let Some(model) = m.get("model").and_then(Value::as_str) {
                        self.model = model.chars().take(100).collect();
                    }
                    if let Some(u) = m.get("usage") {
                        self.merge_usage(u);
                        sink(StreamEvent::Usage(self.usage));
                    }
                }
            }
            "content_block_start" => {
                let i = Self::index(&v)?;
                let cb = v.get("content_block").ok_or_else(|| LlmError::bad_stream("a block start had no block"))?;
                let block = match cb.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => Block::Text(String::new()),
                    "thinking" => Block::Thinking { text: String::new(), signature: String::new() },
                    "tool_use" => {
                        let id = cb.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                        let name = cb.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                        if id.is_empty() || name.is_empty() {
                            return Err(LlmError::bad_stream("a tool call had no id or name"));
                        }
                        sink(StreamEvent::ToolUseStarted { id: id.clone(), name: name.clone() });
                        Block::Tool { id, name, json: String::new() }
                    }
                    _ => Block::Other(cb.clone()),
                };
                if self.blocks.len() <= i {
                    self.blocks.resize_with(i + 1, || None);
                }
                if let Some(slot) = self.blocks.get_mut(i) {
                    *slot = Some(block);
                }
            }
            "content_block_delta" => {
                let i = Self::index(&v)?;
                let delta = v.get("delta").ok_or_else(|| LlmError::bad_stream("a block delta had no delta"))?;
                let Some(Some(block)) = self.blocks.get_mut(i) else { return Err(LlmError::bad_stream("a delta arrived for a block that was never started")) };
                match (delta.get("type").and_then(Value::as_str).unwrap_or(""), block) {
                    ("text_delta", Block::Text(t)) => {
                        let piece = delta.get("text").and_then(Value::as_str).unwrap_or("");
                        self.text_bytes = self.text_bytes.saturating_add(piece.len());
                        if self.text_bytes > MAX_TEXT {
                            return Err(LlmError::bad_stream("the reply is longer than the allowed size"));
                        }
                        t.push_str(piece);
                        if !piece.is_empty() {
                            sink(StreamEvent::Text(piece.to_string()));
                        }
                    }
                    ("thinking_delta", Block::Thinking { text, .. }) => {
                        let piece = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
                        self.text_bytes = self.text_bytes.saturating_add(piece.len());
                        if self.text_bytes > MAX_TEXT {
                            return Err(LlmError::bad_stream("the reply is longer than the allowed size"));
                        }
                        text.push_str(piece);
                        if !piece.is_empty() {
                            sink(StreamEvent::Thinking(piece.to_string()));
                        }
                    }
                    ("signature_delta", Block::Thinking { signature, .. }) => {
                        signature.push_str(delta.get("signature").and_then(Value::as_str).unwrap_or(""));
                    }
                    ("input_json_delta", Block::Tool { json, .. }) => {
                        let piece = delta.get("partial_json").and_then(Value::as_str).unwrap_or("");
                        if json.len().saturating_add(piece.len()) > MAX_TOOL_INPUT {
                            return Err(LlmError::bad_stream("a tool call's input is larger than the allowed size"));
                        }
                        json.push_str(piece);
                    }
                    // Citations and anything newer: not interpreted.
                    _ => {}
                }
            }
            "content_block_stop" => {
                let i = Self::index(&v)?;
                let Some(block) = self.blocks.get_mut(i).and_then(Option::take) else {
                    return Err(LlmError::bad_stream("a block ended that was never started"));
                };
                match block {
                    Block::Text(t) => {
                        // The API rejects empty text blocks when they are sent back.
                        if !t.is_empty() {
                            self.parts.push(Part::Text { text: t });
                        }
                    }
                    Block::Thinking { text, signature } => {
                        self.parts.push(Part::Opaque {
                            provider: PROVIDER.to_string(),
                            value: json!({"type": "thinking", "thinking": text, "signature": signature}),
                        });
                    }
                    Block::Tool { id, name, json } => {
                        let parsed: Result<Value, _> = if json.trim().is_empty() { Ok(json!({})) } else { serde_json::from_str(&json) };
                        let (input, input_error) = match parsed {
                            Ok(v) if v.is_object() => (v, None),
                            Ok(_) => (json!({}), Some("the tool input was not a JSON object".to_string())),
                            Err(_) => (json!({}), Some("the tool input was not valid JSON".to_string())),
                        };
                        sink(StreamEvent::ToolUseFinished { id: id.clone() });
                        self.parts.push(Part::ToolUse { id, name, input, input_error });
                    }
                    Block::Other(value) => self.parts.push(Part::Opaque { provider: PROVIDER.to_string(), value }),
                }
            }
            "message_delta" => {
                if let Some(d) = v.get("delta")
                    && let Some(s) = d.get("stop_reason").and_then(Value::as_str)
                {
                    self.stop_reason = Some(stop_reason(s));
                }
                if let Some(u) = v.get("usage") {
                    self.merge_usage(u);
                    sink(StreamEvent::Usage(self.usage));
                }
            }
            "message_stop" => self.finished = true,
            "ping" => {}
            "error" => {
                let e = v.get("error").cloned().unwrap_or(Value::Null);
                let kind = e.get("type").and_then(Value::as_str).unwrap_or("");
                let message = e.get("message").and_then(Value::as_str).unwrap_or("");
                return Err(crate::error::error_for(0, kind, message));
            }
            // Event types added later: ignored, as the API's versioning promises is safe.
            _ => {}
        }
        Ok(())
    }

    /// The whole reply, once the stream has ended. An early end is an error: nothing half-finished
    /// is ever returned as if it were complete.
    pub fn finish(self) -> Result<Response, LlmError> {
        if !self.started || !self.finished {
            return Err(LlmError::bad_stream("the stream ended before the reply was complete"));
        }
        if self.blocks.iter().any(Option::is_some) {
            return Err(LlmError::bad_stream("the stream ended inside a part of the reply"));
        }
        let stop_reason = self.stop_reason.ok_or_else(|| LlmError::bad_stream("the reply never said why it stopped"))?;
        Ok(Response { message: Message { role: Role::Assistant, parts: self.parts }, stop_reason, usage: self.usage, model: self.model })
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Recorded-style streams, as the API sends them (an `event:` line, a `data:` line, a blank line).

    pub fn ev(kind: &str, data: &str) -> String {
        format!("event: {kind}\ndata: {data}\n\n")
    }

    pub fn start(input: u64) -> String {
        ev(
            "message_start",
            &format!(
                r#"{{"type":"message_start","message":{{"id":"msg_1","type":"message","role":"assistant","model":"claude-haiku-5-5","content":[],"usage":{{"input_tokens":{input},"cache_read_input_tokens":7,"cache_creation_input_tokens":3,"output_tokens":1}}}}}}"#
            ),
        )
    }

    pub fn text_block(i: u32, pieces: &[&str]) -> String {
        let mut s = ev("content_block_start", &format!(r#"{{"type":"content_block_start","index":{i},"content_block":{{"type":"text","text":""}}}}"#));
        for p in pieces {
            s += &ev(
                "content_block_delta",
                &format!(
                    r#"{{"type":"content_block_delta","index":{i},"delta":{{"type":"text_delta","text":{}}}}}"#,
                    serde_json::to_string(p).unwrap_or_default()
                ),
            );
        }
        s + &ev("content_block_stop", &format!(r#"{{"type":"content_block_stop","index":{i}}}"#))
    }

    pub fn tool_block(i: u32, id: &str, name: &str, json_pieces: &[&str]) -> String {
        let mut s = ev(
            "content_block_start",
            &format!(r#"{{"type":"content_block_start","index":{i},"content_block":{{"type":"tool_use","id":"{id}","name":"{name}","input":{{}}}}}}"#),
        );
        for p in json_pieces {
            s += &ev(
                "content_block_delta",
                &format!(
                    r#"{{"type":"content_block_delta","index":{i},"delta":{{"type":"input_json_delta","partial_json":{}}}}}"#,
                    serde_json::to_string(p).unwrap_or_default()
                ),
            );
        }
        s + &ev("content_block_stop", &format!(r#"{{"type":"content_block_stop","index":{i}}}"#))
    }

    pub fn thinking_block(i: u32, text: &str, sig: &str) -> String {
        let mut s = ev(
            "content_block_start",
            &format!(r#"{{"type":"content_block_start","index":{i},"content_block":{{"type":"thinking","thinking":"","signature":""}}}}"#),
        );
        s += &ev(
            "content_block_delta",
            &format!(
                r#"{{"type":"content_block_delta","index":{i},"delta":{{"type":"thinking_delta","thinking":{}}}}}"#,
                serde_json::to_string(text).unwrap_or_default()
            ),
        );
        s += &ev("content_block_delta", &format!(r#"{{"type":"content_block_delta","index":{i},"delta":{{"type":"signature_delta","signature":"{sig}"}}}}"#));
        s + &ev("content_block_stop", &format!(r#"{{"type":"content_block_stop","index":{i}}}"#))
    }

    pub fn end(reason: &str, out: u64) -> String {
        ev(
            "message_delta",
            &format!(r#"{{"type":"message_delta","delta":{{"stop_reason":"{reason}","stop_sequence":null}},"usage":{{"output_tokens":{out}}}}}"#),
        ) + &ev("message_stop", r#"{"type":"message_stop"}"#)
    }

    pub fn ping() -> String {
        ev("ping", r#"{"type":"ping"}"#)
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    /// Feeds a whole recorded stream; returns the events seen and the result.
    pub(super) fn run(stream: &str) -> (Vec<StreamEvent>, Result<Response, LlmError>) {
        let mut a = StreamAssembler::new();
        let mut events = Vec::new();
        for line in stream.lines() {
            if let Err(e) = a.feed_line(line, &mut |e| events.push(e)) {
                return (events, Err(e));
            }
        }
        (events, a.finish())
    }

    #[test]
    fn plain_text_streams_in_pieces_and_assembles() {
        let s = start(40) + &ping() + &text_block(0, &["Hel", "lo ", "there."]) + &end("end_turn", 9);
        let (events, r) = run(&s);
        let r = r.unwrap();
        assert_eq!(r.message.text(), "Hello there.");
        assert_eq!(r.stop_reason, StopReason::EndTurn);
        assert_eq!(r.model, "claude-haiku-5-5");
        assert_eq!(r.usage, Usage { input_tokens: 40, output_tokens: 9, cache_read_tokens: 7, cache_write_tokens: 3 });
        let texts: Vec<_> = events.iter().filter_map(|e| if let StreamEvent::Text(t) = e { Some(t.as_str()) } else { None }).collect();
        assert_eq!(texts, ["Hel", "lo ", "there."]);
    }

    #[test]
    fn thinking_is_kept_whole_for_the_next_request_and_shown_as_a_summary() {
        let s = start(10) + &thinking_block(0, "The cut jumps.", "SIG123") + &text_block(1, &["It does."]) + &end("end_turn", 20);
        let (events, r) = run(&s);
        let r = r.unwrap();
        assert_eq!(
            r.message.parts[0],
            Part::Opaque { provider: "anthropic".into(), value: json!({"type": "thinking", "thinking": "The cut jumps.", "signature": "SIG123"}) }
        );
        assert_eq!(r.message.text(), "It does.");
        assert!(events.contains(&StreamEvent::Thinking("The cut jumps.".into())));
    }

    #[test]
    fn a_tool_call_arriving_in_pieces_becomes_one_valid_call() {
        let s = start(5)
            + &text_block(0, &["Looking."])
            + &tool_block(1, "toolu_a", "project_inspect", &["{\"sequen", "ce\":\"Main", " Edit\"}"])
            + &end("tool_use", 30);
        let (events, r) = run(&s);
        let r = r.unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        assert_eq!(
            r.message.parts[1],
            Part::ToolUse { id: "toolu_a".into(), name: "project_inspect".into(), input: json!({"sequence": "Main Edit"}), input_error: None }
        );
        assert!(events.contains(&StreamEvent::ToolUseStarted { id: "toolu_a".into(), name: "project_inspect".into() }));
        assert!(events.contains(&StreamEvent::ToolUseFinished { id: "toolu_a".into() }));
    }

    #[test]
    fn parallel_tool_calls_keep_their_order_and_ids() {
        let s = start(5) + &tool_block(0, "toolu_1", "markers", &["{}"]) + &tool_block(1, "toolu_2", "transcript", &["{\"from\":1}"]) + &end("tool_use", 12);
        let r = run(&s).1.unwrap();
        let ids: Vec<_> = r.message.tool_uses().filter_map(|p| if let Part::ToolUse { id, .. } = p { Some(id.as_str()) } else { None }).collect();
        assert_eq!(ids, ["toolu_1", "toolu_2"]);
    }

    #[test]
    fn a_tool_call_with_no_input_is_an_empty_object() {
        let s = start(5) + &tool_block(0, "toolu_1", "markers", &[]) + &end("tool_use", 4);
        let r = run(&s).1.unwrap();
        assert!(matches!(&r.message.parts[0], Part::ToolUse { input, input_error: None, .. } if *input == json!({})));
    }

    #[test]
    fn broken_tool_input_is_marked_not_run_and_not_fatal() {
        for bad in [&["{\"a\": "][..], &["[1,2]"], &["\"str\""], &["{\"a\":1}}"], &["nul"]] {
            let s = start(5) + &tool_block(0, "toolu_1", "markers", bad) + &end("tool_use", 4);
            let r = run(&s).1.unwrap();
            assert!(matches!(&r.message.parts[0], Part::ToolUse { input, input_error: Some(_), .. } if *input == json!({})), "{bad:?}");
        }
    }

    #[test]
    fn a_refusal_and_a_cut_off_reply_are_reported_by_stop_reason() {
        let r = run(&(start(5) + &text_block(0, &["I can't"]) + &end("refusal", 3))).1.unwrap();
        assert_eq!(r.stop_reason, StopReason::Refusal);
        let r = run(&(start(5) + &tool_block(0, "t", "n", &["{\"a\":"]) + &end("max_tokens", 3))).1.unwrap();
        assert_eq!(r.stop_reason, StopReason::MaxTokens);
        let r = run(&(start(5) + &end("something_new", 3))).1.unwrap();
        assert_eq!(r.stop_reason, StopReason::Other("something_new".into()));
    }

    #[test]
    fn an_error_event_mid_stream_stops_the_reply_with_a_mapped_error() {
        let s = start(5) + &text_block(0, &["part"]) + &ev("error", r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#);
        let (events, r) = run(&s);
        assert_eq!(r.unwrap_err(), LlmError::Overloaded);
        assert!(events.contains(&StreamEvent::Text("part".into())), "what arrived before the error was still shown");
        let s = start(5) + &ev("error", r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#);
        assert!(matches!(run(&s).1, Err(LlmError::Auth(_))));
    }

    #[test]
    fn a_stream_that_ends_early_is_an_error_not_a_short_reply() {
        let whole = start(5) + &text_block(0, &["Hello"]) + &end("end_turn", 2);
        // every strict prefix that cuts the stream is refused
        let lines: Vec<&str> = whole.lines().collect();
        for n in 0..lines.len() {
            let cut = lines[..n].join("\n");
            let (_, r) = run(&cut);
            if n < lines.len() - 1 {
                assert!(r.is_err(), "accepted a stream cut after {n} lines");
            }
        }
        // ends inside a block
        let s = start(5)
            + &ev("content_block_start", r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#)
            + &end("end_turn", 1);
        assert!(run(&s).1.is_err());
        // no stop reason
        let s = start(5) + &ev("message_stop", r#"{"type":"message_stop"}"#);
        assert!(run(&s).1.is_err());
    }

    #[test]
    fn garbage_is_an_error_never_a_panic() {
        for line in [
            "data: not json",
            "data: {\"type\":",
            "data: {\"type\":\"content_block_delta\"}",
            "data: {\"type\":\"content_block_delta\",\"index\":5,\"delta\":{}}",
            "data: {\"type\":\"content_block_start\",\"index\":999999999999,\"content_block\":{}}",
            "data: {\"type\":\"content_block_stop\",\"index\":0}",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\"}}",
        ] {
            let mut a = StreamAssembler::new();
            assert!(a.feed_line(line, &mut |_| {}).is_err(), "{line}");
        }
        // harmless lines
        let mut a = StreamAssembler::new();
        for line in ["", ": comment", "event: ping", "data:", "data: [DONE]", "data: {\"type\":\"future_event\"}", "retry: 1000", "data: {\"no_type\":1}"] {
            assert!(a.feed_line(line, &mut |_| {}).is_ok(), "{line}");
        }
    }

    #[test]
    fn oversized_tool_input_and_too_many_blocks_are_refused() {
        let mut a = StreamAssembler::new();
        a.feed_line(r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"n","input":{}}}"#, &mut |_| {})
            .unwrap();
        let big = format!(
            r#"data: {{"type":"content_block_delta","index":0,"delta":{{"type":"input_json_delta","partial_json":"{}"}}}}"#,
            "x".repeat(MAX_TOOL_INPUT / 2 + 10)
        );
        a.feed_line(&big, &mut |_| {}).unwrap();
        assert!(a.feed_line(&big, &mut |_| {}).is_err());
        let mut a = StreamAssembler::new();
        assert!(
            a.feed_line(&format!(r#"data: {{"type":"content_block_start","index":{MAX_BLOCKS},"content_block":{{"type":"text","text":""}}}}"#), &mut |_| {})
                .is_err()
        );
    }

    #[test]
    fn unknown_blocks_are_kept_and_echoed_whole() {
        let s = start(5)
            + &ev("content_block_start", r#"{"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"ENC"}}"#)
            + &ev("content_block_stop", r#"{"type":"content_block_stop","index":0}"#)
            + &text_block(1, &["ok"])
            + &end("end_turn", 2);
        let r = run(&s).1.unwrap();
        assert_eq!(r.message.parts[0], Part::Opaque { provider: "anthropic".into(), value: json!({"type": "redacted_thinking", "data": "ENC"}) });
    }

    #[test]
    fn empty_text_blocks_are_dropped_so_the_next_request_is_valid() {
        let s = start(5) + &text_block(0, &[]) + &text_block(1, &["x"]) + &end("end_turn", 2);
        let r = run(&s).1.unwrap();
        assert_eq!(r.message.parts.len(), 1);
    }
}
