//! The HTTPS side of the sources (feature `net`): searching, fetching thumbnails, and downloading a
//! chosen file, all through `edith-net` (Offline mode, the redacted request log, timeouts, size caps,
//! per-site rate limits). A site's key is read from a [`Keys`] source on every request and never kept.

use crate::catalog::info;
use crate::types::{Candidate, Kind, Page, Request, SourceError};
use crate::{archive, freesound, nasa, openverse, pexels, pixabay, wikimedia};
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

/// The biggest picture shown to Edith, bytes.
const MAX_LOOK_BYTES: usize = 500_000;

/// A picture to look at: its kind and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

/// What kind of picture `b` is, from its first bytes; `None` for anything a model cannot be shown.
fn sniff_picture(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if b.starts_with(b"GIF8") {
        Some("image/gif")
    } else if b.len() >= 12 && b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}

/// `n` of `list`, evenly spread from the first to the last (the middle one when `n` is 1).
fn spread(list: &[String], n: usize) -> Vec<String> {
    if list.len() <= n {
        return list.to_vec();
    }
    (0..n)
        .filter_map(|i| {
            let at = if n == 1 { list.len() / 2 } else { i * (list.len() - 1) / (n - 1) };
            list.get(at).cloned()
        })
        .collect()
}

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
        self.get_up_to(source, url, headers, cancel, 4 * 1024 * 1024)
    }

    fn get_up_to(&self, source: &str, url: &str, headers: &[(&str, &str)], cancel: &Cancel, max: u64) -> Result<Vec<u8>, SourceError> {
        let mut req = NetRequest::get(url).cancel_with(cancel.clone());
        for (k, v) in headers {
            req = req.header(k, v);
        }
        req.max_bytes = Some(max);
        let resp = self.client.send(&req).map_err(|e| if e == NetError::Cancelled { SourceError::Cancelled } else { SourceError::Net(e) })?;
        if !resp.ok() {
            return Err(status_error(source, resp.status));
        }
        Ok(resp.body)
    }

    /// One source's answer to `req` for every kind it has that was asked for, as one page.
    pub fn search(&self, source: &str, req: &Request, cancel: &Cancel) -> Result<Page, SourceError> {
        let req = req.clone().sanitized();
        // the open sources (Wikimedia Commons, the Internet Archive) need no key
        let key = if info(source).is_some_and(|i| i.needs_key) { (self.keys)(source)? } else { Zeroizing::new(String::new()) };
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
            "wikimedia" => {
                for kind in [Kind::Image, Kind::Video, Kind::Audio].into_iter().filter(|k| req.wants(*k)) {
                    let Some(url) = wikimedia::search_url(&req, kind) else { continue };
                    let body = self.get(source, &self.at(url), &[], cancel)?;
                    let p = wikimedia::parse(&body)?;
                    page.total += p.total;
                    page.candidates.extend(p.candidates);
                }
            }
            "archive" => {
                if let Some(url) = archive::search_url(&req) {
                    let body = self.get(source, &self.at(url), &[], cancel)?;
                    let p = archive::parse(&body)?;
                    page.total += p.total;
                    page.candidates.extend(p.candidates);
                }
            }
            "openverse" => {
                for kind in [Kind::Image, Kind::Audio].into_iter().filter(|k| req.wants(*k)) {
                    let Some(url) = openverse::search_url(&req, kind) else { continue };
                    let body = self.get(source, &self.at(url), &[], cancel)?;
                    let p = openverse::parse(kind, &body)?;
                    page.total += p.total;
                    page.candidates.extend(p.candidates);
                }
            }
            "nasa" => {
                if let Some(url) = nasa::search_url(&req) {
                    let body = self.get(source, &self.at(url), &[], cancel)?;
                    let p = nasa::parse(&body)?;
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

    /// Up to `frames` small pictures of a result, to look at before it is offered: the stills the
    /// site gives from inside a video (spread through it), else the result's thumbnail. A picture
    /// that is not a JPEG, PNG, GIF or WebP, or is over 500 KB, is left out; a result that has no
    /// picture gives an empty list, and a picture that cannot be fetched is an error only when it
    /// was the one there was.
    pub fn look_pictures(&self, c: &Candidate, frames: usize, cancel: &Cancel) -> Result<Vec<Picture>, SourceError> {
        let frames = frames.clamp(1, 3);
        let mut urls = spread(&c.previews, frames);
        // NASA gives the stills of a film in the item's list of files, not in the search
        if urls.is_empty()
            && c.source == nasa::ID
            && c.kind == Kind::Video
            && let Some(manifest) = c.download.as_ref().map(|d| d.url.clone()).filter(|u| u.starts_with("https://images-api.nasa.gov/asset/"))
        {
            let body = self.get_up_to(&c.source, &self.at(manifest), &[], cancel, 1024 * 1024)?;
            urls = spread(&nasa::preview_urls(&body), frames).into_iter().map(|u| self.at(u)).collect();
        }
        if urls.is_empty() && c.thumb.starts_with("http") {
            urls.push(c.thumb.clone());
        }
        let mut out = Vec::new();
        let mut last_error = None;
        for url in urls {
            match self.thumbnail(&url, cancel) {
                Ok(bytes) if bytes.len() <= MAX_LOOK_BYTES => {
                    if let Some(mime) = sniff_picture(&bytes) {
                        out.push(Picture { mime, bytes });
                    }
                }
                Ok(_) => {}
                Err(SourceError::Cancelled) => return Err(SourceError::Cancelled),
                Err(e) => last_error = Some(e),
            }
        }
        match (out.is_empty(), last_error) {
            (true, Some(e)) => Err(e),
            _ => Ok(out),
        }
    }

    /// Fetches the chosen file to `path`. Returns what was written, with its SHA-256.
    pub fn download(&self, c: &Candidate, path: &Path, cancel: &Cancel, progress: impl FnMut(u64, Option<u64>)) -> Result<Downloaded, SourceError> {
        self.download_up_to(c, path, cancel, None, progress)
    }

    /// Like [`Self::download`] but never more than `cap` bytes (a sound fetched only to be listened
    /// to): a larger file is refused, and a source that can pick between copies picks a small one.
    pub fn download_up_to(
        &self,
        c: &Candidate,
        path: &Path,
        cancel: &Cancel,
        cap: Option<u64>,
        progress: impl FnMut(u64, Option<u64>),
    ) -> Result<Downloaded, SourceError> {
        let d = c.download.as_ref().ok_or_else(|| SourceError::Unavailable("this result has no file to download".to_string()))?;
        let max = MAX_BYTES.iter().find(|(k, _)| *k == c.kind).map_or(100 * 1024 * 1024, |(_, m)| *m);
        let max = cap.map_or(max, |c| c.min(max));
        // An Internet Archive or NASA result names the item, not a file: pick the best file now.
        let url = if d.resolve {
            let body = self.get_up_to(&c.source, &self.at(d.url.clone()), &[], cancel, 16 * 1024 * 1024)?;
            match c.source.as_str() {
                nasa::ID => self.at(nasa::pick_file(&body, c.kind)?),
                _ => {
                    let id = c.id.strip_prefix("archive:").unwrap_or("");
                    let (name, _) = archive::pick_file(&body, c.kind, max)?;
                    self.at(archive::file_url(id, &name).ok_or_else(|| SourceError::Unavailable("the Internet Archive item has an unusable name".to_string()))?)
                }
            }
        } else {
            d.url.clone()
        };
        let mut req = NetRequest::get(&url).cancel_with(cancel.clone());
        // the files of every source are fetched without the site's key
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

    #[test]
    fn the_open_sources_need_no_key_and_send_none() {
        let wiki = r#"{"query":{"pages":[{"pageid":5,"title":"File:Fog.jpg","index":1,"imageinfo":[{"url":"https://upload.wikimedia.org/a/Fog.jpg","thumburl":"https://upload.wikimedia.org/thumb/a/Fog.jpg/1920px-Fog.jpg","mediatype":"BITMAP","descriptionurl":"https://commons.wikimedia.org/wiki/File:Fog.jpg","extmetadata":{"LicenseShortName":{"value":"Public domain"}}}]}]}}"#;
        let archive = r#"{"response":{"numFound":1,"docs":[{"identifier":"fog_film","title":"Fog","licenseurl":"http://creativecommons.org/licenses/publicdomain/","runtime":"10:00","mediatype":"movies"}]}}"#;
        let s = serve(move |_, req| response(200, &[], if req.starts_with("GET /w/api.php") { wiki } else { archive }.as_bytes()));
        let no_key: Keys = Arc::new(|_| Err(SourceError::Auth("no key".into())));
        let sources = Sources::new(client(), no_key).with_origin(&s.url);
        let w = sources.search("wikimedia", &ask(&[Kind::Image]), &cancel()).unwrap();
        assert_eq!(w.candidates.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["wikimedia:5"]);
        let a = sources.search("archive", &ask(&[Kind::Video]), &cancel()).unwrap();
        assert_eq!(a.candidates.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["archive:fog_film"]);
        assert!(s.requests.lock().unwrap().iter().all(|r| !r.to_ascii_lowercase().contains("authorization")), "no key was sent");
        assert!(s.requests.lock().unwrap()[0].starts_with("GET /w/api.php?action=query"), "{}", s.requests.lock().unwrap()[0]);
    }

    #[test]
    fn an_archive_item_is_resolved_to_its_best_file_when_it_is_added() {
        let item = r#"{"files":[{"name":"Fog_512kb.mp4","format":"512Kb MPEG4","size":"5000"},{"name":"Fog.mp4","format":"h.264","size":"9000"},{"name":"Fog.ogv","format":"Ogg Video","size":"99999"}]}"#;
        let s = serve(move |_, req| {
            if req.starts_with("GET /metadata/fog_film") {
                response(200, &[], item.as_bytes())
            } else if req.starts_with("GET /download/fog_film/Fog.mp4") {
                response(200, &[], &[1u8; 9000])
            } else {
                response(404, &[], b"")
            }
        });
        let sources = Sources::new(client(), keys()).with_origin(&s.url);
        let doc = r#"{"response":{"numFound":1,"docs":[{"identifier":"fog_film","title":"Fog","licenseurl":"http://creativecommons.org/licenses/publicdomain/","mediatype":"movies"}]}}"#;
        let c = crate::archive::parse(doc.as_bytes()).unwrap().candidates.remove(0);
        let dir = std::env::temp_dir().join(format!("edith-archive-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let d = sources.download(&c, &dir.join("fog.mp4"), &cancel(), |_, _| {}).unwrap();
        assert_eq!(d.bytes, 9000, "the h.264 copy, not the small or the ogv");
        let seen: Vec<String> = s.requests.lock().unwrap().iter().map(|r| r.lines().next().unwrap_or("").to_string()).collect();
        assert!(seen[0].starts_with("GET /metadata/fog_film") && seen[1].starts_with("GET /download/fog_film/Fog.mp4"), "{seen:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Searches each site with the keys stored in the keychain (one request each, nothing downloaded).
    /// `cargo test -p edith-sources --features keychain -- --ignored real_sites`
    #[test]
    #[ignore = "uses the real network and the stored Pexels, Pixabay and Freesound keys"]
    fn real_sites_answer_a_search_in_the_shape_we_read() {
        let sources = Sources::new(Arc::new(Client::new(ClientConfig::default(), Arc::new(NetState::new()))), keychain_keys());
        for (id, kinds) in [
            ("pexels", vec![Kind::Video, Kind::Image]),
            ("pixabay", vec![Kind::Video, Kind::Image]),
            ("wikimedia", vec![Kind::Image, Kind::Video, Kind::Audio]),
            ("archive", vec![Kind::Video, Kind::Audio]),
            ("freesound", vec![Kind::Audio]),
        ] {
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

    /// The two sources that need no key, asked for real (no keychain involved): one search each, and
    /// one real download of a small file from each. `cargo test -p edith-sources --features net -- --ignored real_open_sites`
    #[test]
    #[ignore = "uses the real network (Wikimedia Commons, the Internet Archive, Openverse and NASA; no key)"]
    fn real_open_sites_answer_and_serve_a_file() {
        let no_key: Keys = Arc::new(|_| Err(SourceError::Auth("no key".into())));
        let sources = Sources::new(Arc::new(Client::new(ClientConfig::default(), Arc::new(NetState::new()))), no_key);
        let dir = std::env::temp_dir().join(format!("edith-real-open-{}", std::process::id()));
        for (id, kinds) in [
            ("wikimedia", vec![Kind::Image, Kind::Video, Kind::Audio]),
            ("archive", vec![Kind::Video, Kind::Audio]),
            ("openverse", vec![Kind::Image, Kind::Audio]),
            ("nasa", vec![Kind::Video, Kind::Image]),
        ] {
            let req = Request { text: "fog over a valley".into(), kinds, count: 5, ..Request::default() };
            let p = sources.search(id, &req, &cancel()).unwrap();
            eprintln!("{id}: {} results", p.candidates.len());
            for c in p.candidates.iter().take(6) {
                eprintln!(
                    "  {:?} | {} | {} | {} | {:?} | by {}",
                    c.kind,
                    c.id,
                    c.title,
                    c.rights.name,
                    (c.rights.commercial, c.rights.attribution_required),
                    c.author
                );
            }
            assert!(!p.candidates.is_empty(), "{id} returned nothing");
            // the smallest picture or sound of the lot, downloaded for real
            let pick = p.candidates.iter().filter(|c| c.kind != Kind::Video).find(|c| c.download.is_some());
            if let Some(c) = pick {
                let ext = c.download.as_ref().map(|d| d.ext.clone()).unwrap_or_default();
                match sources.download(c, &dir.join(format!("{id}.{ext}")), &cancel(), |_, _| {}) {
                    Ok(d) => eprintln!("  downloaded {} ({} bytes, sha256 {}…)", c.id, d.bytes, &d.sha256[..12]),
                    Err(e) => panic!("{id} download of {}: {e}", c.id),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn openverse_is_asked_once_per_kind_without_a_key_and_a_busy_answer_says_so() {
        let pics = r#"{"result_count":1,"results":[{"id":"aaaaaaaa-0000-0000-0000-000000000001","title":"Fog","url":"https://live.staticflickr.com/a.jpg","license":"by","license_version":"2.0","width":10,"height":10}]}"#;
        let sound = r#"{"result_count":1,"results":[{"id":"bbbbbbbb-0000-0000-0000-000000000002","title":"Rain","url":"https://cdn.freesound.org/r.mp3","license":"cc0","license_version":"1.0","filetype":"mp3","duration":5000}]}"#;
        let s = serve(move |_, req| response(200, &[], if req.starts_with("GET /v1/images/") { pics } else { sound }.as_bytes()));
        let no_key: Keys = Arc::new(|_| Err(SourceError::Auth("no key".into())));
        let sources = Sources::new(client(), no_key).with_origin(&s.url);
        let page = sources.search("openverse", &ask(&[Kind::Image, Kind::Audio, Kind::Video]), &cancel()).unwrap();
        assert_eq!(
            page.candidates.iter().map(|c| (c.kind, c.rights.name.as_str())).collect::<Vec<_>>(),
            [(Kind::Image, "CC BY 2.0"), (Kind::Audio, "CC0 1.0")]
        );
        let seen = s.requests.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "video is not asked of Openverse");
        assert!(seen[0].starts_with("GET /v1/images/?q=fog%20valley&license_type=commercial"), "{}", seen[0]);
        assert!(seen.iter().all(|r| !r.to_ascii_lowercase().contains("authorization")), "no key was sent");
        let busy = serve(|_, _| response(429, &[], b""));
        let e = Sources::new(client(), keys()).with_origin(&busy.url).search("openverse", &ask(&[Kind::Audio]), &cancel()).unwrap_err();
        assert!(matches!(e, SourceError::RateLimited(_)) && e.to_string().contains("openverse"), "{e}");
    }

    #[test]
    fn a_nasa_item_is_resolved_to_its_medium_copy_when_it_is_added() {
        let found = r#"{"collection":{"metadata":{"total_hits":1},"items":[{"data":[{"center":"AFRC","media_type":"video","nasa_id":"Moon Film","title":"Moon"}],"links":[]}]}}"#;
        let manifest = r#"{"collection":{"items":[{"href":"http://images-assets.nasa.gov/video/Moon Film/Moon Film~orig.mp4"},{"href":"http://images-assets.nasa.gov/video/Moon Film/Moon Film~medium.mp4"}]}}"#;
        let s = serve(move |_, req| {
            if req.starts_with("GET /search?q=moon") {
                response(200, &[], found.as_bytes())
            } else if req.starts_with("GET /asset/Moon%20Film ") {
                response(200, &[], manifest.as_bytes())
            } else if req.starts_with("GET /video/Moon%20Film/Moon%20Film~medium.mp4 ") {
                response(200, &[], &[3u8; 7000])
            } else {
                response(404, &[], b"")
            }
        });
        let no_key: Keys = Arc::new(|_| Err(SourceError::Auth("no key".into())));
        let sources = Sources::new(client(), no_key).with_origin(&s.url);
        let req = Request { text: "moon".into(), kinds: vec![Kind::Video, Kind::Image], ..Request::default() };
        let c = sources.search("nasa", &req, &cancel()).unwrap().candidates.remove(0);
        assert_eq!((c.id.as_str(), c.rights.commercial), ("nasa:Moon Film", Some(true)));
        let dir = std::env::temp_dir().join(format!("edith-nasa-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let d = sources.download(&c, &dir.join("moon.mp4"), &cancel(), |_, _| {}).unwrap();
        assert_eq!(d.bytes, 7000, "the medium copy, not the original");
        // an item whose renditions cannot be read is refused, not guessed at
        let bare =
            serve(|_, req| response(200, &[], if req.starts_with("GET /asset/") { br#"{"collection":{"items":[]}}"#.as_slice() } else { b"x".as_slice() }));
        let e = Sources::new(client(), keys()).with_origin(&bare.url).download(&c, &dir.join("no.mp4"), &cancel(), |_, _| {}).unwrap_err();
        assert!(matches!(e, SourceError::Unavailable(_)), "{e:?}");
        assert!(!dir.join("no.mp4").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn jpeg() -> Vec<u8> {
        let mut b = vec![0xFF, 0xD8, 0xFF, 0xE0];
        b.extend_from_slice(&[7u8; 300]);
        b
    }

    #[test]
    fn a_result_is_shown_as_its_own_stills_spread_through_it_or_its_thumbnail_and_only_real_pictures_pass() {
        let s = serve(move |_, req| {
            if req.starts_with("GET /still") {
                response(200, &[], &jpeg())
            } else if req.starts_with("GET /text") {
                response(200, &[], b"<html>not a picture</html>")
            } else if req.starts_with("GET /big") {
                let mut b = jpeg();
                b.resize(600_000, 1);
                response(200, &[], &b)
            } else {
                response(404, &[], b"")
            }
        });
        let sources = Sources::new(client(), keys());
        let mut c = crate::pexels::parse(Kind::Video, PEXELS_VIDEOS.as_bytes()).unwrap().candidates.remove(0);
        c.thumb = format!("{}/still-thumb.jpg", s.url);
        // five stills in the video: three are asked for, the first, the middle and the last
        c.previews = (1..=5).map(|i| format!("{}/still{i}.jpg", s.url)).collect();
        let got = sources.look_pictures(&c, 3, &cancel()).unwrap();
        assert_eq!(got.len(), 3);
        assert!(got.iter().all(|p| p.mime == "image/jpeg"));
        let seen: Vec<String> = s.requests.lock().unwrap().iter().map(|r| r.lines().next().unwrap_or("").to_string()).collect();
        assert!(seen[0].starts_with("GET /still1.jpg") && seen[1].starts_with("GET /still3.jpg") && seen[2].starts_with("GET /still5.jpg"), "{seen:?}");
        // no stills: the thumbnail
        c.previews.clear();
        assert_eq!(sources.look_pictures(&c, 2, &cancel()).unwrap().len(), 1);
        // something that is not a picture, or is too big to be one worth sending, is left out
        c.thumb = format!("{}/text", s.url);
        assert!(sources.look_pictures(&c, 1, &cancel()).unwrap().is_empty());
        c.thumb = format!("{}/big.jpg", s.url);
        assert!(sources.look_pictures(&c, 1, &cancel()).unwrap().is_empty());
        // a picture that cannot be had is an error when it was the only one, and nothing when there is no picture at all
        c.thumb = format!("{}/missing", s.url);
        assert!(sources.look_pictures(&c, 1, &cancel()).is_err());
        c.thumb = String::new();
        assert!(sources.look_pictures(&c, 1, &cancel()).unwrap().is_empty());
        assert_eq!(spread(&["a".into(), "b".into(), "c".into(), "d".into()], 1), ["c"]);
        assert_eq!(sniff_picture(b"RIFF\x00\x00\x00\x00WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_picture(b"GIF89a"), Some("image/gif"));
    }

    #[test]
    fn a_nasa_films_stills_come_from_its_list_of_files() {
        let manifest = r#"{"collection":{"items":[{"href":"http://images-assets.nasa.gov/video/M/M~small_1.jpg"},{"href":"http://images-assets.nasa.gov/video/M/M~small_2.jpg"},{"href":"http://images-assets.nasa.gov/video/M/M~medium.mp4"}]}}"#;
        let s = serve(move |_, req| {
            if req.starts_with("GET /asset/") {
                response(200, &[], manifest.as_bytes())
            } else if req.starts_with("GET /video/M/M~small_") {
                response(200, &[], &jpeg())
            } else {
                response(404, &[], b"")
            }
        });
        let found = r#"{"collection":{"metadata":{"total_hits":1},"items":[{"data":[{"media_type":"video","nasa_id":"M","title":"Moon"}],"links":[]}]}}"#;
        let fake = serve(move |_, _| response(200, &[], found.as_bytes()));
        let no_key: Keys = Arc::new(|_| Err(SourceError::Auth("no key".into())));
        let c = Sources::new(client(), no_key.clone())
            .with_origin(&fake.url)
            .search("nasa", &Request { text: "moon".into(), ..Request::default() }, &cancel())
            .unwrap()
            .candidates
            .remove(0);
        let got = Sources::new(client(), no_key).with_origin(&s.url).look_pictures(&c, 2, &cancel()).unwrap();
        assert_eq!(got.len(), 2, "two stills from inside the film, not its poster");
    }

    #[test]
    fn a_sound_fetched_to_be_listened_to_is_capped() {
        let s = serve(|_, _| response(200, &[], &[5u8; 9000]));
        let sources = Sources::new(client(), keys());
        let doc = r#"{"result_count":1,"results":[{"id":"aaaaaaaa-0000-0000-0000-000000000001","title":"Rain","url":"FILE","license":"cc0","license_version":"1.0","filetype":"mp3"}]}"#;
        let c = crate::openverse::parse(Kind::Audio, doc.replace("FILE", &format!("{}/rain.mp3", s.url)).as_bytes()).unwrap().candidates.remove(0);
        let dir = std::env::temp_dir().join(format!("edith-listen-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(sources.download_up_to(&c, &dir.join("a.mp3"), &cancel(), Some(10_000), |_, _| {}).unwrap().bytes, 9000);
        assert!(sources.download_up_to(&c, &dir.join("b.mp3"), &cancel(), Some(5_000), |_, _| {}).is_err(), "over the cap is refused");
        assert!(!dir.join("b.mp3").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebasing_keeps_the_path_and_query() {
        assert_eq!(rebase("https://api.pexels.com/v1/search?query=a%20b", "http://127.0.0.1:5"), "http://127.0.0.1:5/v1/search?query=a%20b");
        assert_eq!(rebase("nonsense", "http://x"), "nonsense");
    }
}
