//! The Direction menu of the Text to Speech panel (fork roadmap F06.3): put a direction marker
//! (`[pace 0.9]`, `[pitch -2]`, `[normal]`) into the script where the cursor is, so what follows is
//! read slower, faster, lower or higher. The markers are plain text; typing them works the same.
//!
//! Automation ids: `tts.direction` (the menu), `tts.direction.slower|faster|lower|higher|normal`.

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;

/// The egui id of the script's text box.
pub fn text_id() -> egui::Id {
    egui::Id::new("tts-text")
}

/// The menu's items: automation id, label, marker.
pub fn items() -> [(&'static str, &'static str, &'static str); 5] {
    [
        ("slower", tl!("Slower"), "[pace 0.9]"),
        ("faster", tl!("Faster"), "[pace 1.1]"),
        ("lower", tl!("Lower voice"), "[pitch -2]"),
        ("higher", tl!("Higher voice"), "[pitch +2]"),
        ("normal", tl!("Back to normal"), "[normal]"),
    ]
}

/// `text` with `marker` put in at character position `at` (past the end counts as the end), a space
/// before it unless it starts a line or follows a space, and one after it. Returns the new text and
/// the character position just after the marker and the space that follows it.
pub fn splice(text: &str, at: usize, marker: &str) -> (String, usize) {
    let total = text.chars().count();
    let at = at.min(total);
    let byte = text.char_indices().nth(at).map_or(text.len(), |(b, _)| b);
    let (head, tail) = text.split_at(byte);
    let lead = !head.is_empty() && !head.ends_with(char::is_whitespace);
    let trail = !tail.starts_with(' ');
    let piece = format!("{}{marker}{}", if lead { " " } else { "" }, if trail { " " } else { "" });
    let pos = at + piece.chars().count() + usize::from(!trail);
    (format!("{head}{piece}{tail}"), pos)
}

/// Put `marker` into `text` at the cursor of the text box `id` (at the end when it has none) and
/// move the cursor after it.
pub fn insert_at_cursor(ctx: &egui::Context, id: egui::Id, text: &mut String, marker: &str) {
    let mut state = TextEditState::load(ctx, id).unwrap_or_default();
    let at = state.cursor.char_range().map_or(usize::MAX, |r| r.primary.index.into());
    let (new, pos) = splice(text, at, marker);
    *text = new;
    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(pos))));
    state.store(ctx, id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_marker_goes_in_at_the_cursor_with_tidy_spacing() {
        assert_eq!(splice("", 0, "[normal]"), ("[normal] ".to_string(), 9));
        assert_eq!(splice("Hello there.", 0, "[pace 0.9]"), ("[pace 0.9] Hello there.".to_string(), 11));
        assert_eq!(splice("One. Two.", 5, "[pitch -2]"), ("One. [pitch -2] Two.".to_string(), 16));
        assert_eq!(splice("One.", 4, "[pitch -2]"), ("One. [pitch -2] ".to_string(), 16));
        assert_eq!(splice("One.", usize::MAX, "[normal]"), ("One. [normal] ".to_string(), 14));
        assert_eq!(splice("One.\nTwo.", 5, "[pace 1.1]"), ("One.\n[pace 1.1] Two.".to_string(), 16));
        // a space already after the cursor is not doubled
        assert_eq!(splice("One. Two.", 4, "[normal]"), ("One. [normal] Two.".to_string(), 14));
        // accented and wide characters count as one position each
        assert_eq!(splice("Réykjavík. Ok.", 11, "[pace 0.9]"), ("Réykjavík. [pace 0.9] Ok.".to_string(), 22));
        assert_eq!(splice("日本語", 1, "[normal]"), ("日 [normal] 本語".to_string(), 11));
    }
}
