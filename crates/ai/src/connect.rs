//! Signing in to OpenRouter from the Connections panel instead of copying a key (feature `signin`).
//!
//! The person approves in OpenRouter's own page; OpenRouter sends the browser back to a one-shot
//! receiver on this computer with a short-lived code, and Edith trades the code (plus the PKCE
//! secret it made) for an ordinary API key that the person can see and revoke in their OpenRouter
//! account. The key goes straight to the caller to store in the keychain; nothing here logs it.
//!
//! The pieces are separate so each can be tested and so the engine can run the long wait as a
//! cancellable job: [`OpenRouter::start`] (binds the receiver, builds the address),
//! [`edith_net::oauth::Flow::wait_until`] (the wait), [`OpenRouter::exchange`] (the code for a key).

use edith_net::NetError;
use edith_net::http::Client;
use edith_net::oauth::{AuthCode, Flow, Pkce, build_url, post_for_json};
use zeroize::Zeroizing;

/// Where OpenRouter's sign-in page lives. Fields are public so tests can point at a mock.
pub struct OpenRouter {
    pub auth_url: String,
    pub key_url: String,
}

impl Default for OpenRouter {
    fn default() -> Self {
        Self { auth_url: "https://openrouter.ai/auth".to_string(), key_url: "https://openrouter.ai/api/v1/auth/keys".to_string() }
    }
}

/// A key as OpenRouter returns it: visible characters only, a plausible length.
fn plausible_key(k: &str) -> bool {
    (16..=512).contains(&k.len()) && k.bytes().all(|b| b.is_ascii_graphic())
}

impl OpenRouter {
    /// Binds the receiver and builds the sign-in address. OpenRouter documents `localhost` (any
    /// port) as the redirect.
    pub fn start(&self) -> Result<Flow, NetError> {
        Flow::start_on("localhost", |p| {
            build_url(
                &self.auth_url,
                &[("callback_url", p.redirect_uri), ("code_challenge", p.code_challenge), ("code_challenge_method", Pkce::METHOD), ("state", p.state)],
            )
        })
    }

    /// Trades the code for the person's API key. Only the status is reported on failure.
    pub fn exchange(&self, client: &Client, code: &AuthCode, pkce: &Pkce) -> Result<Zeroizing<String>, NetError> {
        let body =
            Zeroizing::new(serde_json::json!({"code": code.as_str(), "code_verifier": pkce.verifier(), "code_challenge_method": Pkce::METHOD}).to_string());
        let mut answer = post_for_json(client, &self.key_url, "application/json", body.as_bytes().to_vec())?;
        let key = match answer.get_mut("key").map(serde_json::Value::take) {
            Some(serde_json::Value::String(k)) => Zeroizing::new(k),
            _ => return Err(NetError::OAuth("OpenRouter did not return a key".to_string())),
        };
        if !plausible_key(&key) {
            return Err(NetError::OAuth("OpenRouter returned something that is not a key".to_string()));
        }
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edith_net::NetState;
    use edith_net::http::ClientConfig;
    use edith_net::testserver::{response, serve};
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::Arc;
    use std::time::Duration;

    const KEY: &str = "sk-or-v1-0123456789abcdef0123456789abcdef";

    fn client() -> Client {
        Client::new(ClientConfig { allow_loopback_http: true, min_host_interval: Duration::ZERO, ..ClientConfig::default() }, Arc::new(NetState::new()))
    }

    fn param(url: &str, name: &str) -> String {
        let q = url.split_once('?').map(|x| x.1).unwrap_or("");
        let v = q.split('&').find_map(|p| p.split_once('=').filter(|(k, _)| *k == name).map(|x| x.1)).unwrap_or("");
        percent_decode(v)
    }

    fn percent_decode(s: &str) -> String {
        let b = s.as_bytes();
        let (mut out, mut i) = (Vec::new(), 0);
        while i < b.len() {
            if b[i] == b'%'
                && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                i += 3;
                continue;
            }
            out.push(b[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// The browser: follows the redirect to the receiver, as a browser using `localhost` would.
    fn browser(redirect: &str, query: &str) -> String {
        let port: u16 = redirect.rsplit(':').next().unwrap().split('/').next().unwrap().parse().unwrap();
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let _ = c.write_all(format!("GET /callback?{query} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes());
        let mut out = String::new();
        let _ = c.read_to_string(&mut out);
        out
    }

    #[test]
    fn the_sign_in_address_asks_for_a_localhost_redirect_with_pkce_and_state() {
        let flow = OpenRouter::default().start().unwrap();
        let url = flow.authorize_url.clone();
        assert!(url.starts_with("https://openrouter.ai/auth?"), "{url}");
        let redirect = param(&url, "callback_url");
        assert!(redirect.starts_with("http://localhost:") && redirect.ends_with("/callback"), "{redirect}");
        assert_eq!(redirect, flow.redirect_uri());
        assert_eq!(param(&url, "code_challenge_method"), "S256");
        assert_eq!(param(&url, "code_challenge"), flow.pkce.challenge);
        assert!(param(&url, "state").len() >= 24);
        assert!(!url.contains(flow.pkce.verifier()), "the PKCE secret never goes in the address");
    }

    #[test]
    fn the_whole_sign_in_against_a_mock_openrouter() {
        let exchange = serve(|_, _| response(200, &[("Content-Type", "application/json")], format!(r#"{{"key":"{KEY}"}}"#).as_bytes()));
        let or = OpenRouter { auth_url: "https://openrouter.ai/auth".into(), key_url: format!("{}/api/v1/auth/keys", exchange.url) };
        let flow = or.start().unwrap();
        let (redirect, state) = (param(&flow.authorize_url, "callback_url"), param(&flow.authorize_url, "state"));
        let page = std::thread::spawn(move || browser(&redirect, &format!("code=THE-CODE&state={state}")));
        let (code, pkce) = flow.wait_until(Duration::from_secs(5), &|| false).unwrap();
        assert!(page.join().unwrap().starts_with("HTTP/1.1 200"));
        let c = client();
        let key = or.exchange(&c, &code, &pkce).unwrap();
        assert_eq!(key.as_str(), KEY);
        let seen = exchange.requests.lock().unwrap()[0].clone();
        assert!(seen.starts_with("POST /api/v1/auth/keys"), "{seen}");
        let body: serde_json::Value = serde_json::from_str(seen.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["code"], "THE-CODE");
        assert_eq!(body["code_challenge_method"], "S256");
        assert_eq!(Pkce::from_verifier(body["code_verifier"].as_str().unwrap()).challenge, pkce.challenge, "the verifier matches the challenge sent");
        // nothing the log keeps carries the code, the verifier or the key
        let log = format!("{:?}", c.state().entries(None));
        for secret in ["THE-CODE", pkce.verifier(), KEY, "0123456789abcdef"] {
            assert!(!log.contains(secret), "{log}");
        }
    }

    #[test]
    fn declining_in_the_browser_ends_the_wait_with_no_key_and_no_request() {
        let exchange = serve(|_, _| response(200, &[], format!(r#"{{"key":"{KEY}"}}"#).as_bytes()));
        let or = OpenRouter { auth_url: "https://openrouter.ai/auth".into(), key_url: exchange.url.clone() };
        let flow = or.start().unwrap();
        let redirect = param(&flow.authorize_url, "callback_url");
        // OpenRouter does not send `state` back when the person declines
        let page = std::thread::spawn(move || browser(&redirect, "error=access_denied"));
        let e = flow.wait_until(Duration::from_secs(5), &|| false).err().unwrap();
        page.join().unwrap();
        assert!(matches!(&e, NetError::OAuth(m) if m.contains("access_denied")), "{e:?}");
        assert_eq!(exchange.hits(), 0);
    }

    #[test]
    fn cancelling_stops_the_wait() {
        let flow = OpenRouter::default().start().unwrap();
        let e = flow.wait_until(Duration::from_secs(5), &|| true).err().unwrap();
        assert_eq!(e, NetError::Cancelled);
    }

    #[test]
    fn a_bad_answer_from_the_key_endpoint_is_an_error_that_never_echoes_it() {
        let (pkce, code) = {
            // a real code object needs a real redirect; get one through the receiver
            let or = OpenRouter { auth_url: "https://openrouter.ai/auth".into(), key_url: String::new() };
            let flow = or.start().unwrap();
            let (redirect, state) = (param(&flow.authorize_url, "callback_url"), param(&flow.authorize_url, "state"));
            let h = std::thread::spawn(move || browser(&redirect, &format!("code=C&state={state}")));
            let (code, pkce) = flow.wait_until(Duration::from_secs(5), &|| false).unwrap();
            h.join().unwrap();
            (pkce, code)
        };
        for (status, body) in [
            (200u16, r#"{"key":""}"#),
            (200, r#"{"key":"short"}"#),
            (200, r#"{"key":"has a space in it, long enough to pass the length check"}"#),
            (200, r#"{"key":5}"#),
            (200, r#"{"nokey":true}"#),
            (200, "not json"),
            (400, r#"{"error":"invalid_grant","echo":"C"}"#),
        ] {
            let s = serve(move |_, _| response(status, &[], body.as_bytes()));
            let or = OpenRouter { auth_url: String::new(), key_url: s.url.clone() };
            let e = or.exchange(&client(), &code, &pkce).unwrap_err();
            assert!(!e.to_string().contains("invalid_grant") && !e.to_string().contains("has a space"), "{status} {body}: {e}");
        }
    }
}
