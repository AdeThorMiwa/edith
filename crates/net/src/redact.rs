//! Removing credentials from anything that is shown or logged.

/// The scheme, host, port and path of `url`: no userinfo, query string or fragment (API keys and
/// OAuth codes travel in those). Returns `"(invalid address)"` for text that is not an
/// `http(s)` URL, so a pasted secret can never be echoed back.
pub fn url(url: &str) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) if s.eq_ignore_ascii_case("http") || s.eq_ignore_ascii_case("https") => (s.to_ascii_lowercase(), r),
        _ => return "(invalid address)".to_string(),
    };
    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    let rest = rest.get(..end).unwrap_or(rest);
    let (authority, path) = match rest.find('/') {
        Some(i) => (rest.get(..i).unwrap_or(rest), rest.get(i..).unwrap_or("")),
        None => (rest, ""),
    };
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if host.is_empty() || host.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return "(invalid address)".to_string();
    }
    let path: String = path.chars().take(200).collect();
    format!("{scheme}://{host}{path}")
}

/// The host (and port) of `url`, lower-cased, or `None` when it is not an `http(s)` URL.
pub fn host(url: &str) -> Option<String> {
    let r = self::url(url);
    let rest = r.split_once("://")?.1;
    let end = rest.find('/').unwrap_or(rest.len());
    Some(rest.get(..end)?.to_ascii_lowercase())
}

/// Words shaped like the API keys providers issue (`sk-ant-…`, `sk-or-…`, `sk-proj-…`, Google's
/// `AIza…`): a prefix and a long run of key characters. A heuristic, in addition to hiding the
/// exact key where the caller knows it.
fn looks_like_a_key(word: &str) -> bool {
    let w = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
    let long_enough = w.len() >= 16 && w.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    long_enough && (w.starts_with("sk-") || w.starts_with("AIza") || w.starts_with("pplx-") || w.starts_with("gsk_"))
}

/// `message` with query strings, `Bearer` tokens and `key=value` credentials blanked out and
/// control characters replaced, cut to 300 characters. Use for error text from libraries, which
/// may quote the request URL.
pub fn message(message: &str) -> String {
    const SECRET_KEYS: [&str; 9] = ["key", "api_key", "apikey", "token", "access_token", "refresh_token", "code", "secret", "password"];
    let mut out = Vec::new();
    let mut hide_next = false;
    for word in message.split_whitespace() {
        let mut w = word.to_string();
        if hide_next {
            out.push("[hidden]".to_string());
            hide_next = false;
            continue;
        }
        if word.eq_ignore_ascii_case("bearer") || word.eq_ignore_ascii_case("basic") {
            hide_next = true;
        }
        if looks_like_a_key(word) {
            out.push("[hidden]".to_string());
            continue;
        }
        if let Some(i) = w.find('?') {
            w.truncate(i);
            w.push_str("?[hidden]");
        } else if let Some((k, _)) = w.split_once('=') {
            let k = k.trim_start_matches(|c: char| !c.is_alphanumeric() && c != '_').to_ascii_lowercase();
            if SECRET_KEYS.contains(&k.as_str()) {
                w = format!("{k}=[hidden]");
            }
        }
        out.push(w);
    }
    let joined: String = out.join(" ").chars().map(|c| if c.is_control() { ' ' } else { c }).take(300).collect();
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_drops_query_userinfo_and_fragment() {
        assert_eq!(url("https://user:pw@api.example.com:8443/v1/chat?key=SECRET#frag"), "https://api.example.com:8443/v1/chat");
        assert_eq!(url("HTTP://Example.com"), "http://Example.com");
        assert_eq!(url("https://example.com/a/b"), "https://example.com/a/b");
    }

    #[test]
    fn url_rejects_other_text() {
        for bad in ["", "sk-ant-api03-SECRET", "ftp://host/x", "file:///etc/passwd", "https://", "https://a b/", "https://\u{7}x/"] {
            assert_eq!(url(bad), "(invalid address)", "{bad:?}");
        }
    }

    #[test]
    fn url_caps_the_path() {
        let long = format!("https://h/{}", "a".repeat(5000));
        assert!(url(&long).len() < 260);
    }

    #[test]
    fn host_is_lowercase_and_keeps_the_port() {
        assert_eq!(host("https://API.Example.com:8443/x?y=1").as_deref(), Some("api.example.com:8443"));
        assert_eq!(host("not a url"), None);
    }

    #[test]
    fn message_hides_credentials() {
        let m = message("GET https://x.test/v1?key=SECRET failed with Authorization: Bearer sk-12345 and token=abc code=zzz ok=1");
        for leak in ["SECRET", "sk-12345", "abc", "zzz"] {
            assert!(!m.contains(leak), "{m}");
        }
        assert!(m.contains("ok=1"));
    }

    #[test]
    fn message_hides_words_shaped_like_keys() {
        let m = message("invalid x-api-key sk-ant-api03-ABCDEFGHIJKLMNOP, tried (sk-or-v1-0123456789abcdef) and AIzaSyA1234567890abcdefgh.");
        for leak in ["ABCDEFGHIJKLMNOP", "0123456789abcdef", "AIzaSy"] {
            assert!(!m.contains(leak), "{m}");
        }
        // ordinary words and short tokens survive
        assert!(message("the sk-1 token and sky-high prices").contains("sky-high"));
    }

    #[test]
    fn message_is_bounded_and_single_line() {
        let m = message(&format!("a\nb\u{0}{}", "x".repeat(2000)));
        assert!(m.chars().count() <= 300);
        assert!(!m.chars().any(char::is_control));
    }
}
