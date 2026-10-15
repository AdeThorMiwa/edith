//! Building the request body.

use super::PROVIDER;
use crate::{LlmError, Message, Part, Request, Role};
use serde_json::{Value, json};

fn part_json(part: &Part) -> Option<Value> {
    Some(match part {
        Part::Text { text } => json!({"type": "text", "text": text}),
        Part::ToolUse { id, name, input, input_error } => {
            // A call whose input could not be parsed is echoed with an empty object: the API needs
            // a valid one, and the matching result tells the model what went wrong.
            let input = if input_error.is_some() || !input.is_object() { json!({}) } else { input.clone() };
            json!({"type": "tool_use", "id": id, "name": name, "input": input})
        }
        Part::ToolResult { tool_use_id, content, is_error } => {
            let mut v = json!({"type": "tool_result", "tool_use_id": tool_use_id, "content": content});
            if *is_error && let Some(o) = v.as_object_mut() {
                o.insert("is_error".to_string(), Value::Bool(true));
            }
            v
        }
        // Another provider's blocks mean nothing here: drop them (the API would reject them).
        Part::Opaque { provider, value } => {
            if provider == PROVIDER {
                value.clone()
            } else {
                return None;
            }
        }
    })
}

fn message_json(m: &Message) -> Option<Value> {
    let content: Vec<Value> = m.parts.iter().filter_map(part_json).collect();
    if content.is_empty() {
        return None;
    }
    let role = match m.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    Some(json!({"role": role, "content": content}))
}

/// The JSON body of a streaming `POST /v1/messages` request.
///
/// Notes on choices: the tool list and system prompt come first so a `cache` marker on the last
/// system part caches both; thinking is `adaptive` (the only mode current models accept) and is
/// only sent when a summary is wanted, since the default is no visible summary; tools are sent
/// without `eager_input_streaming` so the API validates their input before it reaches us.
pub fn build_body(req: &Request) -> Result<Value, LlmError> {
    if req.model.trim().is_empty() {
        return Err(LlmError::BadRequest("no model chosen".to_string()));
    }
    let messages: Vec<Value> = req.messages.iter().filter_map(message_json).collect();
    if messages.is_empty() {
        return Err(LlmError::BadRequest("there is nothing to send".to_string()));
    }
    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_output_tokens.max(1),
        "stream": true,
        "messages": messages,
    });
    let Some(obj) = body.as_object_mut() else { return Err(LlmError::bad_stream("internal: body is not an object")) };
    if !req.system.is_empty() {
        let system: Vec<Value> = req
            .system
            .iter()
            .map(|p| {
                let mut v = json!({"type": "text", "text": p.text});
                if p.cache
                    && let Some(o) = v.as_object_mut()
                {
                    o.insert("cache_control".to_string(), json!({"type": "ephemeral"}));
                }
                v
            })
            .collect();
        obj.insert("system".to_string(), Value::Array(system));
    }
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req.tools.iter().map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema})).collect();
        obj.insert("tools".to_string(), Value::Array(tools));
    }
    if req.thinking_summary {
        obj.insert("thinking".to_string(), json!({"type": "adaptive", "display": "summarized"}));
    }
    if let Some(e) = req.effort {
        obj.insert("output_config".to_string(), json!({"effort": e.as_str()}));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Effort, SystemPart, ToolSpec};

    fn req() -> Request {
        let mut r = Request::new("claude-haiku-5-5");
        r.system = vec![SystemPart { text: "You are Edith.".into(), cache: false }, SystemPart { text: "Stable rules.".into(), cache: true }];
        r.tools = vec![ToolSpec {
            name: "project_inspect".into(),
            description: "Look at the project.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        }];
        r.messages = vec![Message::user_text("Why does the cut at 00:42 feel abrupt?")];
        r
    }

    #[test]
    fn the_body_has_the_shape_the_api_expects() {
        let b = build_body(&req()).unwrap();
        assert_eq!(b["model"], "claude-haiku-5-5");
        assert_eq!(b["stream"], true);
        assert!(b["max_tokens"].as_u64().unwrap() >= 1);
        assert_eq!(b["system"][1]["cache_control"], json!({"type": "ephemeral"}));
        assert!(b["system"][0].get("cache_control").is_none());
        assert_eq!(b["tools"][0]["name"], "project_inspect");
        assert!(b["tools"][0].get("eager_input_streaming").is_none());
        assert_eq!(b["messages"][0], json!({"role": "user", "content": [{"type": "text", "text": "Why does the cut at 00:42 feel abrupt?"}]}));
        assert!(b.get("thinking").is_none() && b.get("output_config").is_none());
    }

    #[test]
    fn thinking_and_effort_are_sent_only_when_asked() {
        let mut r = req();
        r.thinking_summary = true;
        r.effort = Some(Effort::Medium);
        let b = build_body(&r).unwrap();
        assert_eq!(b["thinking"], json!({"type": "adaptive", "display": "summarized"}));
        assert_eq!(b["output_config"], json!({"effort": "medium"}));
    }

    #[test]
    fn tool_calls_and_results_round_trip_into_blocks() {
        let mut r = req();
        r.messages.push(Message {
            role: Role::Assistant,
            parts: vec![
                Part::Opaque { provider: "anthropic".into(), value: json!({"type": "thinking", "thinking": "hm", "signature": "SIG"}) },
                Part::Text { text: "Let me look.".into() },
                Part::ToolUse { id: "toolu_1".into(), name: "project_inspect".into(), input: json!({"a": 1}), input_error: None },
                Part::ToolUse { id: "toolu_2".into(), name: "project_inspect".into(), input: json!({}), input_error: Some("cut off".into()) },
            ],
        });
        r.messages.push(Message::tool_results(vec![
            Part::ToolResult { tool_use_id: "toolu_1".into(), content: "ok".into(), is_error: false },
            Part::ToolResult { tool_use_id: "toolu_2".into(), content: "bad input".into(), is_error: true },
        ]));
        let b = build_body(&r).unwrap();
        let a = &b["messages"][1]["content"];
        assert_eq!(a[0], json!({"type": "thinking", "thinking": "hm", "signature": "SIG"}), "thinking blocks are echoed unchanged and first");
        assert_eq!(a[2], json!({"type": "tool_use", "id": "toolu_1", "name": "project_inspect", "input": {"a": 1}}));
        assert_eq!(a[3]["input"], json!({}));
        let res = &b["messages"][2]["content"];
        assert_eq!(res.as_array().unwrap().len(), 2, "all results in one message");
        assert!(res[0].get("is_error").is_none());
        assert_eq!(res[1]["is_error"], true);
    }

    #[test]
    fn another_providers_blocks_are_dropped_and_empty_messages_vanish() {
        let mut r = req();
        r.messages.push(Message { role: Role::Assistant, parts: vec![Part::Opaque { provider: "other".into(), value: json!({"x": 1}) }] });
        r.messages.push(Message::user_text("and then?"));
        let b = build_body(&r).unwrap();
        assert_eq!(b["messages"].as_array().unwrap().len(), 2, "the assistant message with only a foreign block is gone");
    }

    #[test]
    fn nothing_to_send_or_no_model_is_an_error() {
        let mut r = req();
        r.messages.clear();
        assert!(matches!(build_body(&r), Err(LlmError::BadRequest(_))));
        let mut r = req();
        r.model = "  ".into();
        assert!(matches!(build_body(&r), Err(LlmError::BadRequest(_))));
    }
}
