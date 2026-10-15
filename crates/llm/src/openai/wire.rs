//! Building the request body.

use super::{EffortStyle, Flavor};
use crate::{LlmError, Message, Part, Request, Role};
use serde_json::{Value, json};

/// Largest picture sent, in bytes of base64.
const MAX_IMAGE_BASE64: usize = 6 * 1024 * 1024;

/// A picture as a content part. A type servers do not take, or a picture too large, becomes a
/// line of text saying so: sending it could make the whole request fail.
fn image_part(media_type: &str, data: &str) -> Value {
    if !matches!(media_type, "image/jpeg" | "image/png" | "image/gif" | "image/webp") {
        return json!({"type": "text", "text": "[a picture of a type that cannot be sent was left out]"});
    }
    if data.len() > MAX_IMAGE_BASE64 {
        return json!({"type": "text", "text": "[a picture that was too large to send was left out]"});
    }
    json!({"type": "image_url", "image_url": {"url": format!("data:{media_type};base64,{data}")}})
}

/// Content for a user message: a plain string when it is all text (every server takes that),
/// otherwise a list of parts.
fn user_content(parts: Vec<Value>) -> Value {
    if parts.iter().all(|p| p.get("type").and_then(Value::as_str) == Some("text")) {
        let text: Vec<&str> = parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect();
        return Value::String(text.join("\n"));
    }
    Value::Array(parts)
}

fn user_messages(m: &Message, out: &mut Vec<Value>) {
    // Answers to tool calls come first, one `tool` message each; this API has no place for a
    // picture inside one, so pictures a tool returned ride in the user message that follows.
    let mut pictures: Vec<Value> = Vec::new();
    for p in &m.parts {
        if let Part::ToolResult { tool_use_id, content, is_error, images } = p {
            let mut text = if content.is_empty() { "(no output)".to_string() } else { content.clone() };
            if *is_error && !text.starts_with("Error") {
                text = format!("Error: {text}");
            }
            out.push(json!({"role": "tool", "tool_call_id": tool_use_id, "content": text}));
            if !images.is_empty() {
                pictures.push(json!({"type": "text", "text": format!("Picture from tool call {tool_use_id}:")}));
                pictures.extend(images.iter().map(|i| image_part(&i.media_type, &i.data)));
            }
        }
    }
    let mut parts = pictures;
    for p in &m.parts {
        match p {
            Part::Text { text } if !text.is_empty() => parts.push(json!({"type": "text", "text": text})),
            Part::Image { media_type, data } => parts.push(image_part(media_type, data)),
            _ => {}
        }
    }
    if !parts.is_empty() {
        out.push(json!({"role": "user", "content": user_content(parts)}));
    }
}

fn assistant_message(m: &Message) -> Option<Value> {
    let text = m.text();
    let calls: Vec<Value> = m
        .parts
        .iter()
        .filter_map(|p| match p {
            Part::ToolUse { id, name, input, input_error } => {
                // A call whose input could not be parsed is echoed with an empty object: the
                // matching result tells the model what went wrong.
                let input = if input_error.is_some() || !input.is_object() { json!({}) } else { input.clone() };
                Some(json!({"id": id, "type": "function", "function": {"name": name, "arguments": input.to_string()}}))
            }
            _ => None,
        })
        .collect();
    if text.is_empty() && calls.is_empty() {
        return None;
    }
    let mut v = json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { Value::String(text) }});
    if !calls.is_empty()
        && let Some(o) = v.as_object_mut()
    {
        o.insert("tool_calls".to_string(), Value::Array(calls));
    }
    Some(v)
}

/// The JSON body of a streaming `POST {base}/chat/completions` request.
///
/// Reasoning effort is sent only when the person chose one, so a model is never switched into a
/// slower, dearer mode behind their back. `thinking_summary` has no equivalent here: whatever
/// reasoning text a server streams is shown as it comes. Cache markers are ignored (not part of
/// this API).
pub fn build_body(req: &Request, flavor: &Flavor) -> Result<Value, LlmError> {
    if req.model.trim().is_empty() {
        return Err(LlmError::BadRequest("no model chosen".to_string()));
    }
    let mut messages: Vec<Value> = Vec::new();
    let system: Vec<&str> = req.system.iter().map(|p| p.text.as_str()).filter(|t| !t.is_empty()).collect();
    if !system.is_empty() {
        messages.push(json!({"role": "system", "content": system.join("\n\n")}));
    }
    let first = messages.len();
    for m in &req.messages {
        match m.role {
            Role::User => user_messages(m, &mut messages),
            Role::Assistant => messages.extend(assistant_message(m)),
        }
    }
    if messages.len() == first {
        return Err(LlmError::BadRequest("there is nothing to send".to_string()));
    }
    let mut body = json!({
        "model": req.model,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    let Some(obj) = body.as_object_mut() else { return Err(LlmError::bad_stream("internal: body is not an object")) };
    obj.insert(flavor.max_tokens_field.to_string(), json!(req.max_output_tokens.max(1)));
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.input_schema}}))
            .collect();
        obj.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(e) = req.effort {
        match flavor.effort {
            EffortStyle::None => {}
            EffortStyle::ReasoningObject => {
                obj.insert("reasoning".to_string(), json!({"effort": openai_effort(e)}));
            }
            EffortStyle::ReasoningEffort => {
                obj.insert("reasoning_effort".to_string(), json!(openai_effort(e)));
            }
        }
    }
    Ok(body)
}

/// This API's effort words are `low`, `medium` and `high` everywhere; the larger Anthropic steps
/// map to `high`.
fn openai_effort(e: crate::Effort) -> &'static str {
    match e {
        crate::Effort::Low => "low",
        crate::Effort::Medium => "medium",
        crate::Effort::High | crate::Effort::XHigh | crate::Effort::Max => "high",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Effort, Image, SystemPart, ToolSpec};

    fn req() -> Request {
        let mut r = Request::new("anthropic/claude-haiku-5-5");
        r.system = vec![SystemPart { text: "You are Edith.".into(), cache: false }, SystemPart { text: "Stable rules.".into(), cache: true }];
        r.tools = vec![ToolSpec {
            name: "project_overview".into(),
            description: "Look at the project.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        }];
        r.messages = vec![Message::user_text("Why does the cut at 00:42 feel abrupt?")];
        r
    }

    #[test]
    fn the_body_has_the_shape_the_api_expects() {
        let b = build_body(&req(), &Flavor::OPENROUTER).unwrap();
        assert_eq!(b["model"], "anthropic/claude-haiku-5-5");
        assert_eq!((b["stream"].clone(), b["stream_options"].clone()), (json!(true), json!({"include_usage": true})));
        assert_eq!(b["max_tokens"], 8192);
        assert_eq!(b["messages"][0], json!({"role": "system", "content": "You are Edith.\n\nStable rules."}));
        assert_eq!(b["messages"][1], json!({"role": "user", "content": "Why does the cut at 00:42 feel abrupt?"}));
        assert_eq!(
            b["tools"][0],
            json!({"type": "function", "function": {"name": "project_overview", "description": "Look at the project.", "parameters": {"type": "object", "properties": {}}}})
        );
        assert!(b.get("reasoning").is_none() && b.get("reasoning_effort").is_none() && b.get("tool_choice").is_none());
    }

    #[test]
    fn each_server_gets_its_own_token_limit_and_effort_field() {
        let mut r = req();
        r.effort = Some(Effort::XHigh);
        let or = build_body(&r, &Flavor::OPENROUTER).unwrap();
        assert_eq!(or["reasoning"], json!({"effort": "high"}));
        assert!(or.get("reasoning_effort").is_none());
        let oa = build_body(&r, &Flavor::OPENAI).unwrap();
        assert_eq!((oa["reasoning_effort"].clone(), oa["max_completion_tokens"].clone()), (json!("high"), json!(8192)));
        assert!(oa.get("max_tokens").is_none() && oa.get("reasoning").is_none());
        let local = build_body(&r, &Flavor::LMSTUDIO).unwrap();
        assert!(local.get("reasoning").is_none() && local.get("reasoning_effort").is_none());
        assert!(build_body(&req(), &Flavor::OPENAI).unwrap().get("reasoning_effort").is_none(), "no effort chosen, none sent");
    }

    #[test]
    fn tool_calls_and_results_become_tool_messages_in_order() {
        let mut r = req();
        r.messages.push(Message {
            role: Role::Assistant,
            parts: vec![
                Part::Opaque { provider: "anthropic".into(), value: json!({"type": "thinking"}) },
                Part::Text { text: "Let me look.".into() },
                Part::ToolUse { id: "call_1".into(), name: "project_overview".into(), input: json!({"a": 1}), input_error: None },
                Part::ToolUse { id: "call_2".into(), name: "project_overview".into(), input: json!({"x": 1}), input_error: Some("cut off".into()) },
            ],
        });
        r.messages.push(Message::tool_results(vec![
            Part::ToolResult { tool_use_id: "call_1".into(), content: "ok".into(), is_error: false, images: Vec::new() },
            Part::ToolResult { tool_use_id: "call_2".into(), content: "bad input".into(), is_error: true, images: Vec::new() },
        ]));
        let b = build_body(&r, &Flavor::OPENROUTER).unwrap();
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m.len(), 5, "system, user, assistant, two tool messages");
        assert_eq!(m[2]["content"], "Let me look.");
        assert_eq!(m[2]["tool_calls"][0], json!({"id": "call_1", "type": "function", "function": {"name": "project_overview", "arguments": "{\"a\":1}"}}));
        assert_eq!(m[2]["tool_calls"][1]["function"]["arguments"], "{}", "an unparsed input is echoed as an empty object");
        assert_eq!(m[3], json!({"role": "tool", "tool_call_id": "call_1", "content": "ok"}));
        assert_eq!(m[4], json!({"role": "tool", "tool_call_id": "call_2", "content": "Error: bad input"}));
    }

    #[test]
    fn an_assistant_message_with_only_calls_has_null_content_and_a_foreign_only_one_vanishes() {
        let mut r = req();
        r.messages
            .push(Message { role: Role::Assistant, parts: vec![Part::ToolUse { id: "c".into(), name: "n".into(), input: json!({}), input_error: None }] });
        r.messages.push(Message { role: Role::Assistant, parts: vec![Part::Opaque { provider: "other".into(), value: json!({"x": 1}) }] });
        let b = build_body(&r, &Flavor::OPENAI).unwrap();
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m.len(), 3);
        assert!(m[2]["content"].is_null() && m[2]["tool_calls"].is_array());
    }

    #[test]
    fn pictures_from_a_tool_follow_all_the_tool_messages_in_one_user_message() {
        let img = Image { media_type: "image/png".into(), data: "REVG".into() };
        let mut r = req();
        r.messages.push(Message {
            role: Role::Assistant,
            parts: vec![
                Part::ToolUse { id: "t1".into(), name: "frame_view".into(), input: json!({}), input_error: None },
                Part::ToolUse { id: "t2".into(), name: "frame_view".into(), input: json!({}), input_error: None },
            ],
        });
        r.messages.push(Message::tool_results(vec![
            Part::ToolResult { tool_use_id: "t1".into(), content: "the frame".into(), is_error: false, images: vec![img.clone()] },
            Part::ToolResult { tool_use_id: "t2".into(), content: String::new(), is_error: false, images: vec![] },
        ]));
        let b = build_body(&r, &Flavor::OPENROUTER).unwrap();
        let m = b["messages"].as_array().unwrap();
        assert_eq!((m[3]["role"].clone(), m[4]["role"].clone()), (json!("tool"), json!("tool")));
        assert_eq!(m[4]["content"], "(no output)");
        assert_eq!(m[5]["role"], "user");
        assert_eq!(m[5]["content"][0], json!({"type": "text", "text": "Picture from tool call t1:"}));
        assert_eq!(m[5]["content"][1], json!({"type": "image_url", "image_url": {"url": "data:image/png;base64,REVG"}}));
        assert_eq!(m.len(), 6);
    }

    #[test]
    fn an_attached_picture_makes_the_content_a_list_and_bad_pictures_become_text() {
        let mut r = req();
        r.messages = vec![Message {
            role: Role::User,
            parts: vec![
                Part::Image { media_type: "image/jpeg".into(), data: "QUJD".into() },
                Part::Image { media_type: "image/svg+xml".into(), data: "QUJD".into() },
                Part::Image { media_type: "image/png".into(), data: "A".repeat(7 * 1024 * 1024) },
                Part::Text { text: "what is wrong with this frame?".into() },
            ],
        }];
        let b = build_body(&r, &Flavor::OPENROUTER).unwrap();
        let c = &b["messages"][1]["content"];
        assert_eq!(c[0]["image_url"]["url"], "data:image/jpeg;base64,QUJD");
        assert!(c[1]["text"].as_str().unwrap().contains("cannot be sent") && c[2]["text"].as_str().unwrap().contains("too large"));
        assert_eq!(c[3]["text"], "what is wrong with this frame?");
        assert!(b.to_string().len() < 2000, "the oversize picture is not in the request");
    }

    #[test]
    fn nothing_to_send_or_no_model_is_an_error() {
        let mut r = req();
        r.messages.clear();
        assert!(matches!(build_body(&r, &Flavor::OPENAI), Err(LlmError::BadRequest(_))), "a system prompt alone is not a request");
        let mut r = req();
        r.model = "  ".into();
        assert!(matches!(build_body(&r, &Flavor::OPENAI), Err(LlmError::BadRequest(_))));
    }

    #[test]
    fn flavors_are_found_by_service_id() {
        assert_eq!(Flavor::for_service("openai"), Some(Flavor::OPENAI));
        assert_eq!(Flavor::for_service("ollama").map(|f| f.provider), Some("ollama"));
        assert!(Flavor::for_service("anthropic").is_none());
    }
}
