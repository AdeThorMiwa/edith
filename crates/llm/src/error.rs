use edith_net::NetError;

/// Everything that can go wrong talking to a model. Messages are safe to show and log: they never
/// contain a key, and text that came from the provider is scrubbed and cut short.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LlmError {
    #[error(transparent)]
    Net(#[from] NetError),
    /// No key, or the provider refused it (HTTP 401 or 403).
    #[error("{0}")]
    Auth(String),
    #[error("the provider is rate limiting requests; try again in a moment")]
    RateLimited,
    #[error("the provider is overloaded; try again in a moment")]
    Overloaded,
    /// The provider rejected the request itself (HTTP 400).
    #[error("the provider rejected the request: {0}")]
    BadRequest(String),
    #[error("the provider answered {status}: {message}")]
    Api { status: u16, message: String },
    /// The reply stream was malformed or ended early.
    #[error("the reply was not understood: {0}")]
    BadStream(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("stopped")]
    Cancelled,
}

impl LlmError {
    /// Worth trying again later (the request itself was fine).
    pub fn retryable(&self) -> bool {
        matches!(self, LlmError::RateLimited | LlmError::Overloaded | LlmError::Net(NetError::Timeout | NetError::Transport(_)))
    }

    /// This error with every occurrence of `secret` removed from its text. The client calls it with
    /// the key it used, so even a server that echoes the key back cannot leak it.
    pub fn scrubbed(self, secret: &str) -> Self {
        if secret.len() < 8 {
            return self;
        }
        let hide = |s: String| s.replace(secret, "[hidden]");
        match self {
            LlmError::Auth(m) => LlmError::Auth(hide(m)),
            LlmError::BadRequest(m) => LlmError::BadRequest(hide(m)),
            LlmError::Api { status, message } => LlmError::Api { status, message: hide(message) },
            LlmError::BadStream(m) => LlmError::BadStream(hide(m)),
            LlmError::Unavailable(m) => LlmError::Unavailable(hide(m)),
            other => other,
        }
    }

    pub(crate) fn bad_stream(msg: &str) -> Self {
        LlmError::BadStream(edith_net::redact::message(msg))
    }
}

/// Maps a provider error (the `type` and `message` of its error object, and the HTTP status when
/// there is one; 0 for an error that arrived inside a stream) to an [`LlmError`]. The message is
/// scrubbed and cut short: providers sometimes quote part of the request.
pub(crate) fn error_for(status: u16, kind: &str, message: &str) -> LlmError {
    let message: String = edith_net::redact::message(message);
    match (kind, status) {
        ("overloaded_error", _) | (_, 529) => LlmError::Overloaded,
        ("rate_limit_error", _) | (_, 429) => LlmError::RateLimited,
        ("authentication_error" | "permission_error", _) | (_, 401 | 403) => LlmError::Auth(format!("the provider refused the key: {message}")),
        ("invalid_request_error", _) | (_, 400) => LlmError::BadRequest(message),
        (_, 402) => LlmError::Api { status, message: format!("the account is out of credits: {message}") },
        _ => LlmError::Api { status, message },
    }
}

/// Maps a failed HTTP answer (status and the start of the body) to an [`LlmError`].
#[cfg_attr(not(any(test, feature = "net")), allow(dead_code))]
pub(crate) fn error_from_http(status: u16, body: &[u8]) -> LlmError {
    let v: serde_json::Value = serde_json::from_slice(body).unwrap_or(serde_json::Value::Null);
    let e = v.get("error").unwrap_or(&serde_json::Value::Null);
    // Anthropic puts a word in `type`; OpenAI-style servers use `code` (a number or a word) or `type`.
    let kind = e.get("type").and_then(serde_json::Value::as_str).or_else(|| e.get("code").and_then(serde_json::Value::as_str)).unwrap_or("");
    let message = e.get("message").and_then(serde_json::Value::as_str).unwrap_or("");
    error_for(status, kind, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_map_by_kind_and_status_and_never_echo_keys() {
        assert_eq!(error_for(0, "overloaded_error", ""), LlmError::Overloaded);
        assert_eq!(error_for(529, "", ""), LlmError::Overloaded);
        assert_eq!(error_for(429, "rate_limit_error", "slow"), LlmError::RateLimited);
        assert!(matches!(error_for(401, "authentication_error", "invalid x-api-key"), LlmError::Auth(_)));
        assert!(matches!(error_for(403, "", ""), LlmError::Auth(_)));
        assert!(matches!(error_for(400, "invalid_request_error", "max_tokens too big"), LlmError::BadRequest(m) if m.contains("max_tokens")));
        assert!(matches!(error_for(500, "api_error", "oops"), LlmError::Api { status: 500, .. }));
        let leaky = error_for(400, "invalid_request_error", "bad header Authorization: Bearer sk-ant-SECRET and key=ABC123");
        assert!(!leaky.to_string().contains("SECRET") && !leaky.to_string().contains("ABC123"), "{leaky}");
        let long = error_for(500, "", &"x".repeat(5000)).to_string();
        assert!(long.len() < 400);
    }

    #[test]
    fn http_error_bodies_are_read_and_garbage_is_tolerated() {
        let body = br#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        assert!(matches!(error_from_http(401, body), LlmError::Auth(_)));
        assert!(matches!(error_from_http(502, b"<html>bad gateway</html>"), LlmError::Api { status: 502, .. }));
        // OpenAI-style bodies: a numeric `code`, or a word, and no `type`
        let credits = error_from_http(402, br#"{"error":{"code":402,"message":"Insufficient credits"}}"#);
        assert!(
            matches!(&credits, LlmError::Api { status: 402, message } if message.contains("out of credits") && message.contains("Insufficient")),
            "{credits:?}"
        );
        assert!(matches!(error_from_http(401, br#"{"error":{"code":401,"message":"No auth credentials found"}}"#), LlmError::Auth(_)));
        assert_eq!(error_from_http(429, br#"{"error":{"code":"rate_limit_exceeded","message":"slow"}}"#), LlmError::RateLimited);
        assert!(matches!(error_from_http(429, b""), LlmError::RateLimited));
    }

    #[test]
    fn the_exact_key_is_removed_from_any_error_text() {
        let key = "plain-looking-key-1234";
        let e = LlmError::Auth(format!("refused {key} today")).scrubbed(key);
        assert_eq!(e.to_string(), "refused [hidden] today");
        let e = LlmError::Api { status: 500, message: format!("{key}{key}") }.scrubbed(key);
        assert!(!e.to_string().contains(key));
        // a too-short "secret" is ignored rather than mangling every message
        assert_eq!(LlmError::Auth("abc".into()).scrubbed("a").to_string(), "abc");
    }

    #[test]
    fn only_transient_errors_are_retryable() {
        assert!(LlmError::Overloaded.retryable() && LlmError::RateLimited.retryable() && LlmError::Net(NetError::Timeout).retryable());
        assert!(
            !LlmError::Auth("x".into()).retryable()
                && !LlmError::Net(NetError::Offline).retryable()
                && !LlmError::Cancelled.retryable()
                && !LlmError::BadRequest("x".into()).retryable()
        );
    }
}
