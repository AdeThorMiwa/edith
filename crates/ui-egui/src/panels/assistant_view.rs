//! What the Assistant panel shows, as plain data: the agent's event stream (the JSON the
//! `assistant.thread` command returns) folded into messages, tool cards and notes, and a small
//! reader for the answers' light markdown. No egui here, so all of it is tested directly.

use serde_json::Value;

/// One thing in the conversation.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    User(String),
    /// The assistant's words, as they stream in.
    Assistant(String),
    /// A summary of the assistant's reasoning.
    Thinking(String),
    /// A tool the assistant used: what it asked for and what came back.
    Tool {
        id: String,
        name: String,
        input: String,
        result: Option<String>,
        is_error: bool,
        /// How many pictures came back with the answer.
        pictures: u32,
    },
    /// What was sent along with the message above (`playhead`, `selection`, `frame`, …).
    Attached(Vec<String>),
    /// A line from the system: a retry, a limit reached, a failure. `warn` shows it in amber.
    Note {
        text: String,
        warn: bool,
    },
}

/// The conversation of one thread as the panel shows it.
#[derive(Default)]
pub struct ChatLog {
    pub entries: Vec<Entry>,
    /// The next event index to ask `assistant.thread` for.
    pub next: u64,
    /// Where the current step's entries begin: a restarted step discards them.
    step_start: usize,
}

/// A tool's input as one short line: `to: 6, from: "00:00:10:00"`.
pub fn input_summary(v: &Value) -> String {
    let Some(o) = v.as_object() else { return String::new() };
    let parts: Vec<String> = o.iter().map(|(k, v)| format!("{k}: {}", compact(v))).collect();
    let s = parts.join(", ");
    if s.chars().count() > 120 { format!("{}…", s.chars().take(120).collect::<String>()) } else { s }
}

fn compact(v: &Value) -> String {
    match v {
        Value::String(s) => format!("{:?}", s.chars().take(40).collect::<String>()),
        other => other.to_string().chars().take(40).collect(),
    }
}

impl ChatLog {
    pub fn push_user(&mut self, text: &str) {
        self.entries.push(Entry::User(text.to_string()));
    }

    pub fn push_note(&mut self, text: &str, warn: bool) {
        self.entries.push(Entry::Note { text: text.to_string(), warn });
    }

    fn tool_mut(&mut self, id: &str) -> Option<&mut Entry> {
        self.entries.iter_mut().rev().find(|e| matches!(e, Entry::Tool { id: i, .. } if i == id))
    }

    /// Folds one event (as `assistant.thread` returns it) into the log. Unknown events are ignored.
    pub fn apply(&mut self, ev: &Value) {
        let s = |k: &str| ev.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        match ev.get("type").and_then(Value::as_str).unwrap_or("") {
            "user_message" => {
                self.push_user(&s("text"));
                let attached: Vec<String> =
                    ev.get("attached").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
                if !attached.is_empty() {
                    self.entries.push(Entry::Attached(attached));
                }
            }
            "step_started" => self.step_start = self.entries.len(),
            "step_restarted" => self.entries.truncate(self.step_start),
            "text" => match self.entries.last_mut() {
                Some(Entry::Assistant(t)) => t.push_str(&s("text")),
                _ => self.entries.push(Entry::Assistant(s("text"))),
            },
            "thinking" => match self.entries.last_mut() {
                Some(Entry::Thinking(t)) => t.push_str(&s("text")),
                _ => self.entries.push(Entry::Thinking(s("text"))),
            },
            "tool_started" => self.entries.push(Entry::Tool { id: s("id"), name: s("name"), input: String::new(), result: None, is_error: false, pictures: 0 }),
            "tool_call" => {
                let (id, name, input) = (s("id"), s("name"), input_summary(ev.get("input").unwrap_or(&Value::Null)));
                match self.tool_mut(&id) {
                    Some(Entry::Tool { input: i, .. }) => *i = input,
                    _ => self.entries.push(Entry::Tool { id, name, input, result: None, is_error: false, pictures: 0 }),
                }
            }
            "tool_result" => {
                let (id, name, content, err) = (s("id"), s("name"), s("content"), ev.get("is_error").and_then(Value::as_bool).unwrap_or(false));
                let pics = ev.get("pictures").and_then(Value::as_u64).unwrap_or(0) as u32;
                match self.tool_mut(&id) {
                    Some(Entry::Tool { result, is_error, pictures, .. }) => {
                        *result = Some(content);
                        *is_error = err;
                        *pictures = pics;
                    }
                    _ => self.entries.push(Entry::Tool { id, name, input: String::new(), result: Some(content), is_error: err, pictures: pics }),
                }
            }
            "notice" => self.push_note(&s("text"), false),
            "finished" => {
                let f = ev.get("finish").cloned().unwrap_or(Value::Null);
                match f.get("kind").and_then(Value::as_str).unwrap_or("") {
                    "step_limit" => self.push_note("I stopped at the step limit for one question. Ask me to carry on if you want more.", true),
                    "budget_reached" => self.push_note("Stopped: this thread reached its cost limit. You can raise the limit in the settings.", true),
                    "stopped" => self.push_note("Stopped.", false),
                    "refused" => self.push_note("The provider declined to answer that.", true),
                    "cut_off" => self.push_note("The reply was cut off by the output limit. Ask me to continue.", true),
                    "failed" => self.push_note(&format!("Could not finish: {}", f.get("message").and_then(Value::as_str).unwrap_or("unknown error")), true),
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// A run of text with one style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub code: bool,
}

/// The light markdown assistants use, as lines of spans: `**bold**`, `` `code` ``, and `-`, `*` or
/// `1.` list items (shown with a bullet). Headings lose their `#`s and are bold. Nothing else is
/// interpreted, and an unclosed marker is shown as it was written.
pub fn markdown(text: &str) -> Vec<Vec<Span>> {
    text.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let heading = trimmed.starts_with('#');
            let body = if heading {
                trimmed.trim_start_matches('#').trim_start().to_string()
            } else if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
                format!("• {rest}")
            } else {
                line.to_string()
            };
            let mut spans = inline(&body);
            if heading {
                spans.iter_mut().for_each(|s| s.bold = true);
            }
            spans
        })
        .collect()
}

fn inline(line: &str) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut rest = line;
    let mut push = |text: &str, bold: bool, code: bool| {
        if !text.is_empty() {
            out.push(Span { text: text.to_string(), bold, code });
        }
    };
    while !rest.is_empty() {
        let bold_at = rest.find("**");
        let code_at = rest.find('`');
        match (bold_at, code_at) {
            (Some(b), c) if c.is_none_or(|c| b < c) => {
                let after = &rest[b + 2..];
                match after.find("**") {
                    Some(end) => {
                        push(&rest[..b], false, false);
                        push(&after[..end], true, false);
                        rest = &after[end + 2..];
                    }
                    None => {
                        push(rest, false, false);
                        break;
                    }
                }
            }
            (_, Some(c)) => {
                let after = &rest[c + 1..];
                match after.find('`') {
                    Some(end) => {
                        push(&rest[..c], false, false);
                        push(&after[..end], false, true);
                        rest = &after[end + 1..];
                    }
                    None => {
                        push(rest, false, false);
                        break;
                    }
                }
            }
            _ => {
                push(rest, false, false);
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn feed(events: &[Value]) -> ChatLog {
        let mut l = ChatLog::default();
        events.iter().for_each(|e| l.apply(e));
        l
    }

    #[test]
    fn the_persons_message_opens_the_turn() {
        let l = feed(&[
            json!({"type": "user_message", "text": "why is it abrupt?"}),
            json!({"type": "step_started", "step": 1}),
            json!({"type": "text", "text": "Because."}),
        ]);
        assert_eq!(l.entries, vec![Entry::User("why is it abrupt?".into()), Entry::Assistant("Because.".into())]);
    }

    #[test]
    fn what_was_sent_along_shows_under_the_message_and_tool_cards_count_pictures() {
        let l = feed(&[
            json!({"type": "user_message", "text": "look", "attached": ["playhead", "frame"]}),
            json!({"type": "tool_started", "id": "t1", "name": "frame_view"}),
            json!({"type": "tool_result", "id": "t1", "name": "frame_view", "content": "frame", "is_error": false, "pictures": 1}),
            json!({"type": "user_message", "text": "no context"}),
        ]);
        assert_eq!(l.entries[0], Entry::User("look".into()));
        assert_eq!(l.entries[1], Entry::Attached(vec!["playhead".into(), "frame".into()]));
        assert!(matches!(&l.entries[2], Entry::Tool { pictures: 1, .. }));
        assert_eq!(l.entries[3], Entry::User("no context".into()), "no attachments, no extra line");
        assert_eq!(l.entries.len(), 4);
    }

    #[test]
    fn streamed_text_joins_into_one_message() {
        let l = feed(&[
            json!({"type": "step_started", "step": 1}),
            json!({"type": "text", "text": "Hel"}),
            json!({"type": "text", "text": "lo"}),
            json!({"type": "finished", "finish": {"kind": "done"}}),
        ]);
        assert_eq!(l.entries, vec![Entry::Assistant("Hello".into())]);
    }

    #[test]
    fn a_tool_card_goes_from_started_to_called_to_answered() {
        let mut l = ChatLog::default();
        l.apply(&json!({"type": "tool_started", "id": "t1", "name": "sequence_view"}));
        assert!(matches!(&l.entries[0], Entry::Tool { result: None, .. }));
        l.apply(&json!({"type": "tool_call", "id": "t1", "name": "sequence_view", "input": {"to": 6, "from": "00:00:01:00"}}));
        l.apply(&json!({"type": "tool_result", "id": "t1", "name": "sequence_view", "content": "sequence \"Main Edit\"", "is_error": false}));
        assert_eq!(l.entries.len(), 1, "one card, not three");
        let Entry::Tool { name, input, result, is_error, .. } = &l.entries[0] else { panic!() };
        assert_eq!(
            (name.as_str(), input.as_str(), result.as_deref(), *is_error),
            ("sequence_view", "from: \"00:00:01:00\", to: 6", Some("sequence \"Main Edit\""), false)
        );
    }

    #[test]
    fn a_call_with_no_start_event_still_makes_a_card_and_errors_show() {
        let l = feed(&[
            json!({"type": "tool_call", "id": "x", "name": "clip_detail", "input": {}}),
            json!({"type": "tool_result", "id": "x", "name": "clip_detail", "content": "no such clip", "is_error": true}),
        ]);
        assert!(matches!(&l.entries[0], Entry::Tool { is_error: true, result: Some(r), .. } if r == "no such clip"));
        // a result for an unknown id is not lost either
        let l = feed(&[json!({"type": "tool_result", "id": "zz", "name": "n", "content": "c", "is_error": false})]);
        assert_eq!(l.entries.len(), 1);
    }

    #[test]
    fn a_restarted_step_discards_what_it_had_shown_but_not_earlier_steps() {
        let l = feed(&[
            json!({"type": "step_started", "step": 1}),
            json!({"type": "text", "text": "First step text. "}),
            json!({"type": "tool_started", "id": "t1", "name": "a"}),
            json!({"type": "step_started", "step": 2}),
            json!({"type": "thinking", "text": "half a thought"}),
            json!({"type": "step_restarted"}),
            json!({"type": "text", "text": "Second try."}),
        ]);
        assert_eq!(l.entries.len(), 3);
        assert_eq!(l.entries[2], Entry::Assistant("Second try.".into()));
        assert!(matches!(l.entries[1], Entry::Tool { .. }));
    }

    #[test]
    fn thinking_stays_apart_from_the_answer() {
        let l = feed(&[
            json!({"type": "thinking", "text": "The cut "}),
            json!({"type": "thinking", "text": "jumps."}),
            json!({"type": "text", "text": "It jumps."}),
        ]);
        assert_eq!(l.entries, vec![Entry::Thinking("The cut jumps.".into()), Entry::Assistant("It jumps.".into())]);
    }

    #[test]
    fn every_way_a_turn_can_end_has_a_plain_note() {
        let note = |f: Value| {
            let l = feed(&[json!({"type": "finished", "finish": f})]);
            l.entries.first().cloned()
        };
        assert_eq!(note(json!({"kind": "done"})), None);
        assert!(matches!(note(json!({"kind": "step_limit"})), Some(Entry::Note { warn: true, .. })));
        assert!(matches!(note(json!({"kind": "budget_reached"})), Some(Entry::Note { warn: true, text }) if text.contains("cost limit")));
        assert!(matches!(note(json!({"kind": "stopped"})), Some(Entry::Note { warn: false, text }) if text == "Stopped."));
        assert!(matches!(note(json!({"kind": "refused"})), Some(Entry::Note { warn: true, .. })));
        assert!(matches!(note(json!({"kind": "cut_off"})), Some(Entry::Note { warn: true, .. })));
        assert!(
            matches!(note(json!({"kind": "failed", "message": "the provider refused the key"})), Some(Entry::Note { warn: true, text }) if text.contains("refused the key"))
        );
        assert_eq!(note(json!({"kind": "from_the_future"})), None);
    }

    #[test]
    fn odd_events_are_ignored_not_fatal() {
        let l = feed(&[json!(null), json!(5), json!({}), json!({"type": 7}), json!({"type": "usage", "reply": {}, "total": {}}), json!({"type": "text"})]);
        assert_eq!(l.entries, vec![Entry::Assistant(String::new())]);
    }

    #[test]
    fn tool_inputs_are_summarised_short() {
        assert_eq!(input_summary(&json!({})), "");
        assert_eq!(input_summary(&json!(null)), "");
        assert_eq!(input_summary(&json!({"a": "x", "b": 2})), "a: \"x\", b: 2");
        let long = input_summary(&json!({"text": "y".repeat(500), "more": "z".repeat(500)}));
        assert!(long.chars().count() <= 121, "{}", long.chars().count());
    }

    fn plain(lines: &[Vec<Span>]) -> Vec<String> {
        lines.iter().map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>()).collect()
    }

    #[test]
    fn markdown_reads_bold_code_lists_and_headings() {
        let m = markdown("The first clip is **Ocean_Sunset.mp4** at `00:00:04:23`.\n- one\n* two\n## Pacing\nplain");
        assert_eq!(plain(&m), ["The first clip is Ocean_Sunset.mp4 at 00:00:04:23.", "• one", "• two", "Pacing", "plain"]);
        assert_eq!(m[0][1], Span { text: "Ocean_Sunset.mp4".into(), bold: true, code: false });
        assert_eq!(m[0][3], Span { text: "00:00:04:23".into(), bold: false, code: true });
        assert!(m[3].iter().all(|s| s.bold), "headings are bold");
    }

    #[test]
    fn unclosed_markers_and_hostile_text_are_shown_as_written() {
        assert_eq!(plain(&markdown("a **b and `c")), ["a **b and `c"]);
        assert_eq!(plain(&markdown("")), Vec::<String>::new());
        assert_eq!(plain(&markdown("****")), [""], "an empty bold run is just an empty line");
        for odd in ["`", "**", "`**`", "**`**`", "é**é**é", &"*".repeat(10_000), &"`a`".repeat(5_000)] {
            let _ = markdown(odd);
        }
    }
}
