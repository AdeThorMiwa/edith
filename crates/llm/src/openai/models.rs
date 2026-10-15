//! Reading the model list (`GET {base}/models`).
//!
//! OpenRouter's list is rich: context size, modalities, supported parameters and prices (as
//! strings, in dollars per token). The plain OpenAI-style list that OpenAI, Ollama and LM Studio
//! return has only ids, so those models appear with no capability badges: unknown is shown as
//! unknown, never guessed.

use crate::{Capabilities, LlmError, ModelInfo, Price};
use serde_json::Value;

/// Most models read from one list.
const MAX_MODELS: usize = 2000;

fn strings(v: Option<&Value>) -> Vec<&str> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
}

/// Dollars per token, written as a string (`"0.000002"`), as dollars per million tokens. `None`
/// for anything that is not a finite, non-negative number (OpenRouter uses `-1` for models whose
/// price depends on how they are routed).
fn per_mtok(v: Option<&Value>) -> Option<f64> {
    let n = match v? {
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    (n.is_finite() && n >= 0.0).then_some(n * 1_000_000.0)
}

fn openrouter_model(id: &str, m: &Value) -> Option<ModelInfo> {
    let arch = m.get("architecture").unwrap_or(&Value::Null);
    // A model that does not produce text (images, embeddings, speech) is not a chat model.
    let outputs = strings(arch.get("output_modalities"));
    if !outputs.is_empty() && !outputs.contains(&"text") {
        return None;
    }
    let params = strings(m.get("supported_parameters"));
    let top = m.get("top_provider").unwrap_or(&Value::Null);
    let pricing = m.get("pricing").unwrap_or(&Value::Null);
    let cache_read = per_mtok(pricing.get("input_cache_read"));
    let price = match (per_mtok(pricing.get("prompt")), per_mtok(pricing.get("completion"))) {
        (Some(input), Some(output)) => {
            Some(Price { input_per_mtok: input, output_per_mtok: output, cache_read_per_mtok: cache_read, source: "OpenRouter's live model list".to_string() })
        }
        _ => None,
    };
    let reasoning = m.get("reasoning").filter(|r| r.is_object());
    let context = m.get("context_length").and_then(Value::as_u64).or_else(|| top.get("context_length").and_then(Value::as_u64)).unwrap_or(0);
    Some(ModelInfo {
        id: id.chars().take(100).collect(),
        display_name: m.get("name").and_then(Value::as_str).unwrap_or(id).chars().take(100).collect(),
        capabilities: Capabilities {
            tools: params.contains(&"tools"),
            parallel_tools: params.contains(&"parallel_tool_calls"),
            images: strings(arch.get("input_modalities")).contains(&"image"),
            thinking: reasoning.is_some() || params.contains(&"reasoning") || params.contains(&"include_reasoning"),
            effort: params.contains(&"reasoning_effort")
                || reasoning.and_then(|r| r.get("supported_efforts")).and_then(Value::as_array).is_some_and(|a| !a.is_empty()),
            caching: cache_read.is_some_and(|c| c > 0.0),
            context_tokens: context,
            max_output_tokens: top.get("max_completion_tokens").and_then(Value::as_u64).unwrap_or(0),
        },
        price,
    })
}

/// Reads the body of `GET {base}/models` into [`ModelInfo`]s, in the order the server gave.
pub fn parse_models(body: &[u8]) -> Result<Vec<ModelInfo>, LlmError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| LlmError::bad_stream("the model list was not valid JSON"))?;
    let data = v.get("data").and_then(Value::as_array).ok_or_else(|| LlmError::bad_stream("the model list had no data"))?;
    let mut out = Vec::new();
    for m in data.iter().take(MAX_MODELS) {
        let Some(id) = m.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) else { continue };
        if m.get("architecture").is_some_and(Value::is_object) {
            out.extend(openrouter_model(id, m));
        } else {
            out.push(ModelInfo {
                id: id.chars().take(100).collect(),
                display_name: id.chars().take(100).collect(),
                capabilities: Capabilities::default(),
                price: None,
            });
        }
    }
    Ok(out)
}

/// What a local server says about one model.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalModel {
    /// Can chat (embedding models cannot).
    pub chat: bool,
    pub capabilities: Capabilities,
}

/// Ollama's `POST /api/show` answer: `capabilities` is a list of words (`completion`, `vision`,
/// `tools`, `thinking`, `embedding`) and the context length sits in `model_info` under a key that
/// ends in `.context_length`. `None` when the answer is not readable.
pub fn parse_ollama_show(body: &[u8]) -> Option<LocalModel> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let words = strings(v.get("capabilities"));
    // Older servers send no capabilities at all: then nothing is known, and the model stays a chat model.
    let known = v.get("capabilities").is_some_and(Value::is_array);
    let context = v
        .get("model_info")
        .and_then(Value::as_object)
        .and_then(|o| o.iter().find(|(k, _)| k.ends_with(".context_length")).and_then(|(_, n)| n.as_u64()))
        .unwrap_or(0);
    Some(LocalModel {
        chat: !known || words.contains(&"completion"),
        capabilities: Capabilities {
            tools: words.contains(&"tools"),
            images: words.contains(&"vision"),
            thinking: words.contains(&"thinking"),
            context_tokens: context,
            ..Capabilities::default()
        },
    })
}

/// LM Studio's `GET /api/v0/models`: for each model its id, whether it is a language model (`llm`),
/// a vision model (`vlm`) or an embedding model, and its largest context. What it says about tool
/// use is not documented, so that is left unknown.
pub fn parse_lmstudio_models(body: &[u8]) -> Vec<(String, LocalModel)> {
    let Ok(v) = serde_json::from_slice::<Value>(body) else { return Vec::new() };
    let Some(data) = v.get("data").and_then(Value::as_array) else { return Vec::new() };
    data.iter()
        .take(MAX_MODELS)
        .filter_map(|m| {
            let id = m.get("id").and_then(Value::as_str).filter(|id| !id.is_empty())?;
            let kind = m.get("type").and_then(Value::as_str).unwrap_or("");
            Some((
                id.chars().take(100).collect(),
                LocalModel {
                    chat: kind != "embeddings",
                    capabilities: Capabilities {
                        images: kind == "vlm",
                        context_tokens: m.get("max_context_length").and_then(Value::as_u64).unwrap_or(0),
                        ..Capabilities::default()
                    },
                },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPENROUTER: &str = r#"{"data":[
      {"id":"anthropic/claude-haiku-5-5","name":"Anthropic: Claude Haiku 5.5","context_length":1000000,
       "architecture":{"modality":"text+image->text","input_modalities":["text","image"],"output_modalities":["text"]},
       "pricing":{"prompt":"0.0000001","completion":"0.0000005","input_cache_read":"0.00000001"},
       "top_provider":{"context_length":1000000,"max_completion_tokens":128000},
       "supported_parameters":["tools","tool_choice","parallel_tool_calls","reasoning","include_reasoning","max_tokens"],
       "reasoning":{"mandatory":false,"supported_efforts":["low","high"]}},
      {"id":"meta/free-model:free","name":"Free Model","context_length":32768,
       "architecture":{"input_modalities":["text"],"output_modalities":["text"]},
       "pricing":{"prompt":"0","completion":"0"},"top_provider":{"max_completion_tokens":null},"supported_parameters":["max_tokens"]},
      {"id":"openrouter/auto","name":"Auto Router","context_length":2000000,
       "architecture":{"input_modalities":["text"],"output_modalities":["text"]},"pricing":{"prompt":"-1","completion":"-1"},"supported_parameters":["tools"]},
      {"id":"black-forest/image-model","name":"Image","architecture":{"input_modalities":["text"],"output_modalities":["image"]},"pricing":{"prompt":"0.1","completion":"0.1"}},
      {"id":"x/embed","architecture":{"input_modalities":["text"],"output_modalities":["embeddings"]}},
      {"id":"x/odd","architecture":{},"pricing":{"prompt":"lots","completion":5}},
      {"no_id":true}
    ]}"#;

    #[test]
    fn an_openrouter_list_becomes_capability_and_price_badges() {
        let m = parse_models(OPENROUTER.as_bytes()).unwrap();
        let ids: Vec<_> = m.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["anthropic/claude-haiku-5-5", "meta/free-model:free", "openrouter/auto", "x/odd"], "image and embedding models are not chat models");
        let h = &m[0];
        assert_eq!(h.display_name, "Anthropic: Claude Haiku 5.5");
        let c = &h.capabilities;
        assert!(c.tools && c.parallel_tools && c.images && c.thinking && c.effort && c.caching);
        assert_eq!((c.context_tokens, c.max_output_tokens), (1_000_000, 128_000));
        let p = h.price.as_ref().unwrap();
        assert!((p.input_per_mtok - 0.1).abs() < 1e-9 && (p.output_per_mtok - 0.5).abs() < 1e-9 && (p.cache_read_per_mtok.unwrap() - 0.01).abs() < 1e-9);
        let free = &m[1];
        assert!(free.price.as_ref().is_some_and(Price::is_free));
        assert!(!free.capabilities.tools && !free.capabilities.images, "a model without tools is still listed, with the badge missing");
        assert!(m[2].price.is_none(), "a router's price is dynamic: no estimate");
        assert!(m[3].price.is_none(), "an unreadable price is no price");
    }

    #[test]
    fn a_plain_list_has_ids_and_no_badges() {
        let m = parse_models(
            br#"{"object":"list","data":[{"id":"llama3.2:3b","object":"model","owned_by":"library"},{"id":"qwen2.5-vl","object":"model"},{"id":""},5]}"#,
        )
        .unwrap();
        assert_eq!(m.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["llama3.2:3b", "qwen2.5-vl"]);
        assert_eq!(m[0].capabilities, Capabilities::default());
        assert!(m[0].price.is_none());
    }

    #[test]
    fn a_bad_list_is_an_error_and_a_huge_one_is_capped() {
        assert!(parse_models(b"nope").is_err());
        assert!(parse_models(br#"{"x":1}"#).is_err());
        assert!(parse_models(br#"{"data":[]}"#).unwrap().is_empty());
        let many = format!(r#"{{"data":[{}]}}"#, (0..MAX_MODELS + 50).map(|i| format!(r#"{{"id":"m{i}"}}"#)).collect::<Vec<_>>().join(","));
        assert_eq!(parse_models(many.as_bytes()).unwrap().len(), MAX_MODELS);
    }

    #[test]
    fn ollama_says_what_each_model_can_do() {
        let m = parse_ollama_show(
            br#"{"capabilities":["completion","vision","tools"],"model_info":{"general.architecture":"llama","llama.context_length":131072}}"#,
        )
        .unwrap();
        assert!(m.chat && m.capabilities.tools && m.capabilities.images && !m.capabilities.thinking);
        assert_eq!(m.capabilities.context_tokens, 131_072);
        let think = parse_ollama_show(br#"{"capabilities":["completion","thinking"],"model_info":{"qwen3.context_length":40960}}"#).unwrap();
        assert!(think.capabilities.thinking && !think.capabilities.tools);
        assert_eq!(think.capabilities.context_tokens, 40_960);
        let embed = parse_ollama_show(br#"{"capabilities":["embedding"]}"#).unwrap();
        assert!(!embed.chat, "an embedding model cannot chat");
        let old = parse_ollama_show(br#"{"details":{"family":"llama"}}"#).unwrap();
        assert!(old.chat && old.capabilities == Capabilities::default(), "an old server says nothing, so nothing is claimed");
        for bad in [&b"nope"[..], b"[]", b""] {
            assert!(parse_ollama_show(bad).is_none_or(|m| m.capabilities == Capabilities::default()), "{bad:?}");
        }
    }

    #[test]
    fn lm_studio_marks_vision_and_embedding_models() {
        let list = parse_lmstudio_models(
            br#"{"object":"list","data":[
              {"id":"qwen2-vl-7b-instruct","type":"vlm","max_context_length":32768,"state":"not-loaded"},
              {"id":"llama-3.2-3b","type":"llm","max_context_length":131072},
              {"id":"text-embedding-nomic","type":"embeddings","max_context_length":2048},
              {"id":""},{"type":"llm"},5]}"#,
        );
        let ids: Vec<_> = list.iter().map(|(i, _)| i.as_str()).collect();
        assert_eq!(ids, ["qwen2-vl-7b-instruct", "llama-3.2-3b", "text-embedding-nomic"]);
        assert!(list[0].1.capabilities.images && !list[1].1.capabilities.images);
        assert_eq!(list[1].1.capabilities.context_tokens, 131_072);
        assert!(list[1].1.chat && !list[2].1.chat);
        assert!(!list[1].1.capabilities.tools, "tool support is not claimed where the server does not say");
        assert!(parse_lmstudio_models(b"nope").is_empty() && parse_lmstudio_models(br#"{"data":5}"#).is_empty());
    }

    #[test]
    fn prices_are_read_defensively() {
        assert_eq!(per_mtok(Some(&Value::String("0.000002".into()))), Some(2.0));
        assert_eq!(per_mtok(Some(&serde_json::json!(0))), Some(0.0));
        for bad in [serde_json::json!("-1"), serde_json::json!("NaN"), serde_json::json!("inf"), serde_json::json!(null), serde_json::json!([1])] {
            assert_eq!(per_mtok(Some(&bad)), None, "{bad}");
        }
        assert_eq!(per_mtok(None), None);
    }
}
