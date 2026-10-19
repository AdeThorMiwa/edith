//! The Internet Archive (`https://archive.org`): public-domain and Creative Commons films, sound
//! and pictures. No key. Only items that **state a licence** are searched (`licenseurl:*`): the
//! Archive holds a great deal that is not free to reuse, and an item with no stated licence is not
//! offered. The licence address is read into what it allows. A result is a whole item; the best
//! file is chosen when the person adds it (`Download::resolve`), by [`pick_file`].

use crate::types::{Candidate, Download, Kind, Page, Request, SourceError};
use crate::util::{encode, text, uint, with_query};
use serde_json::Value;

pub const ID: &str = "archive";

/// The address of a search over the kinds asked for.
pub fn search_url(req: &Request) -> Option<String> {
    let query = crate::query::keywords(&req.text, 8, 100);
    if query.is_empty() {
        return None;
    }
    let types: Vec<&str> = req
        .kinds
        .iter()
        .map(|k| match k {
            Kind::Video => "movies",
            Kind::Image => "image",
            Kind::Audio => "audio",
        })
        .collect();
    let words: Vec<&str> = query.split(' ').collect();
    let q = format!("({}) AND mediatype:({}) AND licenseurl:*", words.join(" AND "), types.join(" OR "));
    let mut p = vec![("q", q)];
    for f in ["identifier", "title", "creator", "licenseurl", "runtime", "mediatype"] {
        p.push(("fl[]", f.to_string()));
    }
    p.push(("sort[]", "downloads desc".into()));
    p.push(("rows", req.count.clamp(1, 40).to_string()));
    p.push(("page", req.page.to_string()));
    p.push(("output", "json".into()));
    Some(with_query("https://archive.org/advancedsearch.php", &p))
}

/// Seconds from `1:15:54`, `21:28`, `370` or `370.3`.
fn runtime(s: &str) -> Option<f64> {
    let mut total = 0.0f64;
    for part in s.trim().split(':') {
        total = total * 60.0 + part.trim().parse::<f64>().ok().filter(|x| x.is_finite() && *x >= 0.0)?;
    }
    (total < 86_400.0).then_some(total)
}

/// An Archive item name is letters, digits, `_`, `-` and `.`; anything else is not one.
pub fn valid_identifier(id: &str) -> bool {
    !id.is_empty() && id.len() <= 100 && !id.starts_with('.') && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// A field that is a string, or a list of strings (the Archive uses both): the first.
fn first(v: &Value, key: &str, max: usize) -> String {
    match v.get(key) {
        Some(Value::Array(a)) => a.first().map(|x| text(&Value::Object([("v".to_string(), x.clone())].into_iter().collect()), "v", max)).unwrap_or_default(),
        _ => text(v, key, max),
    }
}

fn one(v: &Value) -> Option<Candidate> {
    let id = first(v, "identifier", 120);
    if !valid_identifier(&id) {
        return None;
    }
    let kind = match first(v, "mediatype", 20).as_str() {
        "movies" => Kind::Video,
        "audio" => Kind::Audio,
        "image" => Kind::Image,
        _ => return None,
    };
    let licence = first(v, "licenseurl", 300);
    let ext = match kind {
        Kind::Video => "mp4",
        Kind::Audio => "mp3",
        Kind::Image => "jpg",
    };
    Some(Candidate {
        id: format!("{ID}:{id}"),
        source: ID.into(),
        kind,
        title: first(v, "title", 200),
        author: first(v, "creator", 100),
        author_url: String::new(),
        source_url: format!("https://archive.org/details/{id}"),
        thumb: format!("https://archive.org/services/img/{id}"),
        previews: Vec::new(),
        width: None,
        height: None,
        seconds: runtime(&first(v, "runtime", 20)),
        tags: Vec::new(),
        rights: crate::cc::rights(&licence),
        download: Some(Download {
            url: format!("https://archive.org/metadata/{id}"),
            ext: ext.into(),
            width: None,
            height: None,
            bytes: None,
            note: "The best available copy is chosen when you add it.".into(),
            resolve: true,
        }),
    })
}

pub fn parse(body: &[u8]) -> Result<Page, SourceError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| SourceError::bad("the Internet Archive answered with something that is not JSON"))?;
    let resp = v.get("response").ok_or_else(|| SourceError::bad("the Internet Archive's answer had no results"))?;
    let docs = resp.get("docs").and_then(Value::as_array).ok_or_else(|| SourceError::bad("the Internet Archive's answer had no results list"))?;
    Ok(Page { candidates: docs.iter().take(100).filter_map(one).collect(), total: uint(resp, "numFound").unwrap_or(0) })
}

/// The file of an item to download, from its `/metadata/<id>` answer: its name and size. For video
/// an `mp4`, for sound an `mp3`, for pictures a `jpg`: the biggest not over `limit` bytes (a film's
/// original can be gigabytes; its H.264 copy is the right size), else the smallest there is.
pub fn pick_file(metadata: &[u8], kind: Kind, limit: u64) -> Result<(String, u64), SourceError> {
    let v: Value = serde_json::from_slice(metadata).map_err(|_| SourceError::bad("the Internet Archive's item description was not JSON"))?;
    let files = v.get("files").and_then(Value::as_array).ok_or_else(|| SourceError::bad("the Internet Archive's item had no files"))?;
    let want: &[&str] = match kind {
        Kind::Video => &["mp4", "m4v"],
        Kind::Audio => &["mp3"],
        Kind::Image => &["jpg", "jpeg"],
    };
    let mut ok: Vec<(String, u64, bool)> = files
        .iter()
        .take(2000)
        .filter_map(|f| {
            let name = text(f, "name", 300);
            let lower = name.to_ascii_lowercase();
            let ext = lower.rsplit('.').next().unwrap_or("");
            let format = text(f, "format", 60).to_ascii_lowercase();
            let size = f.get("size").and_then(|s| s.as_str().and_then(|x| x.parse::<u64>().ok()).or_else(|| s.as_u64())).unwrap_or(0);
            let plain_name = !name.is_empty() && !name.contains(['/', '\\']) && !name.contains("..") && !name.chars().any(char::is_control);
            let usable = plain_name && want.contains(&ext) && size > 0 && !format.contains("thumb") && !format.contains("sample");
            usable.then_some((name, size, format.contains("h.264") || format.contains("vbr")))
        })
        .collect();
    ok.sort_by_key(|(_, size, _)| *size);
    let within: Vec<&(String, u64, bool)> = ok.iter().filter(|(_, s, _)| *s <= limit).collect();
    // among the files that fit, the best made for the web, then the biggest
    let best = within.iter().rev().find(|(_, _, preferred)| *preferred).or_else(|| within.last()).map(|x| (x.0.clone(), x.1));
    best.or_else(|| ok.first().map(|x| (x.0.clone(), x.1))).ok_or_else(|| SourceError::Unavailable("this item has no file Edith can use".to_string()))
}

/// The address to download a file of an item.
pub fn file_url(identifier: &str, name: &str) -> Option<String> {
    (valid_identifier(identifier) && !name.is_empty()).then(|| format!("https://archive.org/download/{identifier}/{}", encode(name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = r#"{"responseHeader":{"status":0},"response":{"numFound":763,"start":0,"docs":[
      {"identifier":"SanFrancisco1955CinemascopeFilm","title":"San Francisco (1955 Cinemascope film)","creator":["Pan Am","Someone"],"licenseurl":"http://creativecommons.org/licenses/by-sa/3.0/","runtime":"21:28","mediatype":"movies"},
      {"identifier":"Horror_Hotel","title":"City of the Dead","licenseurl":"http://creativecommons.org/licenses/publicdomain/","runtime":"1:15:54","mediatype":"movies"},
      {"identifier":"TheSoundOfTheRosary","title":"The Sound of the Rosary","creator":"Christian Peschken","licenseurl":"http://creativecommons.org/publicdomain/zero/1.0/","mediatype":"audio"},
      {"identifier":"../evil","title":"x","mediatype":"movies"},
      {"identifier":"texts-item","title":"A book","licenseurl":"http://creativecommons.org/licenses/by/4.0/","mediatype":"texts"},
      {"title":"no id"}]}}"#;

    const ITEM: &str = r#"{"server":"s","files":[
      {"name":"Popeye.mpeg","source":"original","format":"MPEG2","size":"179044352"},
      {"name":"Popeye_512kb.mp4","source":"derivative","format":"512Kb MPEG4","size":"26989036","length":"370.3","width":"320","height":"240"},
      {"name":"Popeye.mp4","source":"derivative","format":"h.264","size":"90000000"},
      {"name":"huge.mp4","source":"derivative","format":"h.264 HD","size":"9000000000"},
      {"name":"thumbs/Popeye_000001.jpg","format":"Thumbnail","size":"500"},
      {"name":"Popeye_thumb.jpg","format":"JPEG Thumb","size":"900"},
      {"name":"../escape.mp4","format":"h.264","size":"10"},
      {"name":"Popeye_archive.torrent","format":"Archive BitTorrent","size":"3000"}]}"#;

    #[test]
    fn items_with_a_stated_licence_become_candidates_and_the_rest_are_dropped() {
        let p = parse(ANSWER.as_bytes()).unwrap();
        assert_eq!((p.total, p.candidates.len()), (763, 3), "a path-like name and a book are not candidates");
        let a = &p.candidates[0];
        assert_eq!((a.id.as_str(), a.kind, a.author.as_str(), a.seconds), ("archive:SanFrancisco1955CinemascopeFilm", Kind::Video, "Pan Am", Some(1288.0)));
        assert_eq!((a.rights.name.as_str(), a.rights.commercial, a.rights.attribution_required), ("CC BY-SA 3.0", Some(true), Some(true)));
        assert_eq!(a.thumb, "https://archive.org/services/img/SanFrancisco1955CinemascopeFilm");
        assert_eq!(a.source_url, "https://archive.org/details/SanFrancisco1955CinemascopeFilm");
        let d = a.download.as_ref().unwrap();
        assert!(d.resolve && d.url == "https://archive.org/metadata/SanFrancisco1955CinemascopeFilm" && d.ext == "mp4");
        let pd = &p.candidates[1];
        assert_eq!((pd.rights.name.as_str(), pd.rights.attribution_required, pd.seconds), ("Public domain", Some(false), Some(4554.0)));
        assert_eq!((p.candidates[2].kind, p.candidates[2].download.as_ref().unwrap().ext.as_str(), p.candidates[2].seconds), (Kind::Audio, "mp3", None));
        for bad in ["nope", "{}", r#"{"response":{"docs":3}}"#] {
            assert!(parse(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_best_file_is_chosen_from_the_item_and_hostile_names_are_never_picked() {
        let (name, size) = pick_file(ITEM.as_bytes(), Kind::Video, 600 * 1024 * 1024).unwrap();
        assert_eq!((name.as_str(), size), ("Popeye.mp4", 90_000_000), "the h.264 copy that fits, not the 9 GB one or the escape path");
        // nothing fits the limit: the smallest there is
        assert_eq!(pick_file(ITEM.as_bytes(), Kind::Video, 1000).unwrap().0, "Popeye_512kb.mp4");
        assert!(pick_file(ITEM.as_bytes(), Kind::Audio, u64::MAX).is_err(), "no mp3 in this item");
        assert!(pick_file(br#"{"files":[]}"#, Kind::Video, 10).is_err() && pick_file(b"nope", Kind::Video, 10).is_err());
        let audio = br#"{"files":[{"name":"a.mp3","format":"VBR MP3","size":"5000000"},{"name":"b.mp3","format":"128Kbps MP3","size":"9000000"}]}"#;
        assert_eq!(pick_file(audio, Kind::Audio, u64::MAX).unwrap().0, "a.mp3", "the web-made copy is preferred over a bigger one");
        let img = br#"{"files":[{"name":"p.jpg","format":"JPEG","size":"300000"},{"name":"p_thumb.jpg","format":"JPEG Thumb","size":"9000"}]}"#;
        assert_eq!(pick_file(img, Kind::Image, u64::MAX).unwrap().0, "p.jpg");
    }

    #[test]
    fn runtimes_identifiers_and_addresses() {
        assert_eq!(
            [runtime("21:28"), runtime("1:15:54"), runtime("370.3"), runtime("x"), runtime("-5"), runtime("")],
            [Some(1288.0), Some(4554.0), Some(370.3), None, None, None]
        );
        assert!(valid_identifier("a-b_c.d1") && !valid_identifier("../x") && !valid_identifier("a b") && !valid_identifier("") && !valid_identifier(".hidden"));
        assert_eq!(file_url("Popeye", "a b/ü.mp4").unwrap(), "https://archive.org/download/Popeye/a%20b%2F%C3%BC.mp4");
        assert!(file_url("../x", "a.mp4").is_none() && file_url("ok", "").is_none());
    }

    #[test]
    fn the_search_asks_only_for_items_with_a_licence_and_the_kinds_wanted() {
        let req = Request { text: "fog over a valley".into(), kinds: vec![Kind::Video, Kind::Audio], count: 6, page: 2, ..Request::default() };
        let u = search_url(&req).unwrap();
        assert!(u.starts_with("https://archive.org/advancedsearch.php?q=%28fog%20AND%20valley%29%20AND%20mediatype%3A%28movies%20OR%20audio%29%20AND%20licenseurl%3A%2A&fl%5B%5D=identifier"), "{u}");
        assert!(u.contains("rows=6") && u.contains("page=2") && u.contains("output=json") && u.contains("sort%5B%5D=downloads%20desc"), "{u}");
        assert!(search_url(&Request { text: "".into(), ..Request::default() }).is_none());
    }
}
