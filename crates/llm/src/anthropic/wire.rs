//! Building the request body.

use super::PROVIDER;
use crate::{LlmError, Message, Part, Request, Role};
use serde_json::{Value, json};

/// Largest picture sent, in bytes of base64 (the API refuses more than 5 MB per image).
const MAX_IMAGE_BASE64: usize = 6 * 1024 * 1024;

/// A picture as the API wants it. A type the API does not take, or a picture too large for it, is
/// replaced by a line of text saying so: sending it would make the whole request fail.
fn image_json(media_type: &str, data: &str) -> Value {
    if !matches!(media_type, "image/jpeg" | "image/png" | "image/gif" | "image/webp") {
        return json!({"type": "text", "text": "[a picture of a type that cannot be sent was left out]"});
    }
    if data.len() > MAX_IMAGE_BASE64 {
        return json!({"type": "text", "text": "[a picture that was too large to send was left out]"});
    }
    json!({"type": "image", "source": {"type": "base64", "media_type": media_type, "data": data}})
}

fn part_json(part: &Part) -> Option<Value> {
    Some(match part {
        Part::Text { text } => json!({"type": "text", "text": text}),
        Part::ToolUse { id, name, input, input_error } => {
            // A call whose input could not be parsed is echoed with an empty object: the API needs
            // a valid one, and the matching result tells the model what went wrong.
            let input = if input_error.is_some() || !input.is_object() { json!({}) } else { input.clone() };
            json!({"type": "tool_use", "id": id, "name": name, "input": input})
        }
        Part::Image { media_type, data } => image_json(media_type, data),
        Part::ToolResult { tool_use_id, content, is_error, images } => {
            // With pictures the content is a list: the words first, then each picture.
            let body = if images.is_empty() {
                json!(content)
            } else {
                let mut blocks: Vec<Value> = Vec::new();
                if !content.is_empty() {
                    blocks.push(json!({"type": "text", "text": content}));
                }
                blocks.extend(images.iter().map(|i| image_json(&i.media_type, &i.data)));
                Value::Array(blocks)
            };
            let mut v = json!({"type": "tool_result", "tool_use_id": tool_use_id, "content": body});
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

/// Marks where the API may cache the conversation so far, so a call that adds a few blocks to the
/// previous one pays full price only for what is new (a cached read costs a tenth of an input token).
/// Two marks: the last block of the newest message, and one three messages earlier, so a step that adds
/// many blocks (several tool calls at once) still finds the previous call's cache. Thinking blocks
/// cannot carry a mark, so the nearest block before one is used.
fn mark_cache_points(messages: &mut [Value]) {
    let n = messages.len();
    let mut from = vec![n.saturating_sub(1)];
    if n >= 4 {
        from.push(n - 3);
    }
    for i in from {
        let Some(blocks) = messages.get_mut(i).and_then(|m| m.get_mut("content")).and_then(Value::as_array_mut) else { continue };
        let Some(block) = blocks.iter_mut().rev().find(|b| !matches!(b.get("type").and_then(Value::as_str), Some("thinking" | "redacted_thinking"))) else {
            continue;
        };
        if let Some(o) = block.as_object_mut() {
            o.insert("cache_control".to_string(), json!({"type": "ephemeral"}));
        }
    }
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
    let mut messages: Vec<Value> = req.messages.iter().filter_map(message_json).collect();
    if messages.is_empty() {
        return Err(LlmError::BadRequest("there is nothing to send".to_string()));
    }
    mark_cache_points(&mut messages);
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
        assert_eq!(
            b["messages"][0],
            json!({"role": "user", "content": [{"type": "text", "text": "Why does the cut at 00:42 feel abrupt?", "cache_control": {"type": "ephemeral"}}]})
        );
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
            Part::ToolResult { tool_use_id: "toolu_1".into(), content: "ok".into(), is_error: false, images: Vec::new() },
            Part::ToolResult { tool_use_id: "toolu_2".into(), content: "bad input".into(), is_error: true, images: Vec::new() },
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

    #[test]
    fn pictures_go_out_as_image_blocks_and_inside_tool_results_as_a_list() {
        let mut r = req();
        r.messages = vec![Message {
            role: Role::User,
            parts: vec![Part::Image { media_type: "image/jpeg".into(), data: "QUJD".into() }, Part::Text { text: "what is wrong with this frame?".into() }],
        }];
        let b = build_body(&r).unwrap();
        assert_eq!(b["messages"][0]["content"][0], json!({"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": "QUJD"}}));
        assert_eq!(b["messages"][0]["content"][1]["type"], "text");

        let img = crate::Image { media_type: "image/png".into(), data: "REVG".into() };
        let mut r = req();
        r.messages.push(Message {
            role: Role::Assistant,
            parts: vec![Part::ToolUse { id: "t1".into(), name: "frame_view".into(), input: json!({}), input_error: None }],
        });
        r.messages.push(Message::tool_results(vec![Part::ToolResult {
            tool_use_id: "t1".into(),
            content: "the frame at 00:00:04:23".into(),
            is_error: false,
            images: vec![img.clone()],
        }]));
        let b = build_body(&r).unwrap();
        let c = &b["messages"][2]["content"][0]["content"];
        assert_eq!(c[0], json!({"type": "text", "text": "the frame at 00:00:04:23"}));
        assert_eq!(c[1]["source"]["media_type"], "image/png");
        // a picture with no words is just the picture; with no picture the content is still a plain string
        r.messages.pop();
        r.messages.push(Message::tool_results(vec![Part::ToolResult { tool_use_id: "t1".into(), content: String::new(), is_error: false, images: vec![img] }]));
        let b = build_body(&r).unwrap();
        assert_eq!(b["messages"][2]["content"][0]["content"].as_array().unwrap().len(), 1);
        r.messages.pop();
        r.messages.push(Message::tool_results(vec![Part::ToolResult {
            tool_use_id: "t1".into(),
            content: "text only".into(),
            is_error: false,
            images: vec![],
        }]));
        assert_eq!(build_body(&r).unwrap()["messages"][2]["content"][0]["content"], "text only");
    }

    #[test]
    fn pictures_the_api_would_refuse_are_replaced_by_a_line_of_text_not_sent() {
        let mut r = req();
        r.messages = vec![Message {
            role: Role::User,
            parts: vec![
                Part::Image { media_type: "image/svg+xml".into(), data: "QUJD".into() },
                Part::Image { media_type: "image/jpeg".into(), data: "A".repeat(7 * 1024 * 1024) },
                Part::Image { media_type: "image/webp".into(), data: "QUJD".into() },
                Part::Text { text: "q".into() },
            ],
        }];
        let b = build_body(&r).unwrap();
        let c = &b["messages"][0]["content"];
        assert_eq!(c[0]["type"], "text");
        assert!(c[0]["text"].as_str().unwrap().contains("cannot be sent"));
        assert_eq!(c[1]["type"], "text");
        assert!(c[1]["text"].as_str().unwrap().contains("too large"));
        assert_eq!(c[2]["type"], "image", "an acceptable picture still goes");
        assert!(b.to_string().len() < 2000, "the oversize picture is not in the request");
    }

    fn marks(b: &Value) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (i, m) in b["messages"].as_array().unwrap().iter().enumerate() {
            for (j, c) in m["content"].as_array().unwrap().iter().enumerate() {
                if c.get("cache_control").is_some() {
                    out.push((i, j));
                }
            }
        }
        out
    }

    #[test]
    fn the_conversation_is_marked_for_caching_at_its_newest_block_and_three_messages_back() {
        let mut r = req();
        // a tool loop: user, assistant (two calls), user (two results), assistant, user (result), assistant, user (result)
        for k in 0..3 {
            r.messages.push(Message {
                role: Role::Assistant,
                parts: vec![
                    Part::Text { text: "Looking.".into() },
                    Part::ToolUse { id: format!("a{k}"), name: "project_inspect".into(), input: json!({}), input_error: None },
                    Part::ToolUse { id: format!("b{k}"), name: "project_inspect".into(), input: json!({}), input_error: None },
                ],
            });
            r.messages.push(Message::tool_results(vec![
                Part::ToolResult { tool_use_id: format!("a{k}"), content: "one".into(), is_error: false, images: Vec::new() },
                Part::ToolResult { tool_use_id: format!("b{k}"), content: "two".into(), is_error: false, images: Vec::new() },
            ]));
        }
        let b = build_body(&r).unwrap();
        let n = b["messages"].as_array().unwrap().len();
        assert_eq!(n, 7);
        // the last block of the newest message, and of the message three before it; no more (the API allows four marks, two are the system's)
        assert_eq!(marks(&b), vec![(n - 3, 1), (n - 1, 1)], "{:?}", marks(&b));
        assert_eq!(b["messages"][n - 1]["content"][1]["type"], "tool_result");
    }

    #[test]
    fn a_short_conversation_gets_one_mark_and_thinking_blocks_never_carry_one() {
        let b = build_body(&req()).unwrap();
        assert_eq!(marks(&b), vec![(0, 0)]);
        // an assistant message that ends in a thinking block: the nearest block before it is marked instead
        let mut r = req();
        r.messages.push(Message {
            role: Role::Assistant,
            parts: vec![
                Part::Text { text: "Hm.".into() },
                Part::Opaque { provider: "anthropic".into(), value: json!({"type": "thinking", "thinking": "x", "signature": "S"}) },
            ],
        });
        let b = build_body(&r).unwrap();
        assert_eq!(marks(&b), vec![(1, 0)]);
        assert!(b["messages"][1]["content"][1].get("cache_control").is_none());
        // a request whose newest message has no usable block is left alone (never a panic)
        let mut r = req();
        r.messages.push(Message {
            role: Role::Assistant,
            parts: vec![Part::Opaque { provider: "anthropic".into(), value: json!({"type": "thinking", "thinking": "x", "signature": "S"}) }],
        });
        assert!(build_body(&r).is_ok());
    }
}
