//! The HTTPS client for Anthropic, over `edith-net` (so Offline mode, the redacted request log,
//! timeouts and the size caps all apply). It reads the key from a source on every request and
//! never keeps it.

use super::{API_VERSION, StreamAssembler, build_body, parse_models};
use crate::error::error_from_http;
use crate::{Cancel, ChatModel, LlmError, ModelInfo, Request, Response, StreamEvent};
use edith_net::NetError;
use edith_net::http::{Client, Request as NetRequest};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use zeroize::Zeroizing;

/// Where the key comes from, asked once per request, and how to drop any copy kept of it. The
/// client never stores the key itself; a source may keep one in wipe-on-drop memory so a
/// conversation costs one keychain prompt instead of one per request.
#[derive(Clone)]
pub struct KeySource {
    get: Arc<dyn Fn() -> Result<Zeroizing<String>, LlmError> + Send + Sync>,
    forget: Arc<dyn Fn() + Send + Sync>,
}

impl KeySource {
    pub fn new(get: impl Fn() -> Result<Zeroizing<String>, LlmError> + Send + Sync + 'static) -> Self {
        Self { get: Arc::new(get), forget: Arc::new(|| {}) }
    }

    /// Called when the provider rejects the key, so a replaced key is read afresh next time.
    pub fn with_forget(mut self, forget: impl Fn() + Send + Sync + 'static) -> Self {
        self.forget = Arc::new(forget);
        self
    }
}

pub struct Anthropic {
    client: Arc<Client>,
    key: KeySource,
    base_url: String,
    timeout: Duration,
}

impl Anthropic {
    pub fn new(client: Arc<Client>, key: KeySource) -> Self {
        Self { client, key, base_url: "https://api.anthropic.com".to_string(), timeout: Duration::from_secs(900) }
    }

    /// Uses the key stored under the Anthropic service in the OS keychain.
    #[cfg(feature = "keychain")]
    pub fn from_keychain(client: Arc<Client>) -> Self {
        let secret = || edith_net::services::find("anthropic").and_then(|s| s.secret);
        let key = KeySource::new(move || {
            let name = secret().ok_or_else(|| LlmError::Unavailable("the Anthropic service is not known".to_string()))?;
            edith_net::secrets::os()?.get_cached(name)?.ok_or_else(|| LlmError::Auth("no Anthropic key is stored; add one in Connections".to_string()))
        })
        .with_forget(move || {
            if let (Some(name), Ok(store)) = (secret(), edith_net::secrets::os()) {
                store.forget_cached(name);
            }
        });
        Self::new(client, key)
    }

    /// Another server (a test server, a proxy). Must be `https`, or loopback `http` when the
    /// network client allows it.
    pub fn with_base_url(mut self, url: &str) -> Self {
        self.base_url = url.trim_end_matches('/').to_string();
        self
    }

    /// How long one reply may take in total.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// `req` with the key and version headers added, and the key itself (for scrubbing errors).
    fn authorize(&self, req: NetRequest) -> Result<(NetRequest, Zeroizing<String>), LlmError> {
        let key = (self.key.get)()?;
        let req = req.header("x-api-key", &key).header("anthropic-version", API_VERSION);
        Ok((req, key))
    }

    /// A rejected key means the copy kept of it may be stale (the person replaced it): drop it.
    fn after_error(&self, e: LlmError) -> LlmError {
        if matches!(e, LlmError::Auth(_)) {
            (self.key.forget)();
        }
        e
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

impl ChatModel for Anthropic {
    fn provider(&self) -> &str {
        super::PROVIDER
    }

    fn models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let (req, key) = self.authorize(NetRequest::get(&format!("{}/v1/models?limit=1000", self.base_url)))?;
        let resp = self.client.send(&req).map_err(|e| LlmError::from(e).scrubbed(&key))?;
        if !resp.ok() {
            return Err(self.after_error(error_from_http(resp.status, &resp.body).scrubbed(&key)));
        }
        parse_models(&resp.body).map_err(|e| e.scrubbed(&key))
    }

    fn stream(&self, request: &Request, cancel: &Cancel, sink: &mut dyn FnMut(StreamEvent)) -> Result<Response, LlmError> {
        let body = serde_json::to_vec(&build_body(request)?).map_err(|_| LlmError::bad_stream("the request could not be encoded"))?;
        let post =
            NetRequest::post(&format!("{}/v1/messages", self.base_url), body).header("content-type", "application/json").header("accept", "text/event-stream");
        let (mut req, key) = self.authorize(post)?;
        req.cancel = Some(cancel.clone());
        req.timeout = Some(self.timeout);
        self.run(&req, cancel, sink).map_err(|e| self.after_error(e.scrubbed(&key)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::stream_fixtures as f;
    use crate::{Message, Part, StopReason};
    use edith_net::http::ClientConfig;
    use edith_net::testserver::{Reply, response, serve};
    use edith_net::{NetState, Outcome};
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    const KEY: &str = "sk-ant-api03-TOPSECRETKEY";

    fn net() -> Arc<Client> {
        Arc::new(Client::new(
            ClientConfig { allow_loopback_http: true, min_host_interval: Duration::ZERO, timeout: Duration::from_secs(5), ..ClientConfig::default() },
            Arc::new(NetState::new()),
        ))
    }

    fn key() -> KeySource {
        KeySource::new(|| Ok(Zeroizing::new(KEY.to_string())))
    }

    fn model(url: &str) -> Anthropic {
        Anthropic::new(net(), key()).with_base_url(url)
    }

    fn cancel() -> Cancel {
        Arc::new(AtomicBool::new(false))
    }

    fn ask() -> Request {
        let mut r = Request::new("claude-haiku-5-5");
        r.messages.push(Message::user_text("hi"));
        r
    }

    fn sse(body: String) -> Reply {
        response(200, &[("content-type", "text/event-stream")], body.as_bytes())
    }

    #[test]
    fn a_reply_streams_through_the_network_client() {
        let s = serve(|_, _| sse(f::start(12) + &f::text_block(0, &["Hel", "lo"]) + &f::end("end_turn", 3)));
        let m = model(&s.url);
        let mut texts = Vec::new();
        let r = m
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
        assert!(seen.starts_with("POST /v1/messages"), "{seen}");
        assert!(
            lower.contains(&format!("x-api-key: {}", KEY.to_ascii_lowercase()))
                && lower.contains("anthropic-version: 2023-06-01")
                && lower.contains("content-type: application/json")
        );
        let body: serde_json::Value = serde_json::from_str(seen.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!((body["stream"].clone(), body["model"].clone()), (serde_json::json!(true), serde_json::json!("claude-haiku-5-5")));
    }

    #[test]
    fn the_key_is_in_no_log_error_or_debug_output() {
        let s = serve(|_, _| {
            response(401, &[], br#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key sk-ant-api03-TOPSECRETKEY"}}"#)
        });
        let client = net();
        let m = Anthropic::new(client.clone(), key()).with_base_url(&s.url);
        let e = m.stream(&ask(), &cancel(), &mut |_| {}).unwrap_err();
        assert!(matches!(e, LlmError::Auth(_)), "{e:?}");
        let everything = format!("{e} {e:?} {:?}", client.state().entries(None));
        assert!(!everything.contains("TOPSECRET"), "{everything}");
        assert_eq!(client.state().entries(None)[0].outcome, Outcome::Failed);
    }

    #[test]
    fn provider_errors_are_mapped() {
        for (status, kind, want_retry) in
            [(429u16, "rate_limit_error", true), (529, "overloaded_error", true), (400, "invalid_request_error", false), (500, "api_error", false)]
        {
            let body = format!(r#"{{"type":"error","error":{{"type":"{kind}","message":"m"}}}}"#);
            let s = serve(move |_, _| response(status, &[], body.as_bytes()));
            let e = model(&s.url).stream(&ask(), &cancel(), &mut |_| {}).unwrap_err();
            assert_eq!(e.retryable(), want_retry, "{status}: {e:?}");
            assert_eq!(s.hits(), 1, "a stream is never retried behind the caller's back");
        }
    }

    #[test]
    fn no_key_means_no_request() {
        let s = serve(|_, _| sse(String::new()));
        let none = KeySource::new(|| Err(LlmError::Auth("no Anthropic key is stored; add one in Connections".into())));
        let e = Anthropic::new(net(), none).with_base_url(&s.url).stream(&ask(), &cancel(), &mut |_| {}).unwrap_err();
        assert!(matches!(e, LlmError::Auth(_)));
        assert_eq!(s.hits(), 0);
    }

    #[test]
    fn the_key_is_asked_for_on_every_request_and_never_kept() {
        let asked = Arc::new(AtomicUsize::new(0));
        let a = asked.clone();
        let counting = KeySource::new(move || {
            a.fetch_add(1, Ordering::SeqCst);
            Ok(Zeroizing::new(KEY.to_string()))
        });
        let s = serve(|_, _| sse(f::start(1) + &f::text_block(0, &["x"]) + &f::end("end_turn", 1)));
        let m = Anthropic::new(net(), counting).with_base_url(&s.url);
        m.stream(&ask(), &cancel(), &mut |_| {}).unwrap();
        m.stream(&ask(), &cancel(), &mut |_| {}).unwrap();
        assert_eq!(asked.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_rejected_key_is_forgotten_so_a_replaced_one_is_read_afresh() {
        let forgotten = Arc::new(AtomicUsize::new(0));
        let f = forgotten.clone();
        let source = KeySource::new(|| Ok(Zeroizing::new(KEY.to_string()))).with_forget(move || {
            f.fetch_add(1, Ordering::SeqCst);
        });
        let refused = serve(|_, _| response(401, &[], br#"{"error":{"type":"authentication_error","message":"invalid x-api-key"}}"#));
        let e = Anthropic::new(net(), source.clone()).with_base_url(&refused.url).stream(&ask(), &cancel(), &mut |_| {}).unwrap_err();
        assert!(matches!(e, LlmError::Auth(_)));
        assert_eq!(forgotten.load(Ordering::SeqCst), 1, "a 401 drops the kept key");
        assert!(matches!(Anthropic::new(net(), source.clone()).with_base_url(&refused.url).models(), Err(LlmError::Auth(_))));
        assert_eq!(forgotten.load(Ordering::SeqCst), 2);
        // success and other errors leave the kept key alone
        let ok = serve(|_, _| sse(f::start(1) + &f::text_block(0, &["x"]) + &f::end("end_turn", 1)));
        Anthropic::new(net(), source.clone()).with_base_url(&ok.url).stream(&ask(), &cancel(), &mut |_| {}).unwrap();
        let busy = serve(|_, _| response(529, &[], br#"{"error":{"type":"overloaded_error","message":"x"}}"#));
        assert!(Anthropic::new(net(), source).with_base_url(&busy.url).stream(&ask(), &cancel(), &mut |_| {}).is_err());
        assert_eq!(forgotten.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn offline_mode_blocks_the_request() {
        let s = serve(|_, _| sse(String::new()));
        let client = net();
        client.state().set_offline(true);
        let e = Anthropic::new(client, key()).with_base_url(&s.url).stream(&ask(), &cancel(), &mut |_| {}).unwrap_err();
        assert_eq!(e, LlmError::Net(NetError::Offline));
        assert_eq!(s.hits(), 0);
    }

    #[test]
    fn stop_before_sending_and_a_stalled_server_both_end_cleanly() {
        let s = serve(|_, _| sse(String::new()));
        let stop = Arc::new(AtomicBool::new(true));
        assert_eq!(model(&s.url).stream(&ask(), &stop, &mut |_| {}).unwrap_err(), LlmError::Cancelled);
        assert_eq!(s.hits(), 0);
        let hang = serve(|_, _| Reply::Hang);
        let started = std::time::Instant::now();
        let e =
            Anthropic::new(net(), key()).with_base_url(&hang.url).with_timeout(Duration::from_millis(300)).stream(&ask(), &cancel(), &mut |_| {}).unwrap_err();
        assert_eq!(e, LlmError::Net(NetError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn a_truncated_or_garbled_stream_is_an_error() {
        let whole = f::start(1) + &f::text_block(0, &["partial"]);
        let s = serve(move |_, _| sse(whole.clone()));
        let mut got = Vec::new();
        let e = model(&s.url).stream(&ask(), &cancel(), &mut |ev| got.push(ev)).unwrap_err();
        assert!(matches!(e, LlmError::BadStream(_)), "{e:?}");
        assert!(got.contains(&StreamEvent::Text("partial".into())), "what arrived was still shown");
        let s = serve(|_, _| sse("data: this is not json\n\n".to_string()));
        assert!(matches!(model(&s.url).stream(&ask(), &cancel(), &mut |_| {}), Err(LlmError::BadStream(_))));
    }

    #[test]
    fn a_tool_call_round_trip_sends_the_results_in_one_message() {
        let s = serve(|n, _| {
            if n == 0 {
                sse(f::start(5)
                    + &f::tool_block(0, "toolu_1", "markers", &["{}"])
                    + &f::tool_block(1, "toolu_2", "transcript", &["{\"from\":2}"])
                    + &f::end("tool_use", 7))
            } else {
                sse(f::start(9) + &f::text_block(0, &["done"]) + &f::end("end_turn", 1))
            }
        });
        let m = model(&s.url);
        let mut req = ask();
        let first = m.stream(&req, &cancel(), &mut |_| {}).unwrap();
        assert_eq!(first.stop_reason, StopReason::ToolUse);
        req.messages.push(first.message.clone());
        req.messages.push(Message::tool_results(vec![
            Part::ToolResult { tool_use_id: "toolu_1".into(), content: "[]".into(), is_error: false, images: Vec::new() },
            Part::ToolResult { tool_use_id: "toolu_2".into(), content: "hello".into(), is_error: false, images: Vec::new() },
        ]));
        assert_eq!(m.stream(&req, &cancel(), &mut |_| {}).unwrap().message.text(), "done");
        let second = s.requests.lock().unwrap()[1].clone();
        let body: serde_json::Value = serde_json::from_str(second.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["messages"][2]["content"].as_array().unwrap().len(), 2);
        assert_eq!(body["messages"][1]["content"][0]["id"], "toolu_1");
    }

    #[test]
    fn the_model_list_is_fetched_and_read() {
        let s = serve(|_, _| {
            response(200, &[("content-type", "application/json")], br#"{"data":[{"id":"claude-haiku-5-5","display_name":"Claude Haiku 5.5","max_input_tokens":1000000,"max_tokens":128000,"capabilities":{"image_input":{"supported":true},"thinking":{"supported":true,"types":{"adaptive":{"supported":true}}},"effort":{"supported":true}}}]}"#)
        });
        let list = model(&s.url).models().unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].capabilities.thinking && list[0].price.is_some());
        let seen = s.requests.lock().unwrap()[0].clone();
        assert!(seen.starts_with("GET /v1/models"), "{seen}");
        let s = serve(|_, _| response(401, &[], br#"{"error":{"type":"authentication_error","message":"no"}}"#));
        assert!(matches!(model(&s.url).models(), Err(LlmError::Auth(_))));
    }

    /// Talks to the real API with the key stored in the keychain: a few tokens, a fraction of a cent.
    /// `cargo test -p edith-llm --features keychain -- --ignored real_anthropic`
    #[test]
    #[ignore = "uses the real network and the stored Anthropic key"]
    #[cfg(feature = "keychain")]
    fn real_anthropic_streams_a_short_reply_and_lists_models() {
        let client = Arc::new(Client::new(ClientConfig::default(), Arc::new(NetState::new())));
        let m = Anthropic::from_keychain(client);
        let models = m.models().unwrap();
        assert!(models.iter().any(|x| x.id == "claude-haiku-5-5"), "{:?}", models.iter().map(|m| &m.id).collect::<Vec<_>>());
        let mut req = Request::new("claude-haiku-5-5");
        req.max_output_tokens = 40;
        req.messages.push(Message::user_text("Reply with exactly three words."));
        let mut pieces = 0;
        let r = m
            .stream(&req, &cancel(), &mut |e| {
                if matches!(e, StreamEvent::Text(_)) {
                    pieces += 1
                }
            })
            .unwrap();
        eprintln!("reply: {:?}  usage: {:?}  pieces: {pieces}", r.message.text(), r.usage);
        assert!(!r.message.text().is_empty() && r.usage.output_tokens > 0);
    }
}
