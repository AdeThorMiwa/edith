//! Pixabay (`https://pixabay.com/api/docs/`): free photos, illustrations and videos. The key is a
//! query parameter. The Pixabay Content License allows commercial use without a credit. Its API
//! terms require results to be cached for 24 hours (the engine does), bar mass downloading, and
//! allow a result's picture to be shown temporarily but not hot-linked for good (the panel only
//! shows thumbnails while you browse, and a chosen file is downloaded).
//! 100 requests a minute per key.

use crate::types::{Candidate, Download, Kind, Orientation, Page, Request, Rights, SourceError};
use crate::util::{dim, seconds, tags, text, uint, url, with_query};
use serde_json::Value;

pub const ID: &str = "pixabay";
const LICENSE_URL: &str = "https://pixabay.com/service/license-summary/";

fn rights() -> Rights {
    Rights { name: "Pixabay Content License".into(), url: LICENSE_URL.into(), commercial: Some(true), attribution_required: Some(false) }
}

/// The address to ask for `kind` with `key`; `None` when Pixabay has no such thing here.
pub fn search_url(req: &Request, kind: Kind, key: &str) -> Option<String> {
    let base = match kind {
        Kind::Video => "https://pixabay.com/api/videos/",
        Kind::Image => "https://pixabay.com/api/",
        Kind::Audio => return None,
    };
    let query = crate::query::keywords(&req.text, 8, 100);
    if query.is_empty() {
        return None;
    }
    // Pixabay's page size is 3 to 200
    let mut p = vec![
        ("key", key.to_string()),
        ("q", query),
        ("per_page", req.count.clamp(3, 40).to_string()),
        ("page", req.page.to_string()),
        ("safesearch", "true".to_string()),
    ];
    if let Some(w) = req.min_width {
        p.push(("min_width", w.to_string()));
    }
    if let Some(h) = req.min_height {
        p.push(("min_height", h.to_string()));
    }
    if kind == Kind::Image {
        p.push(("image_type", "photo".to_string()));
        match req.orientation {
            Some(Orientation::Landscape) => p.push(("orientation", "horizontal".to_string())),
            Some(Orientation::Portrait) => p.push(("orientation", "vertical".to_string())),
            _ => {}
        }
    }
    Some(with_query(base, &p))
}

fn video(v: &Value) -> Option<Candidate> {
    let id = uint(v, "id")?;
    let sizes = v.get("videos").unwrap_or(&Value::Null);
    // the biggest file that is not larger than Full HD, falling back to what exists
    let pick = ["large", "medium", "small", "tiny"]
        .iter()
        .filter_map(|k| sizes.get(*k))
        .find(|f| !url(f, "url").is_empty() && dim(f, "width").is_none_or(|w| w <= 1920));
    let pick = pick.or_else(|| ["medium", "small", "tiny", "large"].iter().filter_map(|k| sizes.get(*k)).find(|f| !url(f, "url").is_empty()));
    let thumb = ["tiny", "small", "medium"]
        .iter()
        .filter_map(|k| sizes.get(*k))
        .map(|f| url(f, "thumbnail"))
        .find(|u| !u.is_empty())
        .or_else(|| {
            // older answers carry only a picture id on the video host's image server
            let pid = text(v, "picture_id", 40);
            (!pid.is_empty() && pid.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).then(|| format!("https://i.vimeocdn.com/video/{pid}_295x166.jpg"))
        })
        .unwrap_or_default();
    Some(Candidate {
        id: format!("{ID}:video-{id}"),
        source: ID.into(),
        kind: Kind::Video,
        title: text(v, "tags", 120),
        author: text(v, "user", 100),
        author_url: uint(v, "user_id").map(|u| format!("https://pixabay.com/users/{}-{u}/", text(v, "user", 60).replace(' ', "-"))).unwrap_or_default(),
        source_url: url(v, "pageURL"),
        thumb,
        previews: Vec::new(),
        width: pick.and_then(|f| dim(f, "width")),
        height: pick.and_then(|f| dim(f, "height")),
        seconds: seconds(v, "duration"),
        tags: tags(&text(v, "tags", 300)),
        rights: rights(),
        download: pick.map(|f| Download {
            url: url(f, "url"),
            ext: "mp4".into(),
            width: dim(f, "width"),
            height: dim(f, "height"),
            bytes: uint(f, "size"),
            note: String::new(),
            resolve: false,
        }),
    })
}

fn image(v: &Value) -> Option<Candidate> {
    let id = uint(v, "id")?;
    let big = ["largeImageURL", "webformatURL"].iter().map(|k| url(v, k)).find(|u| !u.is_empty());
    Some(Candidate {
        id: format!("{ID}:image-{id}"),
        source: ID.into(),
        kind: Kind::Image,
        title: text(v, "tags", 120),
        author: text(v, "user", 100),
        author_url: String::new(),
        source_url: url(v, "pageURL"),
        thumb: ["webformatURL", "previewURL"].iter().map(|k| url(v, k)).find(|u| !u.is_empty()).unwrap_or_default(),
        previews: Vec::new(),
        width: dim(v, "imageWidth"),
        height: dim(v, "imageHeight"),
        seconds: None,
        tags: tags(&text(v, "tags", 300)),
        rights: rights(),
        download: big.map(|u| Download { url: u, ext: "jpg".into(), width: None, height: None, bytes: None, note: String::new(), resolve: false }),
    })
}

pub fn parse(kind: Kind, body: &[u8]) -> Result<Page, SourceError> {
    if kind == Kind::Audio {
        return Ok(Page::default());
    }
    let v: Value = serde_json::from_slice(body).map_err(|_| SourceError::bad("Pixabay answered with something that is not JSON"))?;
    let one: fn(&Value) -> Option<Candidate> = match kind {
        Kind::Video => video,
        Kind::Image => image,
        Kind::Audio => return Ok(Page::default()),
    };
    let hits = v.get("hits").and_then(Value::as_array).ok_or_else(|| SourceError::bad("Pixabay's answer had no results list"))?;
    Ok(Page { candidates: hits.iter().take(200).filter_map(one).collect(), total: uint(&v, "totalHits").or_else(|| uint(&v, "total")).unwrap_or(0) })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIDEOS: &str = r#"{"total":500,"totalHits":500,"hits":[
      {"id":125,"pageURL":"https://pixabay.com/videos/fog-125/","type":"film","tags":"fog, forest, morning","duration":12,"picture_id":"529927645","user_id":7,"user":"Some User",
       "videos":{"large":{"url":"https://cdn.pixabay.com/video/l.mp4","width":3840,"height":2160,"size":9000000,"thumbnail":"https://cdn.pixabay.com/video/l.jpg"},
                 "medium":{"url":"https://cdn.pixabay.com/video/m.mp4","width":1920,"height":1080,"size":4000000,"thumbnail":"https://cdn.pixabay.com/video/m.jpg"},
                 "small":{"url":"https://cdn.pixabay.com/video/s.mp4","width":1280,"height":720,"size":2000000,"thumbnail":""},
                 "tiny":{"url":"https://cdn.pixabay.com/video/t.mp4","width":640,"height":360,"size":900000,"thumbnail":"https://cdn.pixabay.com/video/t.jpg"}}},
      {"id":126,"pageURL":"https://pixabay.com/videos/b-126/","tags":"b","duration":5,"picture_id":"777","user":"U",
       "videos":{"large":{"url":"","width":0,"height":0,"size":0},"medium":{"url":"https://cdn.pixabay.com/video/m2.mp4","width":1920,"height":1080,"size":1},"tiny":{"url":"x"}}},
      {"tags":"no id"}]}"#;

    const IMAGES: &str = r#"{"total":10,"totalHits":10,"hits":[
      {"id":9,"pageURL":"https://pixabay.com/photos/pier-9/","tags":"pier, sea, dawn","previewURL":"https://cdn.pixabay.com/p.jpg","webformatURL":"https://cdn.pixabay.com/w.jpg","largeImageURL":"https://cdn.pixabay.com/l.jpg","imageWidth":5000,"imageHeight":3000,"user":"Ann"}]}"#;

    #[test]
    fn a_video_answer_picks_a_full_hd_file_and_records_the_licence() {
        let p = parse(Kind::Video, VIDEOS.as_bytes()).unwrap();
        assert_eq!((p.total, p.candidates.len()), (500, 2));
        let c = &p.candidates[0];
        assert_eq!((c.id.as_str(), c.author.as_str(), c.seconds), ("pixabay:video-125", "Some User", Some(12.0)));
        assert_eq!(c.rights.name, "Pixabay Content License");
        assert_eq!((c.rights.commercial, c.rights.attribution_required), (Some(true), Some(false)));
        let d = c.download.as_ref().unwrap();
        assert_eq!((d.url.as_str(), d.width, d.bytes), ("https://cdn.pixabay.com/video/m.mp4", Some(1920), Some(4_000_000)), "not the 4K one");
        assert_eq!(c.thumb, "https://cdn.pixabay.com/video/t.jpg");
        assert_eq!(c.tags, ["fog", "forest", "morning"]);
        // an unavailable large file is skipped; with no thumbnail the picture id is used
        let c = &p.candidates[1];
        assert_eq!(c.download.as_ref().unwrap().url, "https://cdn.pixabay.com/video/m2.mp4");
        assert_eq!(c.thumb, "https://i.vimeocdn.com/video/777_295x166.jpg");
    }

    #[test]
    fn an_image_answer_downloads_the_large_picture() {
        let c = &parse(Kind::Image, IMAGES.as_bytes()).unwrap().candidates[0];
        assert_eq!((c.id.as_str(), c.width, c.title.as_str()), ("pixabay:image-9", Some(5000), "pier, sea, dawn"));
        assert_eq!(c.download.as_ref().unwrap().url, "https://cdn.pixabay.com/l.jpg");
        assert_eq!(c.thumb, "https://cdn.pixabay.com/w.jpg");
        for bad in ["nope", "{}", r#"{"hits":"x"}"#] {
            assert!(parse(Kind::Image, bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_address_carries_the_key_and_stays_inside_pixabays_limits() {
        let req = Request {
            text: "a lone figure walking through fog at dawn".into(),
            count: 1,
            orientation: Some(Orientation::Portrait),
            min_width: Some(1920),
            ..Request::default()
        };
        let u = search_url(&req, Kind::Image, "KEY123").unwrap();
        assert!(u.starts_with("https://pixabay.com/api/?key=KEY123&q=lone%20figure%20walking%20fog%20dawn&per_page=3&"), "{u}");
        assert!(u.contains("orientation=vertical") && u.contains("min_width=1920") && u.contains("image_type=photo"), "{u}");
        let v = search_url(&req, Kind::Video, "K").unwrap();
        assert!(v.starts_with("https://pixabay.com/api/videos/?") && !v.contains("orientation"), "{v}");
        assert!(search_url(&req, Kind::Audio, "K").is_none() && search_url(&Request { text: "".into(), ..Request::default() }, Kind::Video, "K").is_none());
        assert!(
            search_url(&Request { text: "x".repeat(500), ..Request::default() }, Kind::Image, "K").unwrap().len() < 400,
            "the query stays under Pixabay's 100 characters"
        );
    }
}
