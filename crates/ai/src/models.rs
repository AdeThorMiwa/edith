//! What the model picker shows, and which model to use. Nothing is hidden: a model without a
//! capability still appears, with a badge missing.

use edith_llm::{ModelInfo, Price};

/// "1M" or "200K" for a token count; "?" when unknown.
pub fn tokens_label(n: u64) -> String {
    match n {
        0 => "?".into(),
        n if n >= 1_000_000 && n % 1_000_000 == 0 => format!("{}M", n / 1_000_000),
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1_000_000.0),
        n if n >= 1000 => format!("{}K", n / 1000),
        n => n.to_string(),
    }
}

/// "$0.10 in, $0.50 out per million tokens" (an estimate, and the badge says so).
pub fn price_label(p: &Price) -> String {
    format!("${:.2} in, ${:.2} out per million tokens (estimate)", p.input_per_mtok, p.output_per_mtok)
}

/// The badges for a model, in a fixed order: what it can do, its context size, its price.
pub fn badges(m: &ModelInfo) -> Vec<String> {
    let c = &m.capabilities;
    let mut out = Vec::new();
    for (on, label) in [(c.tools, "tools"), (c.images, "images"), (c.thinking, "thinking"), (c.effort, "effort"), (c.caching, "caching")] {
        if on {
            out.push(label.to_string());
        }
    }
    if c.context_tokens > 0 {
        out.push(format!("{} context", tokens_label(c.context_tokens)));
    }
    if let Some(p) = &m.price {
        out.push(if p.is_free() { "free".to_string() } else { price_label(p) });
    }
    out
}

/// The model to use: `preferred` if the provider offers it, else the first of the usual choices it
/// does offer (cheap to capable), else its first model. `None` only when there are no models.
pub fn resolve_model<'a>(models: &'a [ModelInfo], preferred: &str) -> Option<&'a ModelInfo> {
    const USUAL: [&str; 3] = ["claude-haiku-5-5", "claude-sonnet-5-5", "claude-opus-5-5"];
    models.iter().find(|m| m.id == preferred).or_else(|| USUAL.iter().find_map(|id| models.iter().find(|m| m.id == *id))).or_else(|| models.first())
}

#[cfg(test)]
mod tests {
    use super::*;
    use edith_llm::Capabilities;

    fn model(id: &str, thinking: bool, price: Option<Price>) -> ModelInfo {
        ModelInfo {
            id: id.into(),
            display_name: id.into(),
            capabilities: Capabilities {
                tools: true,
                parallel_tools: true,
                images: true,
                thinking,
                effort: thinking,
                caching: true,
                context_tokens: 1_000_000,
                max_output_tokens: 128_000,
            },
            price,
        }
    }

    #[test]
    fn badges_show_capabilities_context_and_an_estimated_price() {
        let p = Price { input_per_mtok: 0.10, output_per_mtok: 0.50, cache_read_per_mtok: None, source: "t".into() };
        let b = badges(&model("m", true, Some(p)));
        assert_eq!(b[..5], ["tools", "images", "thinking", "effort", "caching"]);
        assert_eq!(b[5], "1M context");
        assert_eq!(b[6], "$0.10 in, $0.50 out per million tokens (estimate)");
        // a model lacking things is still shown, with fewer badges
        let plain = badges(&model("old", false, None));
        assert!(!plain.contains(&"thinking".to_string()) && plain.contains(&"tools".to_string()) && plain.len() == 4);
        // a model that costs nothing says so instead of "$0.00"
        let free = Price { input_per_mtok: 0.0, output_per_mtok: 0.0, cache_read_per_mtok: None, source: "t".into() };
        assert_eq!(badges(&model("f", false, Some(free))).last().map(String::as_str), Some("free"));
    }

    #[test]
    fn token_counts_read_naturally() {
        assert_eq!(
            [tokens_label(0), tokens_label(950), tokens_label(200_000), tokens_label(1_000_000), tokens_label(1_500_000)],
            ["?", "950", "200K", "1M", "1.5M"]
        );
    }

    #[test]
    fn the_preferred_model_wins_then_the_usual_ones_then_anything() {
        let list = vec![model("claude-opus-5-5", true, None), model("claude-sonnet-5-5", true, None), model("other", true, None)];
        assert_eq!(resolve_model(&list, "other").map(|m| m.id.as_str()), Some("other"));
        assert_eq!(resolve_model(&list, "gone").map(|m| m.id.as_str()), Some("claude-sonnet-5-5"), "haiku is missing, so the next usual one");
        let odd = vec![model("zzz", true, None)];
        assert_eq!(resolve_model(&odd, "gone").map(|m| m.id.as_str()), Some("zzz"));
        assert!(resolve_model(&[], "x").is_none());
    }
}
