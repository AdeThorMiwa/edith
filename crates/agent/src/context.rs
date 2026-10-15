//! What the model is told about the project, and how big results are kept in check.

use edith_llm::{Message, Part, Role};

/// A labelled piece of context, such as the selection or the transcript around the playhead. The
/// person sees the same blocks (the chips), so there is nothing the model sees that they do not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextBlock {
    pub label: String,
    pub text: String,
    /// A picture to go with the text, such as the frame at the playhead.
    pub image: Option<edith_llm::Image>,
}

impl ContextBlock {
    pub fn text(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self { label: label.into(), text: text.into(), image: None }
    }
}

/// `text` cut to `max_chars` characters, with a note of how much was left out.
pub fn clip_text(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars).collect();
    format!("{kept}\n[cut: {} more characters not shown]", total - max_chars)
}

/// The user message for a turn: one text part per context block, then what the person typed.
/// Context goes in the message, not the system prompt, so the cached prompt prefix stays stable.
pub fn user_message(blocks: &[ContextBlock], text: &str, max_block_chars: usize) -> Message {
    let mut parts: Vec<Part> = Vec::new();
    for b in blocks {
        parts.push(Part::Text { text: format!("[{}]\n{}", b.label, clip_text(&b.text, max_block_chars)) });
        if let Some(i) = &b.image {
            parts.push(Part::Image { media_type: i.media_type.clone(), data: i.data.clone() });
        }
    }
    parts.push(Part::Text { text: text.to_string() });
    Message { role: Role::User, parts }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_is_cut_with_a_note_and_short_text_is_untouched() {
        assert_eq!(clip_text("short", 10), "short");
        let c = clip_text(&"é".repeat(50), 10);
        assert!(c.starts_with(&"é".repeat(10)) && c.contains("40 more characters"), "{c}");
        assert_eq!(clip_text("", 0), "");
    }

    #[test]
    fn a_context_picture_follows_its_label_and_comes_before_the_question() {
        let img = edith_llm::Image { media_type: "image/jpeg".into(), data: "QUJD".into() };
        let m = user_message(&[ContextBlock { label: "frame".into(), text: "the frame at 00:00:04:23".into(), image: Some(img) }], "what is wrong here?", 1000);
        assert_eq!(m.parts.len(), 3);
        assert!(matches!(&m.parts[1], Part::Image { media_type, data } if media_type == "image/jpeg" && data == "QUJD"));
        assert!(matches!(&m.parts[2], Part::Text { text } if text == "what is wrong here?"));
    }

    #[test]
    fn context_blocks_come_before_the_question_and_are_labelled() {
        let m = user_message(&[ContextBlock::text("selection", "clip A at 0:42")], "why is this abrupt?", 1000);
        assert_eq!(m.parts.len(), 2);
        assert_eq!(m.text(), "[selection]\nclip A at 0:42why is this abrupt?");
        assert_eq!(m.role, Role::User);
        let big = user_message(&[ContextBlock::text("t", "x".repeat(5000))], "q", 100);
        assert!(big.text().len() < 300);
    }
}
