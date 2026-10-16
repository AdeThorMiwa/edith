//! Pexels (`https://www.pexels.com/api/`): free photos and videos. The key goes in the
//! `Authorization` header. The Pexels licence allows commercial use without a credit; its API terms
//! ask that results link back to Pexels and credit the photographer when possible, which the panel
//! does. 200 requests an hour by default.

use crate::types::{Candidate, Download, Kind, Page, Request, Rights, SourceError};
use crate::util::{dim, seconds, text, uint, url, with_query};
use serde_json::Value;

pub const ID: &str = "pexels";
const LICENSE_URL: &str = "https://www.pexels.com/license/";

fn rights() -> Rights {
    Rights { name: "Pexels License".into(), url: LICENSE_URL.into(), commercial: Some(true), attribution_required: Some(false) }
}

/// The address to ask for `kind` (`Video` or `Image`); `None` when Pexels has no such thing.
pub fn search_url(req: &Request, kind: Kind) -> Option<String> {
    let base = match kind {
        Kind::Video => "https://api.pexels.com/v1/videos/search",
        Kind::Image => "https://api.pexels.com/v1/search",
        Kind::Audio => return None,
    };
    let query = crate::query::keywords(&req.text, 8, 100);
    if query.is_empty() {
        return None;
    }
    let mut p = vec![("query", query), ("per_page", req.count.clamp(1, 40).to_string()), ("page", req.page.to_string())];
    if let Some(o) = req.orientation {
        p.push(("orientation", o.name().to_string()));
    }
    // Pexels filters by a minimum size class, not by pixels
    let longest = req.min_width.unwrap_or(0).max(req.min_height.unwrap_or(0));
    let class = match (kind, longest) {
        (Kind::Video, w) if w >= 3000 => Some("large"),
        (Kind::Video, w) if w >= 1800 => Some("medium"),
        (Kind::Video, w) if w >= 1200 => Some("small"),
        (Kind::Image, w) if w >= 5000 => Some("large"),
        (Kind::Image, w) if w >= 3500 => Some("medium"),
        (Kind::Image, w) if w >= 2000 => Some("small"),
        _ => None,
    };
    if let Some(c) = class {
        p.push(("size", c.to_string()));
    }
    Some(with_query(base, &p))
}

/// The file to download for a video: the biggest mp4 that is no wider than Full HD (a channel
/// edits at 1080p; 4K files are very large), else the smallest mp4 there is.
fn best_video_file(files: &[Value]) -> Option<&Value> {
    let mp4 = |f: &&Value| f.get("file_type").and_then(Value::as_str).is_none_or(|t| t.contains("mp4")) && !url(f, "link").is_empty();
    let width = |f: &Value| dim(f, "width").unwrap_or(0);
    let mut ok: Vec<&Value> = files.iter().filter(mp4).collect();
    ok.sort_by_key(|f| width(f));
    ok.iter().rev().find(|f| width(f) <= 1920 && width(f) > 0).copied().or_else(|| ok.first().copied())
}

fn video(v: &Value) -> Option<Candidate> {
    let id = uint(v, "id")?;
    let files = v.get("video_files").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let best = best_video_file(files);
    let user = v.get("user").unwrap_or(&Value::Null);
    let title = {
        // the page address ends with a readable name: .../video/fog-over-a-valley-1234/
        let u = text(v, "url", 300);
        u.trim_end_matches('/').rsplit('/').next().map(|s| s.replace('-', " ")).unwrap_or_default()
    };
    Some(Candidate {
        id: format!("{ID}:{id}"),
        source: ID.into(),
        kind: Kind::Video,
        title,
        author: text(user, "name", 100),
        author_url: url(user, "url"),
        source_url: url(v, "url"),
        thumb: url(v, "image"),
        width: dim(v, "width"),
        height: dim(v, "height"),
        seconds: seconds(v, "duration"),
        tags: Vec::new(),
        rights: rights(),
        download: best.map(|f| Download {
            url: url(f, "link"),
            ext: "mp4".into(),
            width: dim(f, "width"),
            height: dim(f, "height"),
            bytes: None,
            note: String::new(),
        }),
    })
}

fn photo(v: &Value) -> Option<Candidate> {
    let id = uint(v, "id")?;
    let src = v.get("src").unwrap_or(&Value::Null);
    let big = ["large2x", "large", "original"].iter().map(|k| url(src, k)).find(|u| !u.is_empty());
    Some(Candidate {
        id: format!("{ID}:photo-{id}"),
        source: ID.into(),
        kind: Kind::Image,
        title: text(v, "alt", 200),
        author: text(v, "photographer", 100),
        author_url: url(v, "photographer_url"),
        source_url: url(v, "url"),
        thumb: ["medium", "small", "tiny"].iter().map(|k| url(src, k)).find(|u| !u.is_empty()).unwrap_or_default(),
        width: dim(v, "width"),
        height: dim(v, "height"),
        seconds: None,
        tags: Vec::new(),
        rights: rights(),
        download: big.map(|u| Download { url: u, ext: "jpg".into(), width: None, height: None, bytes: None, note: String::new() }),
    })
}

/// Reads an answer from the video or photo search.
pub fn parse(kind: Kind, body: &[u8]) -> Result<Page, SourceError> {
    if kind == Kind::Audio {
        return Ok(Page::default());
    }
    let v: Value = serde_json::from_slice(body).map_err(|_| SourceError::bad("Pexels answered with something that is not JSON"))?;
    let (key, one): (&str, fn(&Value) -> Option<Candidate>) = match kind {
        Kind::Video => ("videos", video),
        Kind::Image => ("photos", photo),
        Kind::Audio => return Ok(Page::default()),
    };
    let list = v.get(key).and_then(Value::as_array).ok_or_else(|| SourceError::bad("Pexels' answer had no results list"))?;
    Ok(Page { candidates: list.iter().take(80).filter_map(one).collect(), total: uint(&v, "total_results").unwrap_or(0) })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIDEOS: &str = r#"{"page":1,"per_page":2,"total_results":88,"url":"https://www.pexels.com/search/videos/fog/","videos":[
      {"id":3571264,"width":3840,"height":2160,"url":"https://www.pexels.com/video/fog-over-a-valley-3571264/","image":"https://images.pexels.com/videos/3571264/pic.jpeg","duration":14,
       "user":{"id":1,"name":"Jane Doe","url":"https://www.pexels.com/@jane"},
       "video_files":[
         {"id":1,"quality":"sd","file_type":"video/mp4","width":640,"height":360,"link":"https://videos.pexels.com/a-sd.mp4"},
         {"id":2,"quality":"hd","file_type":"video/mp4","width":1920,"height":1080,"fps":25.0,"link":"https://videos.pexels.com/a-hd.mp4"},
         {"id":3,"quality":"hd","file_type":"video/mp4","width":3840,"height":2160,"link":"https://videos.pexels.com/a-uhd.mp4"},
         {"id":4,"quality":"hd","file_type":"video/webm","width":1280,"height":720,"link":"https://videos.pexels.com/a.webm"}],
       "video_pictures":[{"id":9,"picture":"https://images.pexels.com/p.jpeg","nr":0}]},
      {"id":5,"width":1080,"height":1920,"url":"javascript:alert(1)","image":"data:text/html,x","duration":"long","user":{"name":"A\u0007B"},
       "video_files":[{"id":6,"file_type":"video/mp4","width":720,"height":1280,"link":"https://videos.pexels.com/b-sd.mp4"}]},
      {"no_id":true}, 5]}"#;

    const PHOTOS: &str = r#"{"page":1,"per_page":1,"total_results":9,"photos":[
      {"id":42,"width":4000,"height":3000,"url":"https://www.pexels.com/photo/42/","photographer":"Sam","photographer_url":"https://www.pexels.com/@sam","alt":"A pier at dawn",
       "src":{"original":"https://images.pexels.com/o.jpg","large2x":"https://images.pexels.com/l2.jpg","large":"https://images.pexels.com/l.jpg","medium":"https://images.pexels.com/m.jpg","tiny":"https://images.pexels.com/t.jpg"}}]}"#;

    #[test]
    fn a_video_answer_becomes_candidates_with_the_licence_and_the_right_file() {
        let p = parse(Kind::Video, VIDEOS.as_bytes()).unwrap();
        assert_eq!((p.total, p.candidates.len()), (88, 2));
        let c = &p.candidates[0];
        assert_eq!((c.id.as_str(), c.title.as_str(), c.author.as_str(), c.seconds), ("pexels:3571264", "fog over a valley 3571264", "Jane Doe", Some(14.0)));
        assert_eq!(
            c.rights,
            Rights { name: "Pexels License".into(), url: "https://www.pexels.com/license/".into(), commercial: Some(true), attribution_required: Some(false) }
        );
        let d = c.download.as_ref().unwrap();
        assert_eq!(
            (d.url.as_str(), d.width, d.ext.as_str()),
            ("https://videos.pexels.com/a-hd.mp4", Some(1920), "mp4"),
            "Full HD mp4, not the 4K one and not the webm"
        );
        assert_eq!(c.thumb, "https://images.pexels.com/videos/3571264/pic.jpeg");
    }

    #[test]
    fn hostile_fields_are_dropped_not_trusted() {
        let c = &parse(Kind::Video, VIDEOS.as_bytes()).unwrap().candidates[1];
        assert!(c.source_url.is_empty() && c.thumb.is_empty(), "javascript: and data: addresses are not kept");
        assert_eq!((c.author.as_str(), c.seconds), ("AB", None));
        assert_eq!(c.download.as_ref().unwrap().url, "https://videos.pexels.com/b-sd.mp4", "the only mp4 there is");
        for bad in ["nope", "{}", r#"{"videos":5}"#] {
            assert!(parse(Kind::Video, bad.as_bytes()).is_err(), "{bad}");
        }
        assert!(parse(Kind::Video, br#"{"videos":[]}"#).unwrap().candidates.is_empty());
        assert!(parse(Kind::Audio, b"anything").unwrap().candidates.is_empty());
    }

    #[test]
    fn a_photo_answer_gets_a_large_download_and_a_medium_thumbnail() {
        let p = parse(Kind::Image, PHOTOS.as_bytes()).unwrap();
        let c = &p.candidates[0];
        assert_eq!((c.id.as_str(), c.title.as_str(), c.kind), ("pexels:photo-42", "A pier at dawn", Kind::Image));
        assert_eq!(c.download.as_ref().unwrap().url, "https://images.pexels.com/l2.jpg");
        assert_eq!(c.thumb, "https://images.pexels.com/m.jpg");
    }

    #[test]
    fn the_search_address_carries_the_request() {
        let req = Request {
            text: "a lone figure walking through fog at dawn".into(),
            count: 10,
            page: 2,
            orientation: Some(crate::Orientation::Landscape),
            min_width: Some(1920),
            ..Request::default()
        };
        let u = search_url(&req, Kind::Video).unwrap();
        assert!(
            u.starts_with(
                "https://api.pexels.com/v1/videos/search?query=lone%20figure%20walking%20fog%20dawn&per_page=10&page=2&orientation=landscape&size=medium"
            ),
            "{u}"
        );
        assert!(search_url(&req, Kind::Image).unwrap().starts_with("https://api.pexels.com/v1/search?"));
        assert!(search_url(&req, Kind::Audio).is_none());
        assert!(search_url(&Request { text: "   ".into(), ..Request::default() }, Kind::Video).is_none());
        assert!(!search_url(&Request { text: "fog".into(), ..Request::default() }, Kind::Video).unwrap().contains("size="));
    }
}
