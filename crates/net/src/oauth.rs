//! OAuth building blocks for "Connect" buttons: PKCE (RFC 7636), a one-shot loopback receiver for
//! the redirect (RFC 8252 section 7.3), and a helper for the token request.
//!
//! The flow, with a browser the app opens through its host hook:
//! 1. [`Flow::start`] binds `127.0.0.1` on a random port, makes a PKCE pair and a `state`, and
//!    builds the sign-in address with your provider-specific closure.
//! 2. The app opens that address; the user approves in the provider's own page.
//! 3. [`Flow::wait`] receives the one redirect, checks `state`, and returns the code.
//! 4. [`post_for_json`] trades the code (plus the PKCE verifier) for a key or token.
//!
//! The receiver accepts one valid redirect and then stops listening, answers only `GET /callback`
//! on the loopback host names it was bound to (a web page cannot reach it by DNS rebinding), caps
//! the request at 4 KiB, never echoes the code, and gives up after a timeout or on cancel. It
//! listens on `127.0.0.1` and, when the machine has it, `::1` (a browser that resolves
//! `localhost` to IPv6 first still arrives). A provider that refuses the sign-in may send the
//! person back without the `state` (OpenRouter does); that ends the wait with "declined", and can
//! never produce a code.

use crate::http::{Client, Request};
use crate::{NetError, redact};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

const MAX_REQUEST: usize = 4096;
const MAX_CODE: usize = 2048;
const MAX_BAD_REQUESTS: u32 = 20;
/// Unreserved characters stay as they are in a query component (RFC 3986).
const QUERY: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

fn oauth_err(msg: &str) -> NetError {
    NetError::OAuth(msg.to_string())
}

/// `n` random bytes as unpadded base64url.
pub fn random_token(n: usize) -> Result<String, NetError> {
    let mut buf = Zeroizing::new(vec![0u8; n.clamp(1, 128)]);
    getrandom::fill(&mut buf).map_err(|_| oauth_err("the system could not provide random numbers"))?;
    Ok(URL_SAFE_NO_PAD.encode(&*buf))
}

/// A PKCE pair: the secret `verifier` (kept in memory, sent only to the token endpoint) and its
/// SHA-256 `challenge` (sent in the sign-in address).
pub struct Pkce {
    verifier: Zeroizing<String>,
    pub challenge: String,
}

impl Pkce {
    pub fn generate() -> Result<Self, NetError> {
        Ok(Self::from_verifier(&random_token(32)?))
    }

    /// For tests with the RFC's vectors; the verifier must be 43 to 128 unreserved characters.
    pub fn from_verifier(verifier: &str) -> Self {
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self { verifier: Zeroizing::new(verifier.to_string()), challenge }
    }

    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    pub const METHOD: &'static str = "S256";
}

/// `base` with `params` as a percent-encoded query. `base` must be `https` (or loopback `http`).
pub fn build_url(base: &str, params: &[(&str, &str)]) -> Result<String, NetError> {
    let ok = base.starts_with("https://") || base.starts_with("http://127.0.0.1") || base.starts_with("http://localhost");
    if !ok || base.contains(['?', '#']) || base.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(oauth_err("the sign-in address is not valid"));
    }
    let query: Vec<String> = params.iter().map(|(k, v)| format!("{}={}", utf8_percent_encode(k, QUERY), utf8_percent_encode(v, QUERY))).collect();
    Ok(format!("{base}?{}", query.join("&")))
}

/// A form body (`application/x-www-form-urlencoded`).
pub fn form_body(params: &[(&str, &str)]) -> Vec<u8> {
    params.iter().map(|(k, v)| format!("{}={}", utf8_percent_encode(k, QUERY), utf8_percent_encode(v, QUERY))).collect::<Vec<_>>().join("&").into_bytes()
}

/// POSTs `body` and parses the JSON answer. Only the status is reported on failure, never the
/// body (it may echo what you sent).
pub fn post_for_json(client: &Client, url: &str, content_type: &str, body: Vec<u8>) -> Result<serde_json::Value, NetError> {
    let mut req = Request::post(url, body).header("Content-Type", content_type).header("Accept", "application/json");
    req.max_bytes = Some(256 * 1024);
    client.send(&req)?.error_for_status()?.json()
}

/// The redirect's `code`, wiped from memory when dropped.
pub struct AuthCode(Zeroizing<String>);

impl AuthCode {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A one-shot receiver on `127.0.0.1` at a random port.
pub struct Loopback {
    /// `127.0.0.1` first, then `::1` when it could be bound on the same port.
    listeners: Vec<TcpListener>,
    port: u16,
}

impl Loopback {
    pub fn bind() -> Result<Self, NetError> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).map_err(|_| oauth_err("could not open a local port for the sign-in"))?;
        listener.set_nonblocking(true).map_err(|_| oauth_err("could not open a local port for the sign-in"))?;
        let port = listener.local_addr().map_err(|_| oauth_err("could not open a local port for the sign-in"))?.port();
        let mut listeners = vec![listener];
        // The same port on IPv6, if the machine has it; without it only IPv4 is used.
        if let Ok(v6) = TcpListener::bind(SocketAddr::from((Ipv6Addr::LOCALHOST, port)))
            && v6.set_nonblocking(true).is_ok()
        {
            listeners.push(v6);
        }
        Ok(Self { listeners, port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn redirect_uri(&self) -> String {
        self.redirect_uri_for("127.0.0.1")
    }

    /// The redirect address using `host`, which must be `127.0.0.1` or `localhost` (anything else
    /// becomes `127.0.0.1`). OpenRouter documents `localhost`.
    pub fn redirect_uri_for(&self, host: &str) -> String {
        let host = if host == "localhost" { "localhost" } else { "127.0.0.1" };
        format!("http://{host}:{}/callback", self.port)
    }

    /// Waits for the redirect and returns its code. Stops at `timeout`, on `cancel`, or after
    /// 20 unusable requests. Consumes the receiver: the port is closed when this returns.
    pub fn wait(self, expected_state: &str, timeout: Duration, cancel: Option<&Arc<AtomicBool>>) -> Result<AuthCode, NetError> {
        self.wait_until(expected_state, timeout, &|| cancel.is_some_and(|c| c.load(Ordering::SeqCst)))
    }

    /// Like [`Loopback::wait`], with any cancel check (a job's cancel flag, for instance).
    pub fn wait_until(self, expected_state: &str, timeout: Duration, cancelled: &dyn Fn() -> bool) -> Result<AuthCode, NetError> {
        let deadline = Instant::now() + timeout;
        let mut bad = 0u32;
        loop {
            if cancelled() {
                return Err(NetError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(oauth_err("timed out waiting for you to finish signing in"));
            }
            let Some(stream) = self.listeners.iter().find_map(|l| l.accept().ok().map(|(s, _)| s)) else {
                std::thread::sleep(Duration::from_millis(25));
                continue;
            };
            match self.serve(stream, expected_state) {
                Verdict::Code(code) => return Ok(code),
                Verdict::Denied(reason) => return Err(NetError::OAuth(format!("the provider refused: {reason}"))),
                Verdict::Ignored => {
                    bad += 1;
                    if bad >= MAX_BAD_REQUESTS {
                        return Err(oauth_err("too many unexpected requests on the sign-in port"));
                    }
                }
            }
        }
    }

    fn serve(&self, mut stream: TcpStream, expected_state: &str) -> Verdict {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        let mut data = Vec::with_capacity(512);
        let mut chunk = [0u8; 512];
        let header_end = loop {
            if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                break i;
            }
            if data.len() >= MAX_REQUEST {
                return reply(&mut stream, 431, "Request too large.");
            }
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => return Verdict::Ignored,
                Ok(n) => data.extend_from_slice(chunk.get(..n).unwrap_or(&[])),
            }
        };
        let head = data.get(..header_end.saturating_add(4)).unwrap_or(&[]);
        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut req = httparse::Request::new(&mut headers);
        if !matches!(req.parse(head), Ok(httparse::Status::Complete(_))) {
            return reply(&mut stream, 400, "Bad request.");
        }
        let host_ok = req.headers.iter().any(|h| {
            h.name.eq_ignore_ascii_case("host")
                && std::str::from_utf8(h.value)
                    .is_ok_and(|v| v == format!("127.0.0.1:{}", self.port) || v == format!("localhost:{}", self.port) || v == format!("[::1]:{}", self.port))
        });
        if !host_ok {
            return reply(&mut stream, 400, "Unexpected host.");
        }
        if req.method != Some("GET") {
            return reply(&mut stream, 405, "Method not allowed.");
        }
        let target = req.path.unwrap_or("");
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        if path != "/callback" {
            return reply(&mut stream, 404, "Not found.");
        }
        let (mut code, mut state, mut error) = (None, None, None);
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let v = percent_decode_str(&v.replace('+', " ")).decode_utf8_lossy().into_owned();
            match k {
                "code" => code = Some(v),
                "state" => state = Some(v),
                "error" => error = Some(v),
                _ => {}
            }
        }
        let state_ok = state.as_deref().is_some_and(|s| bool::from(s.as_bytes().ct_eq(expected_state.as_bytes())));
        // A refusal may come back with no `state` at all; it carries no code, so accepting it can
        // only end the wait. A wrong `state` is still turned away.
        if let Some(e) = error.filter(|_| state_ok || state.is_none()) {
            let _ = reply(&mut stream, 200, &format!("Sign-in was cancelled. You can close this tab and return to {}.", crate::brand::NAME));
            return Verdict::Denied(redact::message(&e.chars().take(80).collect::<String>()));
        }
        if !state_ok {
            return reply(&mut stream, 400, &format!("This sign-in link does not belong to this session. Start again from {}.", crate::brand::NAME));
        }
        match code {
            Some(c) if !c.is_empty() && c.len() <= MAX_CODE && !c.chars().any(char::is_control) => {
                let _ = reply(&mut stream, 200, &format!("Signed in. You can close this tab and return to {}.", crate::brand::NAME));
                Verdict::Code(AuthCode(Zeroizing::new(c)))
            }
            _ => reply(&mut stream, 400, "The sign-in answer had no code."),
        }
    }
}

enum Verdict {
    Code(AuthCode),
    Denied(String),
    Ignored,
}

/// Writes a small fixed page and reports the request as unusable (callers return `Verdict::Code`
/// themselves for the success page). The code is never echoed.
fn reply(stream: &mut TcpStream, status: u16, text: &str) -> Verdict {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>{name}</title><body style=\"font:16px system-ui;margin:3em\"><p>{text}</p>",
        name = crate::brand::NAME
    );
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
    Verdict::Ignored
}

/// The values a provider's sign-in address needs.
pub struct FlowParams<'a> {
    pub redirect_uri: &'a str,
    pub state: &'a str,
    pub code_challenge: &'a str,
}

/// A started sign-in: open [`Flow::authorize_url`] in the browser, then call [`Flow::wait`].
pub struct Flow {
    pub authorize_url: String,
    pub pkce: Pkce,
    state: String,
    loopback: Loopback,
    host: &'static str,
}

impl Flow {
    /// `build` makes the provider's sign-in address, usually with [`build_url`]. The redirect
    /// uses `127.0.0.1`.
    pub fn start(build: impl FnOnce(&FlowParams<'_>) -> Result<String, NetError>) -> Result<Self, NetError> {
        Self::start_on("127.0.0.1", build)
    }

    /// Like [`Flow::start`], with the redirect on `localhost` or `127.0.0.1` (`host`), for
    /// providers that document one of them.
    pub fn start_on(host: &str, build: impl FnOnce(&FlowParams<'_>) -> Result<String, NetError>) -> Result<Self, NetError> {
        let loopback = Loopback::bind()?;
        let pkce = Pkce::generate()?;
        let state = random_token(24)?;
        let host: &'static str = if host == "localhost" { "localhost" } else { "127.0.0.1" };
        let redirect = loopback.redirect_uri_for(host);
        let authorize_url = build(&FlowParams { redirect_uri: &redirect, state: &state, code_challenge: &pkce.challenge })?;
        Ok(Self { authorize_url, pkce, state, loopback, host })
    }

    pub fn redirect_uri(&self) -> String {
        self.loopback.redirect_uri_for(self.host)
    }

    /// Waits for the redirect; returns the code and the PKCE pair for the token request.
    pub fn wait(self, timeout: Duration, cancel: Option<&Arc<AtomicBool>>) -> Result<(AuthCode, Pkce), NetError> {
        let code = self.loopback.wait(&self.state, timeout, cancel)?;
        Ok((code, self.pkce))
    }

    /// Like [`Flow::wait`], with any cancel check (a job's cancel flag, for instance).
    pub fn wait_until(self, timeout: Duration, cancelled: &dyn Fn() -> bool) -> Result<(AuthCode, Pkce), NetError> {
        let code = self.loopback.wait_until(&self.state, timeout, cancelled)?;
        Ok((code, self.pkce))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NetState;
    use crate::http::ClientConfig;
    use crate::testserver::{response, serve};

    fn raw(port: u16, request: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let _ = s.write_all(request.as_bytes());
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    fn get(port: u16, target: &str) -> String {
        raw(port, &format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"))
    }

    fn waiting(state: &'static str, timeout: Duration) -> (u16, std::thread::JoinHandle<Result<AuthCode, NetError>>) {
        let lb = Loopback::bind().unwrap();
        let port = lb.port();
        (port, std::thread::spawn(move || lb.wait(state, timeout, None)))
    }

    fn query_param(url: &str, name: &str) -> Option<String> {
        let q = url.split_once('?')?.1;
        q.split('&').find_map(|p| p.split_once('=').filter(|(k, _)| *k == name).map(|(_, v)| percent_decode_str(v).decode_utf8_lossy().into_owned()))
    }

    #[test]
    fn pkce_matches_the_rfc_7636_vector() {
        let p = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let g = Pkce::generate().unwrap();
        assert_eq!(g.verifier().len(), 43);
        assert!(g.verifier().bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert_ne!(g.verifier(), Pkce::generate().unwrap().verifier());
    }

    #[test]
    fn random_tokens_differ_and_are_bounded() {
        let a = random_token(24).unwrap();
        assert_eq!(a.len(), 32);
        assert_ne!(a, random_token(24).unwrap());
        assert!(!random_token(0).unwrap().is_empty());
        assert!(random_token(10_000).unwrap().len() <= 171);
    }

    #[test]
    fn urls_are_encoded_and_must_be_https() {
        let u = build_url("https://p.test/auth", &[("redirect_uri", "http://127.0.0.1:1/callback"), ("scope", "a b&c=d é")]).unwrap();
        assert_eq!(u, "https://p.test/auth?redirect_uri=http%3A%2F%2F127.0.0.1%3A1%2Fcallback&scope=a%20b%26c%3Dd%20%C3%A9");
        for bad in ["http://evil.test/", "javascript:alert(1)", "https://p.test/a?x=1", "https://p.test/a b", "", "//p.test"] {
            assert!(build_url(bad, &[]).is_err(), "{bad:?}");
        }
        assert_eq!(form_body(&[("a", "1 2"), ("b", "&")]), b"a=1%202&b=%26");
    }

    #[test]
    fn the_receiver_returns_the_code_once_and_closes() {
        let (port, h) = waiting("S3CRET", Duration::from_secs(5));
        let page = get(port, "/callback?code=abc%20d&state=S3CRET");
        assert!(page.starts_with("HTTP/1.1 200") && !page.contains("abc"), "{page}");
        assert!(page.contains("Cache-Control: no-store") && page.contains("Content-Security-Policy"));
        assert_eq!(h.join().unwrap().unwrap().as_str(), "abc d");
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err(), "the port must be closed after one use");
    }

    #[test]
    fn unusable_requests_are_answered_and_ignored() {
        let (port, h) = waiting("S", Duration::from_secs(10));
        assert!(get(port, "/favicon.ico").starts_with("HTTP/1.1 404"));
        assert!(get(port, "/callback?code=evil&state=WRONG").starts_with("HTTP/1.1 400"));
        assert!(get(port, "/callback?code=evil").starts_with("HTTP/1.1 400"));
        assert!(raw(port, "GET /callback?code=evil&state=S HTTP/1.1\r\nHost: evil.test\r\n\r\n").starts_with("HTTP/1.1 400"));
        assert!(raw(port, &format!("GET /callback?code=evil&state=S HTTP/1.1\r\nHost: 127.0.0.1:{port}.evil.test\r\n\r\n")).starts_with("HTTP/1.1 400"));
        assert!(raw(port, &format!("POST /callback?code=evil&state=S HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")).starts_with("HTTP/1.1 405"));
        assert!(raw(port, "garbage\r\n\r\n").starts_with("HTTP/1.1 400"));
        assert!(
            raw(port, &format!("GET /callback?code=evil&state=S HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX: {}\r\n\r\n", "a".repeat(6000)))
                .starts_with("HTTP/1.1 431")
        );
        assert!(get(port, "/callback?state=S").starts_with("HTTP/1.1 400"));
        assert!(get(port, &format!("/callback?code={}&state=S", "a".repeat(3000))).starts_with("HTTP/1.1 400"));
        assert!(get(port, "/callback?code=good&state=S").starts_with("HTTP/1.1 200"));
        assert_eq!(h.join().unwrap().unwrap().as_str(), "good");
    }

    #[test]
    fn a_provider_error_with_the_right_state_stops_the_wait() {
        let (port, h) = waiting("S", Duration::from_secs(5));
        let page = get(port, "/callback?error=access_denied&state=S");
        assert!(page.contains("cancelled"));
        let e = h.join().unwrap().err().unwrap();
        assert!(matches!(&e, NetError::OAuth(m) if m.contains("access_denied")), "{e:?}");
    }

    #[test]
    fn a_refusal_without_state_ends_the_wait_but_a_wrong_state_does_not() {
        let (port, h) = waiting("S", Duration::from_secs(5));
        // someone else's link with the wrong state is turned away and the wait goes on
        assert!(get(port, "/callback?error=access_denied&state=WRONG").starts_with("HTTP/1.1 400"));
        // a provider that sends the person back with no state at all, on a refusal
        let page = get(port, "/callback?error=access_denied");
        assert!(page.starts_with("HTTP/1.1 200") && page.contains("cancelled"), "{page}");
        let e = h.join().unwrap().err().unwrap();
        assert!(matches!(&e, NetError::OAuth(m) if m.contains("access_denied")), "{e:?}");
        // and a bare callback with nothing in it is still just unusable
        let (port, h) = waiting("S", Duration::from_millis(400));
        assert!(get(port, "/callback").starts_with("HTTP/1.1 400"));
        assert!(matches!(h.join().unwrap().err().unwrap(), NetError::OAuth(m) if m.contains("timed out")));
    }

    #[test]
    fn a_refusal_can_never_produce_a_code() {
        let (port, h) = waiting("S", Duration::from_secs(5));
        let _ = get(port, "/callback?error=denied&code=EVIL");
        let e = h.join().unwrap().err().unwrap();
        assert!(matches!(&e, NetError::OAuth(m) if m.contains("denied")), "a code next to an error is not a code: {e:?}");
    }

    #[test]
    fn the_receiver_answers_on_localhost_and_on_ipv6_when_the_machine_has_it() {
        let (port, h) = waiting("S", Duration::from_secs(5));
        let page = raw(port, &format!("GET /callback?code=viaName&state=S HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"));
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert_eq!(h.join().unwrap().unwrap().as_str(), "viaName");

        let lb = Loopback::bind().unwrap();
        let port = lb.port();
        assert_eq!(lb.redirect_uri_for("localhost"), format!("http://localhost:{port}/callback"));
        assert_eq!(lb.redirect_uri_for("evil.example"), format!("http://127.0.0.1:{port}/callback"), "only the two loopback names are advertised");
        let has_v6 = lb.listeners.len() == 2;
        let h = std::thread::spawn(move || lb.wait("S", Duration::from_secs(5), None));
        if has_v6 {
            let mut c = TcpStream::connect(("::1", port)).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let _ = c.write_all(format!("GET /callback?code=v6&state=S HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes());
            let mut out = String::new();
            let _ = c.read_to_string(&mut out);
            assert!(out.starts_with("HTTP/1.1 200"), "{out}");
        } else {
            let _ = get(port, "/callback?code=v6&state=S");
        }
        assert_eq!(h.join().unwrap().unwrap().as_str(), "v6");
    }

    #[test]
    fn a_job_can_stop_the_wait_with_its_own_cancel_check() {
        let lb = Loopback::bind().unwrap();
        let polls = std::sync::atomic::AtomicU32::new(0);
        let e = lb.wait_until("S", Duration::from_secs(10), &|| polls.fetch_add(1, Ordering::SeqCst) >= 3).err().unwrap();
        assert_eq!(e, NetError::Cancelled);
    }

    #[test]
    fn the_wait_ends_on_timeout_cancel_and_flooding() {
        let lb = Loopback::bind().unwrap();
        let e = lb.wait("S", Duration::from_millis(150), None).err().unwrap();
        assert!(matches!(&e, NetError::OAuth(m) if m.contains("timed out")), "{e:?}");

        let flag = Arc::new(AtomicBool::new(true));
        let lb = Loopback::bind().unwrap();
        assert_eq!(lb.wait("S", Duration::from_secs(5), Some(&flag)).err().unwrap(), NetError::Cancelled);

        let (port, h) = waiting("S", Duration::from_secs(20));
        for _ in 0..MAX_BAD_REQUESTS {
            let _ = get(port, "/nope");
        }
        let e = h.join().unwrap().err().unwrap();
        assert!(matches!(&e, NetError::OAuth(m) if m.contains("too many")), "{e:?}");
    }

    #[test]
    fn a_connection_that_sends_nothing_does_not_block_the_flow() {
        let (port, h) = waiting("S", Duration::from_secs(10));
        let _idle = TcpStream::connect(("127.0.0.1", port)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        // The idle connection is dropped after its 2 s read timeout; the real redirect then works.
        let t = Instant::now();
        let page = get(port, "/callback?code=ok&state=S");
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(t.elapsed() < Duration::from_secs(6));
        assert_eq!(h.join().unwrap().unwrap().as_str(), "ok");
    }

    #[test]
    fn a_full_flow_against_a_mock_provider() {
        let token_server = serve(|_, _| response(200, &[("Content-Type", "application/json")], br#"{"key":"sk-or-test"}"#));
        let flow = Flow::start(|p| {
            build_url(
                "https://provider.test/auth",
                &[("callback_url", p.redirect_uri), ("code_challenge", p.code_challenge), ("code_challenge_method", Pkce::METHOD), ("state", p.state)],
            )
        })
        .unwrap();
        let auth = flow.authorize_url.clone();
        assert!(auth.starts_with("https://provider.test/auth?"));
        let challenge = query_param(&auth, "code_challenge").unwrap();
        assert_eq!(challenge, flow.pkce.challenge);
        let redirect = query_param(&auth, "callback_url").unwrap();
        assert_eq!(redirect, flow.redirect_uri());
        let state = query_param(&auth, "state").unwrap();
        // The "browser": approve, then follow the redirect with the provider's code.
        let port: u16 = redirect.rsplit(':').next().unwrap().split('/').next().unwrap().parse().unwrap();
        let browser = std::thread::spawn(move || get(port, &format!("/callback?code=THE-CODE&state={state}")));
        let (code, pkce) = flow.wait(Duration::from_secs(5), None).unwrap();
        assert!(browser.join().unwrap().starts_with("HTTP/1.1 200"));
        assert_eq!(code.as_str(), "THE-CODE");
        assert_eq!(challenge, Pkce::from_verifier(pkce.verifier()).challenge);

        let client =
            Client::new(ClientConfig { allow_loopback_http: true, min_host_interval: Duration::ZERO, ..ClientConfig::default() }, Arc::new(NetState::new()));
        let body = format!("{{\"code\":\"{}\",\"code_verifier\":\"{}\"}}", code.as_str(), pkce.verifier()).into_bytes();
        let json = post_for_json(&client, &format!("{}/token", token_server.url), "application/json", body).unwrap();
        assert_eq!(json["key"], "sk-or-test");
        let seen = token_server.requests.lock().unwrap()[0].clone();
        assert!(seen.contains("THE-CODE") && seen.contains(pkce.verifier()) && seen.starts_with("POST /token"));
        // The log of that request carries neither code nor verifier.
        let log = format!("{:?}", client.state().entries(None));
        assert!(!log.contains("THE-CODE") && !log.contains(pkce.verifier()), "{log}");
    }

    #[test]
    fn a_failed_token_request_reports_only_the_status() {
        let s = serve(|_, _| response(400, &[], br#"{"error":"invalid_grant","echo":"THE-CODE"}"#));
        let client =
            Client::new(ClientConfig { allow_loopback_http: true, min_host_interval: Duration::ZERO, ..ClientConfig::default() }, Arc::new(NetState::new()));
        let e = post_for_json(&client, &s.url, "application/x-www-form-urlencoded", form_body(&[("code", "THE-CODE")])).unwrap_err();
        assert_eq!(e, NetError::Status(400));
        assert!(!e.to_string().contains("THE-CODE"));
    }
}
