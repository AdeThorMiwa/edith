//! The HTTP client every networked feature uses.
//!
//! One choke point ([`Client::send`], [`Client::stream_lines`]) does, in order: validate the
//! address and headers, check Offline mode, wait for the host's rate-limit slot, send with
//! timeouts, retry transient failures, cap the body, cache validated responses, and write one
//! redacted log entry. Requests never panic: the transport runs under `catch_unwind`.

use crate::cache::{Cache, Meta};
use crate::{NetError, NetState, Outcome, redact, tls};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::io::{BufRead, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

const MAX_URL: usize = 2048;
const MAX_HEADERS: usize = 32;
const MAX_HEADER_VALUE: usize = 8192;
const MAX_LINE: u64 = 1 << 20;
const ERROR_BODY: u64 = 8 * 1024;
const MAX_HOSTS: usize = 256;
const RETRY_STATUSES: [u16; 6] = [408, 429, 500, 502, 503, 504];
/// Headers the caller may not set: the transport owns them (request smuggling, framing).
const RESERVED: [&str; 6] = ["host", "content-length", "transfer-encoding", "connection", "upgrade", "te"];

#[derive(Clone, Debug)]
pub struct ClientConfig {
    pub user_agent: String,
    pub connect_timeout: Duration,
    /// Whole-request limit for [`Client::send`].
    pub timeout: Duration,
    /// Largest response body accepted, in bytes.
    pub max_body: u64,
    /// Extra attempts after the first, for idempotent requests only.
    pub retries: u32,
    /// Minimum gap between two requests to the same host.
    pub min_host_interval: Duration,
    /// Where cached responses live (`None` disables the cache).
    pub cache_dir: Option<PathBuf>,
    /// Allow plain `http` to `127.0.0.1`/`localhost` (tests and local model servers).
    pub allow_loopback_http: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            user_agent: crate::brand::user_agent(),
            connect_timeout: Duration::from_secs(10),
            timeout: Duration::from_secs(60),
            max_body: 16 * 1024 * 1024,
            retries: 2,
            min_host_interval: Duration::from_millis(100),
            cache_dir: None,
            allow_loopback_http: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
        }
    }
}

/// A request. `Debug` hides header values and the body (they may hold a key).
#[derive(Clone)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    /// Body size limit for this request (never above the client's `max_body`).
    pub max_bytes: Option<u64>,
    /// Use the on-disk cache (GET only; never combine with credentials in headers).
    pub use_cache: bool,
    /// Retry even though the method is not GET (the caller says it is safe to repeat).
    pub idempotent: bool,
    pub timeout: Option<Duration>,
    pub cancel: Option<Arc<AtomicBool>>,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request").field("method", &self.method).field("url", &redact::url(&self.url)).field("headers", &self.headers.len()).finish()
    }
}

impl Request {
    fn new(method: Method, url: &str) -> Self {
        Self {
            method,
            url: url.to_string(),
            headers: Vec::new(),
            body: None,
            max_bytes: None,
            use_cache: false,
            idempotent: false,
            timeout: None,
            cancel: None,
        }
    }

    pub fn get(url: &str) -> Self {
        Self::new(Method::Get, url)
    }

    pub fn post(url: &str, body: Vec<u8>) -> Self {
        let mut r = Self::new(Method::Post, url);
        r.body = Some(body);
        r
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn cached(mut self) -> Self {
        self.use_cache = true;
        self
    }

    pub fn cancel_with(mut self, flag: Arc<AtomicBool>) -> Self {
        self.cancel = Some(flag);
        self
    }

    fn retryable(&self) -> bool {
        self.method == Method::Get || self.idempotent
    }

    fn cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(|c| c.load(Ordering::SeqCst))
    }
}

#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    /// Served from the on-disk cache after a `304 Not Modified`.
    pub from_cache: bool,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// `Err(Status)` unless the status is 2xx.
    pub fn error_for_status(self) -> Result<Self, NetError> {
        if self.ok() { Ok(self) } else { Err(NetError::Status(self.status)) }
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json<T: DeserializeOwned>(&self) -> Result<T, NetError> {
        serde_json::from_slice(&self.body).map_err(|_| NetError::Transport("the server sent invalid JSON".to_string()))
    }
}

/// What a finished stream reports. For a status of 400 or more, `error_body` holds the start of
/// the body (usually the provider's error JSON) and no lines were delivered.
/// What [`Client::download`] wrote.
#[derive(Clone, Debug)]
pub struct Downloaded {
    pub status: u16,
    pub bytes: u64,
    /// SHA-256 of the file, as hex (empty when nothing was written).
    pub sha256: String,
}

#[derive(Clone, Debug)]
pub struct StreamEnd {
    pub status: u16,
    pub error_body: Vec<u8>,
}

pub struct Client {
    cfg: ClientConfig,
    state: Arc<NetState>,
    agent: ureq::Agent,
    stream_agent: ureq::Agent,
    cache: Option<Cache>,
    next_free: Mutex<HashMap<String, Instant>>,
}

impl Client {
    pub fn new(cfg: ClientConfig, state: Arc<NetState>) -> Self {
        let https_only = !cfg.allow_loopback_http;
        let agent = tls::agent(&cfg.user_agent, cfg.connect_timeout, cfg.timeout, https_only, 5);
        let stream_agent = tls::agent(&cfg.user_agent, cfg.connect_timeout, Duration::from_secs(900), https_only, 5);
        let cache = cfg.cache_dir.as_deref().map(Cache::new);
        Self { cfg, state, agent, stream_agent, cache, next_free: Mutex::new(HashMap::new()) }
    }

    pub fn state(&self) -> &Arc<NetState> {
        &self.state
    }

    /// Sends `req` and returns the whole (size-capped) response. Any HTTP status is `Ok`; use
    /// [`Response::error_for_status`] to turn 4xx/5xx into an error.
    pub fn send(&self, req: &Request) -> Result<Response, NetError> {
        let started = Instant::now();
        let result = self.send_inner(req);
        let millis = elapsed_ms(started);
        match &result {
            Ok(r) => {
                let outcome = if !r.ok() {
                    Outcome::Failed
                } else if r.from_cache {
                    Outcome::Cached
                } else {
                    Outcome::Ok
                };
                let err = (!r.ok()).then(|| format!("the server answered {}", r.status));
                self.state.record(req.method.as_str(), &req.url, outcome, Some(r.status), r.body.len() as u64, millis, err.as_deref());
            }
            Err(e) => self.record_error(req, e, millis),
        }
        result
    }

    /// Sends `req` and calls `on_line` for each line of the body as it arrives (server-sent
    /// events). `on_line` returns `false` to stop. No retries: a half-delivered stream is not
    /// safe to repeat.
    pub fn stream_lines(&self, req: &Request, mut on_line: impl FnMut(&str) -> bool) -> Result<StreamEnd, NetError> {
        let started = Instant::now();
        let mut bytes = 0u64;
        let result = self.stream_inner(req, &mut on_line, &mut bytes);
        let millis = elapsed_ms(started);
        match &result {
            Ok(end) => {
                let outcome = if end.status >= 400 { Outcome::Failed } else { Outcome::Ok };
                let err = (end.status >= 400).then(|| format!("the server answered {}", end.status));
                self.state.record(req.method.as_str(), &req.url, outcome, Some(end.status), bytes, millis, err.as_deref());
            }
            Err(e) => self.record_error(req, e, millis),
        }
        result
    }

    /// Sends `req` and writes the body to `path` as it arrives (a file too big to hold in memory:
    /// video). The file is written as `<path>.part` and renamed only when the whole body has
    /// arrived and is within `max_bytes`; on any failure nothing is left behind. `progress` is
    /// told the bytes so far and the total when the server says it. Returns the status, the size
    /// and the SHA-256 of what was written. A status of 400 or more writes nothing.
    pub fn download(&self, req: &Request, path: &Path, max_bytes: u64, mut progress: impl FnMut(u64, Option<u64>)) -> Result<Downloaded, NetError> {
        use sha2::{Digest, Sha256};
        use std::io::Write;
        let started = Instant::now();
        let part = {
            let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
            name.push(".part");
            path.with_file_name(name)
        };
        let mut bytes = 0u64;
        let result = (|| {
            let host = self.admit(req)?;
            self.wait_for_host(&host, req)?;
            let http_req = build_request(req, None)?;
            let agent = &self.stream_agent;
            let timeout = req.timeout.unwrap_or(Duration::from_secs(1800));
            let sent = catch_unwind(AssertUnwindSafe(|| run(agent, http_req, None, Some(timeout))));
            let mut response = sent.map_err(|_| internal_error())?.map_err(map_err)?;
            let status = response.status().as_u16();
            if status >= 400 {
                return Ok(Downloaded { status, bytes: 0, sha256: String::new() });
            }
            let total = response.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
            if total.is_some_and(|t| t > max_bytes) {
                return Err(NetError::TooLarge(max_bytes));
            }
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| io_error(&e))?;
            }
            let mut file = std::fs::File::create(&part).map_err(|e| io_error(&e))?;
            let mut reader = response.body_mut().with_config().limit(max_bytes).reader();
            let mut hasher = Sha256::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                if req.cancelled() {
                    return Err(NetError::Cancelled);
                }
                let n = reader.read(&mut buf).map_err(|e| io_error(&e))?;
                if n == 0 {
                    break;
                }
                let chunk = buf.get(..n).unwrap_or(&[]);
                file.write_all(chunk).map_err(|e| io_error(&e))?;
                hasher.update(chunk);
                bytes = bytes.saturating_add(n as u64);
                progress(bytes, total);
            }
            file.flush().map_err(|e| io_error(&e))?;
            drop(file);
            if total.is_some_and(|t| t != bytes) {
                return Err(NetError::Transport("the download ended early".to_string()));
            }
            std::fs::rename(&part, path).map_err(|e| io_error(&e))?;
            let digest = hasher.finalize();
            Ok(Downloaded { status, bytes, sha256: digest.iter().map(|b| format!("{b:02x}")).collect() })
        })();
        if result.as_ref().map_or(true, |d| d.status >= 400) {
            let _ = std::fs::remove_file(&part);
        }
        let millis = elapsed_ms(started);
        match &result {
            Ok(d) => {
                let outcome = if d.status >= 400 { Outcome::Failed } else { Outcome::Ok };
                let err = (d.status >= 400).then(|| format!("the server answered {}", d.status));
                self.state.record(req.method.as_str(), &req.url, outcome, Some(d.status), bytes, millis, err.as_deref());
            }
            Err(e) => self.record_error(req, e, millis),
        }
        result
    }

    fn record_error(&self, req: &Request, e: &NetError, millis: u64) {
        let outcome = if *e == NetError::Offline { Outcome::Offline } else { Outcome::Failed };
        self.state.record(req.method.as_str(), &req.url, outcome, None, 0, millis, Some(&e.to_string()));
    }

    fn send_inner(&self, req: &Request) -> Result<Response, NetError> {
        let host = self.admit(req)?;
        let attempts = if req.retryable() { self.cfg.retries.saturating_add(1) } else { 1 };
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            self.wait_for_host(&host, req)?;
            let result = self.attempt(req);
            let last = attempt >= attempts;
            let delay = match &result {
                Ok((resp, retry_after)) if !last && RETRY_STATUSES.contains(&resp.status) => Some(retry_after.unwrap_or_else(|| backoff(attempt))),
                Err(NetError::Timeout | NetError::Transport(_)) if !last => Some(backoff(attempt)),
                _ => None,
            };
            match delay {
                Some(d) => sleep_cancelable(d.min(Duration::from_secs(10)), req)?,
                None => return result.map(|(resp, _)| resp),
            }
        }
    }

    fn stream_inner(&self, req: &Request, on_line: &mut dyn FnMut(&str) -> bool, bytes: &mut u64) -> Result<StreamEnd, NetError> {
        let host = self.admit(req)?;
        self.wait_for_host(&host, req)?;
        let timeout = req.timeout.unwrap_or(Duration::from_secs(900));
        let max = req.max_bytes.unwrap_or(64 * 1024 * 1024);
        let http_req = build_request(req, None)?;
        let agent = &self.stream_agent;
        let sent = catch_unwind(AssertUnwindSafe(|| run(agent, http_req, req.body.as_deref(), Some(timeout))));
        let mut response = sent.map_err(|_| internal_error())?.map_err(map_err)?;
        let status = response.status().as_u16();
        if status >= 400 {
            let body = response.body_mut().with_config().limit(ERROR_BODY).read_to_vec().unwrap_or_default();
            return Ok(StreamEnd { status, error_body: body });
        }
        let mut reader = std::io::BufReader::new(response.body_mut().with_config().limit(max).reader());
        let mut buf = Vec::new();
        loop {
            if req.cancelled() {
                return Err(NetError::Cancelled);
            }
            buf.clear();
            let n = reader.by_ref().take(MAX_LINE + 1).read_until(b'\n', &mut buf).map_err(|e| io_error(&e))?;
            if n == 0 {
                break;
            }
            *bytes = bytes.saturating_add(n as u64);
            if n as u64 > MAX_LINE {
                return Err(NetError::TooLarge(MAX_LINE));
            }
            let line = String::from_utf8_lossy(&buf);
            if !on_line(line.trim_end_matches(['\r', '\n'])) {
                break;
            }
        }
        Ok(StreamEnd { status, error_body: Vec::new() })
    }

    /// Everything that must hold before any byte leaves: address, headers, cancel, Offline.
    /// Returns the host (for rate limiting).
    fn admit(&self, req: &Request) -> Result<String, NetError> {
        let host = self.validate_url(&req.url)?;
        validate_headers(&req.headers)?;
        if req.cancelled() {
            return Err(NetError::Cancelled);
        }
        self.state.check()?;
        Ok(host)
    }

    fn validate_url(&self, url: &str) -> Result<String, NetError> {
        if url.len() > MAX_URL || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(NetError::InvalidUrl("not a valid web address".to_string()));
        }
        let (scheme, rest) = url.split_once("://").ok_or_else(|| NetError::InvalidUrl("not a valid web address".to_string()))?;
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if authority.contains('@') {
            return Err(NetError::Blocked("addresses with a user name or password are not allowed".to_string()));
        }
        let host = redact::host(url).ok_or_else(|| NetError::InvalidUrl("not a valid web address".to_string()))?;
        match scheme.to_ascii_lowercase().as_str() {
            "https" => Ok(host),
            "http" if self.cfg.allow_loopback_http && is_loopback(&host) => Ok(host),
            "http" => Err(NetError::Blocked("only https addresses are allowed".to_string())),
            _ => Err(NetError::InvalidUrl("not a valid web address".to_string())),
        }
    }

    fn wait_for_host(&self, host: &str, req: &Request) -> Result<(), NetError> {
        if self.cfg.min_host_interval.is_zero() {
            return Ok(());
        }
        let now = Instant::now();
        let at = {
            let mut map = self.next_free.lock().unwrap_or_else(PoisonError::into_inner);
            if map.len() >= MAX_HOSTS {
                map.retain(|_, t| *t > now);
            }
            let slot = map.entry(host.to_string()).or_insert(now);
            let at = (*slot).max(now);
            *slot = at + self.cfg.min_host_interval;
            at
        };
        sleep_cancelable(at.saturating_duration_since(now), req)
    }

    /// One try: returns the response and any `Retry-After`.
    fn attempt(&self, req: &Request) -> Result<(Response, Option<Duration>), NetError> {
        let max = req.max_bytes.unwrap_or(self.cfg.max_body).min(self.cfg.max_body);
        let cached = if req.use_cache && req.method == Method::Get { self.cache.as_ref().and_then(|c| c.load(&req.url, max)) } else { None };
        let http_req = build_request(req, cached.as_ref().map(|e| &e.meta))?;
        let agent = &self.agent;
        let timeout = req.timeout;
        let sent = catch_unwind(AssertUnwindSafe(|| run(agent, http_req, req.body.as_deref(), timeout)));
        let mut response = sent.map_err(|_| internal_error())?.map_err(map_err)?;
        let status = response.status().as_u16();
        let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(|v| v.chars().take(512).collect::<String>());
        let retry_after = header("retry-after").and_then(|v| v.trim().parse::<u64>().ok()).map(Duration::from_secs);
        let content_type = header("content-type");
        let etag = header("etag");
        let last_modified = header("last-modified");
        if status == 304
            && let Some(entry) = cached
        {
            return Ok((Response { status: 200, content_type: entry.meta.content_type, body: entry.body, from_cache: true }, None));
        }
        if let Some(len) = response.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok())
            && len > max
        {
            return Err(NetError::TooLarge(max));
        }
        let body = response.body_mut().with_config().limit(max).read_to_vec().map_err(map_err)?;
        if status == 200
            && req.use_cache
            && req.method == Method::Get
            && (etag.is_some() || last_modified.is_some())
            && let Some(c) = &self.cache
        {
            c.store(&req.url, &Meta { etag, last_modified, content_type: content_type.clone() }, &body);
        }
        Ok((Response { status, content_type, body, from_cache: false }, retry_after))
    }
}

fn run(
    agent: &ureq::Agent,
    req: ureq::http::Request<()>,
    body: Option<&[u8]>,
    timeout: Option<Duration>,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    match body {
        Some(b) => {
            let (parts, ()) = req.into_parts();
            let req = ureq::http::Request::from_parts(parts, b);
            agent.run(configure(agent, req, timeout))
        }
        None => agent.run(configure(agent, req, timeout)),
    }
}

fn configure<S: ureq::AsSendBody>(agent: &ureq::Agent, req: ureq::http::Request<S>, timeout: Option<Duration>) -> ureq::http::Request<S> {
    match timeout {
        Some(t) => agent.configure_request(req).timeout_global(Some(t)).build(),
        None => req,
    }
}

fn build_request(req: &Request, validators: Option<&Meta>) -> Result<ureq::http::Request<()>, NetError> {
    let mut b = ureq::http::Request::builder().method(req.method.as_str()).uri(&req.url);
    for (k, v) in &req.headers {
        b = b.header(k.as_str(), v.as_str());
    }
    if let Some(m) = validators {
        if let Some(e) = &m.etag {
            b = b.header("if-none-match", e.as_str());
        }
        if let Some(l) = &m.last_modified {
            b = b.header("if-modified-since", l.as_str());
        }
    }
    b.body(()).map_err(|_| NetError::InvalidUrl("not a valid web address".to_string()))
}

fn validate_headers(headers: &[(String, String)]) -> Result<(), NetError> {
    if headers.len() > MAX_HEADERS {
        return Err(NetError::Blocked("too many headers".to_string()));
    }
    for (name, value) in headers {
        let name_ok = !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !name_ok || RESERVED.contains(&name.to_ascii_lowercase().as_str()) {
            return Err(NetError::Blocked("a header name is not allowed".to_string()));
        }
        if value.len() > MAX_HEADER_VALUE || value.chars().any(|c| c.is_control() && c != '\t') {
            return Err(NetError::Blocked("a header value is not allowed".to_string()));
        }
    }
    Ok(())
}

fn is_loopback(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') { rest.split(']').next().unwrap_or("") } else { host.rsplit_once(':').map_or(host, |(h, _)| h) };
    matches!(name, "localhost" | "127.0.0.1" | "::1")
}

fn map_err(e: ureq::Error) -> NetError {
    match e {
        ureq::Error::Timeout(_) => NetError::Timeout,
        ureq::Error::BodyExceedsLimit(n) => NetError::TooLarge(n),
        ureq::Error::Io(e) => io_error(&e),
        ureq::Error::HostNotFound => NetError::Transport("could not find the server".to_string()),
        ureq::Error::ConnectionFailed => NetError::Transport("could not connect".to_string()),
        ureq::Error::BadUri(_) => NetError::InvalidUrl("not a valid web address".to_string()),
        other => NetError::Transport(redact::message(&other.to_string())),
    }
}

fn io_error(e: &std::io::Error) -> NetError {
    match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => NetError::Timeout,
        _ => NetError::Transport(redact::message(&e.to_string())),
    }
}

fn internal_error() -> NetError {
    NetError::Transport("internal error in the network layer".to_string())
}

fn backoff(attempt: u32) -> Duration {
    let base = 250u64.saturating_mul(1u64 << attempt.saturating_sub(1).min(5));
    let jitter = u64::from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos())) % 100;
    Duration::from_millis(base.saturating_add(jitter).min(8000))
}

fn sleep_cancelable(total: Duration, req: &Request) -> Result<(), NetError> {
    let end = Instant::now() + total.min(Duration::from_secs(30));
    loop {
        if req.cancelled() {
            return Err(NetError::Cancelled);
        }
        let now = Instant::now();
        if now >= end {
            return Ok(());
        }
        std::thread::sleep((end - now).min(Duration::from_millis(25)));
    }
}

fn elapsed_ms(t: Instant) -> u64 {
    u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testserver::{Reply, response, serve};

    fn client_with(f: impl FnOnce(&mut ClientConfig)) -> Client {
        let mut cfg = ClientConfig { allow_loopback_http: true, min_host_interval: Duration::ZERO, timeout: Duration::from_secs(2), ..ClientConfig::default() };
        f(&mut cfg);
        Client::new(cfg, Arc::new(NetState::new()))
    }

    fn client() -> Client {
        client_with(|_| {})
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("edith-net-dl-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_download_is_written_hashed_and_renamed_only_when_whole() {
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let b2 = body.clone();
        let s = serve(move |_, _| response(200, &[("Content-Type", "video/mp4")], &b2));
        let dir = scratch("ok");
        let path = dir.join("sub/clip.mp4");
        let mut seen = Vec::new();
        let d = client().download(&Request::get(&format!("{}/clip.mp4?token=SECRET", s.url)), &path, 10_000_000, |n, t| seen.push((n, t))).unwrap();
        assert_eq!((d.status, d.bytes), (200, 300_000));
        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert!(!path.with_file_name("clip.mp4.part").exists(), "no half file is left");
        use sha2::{Digest, Sha256};
        let want: String = Sha256::digest(&body).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(d.sha256, want);
        assert_eq!(seen.last().copied(), Some((300_000, Some(300_000))));
        assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0));
        let log = format!("{:?}", client().state().entries(None));
        assert!(!log.contains("SECRET"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_download_that_is_too_big_or_refused_or_stopped_leaves_nothing() {
        let s = serve(|_, _| response(200, &[], &[7u8; 5000]));
        let dir = scratch("bad");
        let path = dir.join("f.bin");
        // bigger than allowed: refused up front from the declared length
        assert_eq!(client().download(&Request::get(&s.url), &path, 1000, |_, _| {}).unwrap_err(), NetError::TooLarge(1000));
        assert!(!path.exists() && !dir.join("f.bin.part").exists());
        // the server says no
        let nope = serve(|_, _| response(404, &[], b"not here"));
        let d = client().download(&Request::get(&nope.url), &path, 1000, |_, _| {}).unwrap();
        assert_eq!((d.status, d.bytes), (404, 0));
        assert!(!path.exists() && !dir.join("f.bin.part").exists());
        // stopped before it starts
        let stop = Arc::new(AtomicBool::new(true));
        assert_eq!(client().download(&Request::get(&s.url).cancel_with(stop), &path, 10_000, |_, _| {}).unwrap_err(), NetError::Cancelled);
        // Offline mode blocks it
        let off = client();
        off.state().set_offline(true);
        assert_eq!(off.download(&Request::get(&s.url), &path, 10_000, |_, _| {}).unwrap_err(), NetError::Offline);
        assert_eq!(s.hits(), 1, "only the first (too big) request ever reached the server");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn get_returns_the_body_and_logs_without_the_query() {
        let s = serve(|_, _| response(200, &[("Content-Type", "application/json")], br#"{"a":1}"#));
        let c = client();
        let r = c.send(&Request::get(&format!("{}/v1/x?key=SECRET", s.url)).header("X-Api-Key", "TOPSECRET")).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.json::<serde_json::Value>().unwrap()["a"], 1);
        let req_text = s.requests.lock().unwrap()[0].to_ascii_lowercase();
        assert!(req_text.contains("x-api-key: topsecret") && req_text.contains("user-agent: edith/"));
        let log = c.state().entries(None);
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].outcome, Outcome::Ok);
        let text = format!("{log:?}");
        assert!(!text.contains("SECRET"), "{text}");
        assert!(log[0].url.ends_with("/v1/x"));
    }

    #[test]
    fn offline_fails_fast_without_connecting() {
        let s = serve(|_, _| response(200, &[], b"x"));
        let c = client();
        c.state().set_offline(true);
        assert_eq!(c.send(&Request::get(&s.url)).unwrap_err(), NetError::Offline);
        assert_eq!(s.hits(), 0);
        assert_eq!(c.state().entries(None)[0].outcome, Outcome::Offline);
        c.state().set_offline(false);
        assert!(c.send(&Request::get(&s.url)).is_ok());
    }

    #[test]
    fn only_https_unless_loopback_is_allowed() {
        let strict = client_with(|c| c.allow_loopback_http = false);
        for url in [
            "http://example.com/",
            "http://127.0.0.1:1/",
            "ftp://example.com/",
            "file:///etc/passwd",
            "example.com",
            "",
            "https://u:p@example.com/",
            "https://exa mple.com/",
        ] {
            let e = strict.send(&Request::get(url)).unwrap_err();
            assert!(matches!(e, NetError::Blocked(_) | NetError::InvalidUrl(_)), "{url}: {e:?}");
        }
        let lenient = client();
        assert!(matches!(lenient.send(&Request::get("http://example.com/")).unwrap_err(), NetError::Blocked(_)));
        assert!(matches!(lenient.send(&Request::get(&format!("https://example.com/{}", "a".repeat(5000)))).unwrap_err(), NetError::InvalidUrl(_)));
    }

    #[test]
    fn hostile_headers_are_refused_before_sending() {
        let s = serve(|_, _| response(200, &[], b"x"));
        let c = client();
        for (k, v) in [("X-A", "a\r\nInjected: 1"), ("X A", "a"), ("", "a"), ("Host", "evil"), ("Content-Length", "5"), ("X-A", "\u{0}")] {
            let e = c.send(&Request::get(&s.url).header(k, v)).unwrap_err();
            assert!(matches!(e, NetError::Blocked(_)), "{k:?}: {e:?}");
        }
        let many = (0..40).fold(Request::get(&s.url), |r, i| r.header(&format!("X-{i}"), "v"));
        assert!(matches!(c.send(&many).unwrap_err(), NetError::Blocked(_)));
        assert_eq!(s.hits(), 0);
    }

    #[test]
    fn retries_a_429_with_retry_after_then_succeeds() {
        let s = serve(|n, _| if n == 0 { response(429, &[("Retry-After", "0")], b"slow down") } else { response(200, &[], b"ok") });
        let r = client().send(&Request::get(&s.url)).unwrap();
        assert_eq!((r.status, s.hits()), (200, 2));
    }

    #[test]
    fn gives_up_after_the_retry_budget_and_returns_the_status() {
        let s = serve(|_, _| response(503, &[("Retry-After", "0")], b"down"));
        let c = client();
        let r = c.send(&Request::get(&s.url)).unwrap();
        assert_eq!((r.status, s.hits()), (503, 3));
        assert_eq!(r.clone().error_for_status().unwrap_err(), NetError::Status(503));
        assert_eq!(c.state().entries(None)[0].outcome, Outcome::Failed);
    }

    #[test]
    fn post_is_not_retried() {
        let s = serve(|_, _| response(503, &[("Retry-After", "0")], b"down"));
        let r = client().send(&Request::post(&s.url, b"{}".to_vec())).unwrap();
        assert_eq!((r.status, s.hits()), (503, 1));
        let body_seen = s.requests.lock().unwrap()[0].ends_with("{}");
        assert!(body_seen);
    }

    #[test]
    fn oversized_bodies_are_refused() {
        let s = serve(|_, _| response(200, &[], &vec![b'x'; 100_000]));
        let c = client_with(|c| c.max_body = 1000);
        assert_eq!(c.send(&Request::get(&s.url)).unwrap_err(), NetError::TooLarge(1000));
        // A server that lies about the length and streams more than the cap:
        let s2 = serve(|_, _| {
            let mut raw = b"HTTP/1.1 200 X\r\nConnection: close\r\n\r\n".to_vec();
            raw.extend(std::iter::repeat_n(b'y', 50_000));
            Reply::Raw(raw)
        });
        assert_eq!(c.send(&Request::get(&s2.url)).unwrap_err(), NetError::TooLarge(1000));
    }

    #[test]
    fn truncated_and_garbage_responses_are_errors_not_panics() {
        for raw in [
            &b"HTTP/1.1 200 X\r\nContent-Length: 100\r\nConnection: close\r\n\r\nshort"[..],
            b"garbage garbage\r\n\r\n",
            b"",
            b"HTTP/1.1 200 X\r\nContent-Length: nope\r\n\r\n",
        ] {
            let raw = raw.to_vec();
            let s = serve(move |_, _| Reply::Raw(raw.clone()));
            let c = client_with(|c| c.retries = 0);
            let r = c.send(&Request::get(&s.url));
            assert!(r.is_err(), "{r:?}");
        }
    }

    #[test]
    fn a_stalled_server_times_out() {
        let s = serve(|_, _| Reply::Hang);
        let c = client_with(|c| {
            c.timeout = Duration::from_millis(300);
            c.retries = 0;
        });
        let t = Instant::now();
        assert_eq!(c.send(&Request::get(&s.url)).unwrap_err(), NetError::Timeout);
        assert!(t.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn invalid_json_is_an_error() {
        let s = serve(|_, _| response(200, &[], b"\xff not json"));
        let r = client().send(&Request::get(&s.url)).unwrap();
        assert!(r.json::<serde_json::Value>().is_err());
        assert!(!r.text().is_empty());
    }

    #[test]
    fn conditional_requests_are_served_from_the_cache_on_304() {
        let dir = std::env::temp_dir().join(format!("fc-net-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = serve(|n, req| {
            if n == 0 {
                response(200, &[("ETag", "\"v1\""), ("Content-Type", "text/plain")], b"hello")
            } else if req.to_ascii_lowercase().contains("if-none-match: \"v1\"") {
                response(304, &[], b"")
            } else {
                response(500, &[], b"missing validator")
            }
        });
        let c = client_with(|c| c.cache_dir = Some(dir.clone()));
        let first = c.send(&Request::get(&s.url).cached()).unwrap();
        assert!(!first.from_cache);
        let second = c.send(&Request::get(&s.url).cached()).unwrap();
        assert!(second.from_cache);
        assert_eq!((second.status, second.text()), (200, "hello".to_string()));
        assert_eq!(c.state().entries(None)[1].outcome, Outcome::Cached);
        // The cache holds hashes, not URLs.
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            assert!(!e.file_name().to_string_lossy().contains("127"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn requests_without_cache_opt_in_are_never_stored() {
        let dir = std::env::temp_dir().join(format!("fc-net-nocache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = serve(|_, _| response(200, &[("ETag", "\"v1\"")], b"x"));
        let c = client_with(|c| c.cache_dir = Some(dir.clone()));
        c.send(&Request::get(&s.url).header("Authorization", "Bearer SECRET")).unwrap();
        assert!(!dir.exists() || std::fs::read_dir(&dir).unwrap().next().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_rate_limit_spaces_requests() {
        let s = serve(|_, _| response(200, &[], b"x"));
        let c = client_with(|c| c.min_host_interval = Duration::from_millis(150));
        let t = Instant::now();
        for _ in 0..3 {
            c.send(&Request::get(&s.url)).unwrap();
        }
        assert!(t.elapsed() >= Duration::from_millis(290), "{:?}", t.elapsed());
    }

    #[test]
    fn cancel_stops_before_sending() {
        let s = serve(|_, _| response(200, &[], b"x"));
        let flag = Arc::new(AtomicBool::new(true));
        let e = client().send(&Request::get(&s.url).cancel_with(flag)).unwrap_err();
        assert_eq!(e, NetError::Cancelled);
        assert_eq!(s.hits(), 0);
    }

    #[test]
    fn stream_delivers_lines_and_can_stop_early() {
        let body = b"data: one\n\ndata: two\r\n\ndata: [DONE]\n";
        let s = serve(move |_, _| response(200, &[("Content-Type", "text/event-stream")], body));
        let c = client();
        let mut seen = Vec::new();
        let end = c.stream_lines(&Request::get(&s.url), |l| {
            seen.push(l.to_string());
            true
        });
        assert_eq!(end.unwrap().status, 200);
        assert_eq!(seen, ["data: one", "", "data: two", "", "data: [DONE]"]);
        let mut first = Vec::new();
        c.stream_lines(&Request::get(&s.url), |l| {
            first.push(l.to_string());
            false
        })
        .unwrap();
        assert_eq!(first, ["data: one"]);
    }

    #[test]
    fn stream_reports_error_bodies_and_refuses_endless_lines() {
        let s = serve(|_, _| response(401, &[], br#"{"error":"bad key"}"#));
        let mut called = false;
        let end = client().stream_lines(&Request::get(&s.url), |_| {
            called = true;
            true
        });
        let end = end.unwrap();
        assert_eq!(end.status, 401);
        assert!(!called && String::from_utf8_lossy(&end.error_body).contains("bad key"));
        let s2 = serve(|_, _| response(200, &[], &vec![b'z'; (MAX_LINE as usize) + 10]));
        let e = client().stream_lines(&Request::get(&s2.url), |_| true).unwrap_err();
        assert_eq!(e, NetError::TooLarge(MAX_LINE));
    }

    #[test]
    fn debug_output_hides_headers_and_query() {
        let r = Request::get("https://h.test/x?key=SECRET").header("Authorization", "Bearer TOPSECRET");
        let d = format!("{r:?}");
        assert!(!d.contains("SECRET"), "{d}");
    }

    /// Needs the real internet, so it is skipped by default:
    /// `cargo test -p edith-net --features http -- --ignored real_https`.
    #[test]
    #[ignore = "uses the real network"]
    fn real_https_handshake_with_the_os_certificate_store() {
        let c = Client::new(ClientConfig::default(), Arc::new(NetState::new()));
        let r = c.send(&Request::get("https://example.com/")).unwrap();
        assert_eq!(r.status, 200);
        assert!(r.text().contains("Example Domain"));
        let bad = c.send(&Request::get("https://expired.badssl.com/"));
        assert!(bad.is_err(), "an expired certificate must be refused");
    }
}
