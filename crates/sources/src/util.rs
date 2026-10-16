//! Small helpers the adapters share: reading JSON defensively, building query strings, and
//! applying the parts of a request a site cannot filter itself.

use crate::types::{Candidate, Page, Request};
use serde_json::Value;

/// A string field, cut to `max` characters, with control characters removed.
pub fn text(v: &Value, key: &str, max: usize) -> String {
    v.get(key).and_then(Value::as_str).map(|s| s.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()).unwrap_or_default()
}

pub fn uint(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(|x| x.as_u64().or_else(|| x.as_f64().filter(|f| f.is_finite() && *f >= 0.0 && *f < 1e12).map(|f| f as u64)))
}

pub fn dim(v: &Value, key: &str) -> Option<u32> {
    uint(v, key).and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0)
}

pub fn seconds(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64).filter(|s| s.is_finite() && *s >= 0.0 && *s < 86_400.0)
}

/// A web address kept only if it is plain `https` (or `http`, which a few sites still return): it
/// is fetched and shown, so nothing else may pass.
pub fn url(v: &Value, key: &str) -> String {
    let s = text(v, key, 1000);
    let lower = s.to_ascii_lowercase();
    if (lower.starts_with("https://") || lower.starts_with("http://")) && !s.chars().any(char::is_whitespace) { s } else { String::new() }
}

/// `s` percent-encoded for a query string (unreserved characters stay).
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `base` with `params` as a query string.
pub fn with_query(base: &str, params: &[(&str, String)]) -> String {
    let q: Vec<String> = params.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect();
    format!("{base}?{}", q.join("&"))
}

/// The tags of a comma-separated string, at most 12.
pub fn tags(s: &str) -> Vec<String> {
    s.split(',').map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).take(12).collect()
}

/// Drops candidates that do not meet what the request asked for and the site could not filter:
/// the length range and the smallest picture. A candidate whose size or length is unknown is kept
/// (the person can judge it), never silently hidden.
pub fn apply_request(page: &mut Page, req: &Request) {
    page.candidates.retain(|c: &Candidate| {
        let long_enough = match (c.seconds, req.min_seconds) {
            (Some(s), Some(min)) => s >= min,
            _ => true,
        };
        let short_enough = match (c.seconds, req.max_seconds) {
            (Some(s), Some(max)) => s <= max,
            _ => true,
        };
        let wide = match (c.width, req.min_width) {
            (Some(w), Some(min)) => w >= min,
            _ => true,
        };
        let tall = match (c.height, req.min_height) {
            (Some(h), Some(min)) => h >= min,
            _ => true,
        };
        // a shape is judged only when both sides are known
        let shaped = match (req.orientation, c.width, c.height) {
            (Some(o), Some(w), Some(h)) => {
                let (w, h) = (i64::from(w), i64::from(h));
                match o {
                    crate::types::Orientation::Landscape => w > h,
                    crate::types::Orientation::Portrait => h > w,
                    crate::types::Orientation::Square => (w - h).abs() * 10 <= w.max(h),
                }
            }
            _ => true,
        };
        long_enough && short_enough && wide && tall && shaped
    });
}
