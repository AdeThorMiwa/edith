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
        /// A view for the person (a timeline, a findings card…) the assistant placed with `show`.
        view: Option<Value>,
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
            "tool_started" => {
                self.entries.push(Entry::Tool { id: s("id"), name: s("name"), input: String::new(), result: None, is_error: false, pictures: 0, view: None })
            }
            "tool_call" => {
                let (id, name, input) = (s("id"), s("name"), input_summary(ev.get("input").unwrap_or(&Value::Null)));
                match self.tool_mut(&id) {
                    Some(Entry::Tool { input: i, .. }) => *i = input,
                    _ => self.entries.push(Entry::Tool { id, name, input, result: None, is_error: false, pictures: 0, view: None }),
                }
            }
            "tool_result" => {
                let (id, name, content, err) = (s("id"), s("name"), s("content"), ev.get("is_error").and_then(Value::as_bool).unwrap_or(false));
                let pics = ev.get("pictures").and_then(Value::as_u64).unwrap_or(0) as u32;
                let shown = ev.get("view").filter(|v| v.is_object()).cloned();
                match self.tool_mut(&id) {
                    Some(Entry::Tool { result, is_error, pictures, view, .. }) => {
                        *result = Some(content);
                        *is_error = err;
                        *pictures = pics;
                        *view = shown;
                    }
                    _ => self.entries.push(Entry::Tool { id, name, input: String::new(), result: Some(content), is_error: err, pictures: pics, view: shown }),
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

/// One block of an answer, as the panel draws it.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Paragraph(Vec<Span>),
    Heading(Vec<Span>),
    /// A list item. `depth` is how far it is indented (0 to 3); `marker` is the bullet or `1.`.
    Item {
        depth: usize,
        marker: String,
        spans: Vec<Span>,
    },
    Code(String),
    Table {
        header: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
    },
    Rule,
}

const MAX_BLOCKS: usize = 2000;
const MAX_TABLE_COLUMNS: usize = 12;
const MAX_TABLE_ROWS: usize = 200;

fn table_cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').take(MAX_TABLE_COLUMNS).map(|c| c.trim().to_string()).collect()
}

fn is_table_divider(line: &str) -> bool {
    let cells = table_cells(line);
    line.contains('-') && !cells.is_empty() && cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')) && c.contains('-'))
}

fn list_item(line: &str) -> Option<(usize, String, String)> {
    let indent = line.len() - line.trim_start().len();
    let depth = (indent / 2).min(3);
    let t = line.trim_start();
    for bullet in ["- ", "* ", "• "] {
        if let Some(rest) = t.strip_prefix(bullet) {
            return Some((depth, "•".to_string(), rest.to_string()));
        }
    }
    let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
    if (1..=3).contains(&digits.len())
        && let Some(rest) = t[digits.len()..].strip_prefix(". ").or_else(|| t[digits.len()..].strip_prefix(") "))
    {
        return Some((depth, format!("{digits}."), rest.to_string()));
    }
    None
}

/// The light markdown assistants use, as blocks: paragraphs, headings, bullet and numbered lists,
/// fenced code, tables and rules, with `**bold**` and `` `code` `` inside text. Nothing else is
/// interpreted; anything unclear is shown as it was written. Bounded, so a hostile answer cannot
/// make the panel allocate without limit.
pub fn blocks(text: &str) -> Vec<Block> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<Block> = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let flush = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Paragraph(inline(&para.join("\n"))));
            para.clear();
        }
    };
    let mut i = 0;
    while i < lines.len() && out.len() < MAX_BLOCKS {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            flush(&mut para, &mut out);
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim().starts_with("```") {
                code.push(lines[i]);
                i += 1;
            }
            out.push(Block::Code(code.join("\n")));
            i += 1;
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut para, &mut out);
        } else if trimmed.starts_with('|') && lines.get(i + 1).is_some_and(|n| is_table_divider(n)) {
            flush(&mut para, &mut out);
            let header: Vec<Vec<Span>> = table_cells(trimmed).iter().map(|c| inline(c)).collect();
            let mut rows = Vec::new();
            i += 2;
            while i < lines.len() && lines[i].trim().starts_with('|') && rows.len() < MAX_TABLE_ROWS {
                rows.push(table_cells(lines[i]).iter().map(|c| inline(c)).collect());
                i += 1;
            }
            out.push(Block::Table { header, rows });
            continue;
        } else if trimmed.len() >= 3 && trimmed.chars().all(|c| c == '-') || trimmed.len() >= 3 && trimmed.chars().all(|c| c == '*') {
            flush(&mut para, &mut out);
            out.push(Block::Rule);
        } else if trimmed.starts_with('#') {
            flush(&mut para, &mut out);
            let mut spans = inline(trimmed.trim_start_matches('#').trim_start());
            spans.iter_mut().for_each(|s| s.bold = true);
            out.push(Block::Heading(spans));
        } else if let Some((depth, marker, rest)) = list_item(line) {
            flush(&mut para, &mut out);
            out.push(Block::Item { depth, marker, spans: inline(&rest) });
        } else {
            para.push(line);
        }
        i += 1;
    }
    flush(&mut para, &mut out);
    out
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

    fn flat(spans: &[Span]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn answers_become_paragraphs_lists_code_tables_and_rules() {
        let b = blocks(
            "The edit is **26 s** long.\nSecond line.\n\n## Pacing\n- one\n  - nested\n1. first\n2) second\n\n```\nlet x = 1;\n\nlet y = 2;\n```\n---\nafter",
        );
        assert_eq!(b.len(), 9, "{b:#?}");
        assert!(matches!(&b[0], Block::Paragraph(s) if flat(s) == "The edit is 26 s long.\nSecond line."));
        assert!(matches!(&b[1], Block::Heading(s) if flat(s) == "Pacing" && s.iter().all(|x| x.bold)));
        assert!(matches!(&b[2], Block::Item { depth: 0, marker, spans } if marker == "•" && flat(spans) == "one"));
        assert!(matches!(&b[3], Block::Item { depth: 1, .. }));
        assert!(matches!(&b[4], Block::Item { marker, .. } if marker == "1."));
        assert!(matches!(&b[5], Block::Item { marker, .. } if marker == "2."));
        assert!(matches!(&b[6], Block::Code(c) if c == "let x = 1;\n\nlet y = 2;"), "blank lines stay inside code");
        assert!(matches!(&b[7], Block::Rule));
        assert!(matches!(&b[8], Block::Paragraph(s) if flat(s) == "after"));
    }

    #[test]
    fn a_table_has_a_header_and_rows_and_a_pipe_in_prose_is_just_text() {
        let b = blocks("| Timecode | Name |\n|---|:--:|\n| 00:00:09:11 | **Music hit** |\n| 00:00:17:23 | Midpoint |\n\nthen text");
        let Block::Table { header, rows } = &b[0] else { panic!("{b:#?}") };
        assert_eq!((header.len(), rows.len()), (2, 2));
        assert_eq!(flat(&header[0]), "Timecode");
        assert!(rows[0][1][0].bold && flat(&rows[1][1]) == "Midpoint");
        assert!(matches!(&b[1], Block::Paragraph(_)));
        assert!(matches!(&blocks("use a | b to pipe")[0], Block::Paragraph(_)));
        assert!(matches!(&blocks("| not a table\nbecause no divider")[0], Block::Paragraph(_)));
    }

    #[test]
    fn hostile_answers_are_bounded_and_never_panic() {
        for odd in [
            "```",
            "```\nunclosed",
            "|",
            "|\n|-|",
            "| a |\n|---|",
            "#",
            "-",
            "---",
            "1.",
            "999. x",
            "- ",
            &"- x\n".repeat(10_000),
            &"|a|\n|-|\n".repeat(5_000),
            &"```\n".repeat(5_000),
            &"|a|b|c|d|e|f|g|h|i|j|k|l|m|n|o|p|\n|-|-|\n".repeat(3),
        ] {
            let b = blocks(odd);
            assert!(b.len() <= MAX_BLOCKS + 1);
            if let Some(Block::Table { header, rows }) = b.first() {
                assert!(header.len() <= MAX_TABLE_COLUMNS && rows.len() <= MAX_TABLE_ROWS);
            }
        }
        assert!(blocks("").is_empty());
    }
}
