//! The HTTPS client for OpenAI-style servers, over `edith-net` (so Offline mode, the redacted
//! request log, timeouts and the size caps all apply). It reads the key from a source on every
//! request and never keeps it; a local server needs no key at all.

use super::{Flavor, StreamAssembler, build_body, parse_models};
use crate::error::error_from_http;
use crate::key_source::KeySource;
use crate::{Cancel, ChatModel, LlmError, ModelInfo, Request, Response, StreamEvent};
use edith_net::NetError;
use edith_net::http::{Client, Request as NetRequest};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use zeroize::Zeroizing;

pub struct OpenAiCompatible {
    client: Arc<Client>,
    flavor: Flavor,
    base_url: String,
    key: Option<KeySource>,
    headers: Vec<(String, String)>,
    timeout: Duration,
}

impl OpenAiCompatible {
    /// `base_url` is the API root without a trailing slash (`https://openrouter.ai/api/v1`).
    /// Without a `key` no `Authorization` header is sent (local servers).
    pub fn new(client: Arc<Client>, flavor: Flavor, base_url: &str, key: Option<KeySource>) -> Self {
        Self { client, flavor, base_url: base_url.trim_end_matches('/').to_string(), key, headers: Vec::new(), timeout: Duration::from_secs(900) }
    }

    /// A header sent with every request (the app's name for OpenRouter's rankings).
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Uses the key stored in the OS keychain for `service` (`openrouter`, `openai`).
    #[cfg(feature = "keychain")]
    pub fn key_from_keychain(service: &'static str) -> KeySource {
        let secret = move || edith_net::services::find(service).and_then(|s| s.secret);
        KeySource::new(move || {
            let name = secret().ok_or_else(|| LlmError::Unavailable(format!("the {service} service is not known")))?;
            edith_net::secrets::os()?.get_cached(name)?.ok_or_else(|| LlmError::Auth(format!("no {service} key is stored; connect it in Connections")))
        })
        .with_forget(move || {
            if let (Some(name), Ok(store)) = (secret(), edith_net::secrets::os()) {
                store.forget_cached(name);
            }
        })
    }

    /// How long one reply may take in total.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// `req` with the app headers and the key; also returns the key itself (for scrubbing errors).
    fn authorize(&self, mut req: NetRequest) -> Result<(NetRequest, Option<Zeroizing<String>>), LlmError> {
        for (n, v) in &self.headers {
            req = req.header(n, v);
        }
        match &self.key {
            Some(source) => {
                let key = (source.get)()?;
                req = req.header("Authorization", &format!("Bearer {}", key.as_str()));
                Ok((req, Some(key)))
            }
            None => Ok((req, None)),
        }
    }

    /// A rejected key means the copy kept of it may be stale (the person replaced it): drop it.
    fn after_error(&self, e: LlmError) -> LlmError {
        if let (LlmError::Auth(_), Some(source)) = (&e, &self.key) {
            (source.forget)();
        }
        e
    }

    fn scrub(e: LlmError, key: &Option<Zeroizing<String>>) -> LlmError {
        match key {
            Some(k) => e.scrubbed(k),
            None => e,
        }
    }

    /// Sends the prepared streaming request and assembles the reply. Every error leaves through
    /// [`ChatModel::stream`], which scrubs the key from it.
    fn run(&self, req: &NetRequest, cancel: &Cancel, sink: &mut dyn FnMut(StreamEvent)) -> Result<Response, LlmError> {
        let mut asm = StreamAssembler::new();
        let mut failure: Option<LlmError> = None;
        let end = self.client.stream_lines(req, |line| {
            if failure.is_some() || cancel.load(Ordering::SeqCst) {
                return false;
            }
            if let Err(e) = asm.feed_line(line, sink) {
                failure = Some(e);
                return false;
            }
            true
        });
        match end {
            Err(NetError::Cancelled) => return Err(LlmError::Cancelled),
            Err(e) => return Err(e.into()),
            Ok(end) if end.status >= 400 => return Err(error_from_http(end.status, &end.error_body)),
            Ok(_) => {}
        }
        if let Some(e) = failure {
            return Err(e);
        }
        if cancel.load(Ordering::SeqCst) {
            return Err(LlmError::Cancelled);
        }
        asm.finish()
    }
}

impl ChatModel for OpenAiCompatible {
    fn provider(&self) -> &str {
        self.flavor.provider
    }

    fn models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let (req, key) = self.authorize(NetRequest::get(&format!("{}/models", self.base_url)))?;
        let resp = self.client.send(&req).map_err(|e| Self::scrub(LlmError::from(e), &key))?;
        if !resp.ok() {
            return Err(self.after_error(Self::scrub(error_from_http(resp.status, &resp.body), &key)));
        }
        parse_models(&resp.body).map_err(|e| Self::scrub(e, &key))
    }

    fn stream(&self, request: &Request, cancel: &Cancel, sink: &mut dyn FnMut(StreamEvent)) -> Result<Response, LlmError> {
        let body = serde_json::to_vec(&build_body(request, &self.flavor)?).map_err(|_| LlmError::bad_stream("the request could not be encoded"))?;
        let post = NetRequest::post(&format!("{}/chat/completions", self.base_url), body)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream");
        let (mut req, key) = self.authorize(post)?;
        req.cancel = Some(cancel.clone());
        req.timeout = Some(self.timeout);
        self.run(&req, cancel, sink).map_err(|e| self.after_error(Self::scrub(e, &key)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openai::stream_fixtures as f;
    use crate::{Message, Part, StopReason};
    use edith_net::http::ClientConfig;
    use edith_net::testserver::{Reply, response, serve};
    use edith_net::{NetState, Outcome};
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    const KEY: &str = "sk-or-v1-TOPSECRETKEYVALUE";

    fn net() -> Arc<Client> {
        Arc::new(Client::new(
            ClientConfig { allow_loopback_http: true, min_host_interval: Duration::ZERO, timeout: Duration::from_secs(5), ..ClientConfig::default() },
            Arc::new(NetState::new()),
        ))
    }

    fn key() -> KeySource {
        KeySource::new(|| Ok(Zeroizing::new(KEY.to_string())))
    }

    fn model(url: &str) -> OpenAiCompatible {
        OpenAiCompatible::new(net(), Flavor::OPENROUTER, url, Some(key())).with_header("X-Title", "Edith")
    }

    fn cancel() -> Cancel {
        Arc::new(AtomicBool::new(false))
    }

    fn ask() -> Request {
        let mut r = Request::new("test/model");
        r.messages.push(Message::user_text("hi"));
        r
    }

    fn sse(body: String) -> Reply {
        response(200, &[("content-type", "text/event-stream")], body.as_bytes())
    }

    fn ok_reply() -> Reply {
        sse(f::start() + &f::text(&["ok"]) + &f::end("stop", 5, 1))
    }

    #[test]
    fn a_reply_streams_through_the_network_client_with_the_right_headers() {
        let s = serve(|_, _| sse(f::start() + &f::text(&["Hel", "lo"]) + &f::end("stop", 12, 3)));
        let mut texts = Vec::new();
        let r = model(&s.url)
            .stream(&ask(), &cancel(), &mut |e| {
                if let StreamEvent::Text(t) = e {
                    texts.push(t)
                }
            })
            .unwrap();
        assert_eq!(r.message.text(), "Hello");
        assert_eq!((r.stop_reason, r.usage.output_tokens), (StopReason::EndTurn, 3));
        assert_eq!(texts, ["Hel", "lo"]);
        let seen = s.requests.lock().unwrap()[0].clone();
        let lower = seen.to_ascii_lowercase();
        assert!(seen.starts_with("POST /chat/completions"), "{seen}");
        assert!(lower.contains(&format!("authorization: bearer {}", KEY.to_ascii_lowercase())) && lower.contains("x-title: edith"), "{seen}");
        let body: serde_json::Value = serde_json::from_str(seen.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!((body["stream"].clone(), body["model"].clone()), (serde_json::json!(true), serde_json::json!("test/model")));
    }

    #[test]
    fn a_local_server_gets_no_authorization_header() {
        let s = serve(|_, _| ok_reply());
        OpenAiCompatible::new(net(), Flavor::OLLAMA, &s.url, None).stream(&ask(), &cancel(), &mut |_| {}).unwrap();
        let seen = s.requests.lock().unwrap()[0].to_ascii_lowercase();
        assert!(!seen.contains("authorization") && !seen.contains("x-title"), "{seen}");
    }

    #[test]
    fn the_key_is_in_no_log_error_or_debug_output() {
        let s = serve(|_, _| response(401, &[], br#"{"error":{"code":401,"message":"No auth credentials; you sent sk-or-v1-TOPSECRETKEYVALUE"}}"#));
        let client = net();
        let m = OpenAiCompatible::new(client.clone(), Flavor::OPENROUTER, &s.url, Some(key()));
        let e = m.stream(&ask(), &cancel(), &mut |_| {}).unwrap_err();
        assert!(matches!(e, LlmError::Auth(_)), "{e:?}");
        let everything = format!("{e} {e:?} {:?}", client.state().entries(None));
        assert!(!everything.contains("TOPSECRET"), "{everything}");
        assert_eq!(client.state().entries(None)[0].outcome, Outcome::Failed);
        // and an error that arrives inside a 200 stream
        let s = serve(|_, _| sse("data: {\"error\":{\"code\":401,\"message\":\"bad key sk-or-v1-TOPSECRETKEYVALUE\"}}\n\n".to_string()));
        let e = model(&s.url).stream(&ask(), &cancel(), &mut |_| {}).unwrap_err();
        assert!(!format!("{e} {e:?}").contains("TOPSECRET"));
    }

    #[test]
    fn provider_errors_are_mapped_and_a_stream_is_never_retried() {
        for (status, body, want_retry) in [
            (429u16, r#"{"error":{"code":429,"message":"slow"}}"#, true),
            (402, r#"{"error":{"code":402,"message":"Insufficient credits"}}"#, false),
            (400, r#"{"error":{"code":400,"message":"Invalid model"}}"#, false),
            (500, r#"{"error":{"code":500,"message":"oops"}}"#, false),
        ] {
            let s = serve(move |_, _| response(status, &[], body.as_bytes()));
            let e = model(&s.url).stream(&ask(), &cancel(), &mut |_| {}).unwrap_err();
            assert_eq!(e.retryable(), want_retry, "{status}: {e:?}");
            assert_eq!(s.hits(), 1);
        }
    }

    #[test]
    fn no_key_means_no_request() {
        let s = serve(|_, _| ok_reply());
        let none = KeySource::new(|| Err(LlmError::Auth("no openrouter key is stored; connect it in Connections".into())));
        let e = OpenAiCompatible::new(net(), Flavor::OPENROUTER, &s.url, Some(none)).stream(&ask(), &cancel(), &mut |_| {}).unwrap_err();
        assert!(matches!(e, LlmError::Auth(_)));
        assert_eq!(s.hits(), 0);
    }

    #[test]
    fn a_rejected_key_is_forgotten_and_other_outcomes_leave_it_alone() {
        let forgotten = Arc::new(AtomicUsize::new(0));
        let f2 = forgotten.clone();
        let source = key().with_forget(move || {
            f2.fetch_add(1, Ordering::SeqCst);
        });
        let refused = serve(|_, _| response(401, &[], br#"{"error":{"code":401,"message":"no"}}"#));
        let m = OpenAiCompatible::new(net(), Flavor::OPENROUTER, &refused.url, Some(source.clone()));
        assert!(matches!(m.stream(&ask(), &cancel(), &mut |_| {}), Err(LlmError::Auth(_))));
        assert_eq!(forgotten.load(Ordering::SeqCst), 1);
        assert!(matches!(m.models(), Err(LlmError::Auth(_))));
        assert_eq!(forgotten.load(Ordering::SeqCst), 2);
        let ok = serve(|_, _| ok_reply());
        OpenAiCompatible::new(net(), Flavor::OPENROUTER, &ok.url, Some(source.clone())).stream(&ask(), &cancel(), &mut |_| {}).unwrap();
        let busy = serve(|_, _| response(500, &[], b"{}"));
        assert!(OpenAiCompatible::new(net(), Flavor::OPENROUTER, &busy.url, Some(source)).stream(&ask(), &cancel(), &mut |_| {}).is_err());
        assert_eq!(forgotten.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn offline_mode_blocks_the_request() {
        let s = serve(|_, _| ok_reply());
        let client = net();
        client.state().set_offline(true);
        let e = OpenAiCompatible::new(client, Flavor::OPENROUTER, &s.url, Some(key())).stream(&ask(), &cancel(), &mut |_| {}).unwrap_err();
        assert_eq!(e, LlmError::Net(NetError::Offline));
        assert_eq!(s.hits(), 0);
    }

    #[test]
    fn stop_before_sending_and_a_stalled_server_both_end_cleanly() {
        let s = serve(|_, _| ok_reply());
        let stop = Arc::new(AtomicBool::new(true));
        assert_eq!(model(&s.url).stream(&ask(), &stop, &mut |_| {}).unwrap_err(), LlmError::Cancelled);
        let hang = serve(|_, _| Reply::Hang);
        let started = std::time::Instant::now();
        let e = model(&hang.url).with_timeout(Duration::from_millis(300)).stream(&ask(), &cancel(), &mut |_| {}).unwrap_err();
        assert_eq!(e, LlmError::Net(NetError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn a_truncated_or_garbled_stream_is_an_error() {
        let s = serve(|_, _| sse(f::start() + &f::text(&["partial"])));
        let mut got = Vec::new();
        let e = model(&s.url).stream(&ask(), &cancel(), &mut |ev| got.push(ev)).unwrap_err();
        assert!(matches!(e, LlmError::BadStream(_)), "{e:?}");
        assert!(got.contains(&StreamEvent::Text("partial".into())), "what arrived was still shown");
        let s = serve(|_, _| sse("data: this is not json\n\n".to_string()));
        assert!(matches!(model(&s.url).stream(&ask(), &cancel(), &mut |_| {}), Err(LlmError::BadStream(_))));
    }

    #[test]
    fn a_tool_call_round_trip_sends_every_result_back_together() {
        let s = serve(|n, _| {
            if n == 0 {
                sse(f::start()
                    + &f::tool(0, "call_1", "markers", &["{}"])
                    + &f::tool(1, "call_2", "transcript", &["{\"from\":2}"])
                    + &f::end("tool_calls", 5, 7))
            } else {
                sse(f::start() + &f::text(&["done"]) + &f::end("stop", 9, 1))
            }
        });
        let m = model(&s.url);
        let mut req = ask();
        let first = m.stream(&req, &cancel(), &mut |_| {}).unwrap();
        assert_eq!(first.stop_reason, StopReason::ToolUse);
        req.messages.push(first.message.clone());
        req.messages.push(Message::tool_results(vec![
            Part::ToolResult { tool_use_id: "call_1".into(), content: "[]".into(), is_error: false, images: Vec::new() },
            Part::ToolResult { tool_use_id: "call_2".into(), content: "hello".into(), is_error: false, images: Vec::new() },
        ]));
        assert_eq!(m.stream(&req, &cancel(), &mut |_| {}).unwrap().message.text(), "done");
        let second = s.requests.lock().unwrap()[1].clone();
        let body: serde_json::Value = serde_json::from_str(second.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 4, "user, assistant with two calls, two tool messages");
        assert_eq!(msgs[1]["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!((msgs[2]["tool_call_id"].clone(), msgs[3]["tool_call_id"].clone()), (serde_json::json!("call_1"), serde_json::json!("call_2")));
    }

    #[test]
    fn the_model_list_is_fetched_and_read() {
        let s = serve(|_, _| {
            response(
                200,
                &[("content-type", "application/json")],
                br#"{"data":[{"id":"a/free:free","name":"Free","architecture":{"input_modalities":["text","image"],"output_modalities":["text"]},"pricing":{"prompt":"0","completion":"0"},"supported_parameters":["tools"]}]}"#,
            )
        });
        let list = model(&s.url).models().unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].capabilities.tools && list[0].capabilities.images && list[0].price.as_ref().is_some_and(|p| p.is_free()));
        let seen = s.requests.lock().unwrap()[0].clone();
        assert!(seen.starts_with("GET /models"), "{seen}");
        let s = serve(|_, _| response(401, &[], br#"{"error":{"code":401,"message":"no"}}"#));
        assert!(matches!(model(&s.url).models(), Err(LlmError::Auth(_))));
    }

    /// Talks to OpenRouter with the key stored in the keychain: lists models, then asks a free one
    /// for a few words. `cargo test -p edith-llm --features keychain -- --ignored real_openrouter`
    #[test]
    #[ignore = "uses the real network and the stored OpenRouter key"]
    #[cfg(feature = "keychain")]
    fn real_openrouter_lists_models_and_streams_a_short_reply() {
        let client = Arc::new(Client::new(ClientConfig::default(), Arc::new(NetState::new())));
        let m = OpenAiCompatible::new(client, Flavor::OPENROUTER, "https://openrouter.ai/api/v1", Some(OpenAiCompatible::key_from_keychain("openrouter")))
            .with_header("X-Title", "Edith");
        let models = m.models().unwrap();
        eprintln!(
            "{} models; {} free with tools",
            models.len(),
            models.iter().filter(|m| m.capabilities.tools && m.price.as_ref().is_some_and(|p| p.is_free())).count()
        );
        let free = models.iter().find(|m| m.capabilities.tools && m.price.as_ref().is_some_and(|p| p.is_free())).expect("a free model with tools");
        let mut req = Request::new(&free.id);
        req.max_output_tokens = 60;
        req.messages.push(Message::user_text("Reply with exactly three words."));
        let r = m.stream(&req, &cancel(), &mut |_| {}).unwrap();
        eprintln!("model: {}  reply: {:?}  usage: {:?}", free.id, r.message.text(), r.usage);
        assert!(!r.message.text().is_empty());
    }
}
