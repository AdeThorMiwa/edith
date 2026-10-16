//! Turning what a person types into what a site searches for. A description is cut down to its
//! content words by simple rules (the first version has no model): "a lone figure walking through
//! fog at dawn" becomes `lone figure walking fog dawn`.

const STOP: &[&str] = &[
    "a", "an", "the", "of", "in", "on", "at", "to", "for", "from", "with", "and", "or", "by", "as", "is", "are", "was", "were", "be", "it", "its", "this",
    "that", "these", "those", "into", "through", "over", "under", "near", "some", "any", "very", "just", "my", "our", "your", "i", "we", "you", "show", "shot",
    "footage", "video", "clip", "picture", "image", "photo", "sound", "audio", "of", "like",
];

/// Content words of `text`, in order, at most `max_words`, at most `max_chars` characters in all.
pub fn keywords(text: &str, max_words: usize, max_chars: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut len = 0usize;
    for w in text.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '-')) {
        let w = w.trim_matches(|c| c == '\'' || c == '-').to_lowercase();
        if w.is_empty() || STOP.contains(&w.as_str()) || out.contains(&w) {
            continue;
        }
        let add = w.chars().count() + usize::from(!out.is_empty());
        if out.len() >= max_words || len + add > max_chars {
            break;
        }
        len += add;
        out.push(w);
    }
    // a request made only of filler words ("a video of it") still searches for something
    if out.is_empty() { text.chars().filter(|c| !c.is_control()).take(max_chars).collect::<String>().trim().to_string() } else { out.join(" ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_description_becomes_its_content_words() {
        assert_eq!(keywords("a lone figure walking through fog at dawn", 8, 100), "lone figure walking fog dawn");
        assert_eq!(keywords("Show me a video of the Ocean, at SUNSET!", 8, 100), "me ocean sunset");
        assert_eq!(keywords("city night drive city", 8, 100), "city night drive", "a word is used once");
    }

    #[test]
    fn the_result_is_bounded_and_never_empty_for_a_real_request() {
        assert_eq!(keywords("one two three four five six", 3, 100), "one two three");
        assert!(keywords(&"word ".repeat(200).replace("word", "abcdefghij"), 50, 100).chars().count() <= 100);
        assert_eq!(keywords("a video of it", 8, 100), "a video of it");
        assert_eq!(keywords("", 8, 100), "");
        assert_eq!(keywords("café-au-lait 'quoted' don't", 8, 100), "café-au-lait quoted don't");
    }
}
