//! Turning the server-sent `chat.completion.chunk` events into one [`Response`].
//!
//! Feed it the lines of the stream one at a time ([`StreamAssembler::feed_line`]); the sink hears
//! what arrived, and [`StreamAssembler::finish`] returns the whole assistant message. Plain code,
//! no network; it treats the stream as hostile (bounded calls, bounded input, no panics).
//!
//! Servers differ at the edges, and this handles what was seen: tool-call fragments joined by
//! `index` (some servers omit it, or send a whole call in one chunk); `finish_reason: "stop"`
//! alongside tool calls (Ollama and LM Studio); usage in a last chunk that has no choices
//! (OpenAI, Ollama) or one with an empty delta (OpenRouter); comment lines such as
//! `: OPENROUTER PROCESSING`; a top-level `error` object in the middle of a 200 stream.

use crate::{LlmError, Message, Part, Response, Role, StopReason, StreamEvent, Usage};
use serde_json::{Value, json};

/// Most tool calls in one reply.
const MAX_CALLS: usize = 64;
/// Most bytes of one tool call's arguments.
const MAX_TOOL_INPUT: usize = 1024 * 1024;
/// Most text in one reply.
const MAX_TEXT: usize = 8 * 1024 * 1024;

struct Call {
    index: Option<u64>,
    id: String,
    name: String,
    args: String,
}

#[derive(Default)]
pub struct StreamAssembler {
    model: String,
    usage: Usage,
    text: String,
    text_bytes: usize,
    calls: Vec<Call>,
    finish_reason: Option<String>,
    chunks: u32,
    done: bool,
}

fn u64_of(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

impl StreamAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// True once the reply is complete: a finish reason has arrived, or `[DONE]` after content.
    pub fn is_finished(&self) -> bool {
        self.finish_reason.is_some() || (self.done && self.chunks > 0)
    }

    fn read_usage(&mut self, u: &Value) {
        let prompt = u64_of(u, "prompt_tokens").or_else(|| u64_of(u, "input_tokens"));
        let completion = u64_of(u, "completion_tokens").or_else(|| u64_of(u, "output_tokens"));
        let details = u.get("prompt_tokens_details").or_else(|| u.get("input_tokens_details")).unwrap_or(&Value::Null);
        let cached = u64_of(details, "cached_tokens").unwrap_or(0);
        let written = u64_of(details, "cache_write_tokens").unwrap_or(0);
        if let Some(p) = prompt {
            // Cached tokens are part of the prompt count here; the neutral usage counts them apart.
            self.usage.input_tokens = p.saturating_sub(cached).saturating_sub(written);
            self.usage.cache_read_tokens = cached;
            self.usage.cache_write_tokens = written;
        }
        if let Some(c) = completion {
            self.usage.output_tokens = c;
        }
    }

    /// The call a fragment belongs to, creating it when it is new. `None` when there are too many.
    fn call_for(&mut self, item: &Value, position: usize, sink: &mut dyn FnMut(StreamEvent)) -> Result<usize, LlmError> {
        let index = u64_of(item, "index");
        let id = item.get("id").and_then(Value::as_str).filter(|s| !s.is_empty());
        let found = match (index, id) {
            (Some(i), _) => self.calls.iter().position(|c| c.index == Some(i)),
            (None, Some(id)) => self.calls.iter().position(|c| c.id == id),
            // No index and no id: a continuation of the latest call.
            (None, None) => self.calls.len().checked_sub(1),
        };
        if let Some(i) = found {
            return Ok(i);
        }
        if self.calls.len() >= MAX_CALLS {
            return Err(LlmError::bad_stream("the reply has too many tool calls"));
        }
        let ordinal = self.calls.len().saturating_add(position);
        let id = id.map(|s| s.chars().take(100).collect::<String>()).unwrap_or_else(|| format!("call_{ordinal}"));
        let name: String = item.get("function").and_then(|f| f.get("name")).and_then(Value::as_str).unwrap_or("").chars().take(100).collect();
        if !name.is_empty() {
            sink(StreamEvent::ToolUseStarted { id: id.clone(), name: name.clone() });
        }
        self.calls.push(Call { index, id, name, args: String::new() });
        Ok(self.calls.len() - 1)
    }

    fn add_text(&mut self, piece: &str) -> Result<(), LlmError> {
        self.text_bytes = self.text_bytes.saturating_add(piece.len());
        if self.text_bytes > MAX_TEXT {
            return Err(LlmError::bad_stream("the reply is longer than the allowed size"));
        }
        Ok(())
    }

    fn read_delta(&mut self, delta: &Value, sink: &mut dyn FnMut(StreamEvent)) -> Result<(), LlmError> {
        if let Some(t) = delta.get("content").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            self.add_text(t)?;
            self.text.push_str(t);
            sink(StreamEvent::Text(t.to_string()));
        }
        // Reasoning text: a plain string (`reasoning`, or `reasoning_content` on some servers), or
        // OpenRouter's `reasoning_details` list when there is no plain string.
        let plain = ["reasoning", "reasoning_content"].iter().find_map(|k| delta.get(*k).and_then(Value::as_str)).filter(|t| !t.is_empty());
        if let Some(t) = plain {
            self.add_text(t)?;
            sink(StreamEvent::Thinking(t.to_string()));
        } else if let Some(items) = delta.get("reasoning_details").and_then(Value::as_array) {
            for item in items.iter().take(32) {
                if let Some(t) = ["text", "summary"].iter().find_map(|k| item.get(*k).and_then(Value::as_str)).filter(|t| !t.is_empty()) {
                    self.add_text(t)?;
                    sink(StreamEvent::Thinking(t.to_string()));
                }
            }
        }
        if let Some(items) = delta.get("tool_calls").and_then(Value::as_array) {
            for (position, item) in items.iter().take(MAX_CALLS).enumerate() {
                let i = self.call_for(item, position, sink)?;
                let Some(call) = self.calls.get_mut(i) else { continue };
                let f = item.get("function").unwrap_or(&Value::Null);
                if call.name.is_empty()
                    && let Some(n) = f.get("name").and_then(Value::as_str).filter(|n| !n.is_empty())
                {
                    call.name = n.chars().take(100).collect();
                    sink(StreamEvent::ToolUseStarted { id: call.id.clone(), name: call.name.clone() });
                }
                // Arguments are a string in pieces; a few servers send an already-parsed object.
                let piece = match f.get("arguments") {
                    Some(Value::String(s)) => s.clone(),
                    Some(v @ Value::Object(_)) => v.to_string(),
                    _ => String::new(),
                };
                if call.args.len().saturating_add(piece.len()) > MAX_TOOL_INPUT {
                    return Err(LlmError::bad_stream("a tool call's input is larger than the allowed size"));
                }
                call.args.push_str(&piece);
            }
        }
        Ok(())
    }

    /// Handles one line of the stream. Anything that is not a `data:` line (comments, `event:`,
    /// blank lines) is ignored.
    pub fn feed_line(&mut self, line: &str, sink: &mut dyn FnMut(StreamEvent)) -> Result<(), LlmError> {
        let Some(data) = line.strip_prefix("data:") else { return Ok(()) };
        let data = data.trim();
        if data.is_empty() {
            return Ok(());
        }
        if data == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let v: Value = serde_json::from_str(data).map_err(|_| LlmError::bad_stream("a stream line was not valid JSON"))?;
        if !v.is_object() {
            return Err(LlmError::bad_stream("a stream line was not an object"));
        }
        // An error in the middle of a 200 stream. The code is a number or a word.
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
            let code = e.get("code");
            let status = code.and_then(Value::as_u64).and_then(|n| u16::try_from(n).ok()).unwrap_or(0);
            let kind = e.get("type").and_then(Value::as_str).or_else(|| code.and_then(Value::as_str)).unwrap_or("");
            let message = e.get("message").and_then(Value::as_str).or_else(|| e.as_str()).unwrap_or("");
            return Err(crate::error::error_for(status, kind, message));
        }
        self.chunks = self.chunks.saturating_add(1);
        if self.model.is_empty()
            && let Some(m) = v.get("model").and_then(Value::as_str)
        {
            self.model = m.chars().take(100).collect();
        }
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            self.read_usage(u);
            sink(StreamEvent::Usage(self.usage));
        }
        let Some(choice) = v.get("choices").and_then(Value::as_array).and_then(|c| c.first()) else { return Ok(()) };
        if let Some(d) = choice.get("delta").filter(|d| d.is_object()) {
            self.read_delta(d, sink)?;
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str).filter(|r| !r.is_empty()) {
            if reason == "error" {
                return Err(LlmError::Api { status: 0, message: "the provider stopped the reply with an error".to_string() });
            }
            if self.finish_reason.is_none() {
                self.finish_reason = Some(reason.chars().take(60).collect());
                for c in &self.calls {
                    if !c.name.is_empty() {
                        sink(StreamEvent::ToolUseFinished { id: c.id.clone() });
                    }
                }
            }
        }
        Ok(())
    }

    /// The whole reply, once the stream has ended. A stream that ended before a finish reason
    /// arrived is an error: nothing half-finished is returned as if it were complete.
    pub fn finish(self) -> Result<Response, LlmError> {
        if self.chunks == 0 {
            return Err(LlmError::bad_stream("the stream ended before the reply began"));
        }
        let Some(reason) = self.finish_reason else {
            return Err(LlmError::bad_stream("the stream ended before the reply was complete"));
        };
        let mut parts = Vec::new();
        if !self.text.is_empty() {
            parts.push(Part::Text { text: self.text });
        }
        let mut any_call = false;
        for c in self.calls {
            if c.name.is_empty() {
                return Err(LlmError::bad_stream("a tool call had no name"));
            }
            any_call = true;
            let parsed: Result<Value, _> = if c.args.trim().is_empty() { Ok(json!({})) } else { serde_json::from_str(&c.args) };
            let (input, input_error) = match parsed {
                Ok(v) if v.is_object() => (v, None),
                Ok(_) => (json!({}), Some("the tool input was not a JSON object".to_string())),
                Err(_) => (json!({}), Some("the tool input was not valid JSON".to_string())),
            };
            parts.push(Part::ToolUse { id: c.id, name: c.name, input, input_error });
        }
        let stop_reason = match (reason.as_str(), any_call) {
            ("stop" | "tool_calls" | "function_call", true) => StopReason::ToolUse,
            ("stop" | "tool_calls" | "function_call", false) => StopReason::EndTurn,
            ("length", _) => StopReason::MaxTokens,
            ("content_filter", _) => StopReason::Refusal,
            (other, _) => StopReason::Other(other.to_string()),
        };
        Ok(Response { message: Message { role: Role::Assistant, parts }, stop_reason, usage: self.usage, model: self.model })
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Recorded-style streams, as these servers send them.

    pub fn chunk(delta: &str, finish: Option<&str>) -> String {
        let finish = finish.map_or("null".to_string(), |f| format!("\"{f}\""));
        format!(
            "data: {{\"id\":\"gen-1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"test/model\",\"choices\":[{{\"index\":0,\"delta\":{delta},\"finish_reason\":{finish}}}]}}\n\n"
        )
    }

    pub fn start() -> String {
        chunk(r#"{"role":"assistant","content":""}"#, None)
    }

    pub fn text(pieces: &[&str]) -> String {
        pieces.iter().map(|p| chunk(&format!(r#"{{"content":{}}}"#, serde_json::to_string(p).unwrap_or_default()), None)).collect()
    }

    /// The first chunk of a tool call (id and name), then its argument pieces.
    pub fn tool(index: u32, id: &str, name: &str, args: &[&str]) -> String {
        let mut s =
            chunk(&format!(r#"{{"tool_calls":[{{"index":{index},"id":"{id}","type":"function","function":{{"name":"{name}","arguments":""}}}}]}}"#), None);
        for a in args {
            s +=
                &chunk(&format!(r#"{{"tool_calls":[{{"index":{index},"function":{{"arguments":{}}}}}]}}"#, serde_json::to_string(a).unwrap_or_default()), None);
        }
        s
    }

    /// The closing chunk, the usage chunk (as OpenRouter sends it: an empty delta that repeats the
    /// finish reason), and `[DONE]`.
    pub fn end(reason: &str, prompt: u64, completion: u64) -> String {
        chunk(r#"{}"#, Some(reason))
            + &format!(
                "data: {{\"id\":\"gen-1\",\"model\":\"test/model\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"\",\"role\":\"assistant\"}},\"finish_reason\":\"{reason}\"}}],\"usage\":{{\"prompt_tokens\":{prompt},\"completion_tokens\":{completion},\"total_tokens\":{},\"prompt_tokens_details\":{{\"cached_tokens\":7}}}}}}\n\n",
                prompt + completion
            )
            + "data: [DONE]\n\n"
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn run(stream: &str) -> (Vec<StreamEvent>, Result<Response, LlmError>) {
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
    fn plain_text_streams_in_pieces_and_assembles_with_usage() {
        let s = ": OPENROUTER PROCESSING\n\n".to_string() + &start() + &text(&["Hel", "lo ", "there."]) + &end("stop", 40, 9);
        let (events, r) = run(&s);
        let r = r.unwrap();
        assert_eq!(r.message.text(), "Hello there.");
        assert_eq!((r.stop_reason, r.model.as_str()), (StopReason::EndTurn, "test/model"));
        assert_eq!(r.usage, Usage { input_tokens: 33, output_tokens: 9, cache_read_tokens: 7, cache_write_tokens: 0 }, "cached tokens are counted apart");
        let texts: Vec<_> = events.iter().filter_map(|e| if let StreamEvent::Text(t) = e { Some(t.as_str()) } else { None }).collect();
        assert_eq!(texts, ["Hel", "lo ", "there."]);
        assert!(events.iter().any(|e| matches!(e, StreamEvent::Usage(u) if u.output_tokens == 9)));
    }

    #[test]
    fn usage_in_a_final_chunk_without_choices_is_read_too() {
        let s = start()
            + &text(&["x"])
            + &chunk("{}", Some("stop"))
            + "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n";
        let r = run(&s).1.unwrap();
        assert_eq!((r.usage.input_tokens, r.usage.output_tokens), (10, 2));
    }

    #[test]
    fn reasoning_is_shown_but_not_kept() {
        let s = start()
            + &chunk(r#"{"reasoning":"The cut jumps."}"#, None)
            + &chunk(r#"{"reasoning_content":" Hm."}"#, None)
            + &chunk(r#"{"reasoning_details":[{"type":"reasoning.text","text":"detail"},{"type":"reasoning.encrypted","data":"ENC"}]}"#, None)
            + &chunk(r#"{"reasoning":"plain","reasoning_details":[{"type":"reasoning.text","text":"dup"}]}"#, None)
            + &text(&["It does."])
            + &end("stop", 5, 5);
        let (events, r) = run(&s);
        let thoughts: Vec<_> = events.iter().filter_map(|e| if let StreamEvent::Thinking(t) = e { Some(t.as_str()) } else { None }).collect();
        assert_eq!(thoughts, ["The cut jumps.", " Hm.", "detail", "plain"], "the plain string wins over the details beside it");
        let r = r.unwrap();
        assert_eq!(r.message.parts, vec![Part::Text { text: "It does.".into() }]);
    }

    #[test]
    fn a_tool_call_arriving_in_pieces_becomes_one_valid_call() {
        let s = start() + &text(&["Looking."]) + &tool(0, "call_a", "project_overview", &["{\"sequen", "ce\":\"Main", " Edit\"}"]) + &end("tool_calls", 5, 30);
        let (events, r) = run(&s);
        let r = r.unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        assert_eq!(
            r.message.parts[1],
            Part::ToolUse { id: "call_a".into(), name: "project_overview".into(), input: json!({"sequence": "Main Edit"}), input_error: None }
        );
        assert!(events.contains(&StreamEvent::ToolUseStarted { id: "call_a".into(), name: "project_overview".into() }));
        assert!(events.contains(&StreamEvent::ToolUseFinished { id: "call_a".into() }));
    }

    #[test]
    fn parallel_calls_keep_their_order_even_when_their_pieces_interleave() {
        let s = start()
            + &tool(0, "c1", "markers", &[])
            + &tool(1, "c2", "transcript", &[])
            + &chunk(r#"{"tool_calls":[{"index":1,"function":{"arguments":"{\"from\":"}},{"index":0,"function":{"arguments":"{}"}}]}"#, None)
            + &chunk(r#"{"tool_calls":[{"index":1,"function":{"arguments":"1}"}}]}"#, None)
            + &end("tool_calls", 5, 12);
        let r = run(&s).1.unwrap();
        let calls: Vec<_> = r.message.tool_uses().cloned().collect();
        assert_eq!(calls.len(), 2);
        assert!(matches!(&calls[0], Part::ToolUse { id, input, .. } if id == "c1" && *input == json!({})));
        assert!(matches!(&calls[1], Part::ToolUse { id, input, .. } if id == "c2" && *input == json!({"from": 1})));
    }

    #[test]
    fn servers_that_send_a_whole_call_at_once_or_leave_out_index_and_id_work() {
        // a whole call in one chunk, no index
        let one = chunk(r#"{"tool_calls":[{"id":"x1","type":"function","function":{"name":"markers","arguments":"{}"}}]}"#, None);
        let r = run(&(start() + &one + &end("stop", 1, 1))).1.unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse, "`stop` with a call in the reply means the model wants the tool");
        assert!(matches!(&r.message.parts[0], Part::ToolUse { id, name, .. } if id == "x1" && name == "markers"));
        // no id at all: one is made up, and later pieces without index or id continue the same call
        let first = chunk(r#"{"tool_calls":[{"function":{"name":"markers","arguments":"{\"a\":"}}]}"#, None);
        let more = chunk(r#"{"tool_calls":[{"function":{"arguments":"1}"}}]}"#, None);
        let r = run(&(start() + &first + &more + &end("stop", 1, 1))).1.unwrap();
        assert!(matches!(&r.message.parts[0], Part::ToolUse { id, input, .. } if id.starts_with("call_") && *input == json!({"a": 1})));
        // arguments already parsed into an object
        let obj = chunk(r#"{"tool_calls":[{"index":0,"id":"o","function":{"name":"markers","arguments":{"k":2}}}]}"#, None);
        let r = run(&(start() + &obj + &end("tool_calls", 1, 1))).1.unwrap();
        assert!(matches!(&r.message.parts[0], Part::ToolUse { input, input_error: None, .. } if *input == json!({"k": 2})));
    }

    #[test]
    fn broken_tool_input_is_marked_and_not_fatal() {
        for bad in [&["{\"a\": "][..], &["[1,2]"], &["\"str\""], &["{\"a\":1}}"], &["nul"]] {
            let r = run(&(start() + &tool(0, "c", "markers", bad) + &end("tool_calls", 1, 1))).1.unwrap();
            assert!(matches!(&r.message.parts[0], Part::ToolUse { input, input_error: Some(_), .. } if *input == json!({})), "{bad:?}");
        }
        let r = run(&(start() + &tool(0, "c", "markers", &[]) + &end("tool_calls", 1, 1))).1.unwrap();
        assert!(matches!(&r.message.parts[0], Part::ToolUse { input_error: None, .. }), "no arguments at all is an empty object");
    }

    #[test]
    fn finish_reasons_map_to_stop_reasons() {
        let r = run(&(start() + &text(&["I can't"]) + &end("content_filter", 1, 1))).1.unwrap();
        assert_eq!(r.stop_reason, StopReason::Refusal);
        let r = run(&(start() + &tool(0, "c", "n", &["{\"a\":"]) + &end("length", 1, 1))).1.unwrap();
        assert_eq!(r.stop_reason, StopReason::MaxTokens);
        let r = run(&(start() + &end("something_new", 1, 1))).1.unwrap();
        assert_eq!(r.stop_reason, StopReason::Other("something_new".into()));
        let r = run(&(start() + &text(&["hi"]) + &end("tool_calls", 1, 1))).1.unwrap();
        assert_eq!(r.stop_reason, StopReason::EndTurn, "tool_calls with no call in the reply ends the turn");
    }

    #[test]
    fn an_error_chunk_mid_stream_stops_the_reply_with_a_mapped_error() {
        let err = r#"data: {"id":"x","model":"m","error":{"code":"server_error","message":"Provider disconnected unexpectedly"},"choices":[{"index":0,"delta":{"content":""},"finish_reason":"error"}]}"#;
        let (events, r) = run(&(start() + &text(&["part"]) + err + "\n\n"));
        assert!(matches!(r, Err(LlmError::Api { .. })), "{r:?}");
        assert!(events.contains(&StreamEvent::Text("part".into())), "what arrived before the error was still shown");
        let rate = r#"data: {"error":{"code":429,"message":"slow down"}}"#;
        assert_eq!(run(rate).1.unwrap_err(), LlmError::RateLimited);
        let auth = r#"data: {"error":{"code":401,"message":"no key sk-or-v1-0123456789abcdefghij"}}"#;
        let e = run(auth).1.unwrap_err();
        assert!(matches!(e, LlmError::Auth(_)) && !e.to_string().contains("0123456789abcdef"), "{e}");
        // a bare finish_reason "error" without an error object is still an error
        let bare = chunk("{}", Some("error"));
        assert!(run(&(start() + &bare)).1.is_err());
    }

    #[test]
    fn a_stream_that_ends_early_is_an_error_not_a_short_reply() {
        assert!(run("").1.is_err());
        assert!(run("data: [DONE]\n\n").1.is_err());
        let s = start() + &text(&["partial"]);
        let (events, r) = run(&s);
        assert!(matches!(r, Err(LlmError::BadStream(_))));
        assert!(events.contains(&StreamEvent::Text("partial".into())));
        // a finish reason without [DONE] is a complete reply
        let r = run(&(start() + &text(&["ok"]) + &chunk("{}", Some("stop")))).1.unwrap();
        assert_eq!(r.message.text(), "ok");
        let mut a = StreamAssembler::new();
        assert!(!a.is_finished());
        a.feed_line(chunk("{}", Some("stop")).lines().next().unwrap_or(""), &mut |_| {}).unwrap();
        assert!(a.is_finished());
    }

    #[test]
    fn garbage_is_an_error_or_ignored_never_a_panic() {
        for line in ["data: not json", "data: {\"choices\":", "data: [1,2]", "data: \"str\""] {
            assert!(StreamAssembler::new().feed_line(line, &mut |_| {}).is_err(), "{line}");
        }
        let mut a = StreamAssembler::new();
        for line in [
            "",
            ": comment",
            "event: ping",
            "data:",
            "retry: 1000",
            "data: {}",
            "data: {\"choices\":[]}",
            "data: {\"choices\":[{}]}",
            "data: {\"choices\":[{\"delta\":5,\"finish_reason\":7}]}",
            "data: {\"choices\":[{\"delta\":{\"content\":5,\"tool_calls\":\"x\"}}]}",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[5,null,{}]}}]}",
            "data: {\"usage\":\"lots\"}",
            "data: {\"usage\":{\"prompt_tokens\":-1,\"completion_tokens\":1.5}}",
        ] {
            let _ = a.feed_line(line, &mut |_| {});
        }
    }

    #[test]
    fn oversized_tool_input_text_and_call_counts_are_refused() {
        let mut a = StreamAssembler::new();
        a.feed_line(tool(0, "t", "n", &[]).lines().next().unwrap_or(""), &mut |_| {}).unwrap();
        let big = chunk(&format!(r#"{{"tool_calls":[{{"index":0,"function":{{"arguments":"{}"}}}}]}}"#, "x".repeat(MAX_TOOL_INPUT / 2 + 10)), None);
        let line = big.lines().next().unwrap_or("");
        a.feed_line(line, &mut |_| {}).unwrap();
        assert!(a.feed_line(line, &mut |_| {}).is_err());
        let mut a = StreamAssembler::new();
        for i in 0..MAX_CALLS {
            let c = chunk(&format!(r#"{{"tool_calls":[{{"index":{i},"id":"i{i}","function":{{"name":"n"}}}}]}}"#), None);
            a.feed_line(c.lines().next().unwrap_or(""), &mut |_| {}).unwrap();
        }
        let c = chunk(r#"{"tool_calls":[{"index":999,"id":"extra","function":{"name":"n"}}]}"#, None);
        assert!(a.feed_line(c.lines().next().unwrap_or(""), &mut |_| {}).is_err());
        let mut a = StreamAssembler::new();
        let c = chunk(&format!(r#"{{"content":"{}"}}"#, "y".repeat(MAX_TEXT / 2 + 10)), None);
        a.feed_line(c.lines().next().unwrap_or(""), &mut |_| {}).unwrap();
        assert!(a.feed_line(c.lines().next().unwrap_or(""), &mut |_| {}).is_err());
    }

    #[test]
    fn a_call_without_a_name_is_an_error_at_the_end() {
        let c = chunk(r#"{"tool_calls":[{"index":0,"id":"c","function":{"arguments":"{}"}}]}"#, None);
        assert!(matches!(run(&(start() + &c + &end("tool_calls", 1, 1))).1, Err(LlmError::BadStream(_))));
    }
}
