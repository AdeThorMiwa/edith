//! The model list, and what the models cost.

use crate::{Capabilities, LlmError, ModelInfo, Price};
use serde_json::Value;

/// When the built-in price table was last checked against Anthropic's published prices.
pub const PRICES_AS_OF: &str = "2026-10-06";

/// Estimated prices in US dollars per million tokens: (model id, input, output, cache read).
/// These are published list prices at the date above; the models list does not carry prices, so
/// every number shown from here is an estimate. Haiku 5.5 is the price up to 100K-token prompts.
const PRICES: &[(&str, f64, f64, Option<f64>)] = &[
    ("claude-fable-5-1", 10.0, 50.0, Some(0.25)),
    ("claude-fable-5", 10.0, 50.0, None),
    ("claude-opus-5-5", 4.0, 20.0, Some(0.20)),
    ("claude-opus-5", 5.0, 25.0, None),
    ("claude-opus-4-8", 5.0, 25.0, None),
    ("claude-sonnet-5-5", 2.0, 10.0, Some(0.20)),
    ("claude-sonnet-5", 2.0, 10.0, None),
    ("claude-sonnet-4-6", 3.0, 15.0, None),
    ("claude-haiku-5-5", 0.10, 0.50, None),
    ("claude-haiku-4-5", 1.0, 5.0, None),
];

/// The estimated price of `model_id`, if it is in the table.
pub fn price_for(model_id: &str) -> Option<Price> {
    PRICES.iter().find(|(id, ..)| *id == model_id).map(|(_, i, o, c)| Price {
        input_per_mtok: *i,
        output_per_mtok: *o,
        cache_read_per_mtok: *c,
        source: format!("Anthropic list prices, checked {PRICES_AS_OF}"),
    })
}

fn supported(v: &Value, path: &[&str]) -> bool {
    let mut cur = v;
    for key in path {
        match cur.get(key) {
            Some(next) => cur = next,
            None => return false,
        }
    }
    cur.get("supported").and_then(Value::as_bool).unwrap_or(false)
}

/// Reads the body of `GET /v1/models` into [`ModelInfo`]s, newest first as the API returns them.
pub fn parse_models(body: &[u8]) -> Result<Vec<ModelInfo>, LlmError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| LlmError::bad_stream("the model list was not valid JSON"))?;
    let data = v.get("data").and_then(Value::as_array).ok_or_else(|| LlmError::bad_stream("the model list had no data"))?;
    let mut out = Vec::new();
    for m in data.iter().take(500) {
        let Some(id) = m.get("id").and_then(Value::as_str) else { continue };
        let caps = m.get("capabilities").cloned().unwrap_or(Value::Null);
        out.push(ModelInfo {
            id: id.chars().take(100).collect(),
            display_name: m.get("display_name").and_then(Value::as_str).unwrap_or(id).chars().take(100).collect(),
            capabilities: Capabilities {
                // Every Claude model on the Messages API can call tools, in parallel by default.
                tools: true,
                parallel_tools: true,
                images: supported(&caps, &["image_input"]),
                thinking: supported(&caps, &["thinking", "types", "adaptive"]),
                effort: supported(&caps, &["effort"]),
                caching: true,
                context_tokens: m.get("max_input_tokens").and_then(Value::as_u64).unwrap_or(0),
                max_output_tokens: m.get("max_tokens").and_then(Value::as_u64).unwrap_or(0),
            },
            price: price_for(id),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"{"data":[
      {"id":"claude-opus-5-5","display_name":"Claude Opus 5.5","max_input_tokens":1000000,"max_tokens":128000,
       "capabilities":{"image_input":{"supported":true},"thinking":{"supported":true,"types":{"enabled":{"supported":false},"adaptive":{"supported":true}}},"effort":{"supported":true,"low":{"supported":true}}}},
      {"id":"claude-haiku-4-5","display_name":"Claude Haiku 4.5","max_input_tokens":200000,"max_tokens":64000,
       "capabilities":{"image_input":{"supported":true},"thinking":{"supported":true,"types":{"enabled":{"supported":true},"adaptive":{"supported":false}}},"effort":{"supported":false}}},
      {"id":"claude-brand-new","max_input_tokens":5,"max_tokens":3},
      {"no_id":true}
    ],"has_more":false}"#;

    #[test]
    fn the_list_becomes_capability_badges() {
        let m = parse_models(LIST.as_bytes()).unwrap();
        assert_eq!(m.len(), 3);
        let opus = &m[0];
        assert_eq!(opus.display_name, "Claude Opus 5.5");
        assert!(opus.capabilities.thinking && opus.capabilities.effort && opus.capabilities.images && opus.capabilities.tools);
        assert_eq!((opus.capabilities.context_tokens, opus.capabilities.max_output_tokens), (1_000_000, 128_000));
        assert!(opus.price.is_some());
        let haiku = &m[1];
        assert!(!haiku.capabilities.thinking && !haiku.capabilities.effort, "adaptive thinking is what the badge means");
        let unknown = &m[2];
        assert_eq!(unknown.display_name, "claude-brand-new");
        assert!(unknown.price.is_none() && !unknown.capabilities.images);
    }

    #[test]
    fn a_bad_list_is_an_error() {
        assert!(parse_models(b"nope").is_err());
        assert!(parse_models(br#"{"x":1}"#).is_err());
        assert!(parse_models(br#"{"data":[]}"#).unwrap().is_empty());
    }

    #[test]
    fn prices_estimate_a_cost_and_say_where_they_come_from() {
        let p = price_for("claude-sonnet-5-5").unwrap();
        assert!(p.source.contains(PRICES_AS_OF));
        let usage = crate::Usage { input_tokens: 10_000, output_tokens: 1_000, cache_read_tokens: 50_000, cache_write_tokens: 0 };
        // 10k in at $2, 50k cached at $0.20, 1k out at $10  = 0.02 + 0.01 + 0.01
        assert!((p.estimate(&usage) - 0.04).abs() < 1e-9, "{}", p.estimate(&usage));
        assert!(price_for("nonexistent").is_none());
        // For plain (uncached) tokens Haiku 5.5 costs a twentieth of Sonnet 5.5; with mostly cached
        // input the gap narrows (Sonnet's cache-read price is already low), here to about six times.
        let haiku = price_for("claude-haiku-5-5").unwrap();
        let plain = crate::Usage { input_tokens: 100_000, output_tokens: 10_000, ..Default::default() };
        assert!((p.estimate(&plain) / haiku.estimate(&plain) - 20.0).abs() < 1e-6);
        let ratio = p.estimate(&usage) / haiku.estimate(&usage);
        assert!((5.0..7.0).contains(&ratio), "{ratio}");
    }
}
