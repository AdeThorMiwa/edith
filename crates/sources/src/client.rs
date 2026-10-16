//! The HTTPS side of the sources (feature `net`): searching, fetching thumbnails, and downloading a
//! chosen file, all through `edith-net` (Offline mode, the redacted request log, timeouts, size caps,
//! per-site rate limits). A site's key is read from a [`Keys`] source on every request and never kept.

use crate::types::{Candidate, Kind, Page, Request, SourceError};
use crate::{freesound, pexels, pixabay};
use edith_net::NetError;
use edith_net::http::{Client, Downloaded, Request as NetRequest};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use zeroize::Zeroizing;

pub type Cancel = Arc<AtomicBool>;

/// Where each site's key comes from.
pub type Keys = Arc<dyn Fn(&str) -> Result<Zeroizing<String>, SourceError> + Send + Sync>;

/// A key read from the OS keychain for the site's service (`pexels` and so on).
#[cfg(feature = "keychain")]
pub fn keychain_keys() -> Keys {
    Arc::new(|source: &str| {
        let name =
            edith_net::services::find(source).and_then(|s| s.secret).ok_or_else(|| SourceError::Unavailable(format!("{source} is not a known service")))?;
        let store = edith_net::secrets::os()?;
        store.get_cached(name)?.ok_or_else(|| SourceError::Auth(format!("no {source} key is stored; add one in Connections")))
    })
}

const MAX_BYTES: [(Kind, u64); 3] = [(Kind::Video, 700 * 1024 * 1024), (Kind::Image, 80 * 1024 * 1024), (Kind::Audio, 120 * 1024 * 1024)];

pub struct Sources {
    client: Arc<Client>,
    keys: Keys,
    /// Replaces the scheme and host of search addresses (tests point it at a local server).
    origin: Option<String>,
}

fn status_error(source: &str, status: u16) -> SourceError {
    match status {
        401 | 403 => SourceError::Auth(format!("{source} refused the key; check it in Connections")),
        429 => SourceError::RateLimited(source.to_string()),
        s => SourceError::Unavailable(format!("{source} answered {s}")),
    }
}

/// `url` with its scheme and host replaced by `origin` (`http://127.0.0.1:9`).
fn rebase(url: &str, origin: &str) -> String {
    match url.split_once("://").and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..])) {
        Some(path) => format!("{}{path}", origin.trim_end_matches('/')),
        None => url.to_string(),
    }
}

impl Sources {
    pub fn new(client: Arc<Client>, keys: Keys) -> Self {
        Self { client, keys, origin: None }
    }

    /// For tests: send every search to `origin` instead of the real sites.
    pub fn with_origin(mut self, origin: &str) -> Self {
        self.origin = Some(origin.to_string());
        self
    }

    fn at(&self, url: String) -> String {
        match &self.origin {
            Some(o) => rebase(&url, o),
            None => url,
        }
    }

    fn get(&self, source: &str, url: &str, headers: &[(&str, &str)], cancel: &Cancel) -> Result<Vec<u8>, SourceError> {
        let mut req = NetRequest::get(url).cancel_with(cancel.clone());
        for (k, v) in headers {
            req = req.header(k, v);
        }
        req.max_bytes = Some(4 * 1024 * 1024);
        let resp = self.client.send(&req).map_err(|e| if e == NetError::Cancelled { SourceError::Cancelled } else { SourceError::Net(e) })?;
        if !resp.ok() {
            return Err(status_error(source, resp.status));
        }
        Ok(resp.body)
    }

    /// One source's answer to `req` for every kind it has that was asked for, as one page.
    pub fn search(&self, source: &str, req: &Request, cancel: &Cancel) -> Result<Page, SourceError> {
        let req = req.clone().sanitized();
        let key = (self.keys)(source)?;
        let mut page = Page::default();
        match source {
            "pexels" => {
                for kind in [Kind::Video, Kind::Image].into_iter().filter(|k| req.wants(*k)) {
                    let Some(url) = pexels::search_url(&req, kind) else { continue };
                    let body = self.get(source, &self.at(url), &[("Authorization", &key)], cancel)?;
                    let p = pexels::parse(kind, &body)?;
                    page.total += p.total;
                    page.candidates.extend(p.candidates);
                }
            }
            "pixabay" => {
                for kind in [Kind::Video, Kind::Image].into_iter().filter(|k| req.wants(*k)) {
                    let Some(url) = pixabay::search_url(&req, kind, &key) else { continue };
                    let body = self.get(source, &self.at(url), &[], cancel).map_err(|e| scrub(e, &key))?;
                    let p = pixabay::parse(kind, &body)?;
                    page.total += p.total;
                    page.candidates.extend(p.candidates);
                }
            }
            "freesound" => {
                if let Some(url) = freesound::search_url(&req) {
                    let auth = format!("Token {}", key.as_str());
                    let body = self.get(source, &self.at(url), &[("Authorization", &auth)], cancel)?;
                    let p = freesound::parse(&body)?;
                    page.total += p.total;
                    page.candidates.extend(p.candidates);
                }
            }
            other => return Err(SourceError::Unavailable(format!("`{}` is not a media source", other.chars().take(30).collect::<String>()))),
        }
        crate::apply_request(&mut page, &req);
        Ok(page)
    }

    /// The bytes of a small picture to show in the grid (at most 1 MB).
    pub fn thumbnail(&self, url: &str, cancel: &Cancel) -> Result<Vec<u8>, SourceError> {
        let mut req = NetRequest::get(url).cancel_with(cancel.clone());
        req.max_bytes = Some(1024 * 1024);
        let resp = self.client.send(&req).map_err(|e| if e == NetError::Cancelled { SourceError::Cancelled } else { SourceError::Net(e) })?;
        if !resp.ok() {
            return Err(SourceError::Unavailable(format!("the picture could not be fetched ({})", resp.status)));
        }
        Ok(resp.body)
    }

    /// Fetches the chosen file to `path`. Returns what was written, with its SHA-256.
    pub fn download(&self, c: &Candidate, path: &Path, cancel: &Cancel, progress: impl FnMut(u64, Option<u64>)) -> Result<Downloaded, SourceError> {
        let d = c.download.as_ref().ok_or_else(|| SourceError::Unavailable("this result has no file to download".to_string()))?;
        let max = MAX_BYTES.iter().find(|(k, _)| *k == c.kind).map_or(100 * 1024 * 1024, |(_, m)| *m);
        let mut req = NetRequest::get(&d.url).cancel_with(cancel.clone());
        // Freesound's preview files are on its own file server and need no key
        req.timeout = Some(std::time::Duration::from_secs(1800));
        let done =
            self.client.download(&req, path, max, progress).map_err(|e| if e == NetError::Cancelled { SourceError::Cancelled } else { SourceError::Net(e) })?;
        if done.status >= 400 {
            return Err(status_error(&c.source, done.status));
        }
        Ok(done)
    }
}

/// An error with the key removed from any text (a key in a query string must never reach a message).
fn scrub(e: SourceError, key: &str) -> SourceError {
    if key.len() < 8 {
        return e;
    }
    let hide = |s: String| s.replace(key, "[hidden]");
    match e {
        SourceError::Auth(m) => SourceError::Auth(hide(m)),
        SourceError::BadReply(m) => SourceError::BadReply(hide(m)),
        SourceError::Unavailable(m) => SourceError::Unavailable(hide(m)),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edith_net::NetState;
    use edith_net::http::ClientConfig;
    use edith_net::testserver::{Reply, response, serve};
    use std::time::Duration;

    const KEY: &str = "TOPSECRETKEY123";

    fn client() -> Arc<Client> {
        Arc::new(Client::new(
            ClientConfig { allow_loopback_http: true, min_host_interval: Duration::ZERO, timeout: Duration::from_secs(5), ..ClientConfig::default() },
            Arc::new(NetState::new()),
        ))
    }

    fn keys() -> Keys {
        Arc::new(|_| Ok(Zeroizing::new(KEY.to_string())))
    }

    fn cancel() -> Cancel {
        Arc::new(AtomicBool::new(false))
    }

    fn ask(kinds: &[Kind]) -> Request {
        Request { text: "fog over a valley".into(), kinds: kinds.to_vec(), ..Request::default() }
    }

    const PEXELS_VIDEOS: &str = r#"{"total_results":1,"videos":[{"id":1,"width":1920,"height":1080,"url":"https://www.pexels.com/video/fog-1/","image":"https://images.pexels.com/i.jpg","duration":9,"user":{"name":"J","url":"https://www.pexels.com/@j"},"video_files":[{"file_type":"video/mp4","width":1920,"height":1080,"link":"https://videos.pexels.com/x.mp4"}]}]}"#;
    const PEXELS_PHOTOS: &str = r#"{"total_results":1,"photos":[{"id":2,"width":100,"height":100,"url":"https://www.pexels.com/photo/2/","photographer":"P","src":{"large2x":"https://images.pexels.com/l.jpg","medium":"https://images.pexels.com/m.jpg"}}]}"#;

    #[test]
    fn pexels_is_asked_once_per_kind_with_the_key_in_the_header_and_the_pages_are_merged() {
        let s = serve(|_, req| {
            response(200, &[("content-type", "application/json")], if req.contains("/videos/search") { PEXELS_VIDEOS } else { PEXELS_PHOTOS }.as_bytes())
        });
        let sources = Sources::new(client(), keys()).with_origin(&s.url);
        let page = sources.search("pexels", &ask(&[Kind::Video, Kind::Image]), &cancel()).unwrap();
        assert_eq!(page.candidates.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["pexels:1", "pexels:photo-2"]);
        assert_eq!(page.total, 2);
        let seen = s.requests.lock().unwrap().clone();
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|r| r.to_ascii_lowercase().contains(&format!("authorization: {}", KEY.to_ascii_lowercase()))), "{seen:?}");
        assert!(seen[0].starts_with("GET /v1/videos/search?query=fog%20valley"), "{}", seen[0]);
    }

    #[test]
    fn pixabay_gets_the_key_in_the_address_which_the_log_and_errors_never_show() {
        let s = serve(|_, _| response(401, &[], format!("bad key {KEY}").as_bytes()));
        let c = client();
        let e = Sources::new(c.clone(), keys()).with_origin(&s.url).search("pixabay", &ask(&[Kind::Image]), &cancel()).unwrap_err();
        assert!(matches!(e, SourceError::Auth(_)), "{e:?}");
        let everything = format!("{e} {e:?} {:?}", c.state().entries(None));
        assert!(!everything.contains(KEY), "{everything}");
        assert!(s.requests.lock().unwrap()[0].contains(&format!("key={KEY}")), "the key did reach the site");
    }

    #[test]
    fn freesound_uses_the_token_header_and_a_length_filter_is_applied_afterwards() {
        let body = r#"{"count":2,"results":[
          {"id":1,"name":"short","username":"a","license":"http://creativecommons.org/publicdomain/zero/1.0/","duration":1,"previews":{"preview-hq-mp3":"https://cdn.freesound.org/a.mp3"}},
          {"id":2,"name":"right","username":"b","license":"http://creativecommons.org/licenses/by/4.0/","duration":10,"previews":{"preview-hq-mp3":"https://cdn.freesound.org/b.mp3"}}]}"#;
        let s = serve(move |_, _| response(200, &[], body.as_bytes()));
        let req = Request { min_seconds: Some(5.0), ..ask(&[Kind::Audio]) };
        let page = Sources::new(client(), keys()).with_origin(&s.url).search("freesound", &req, &cancel()).unwrap();
        assert_eq!(page.candidates.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["right"], "a sound shorter than asked is dropped");
        assert!(s.requests.lock().unwrap()[0].to_ascii_lowercase().contains(&format!("authorization: token {}", KEY.to_ascii_lowercase())));
        // a request that wants no audio asks nothing
        let none = Sources::new(client(), keys()).with_origin(&s.url).search("freesound", &ask(&[Kind::Video]), &cancel()).unwrap();
        assert!(none.candidates.is_empty());
        assert_eq!(s.hits(), 1);
    }

    #[test]
    fn errors_say_what_happened_and_nothing_is_asked_without_a_key_or_when_offline_or_stopped() {
        let busy = serve(|_, _| response(429, &[], b""));
        let broken = serve(|_, _| response(500, &[], b""));
        assert!(matches!(
            Sources::new(client(), keys()).with_origin(&busy.url).search("pexels", &ask(&[Kind::Video]), &cancel()),
            Err(SourceError::RateLimited(_))
        ));
        assert!(matches!(
            Sources::new(client(), keys()).with_origin(&broken.url).search("pexels", &ask(&[Kind::Video]), &cancel()),
            Err(SourceError::Unavailable(_))
        ));
        let s = broken;
        let sources = Sources::new(client(), keys()).with_origin(&s.url);
        let none: Keys = Arc::new(|_| Err(SourceError::Auth("no pexels key is stored; add one in Connections".into())));
        let hits = s.hits();
        assert!(matches!(Sources::new(client(), none).with_origin(&s.url).search("pexels", &ask(&[Kind::Video]), &cancel()), Err(SourceError::Auth(_))));
        assert_eq!(s.hits(), hits, "no key, no request");
        let off = client();
        off.state().set_offline(true);
        assert_eq!(
            Sources::new(off, keys()).with_origin(&s.url).search("pexels", &ask(&[Kind::Video]), &cancel()).unwrap_err(),
            SourceError::Net(NetError::Offline)
        );
        let stop = Arc::new(AtomicBool::new(true));
        assert_eq!(sources.search("pexels", &ask(&[Kind::Video]), &stop).unwrap_err(), SourceError::Cancelled);
        assert!(matches!(sources.search("nowhere", &ask(&[Kind::Video]), &cancel()), Err(SourceError::Unavailable(_))));
        let hang = serve(|_, _| Reply::Hang);
        let slow = Arc::new(Client::new(
            ClientConfig { allow_loopback_http: true, timeout: Duration::from_millis(300), ..ClientConfig::default() },
            Arc::new(NetState::new()),
        ));
        let started = std::time::Instant::now();
        assert!(Sources::new(slow, keys()).with_origin(&hang.url).search("pexels", &ask(&[Kind::Video]), &cancel()).is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_chosen_file_is_downloaded_and_hashed_and_a_refusal_is_reported() {
        let s = serve(|_, req| if req.contains("/gone") { response(404, &[], b"") } else { response(200, &[], &[9u8; 4000]) });
        let dir = std::env::temp_dir().join(format!("edith-src-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sources = Sources::new(client(), keys());
        let mut c = crate::pexels::parse(Kind::Video, PEXELS_VIDEOS.as_bytes()).unwrap().candidates.remove(0);
        c.download.as_mut().unwrap().url = format!("{}/clip.mp4", s.url);
        let d = sources.download(&c, &dir.join("pexels/clip.mp4"), &cancel(), |_, _| {}).unwrap();
        assert_eq!((d.bytes, d.sha256.len()), (4000, 64));
        assert!(dir.join("pexels/clip.mp4").exists());
        c.download.as_mut().unwrap().url = format!("{}/gone", s.url);
        assert!(matches!(sources.download(&c, &dir.join("x.mp4"), &cancel(), |_, _| {}), Err(SourceError::Unavailable(_))));
        assert!(!dir.join("x.mp4").exists());
        c.download = None;
        assert!(sources.download(&c, &dir.join("y.mp4"), &cancel(), |_, _| {}).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Searches each site with the keys stored in the keychain (one request each, nothing downloaded).
    /// `cargo test -p edith-sources --features keychain -- --ignored real_sites`
    #[test]
    #[ignore = "uses the real network and the stored Pexels, Pixabay and Freesound keys"]
    fn real_sites_answer_a_search_in_the_shape_we_read() {
        let sources = Sources::new(Arc::new(Client::new(ClientConfig::default(), Arc::new(NetState::new()))), keychain_keys());
        for (id, kinds) in [("pexels", vec![Kind::Video, Kind::Image]), ("pixabay", vec![Kind::Video, Kind::Image]), ("freesound", vec![Kind::Audio])] {
            let req = Request { text: "fog over a valley".into(), kinds: kinds.clone(), count: 5, ..Request::default() };
            match sources.search(id, &req, &cancel()) {
                Ok(p) => {
                    eprintln!("{id}: {} of {} results", p.candidates.len(), p.total);
                    for c in p.candidates.iter().take(2) {
                        eprintln!("  {} | {} | {} | {} | {:?}", c.id, c.title, c.rights.name, c.author, c.download.as_ref().map(|d| (&d.ext, d.width)));
                    }
                    assert!(!p.candidates.is_empty(), "{id} returned nothing");
                    assert!(p.candidates.iter().all(|c| c.download.is_some() && !c.rights.name.is_empty()));
                }
                Err(SourceError::Auth(m)) => eprintln!("{id}: skipped ({m})"),
                Err(e) => panic!("{id}: {e}"),
            }
        }
    }

    #[test]
    fn rebasing_keeps_the_path_and_query() {
        assert_eq!(rebase("https://api.pexels.com/v1/search?query=a%20b", "http://127.0.0.1:5"), "http://127.0.0.1:5/v1/search?query=a%20b");
        assert_eq!(rebase("nonsense", "http://x"), "nonsense");
    }
}
