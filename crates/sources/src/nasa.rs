//! The NASA Image and Video Library (`https://images-api.nasa.gov`): film and pictures from NASA's
//! archives. No key. NASA's media policy is that its own material is not copyrighted, so results
//! are offered as public domain, **unless the item's own text mentions a copyright or credits a
//! third party**: those are marked "check the terms" and claim nothing. A result is a whole item;
//! the file is chosen when the person adds it (`Download::resolve`), by [`pick_file`], from the
//! item's list of renditions.

use crate::types::{Candidate, Download, Kind, Page, Request, Rights, SourceError};
use crate::util::{text, uint, with_query};
use serde_json::Value;

pub const ID: &str = "nasa";

const POLICY: &str = "https://www.nasa.gov/nasa-brand-center/images-and-media/";

/// The address of a search over the kinds asked for (video and pictures).
pub fn search_url(req: &Request) -> Option<String> {
    let query = crate::query::keywords(&req.text, 6, 80);
    if query.is_empty() {
        return None;
    }
    let kinds: Vec<&str> = req
        .kinds
        .iter()
        .filter_map(|k| match k {
            Kind::Video => Some("video"),
            Kind::Image => Some("image"),
            Kind::Audio => None,
        })
        .collect();
    if kinds.is_empty() {
        return None;
    }
    let p = vec![("q", query), ("media_type", kinds.join(",")), ("page_size", req.count.clamp(1, 40).to_string()), ("page", req.page.to_string())];
    Some(with_query("https://images-api.nasa.gov/search", &p))
}

/// An item name is a file name in NASA's store: no slashes, no way up the tree, no control
/// characters, a sensible length.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().count() <= 150 && !id.contains(['/', '\\', '?', '#', '%']) && !id.contains("..") && !id.chars().any(char::is_control)
}

/// `s` made safe to put in an address: spaces and the like are percent-encoded.
fn encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A file address from NASA's store (they come as `http://` with spaces in the name): kept only
/// if it is on NASA's asset host, and returned as `https` with the name encoded.
fn asset_url(href: &str) -> Option<String> {
    let rest = href.strip_prefix("http://").or_else(|| href.strip_prefix("https://"))?;
    let (host, path) = rest.split_once('/')?;
    if host != "images-assets.nasa.gov" || path.contains("..") || path.contains(['?', '#', '%']) || path.chars().any(|c| c.is_control()) || path.len() > 600 {
        return None;
    }
    Some(format!("https://{host}/{}", encode_path(path)))
}

/// The address of an item's list of renditions.
pub fn manifest_url(nasa_id: &str) -> Option<String> {
    valid_id(nasa_id).then(|| format!("https://images-api.nasa.gov/asset/{}", encode_path(nasa_id).replace('/', "%2F")))
}

fn rights(data: &Value) -> Rights {
    let mut about = text(data, "description", 2000);
    about.push(' ');
    about.push_str(&text(data, "description_508", 2000));
    let about = about.to_ascii_lowercase();
    // a named third party, or a copyright notice, means NASA is not the owner
    let other_owner = about.contains("copyright") || about.contains('©') || about.contains("courtesy of") || about.contains("used with permission");
    if other_owner {
        Rights { name: "NASA library; check the terms".into(), url: POLICY.into(), commercial: None, attribution_required: None }
    } else {
        Rights { name: "NASA media (public domain)".into(), url: POLICY.into(), commercial: Some(true), attribution_required: Some(false) }
    }
}

fn one(v: &Value) -> Option<Candidate> {
    let data = v.get("data").and_then(Value::as_array).and_then(|a| a.first())?;
    let nasa_id = text(data, "nasa_id", 200);
    if !valid_id(&nasa_id) {
        return None;
    }
    let kind = match text(data, "media_type", 10).as_str() {
        "video" => Kind::Video,
        "image" => Kind::Image,
        _ => return None,
    };
    let manifest = manifest_url(&nasa_id)?;
    let thumb = v
        .get("links")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|l| l.get("rel").and_then(Value::as_str) == Some("preview"))
        .and_then(|l| l.get("href").and_then(Value::as_str))
        .and_then(asset_url)
        .unwrap_or_default();
    let author = ["photographer", "secondary_creator"].iter().map(|k| text(data, k, 100)).find(|s| !s.is_empty()).unwrap_or_else(|| {
        let centre = text(data, "center", 20);
        if centre.is_empty() { "NASA".to_string() } else { format!("NASA ({centre})") }
    });
    let tags = data
        .get("keywords")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .take(12)
        .map(|s| s.chars().filter(|c| !c.is_control()).take(30).collect::<String>().to_lowercase())
        .collect();
    Some(Candidate {
        id: format!("{ID}:{nasa_id}"),
        source: ID.into(),
        kind,
        title: text(data, "title", 200),
        author,
        author_url: String::new(),
        source_url: format!("https://images.nasa.gov/details/{}", encode_path(&nasa_id).replace('/', "%2F")),
        thumb,
        previews: Vec::new(),
        width: None,
        height: None,
        seconds: None,
        tags,
        rights: rights(data),
        download: Some(Download {
            url: manifest,
            ext: if kind == Kind::Video { "mp4" } else { "jpg" }.into(),
            width: None,
            height: None,
            bytes: None,
            note: "The best available copy is chosen when you add it.".into(),
            resolve: true,
        }),
    })
}

pub fn parse(body: &[u8]) -> Result<Page, SourceError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| SourceError::bad("NASA's library answered with something that is not JSON"))?;
    let coll = v.get("collection").ok_or_else(|| SourceError::bad("NASA's answer had no results"))?;
    let items = coll.get("items").and_then(Value::as_array).ok_or_else(|| SourceError::bad("NASA's answer had no results list"))?;
    let total = coll.get("metadata").and_then(|m| uint(m, "total_hits")).unwrap_or(0);
    Ok(Page { candidates: items.iter().take(100).filter_map(one).collect(), total })
}

/// The file of an item to download, from its list of renditions (`/asset/<id>`): the address.
/// Video: the medium rendition, else large, small, mobile, and the original only when there is
/// nothing else (originals can be gigabytes). Pictures: the large `jpg`, else medium, else the
/// original if it is a `jpg`, else small.
pub fn pick_file(manifest: &[u8], kind: Kind) -> Result<String, SourceError> {
    let v: Value = serde_json::from_slice(manifest).map_err(|_| SourceError::bad("NASA's list of files for the item was not JSON"))?;
    let items = v
        .get("collection")
        .and_then(|c| c.get("items"))
        .and_then(Value::as_array)
        .ok_or_else(|| SourceError::bad("NASA's list of files for the item was empty"))?;
    let hrefs: Vec<String> = items.iter().take(200).filter_map(|i| i.get("href").and_then(Value::as_str)).map(str::to_string).collect();
    let order: &[&str] = match kind {
        Kind::Video => &["~medium.mp4", "~large.mp4", "~small.mp4", "~mobile.mp4", "~orig.mp4"],
        Kind::Image => &["~large.jpg", "~medium.jpg", "~orig.jpg", "~orig.jpeg", "~small.jpg"],
        Kind::Audio => &[],
    };
    for suffix in order {
        if let Some(url) = hrefs.iter().find(|h| h.to_ascii_lowercase().ends_with(suffix)).and_then(|h| asset_url(h)) {
            return Ok(url);
        }
    }
    Err(SourceError::Unavailable("this NASA item has no file Edith can use".to_string()))
}

/// The stills NASA made from inside a film (`~small_1.jpg`, `~small_2.jpg`…), from its list of
/// renditions: what Edith looks at before offering it. In order, at most 8.
pub fn preview_urls(manifest: &[u8]) -> Vec<String> {
    let Ok(v) = serde_json::from_slice::<Value>(manifest) else { return Vec::new() };
    let mut stills: Vec<String> = v
        .get("collection")
        .and_then(|c| c.get("items"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(200)
        .filter_map(|i| i.get("href").and_then(Value::as_str))
        .filter(|h| {
            let l = h.to_ascii_lowercase();
            l.contains("~small_") && l.ends_with(".jpg")
        })
        .filter_map(asset_url)
        .collect();
    stills.sort();
    stills.truncate(8);
    stills
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = r#"{"collection":{"version":"1.1","metadata":{"total_hits":221},"items":[
      {"href":"https://images-assets.nasa.gov/video/A one/collection.json","data":[{"center":"AFRC","description":"Apollo Digest Series.","keywords":["Apollo","AFRC","filmstock"],
        "media_type":"video","nasa_id":"NDTV000908 Apollo Digest","title":"Apollo Digest Series: Spacecraft for Apollo"}],
       "links":[{"href":"https://images-assets.nasa.gov/video/NDTV000908 Apollo Digest/NDTV000908 Apollo Digest~large.jpg","rel":"alternate","render":"image"},
                {"href":"https://images-assets.nasa.gov/video/NDTV000908 Apollo Digest/NDTV000908 Apollo Digest~thumb.jpg","rel":"preview","render":"image"}]},
      {"data":[{"center":"HQ","description_508":"Full color logo.","keywords":[],"media_type":"image","nasa_id":"APOLLO 50th","title":"Apollo 50th","photographer":"Jane Doe"}],
       "links":[{"href":"http://images-assets.nasa.gov/image/APOLLO 50th/APOLLO 50th~thumb.jpg","rel":"preview"}]},
      {"data":[{"center":"JSC","description":"Photo is copyright Some Agency and used with permission.","media_type":"image","nasa_id":"owned_1","title":"Owned"}],"links":[]},
      {"data":[{"media_type":"audio","nasa_id":"talk","title":"A recording"}]},
      {"data":[{"media_type":"video","nasa_id":"../escape","title":"x"}]},
      {"data":[{"media_type":"video","nasa_id":"has/slash","title":"x"}]},
      {"links":[]},
      7]}}"#;

    #[test]
    fn items_become_candidates_with_the_policy_as_the_licence_and_an_owner_in_the_text_is_flagged() {
        let p = parse(ANSWER.as_bytes()).unwrap();
        let ids: Vec<&str> = p.candidates.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["nasa:NDTV000908 Apollo Digest", "nasa:APOLLO 50th", "nasa:owned_1"], "audio, path-like names and malformed items are dropped");
        assert_eq!(p.total, 221);
        let film = &p.candidates[0];
        assert_eq!((film.kind, film.author.as_str(), film.title.as_str()), (Kind::Video, "NASA (AFRC)", "Apollo Digest Series: Spacecraft for Apollo"));
        assert_eq!(
            (film.rights.name.as_str(), film.rights.commercial, film.rights.attribution_required),
            ("NASA media (public domain)", Some(true), Some(false))
        );
        assert_eq!(
            film.thumb, "https://images-assets.nasa.gov/video/NDTV000908%20Apollo%20Digest/NDTV000908%20Apollo%20Digest~thumb.jpg",
            "the preview, over https, encoded"
        );
        assert_eq!(film.tags, ["apollo", "afrc", "filmstock"]);
        assert_eq!(film.source_url, "https://images.nasa.gov/details/NDTV000908%20Apollo%20Digest");
        let d = film.download.as_ref().unwrap();
        assert!(d.resolve && d.ext == "mp4" && d.url == "https://images-api.nasa.gov/asset/NDTV000908%20Apollo%20Digest", "{}", d.url);
        let pic = &p.candidates[1];
        assert_eq!((pic.kind, pic.author.as_str(), pic.download.as_ref().map(|d| d.ext.as_str())), (Kind::Image, "Jane Doe", Some("jpg")));
        let owned = &p.candidates[2];
        assert_eq!(
            (owned.rights.name.as_str(), owned.rights.commercial),
            ("NASA library; check the terms", None),
            "a copyright notice means NASA may not own it"
        );
        for bad in ["nope", "{}", r#"{"collection":{"items":3}}"#] {
            assert!(parse(bad.as_bytes()).is_err(), "{bad}");
        }
        assert!(parse(br#"{"collection":{"items":[]}}"#).unwrap().candidates.is_empty());
    }

    const MANIFEST: &str = r#"{"collection":{"items":[
      {"href":"http://images-assets.nasa.gov/video/X Y/X Y~orig.mp4"},{"href":"http://images-assets.nasa.gov/video/X Y/X Y.srt"},
      {"href":"http://images-assets.nasa.gov/video/X Y/X Y~large.mp4"},{"href":"http://images-assets.nasa.gov/video/X Y/X Y~medium.mp4"},
      {"href":"http://images-assets.nasa.gov/video/X Y/X Y~preview.mp4"},{"href":"http://elsewhere.example/X~medium.mp4"},{"href":"http://images-assets.nasa.gov/video/X Y/X Y~small.mp4"}]}}"#;

    #[test]
    fn the_medium_copy_is_chosen_and_the_original_only_when_nothing_else_is_there() {
        assert_eq!(pick_file(MANIFEST.as_bytes(), Kind::Video).unwrap(), "https://images-assets.nasa.gov/video/X%20Y/X%20Y~medium.mp4");
        let only_orig = br#"{"collection":{"items":[{"href":"http://images-assets.nasa.gov/video/Z/Z~orig.mp4"},{"href":"http://images-assets.nasa.gov/video/Z/Z~preview.mp4"}]}}"#;
        assert_eq!(pick_file(only_orig, Kind::Video).unwrap(), "https://images-assets.nasa.gov/video/Z/Z~orig.mp4");
        let pics = br#"{"collection":{"items":[{"href":"http://images-assets.nasa.gov/image/P/P~orig.png"},{"href":"http://images-assets.nasa.gov/image/P/P~medium.jpg"},{"href":"http://images-assets.nasa.gov/image/P/P~large.jpg"}]}}"#;
        assert_eq!(pick_file(pics, Kind::Image).unwrap(), "https://images-assets.nasa.gov/image/P/P~large.jpg");
        let png_only = br#"{"collection":{"items":[{"href":"http://images-assets.nasa.gov/image/P/P~orig.png"}]}}"#;
        assert!(pick_file(png_only, Kind::Image).is_err(), "a png original is not chosen, there is nothing else to fetch");
        // a file on another host, or a way up the tree, is never chosen
        let hostile = br#"{"collection":{"items":[{"href":"http://elsewhere.example/a~medium.mp4"},{"href":"http://images-assets.nasa.gov/video/../../etc~medium.mp4"}]}}"#;
        assert!(pick_file(hostile, Kind::Video).is_err());
        for bad in ["nope", "{}", r#"{"collection":{"items":[]}}"#] {
            assert!(pick_file(bad.as_bytes(), Kind::Video).is_err(), "{bad}");
        }
        assert!(pick_file(MANIFEST.as_bytes(), Kind::Audio).is_err());
    }

    #[test]
    fn the_stills_made_from_inside_a_film_are_listed_in_order() {
        let m = br#"{"collection":{"items":[{"href":"http://images-assets.nasa.gov/video/X Y/X Y~small_3.jpg"},{"href":"http://images-assets.nasa.gov/video/X Y/X Y~small_1.jpg"},
          {"href":"http://images-assets.nasa.gov/video/X Y/X Y~small_2.jpg"},{"href":"http://images-assets.nasa.gov/video/X Y/X Y~medium.mp4"},{"href":"http://elsewhere.example/a~small_1.jpg"}]}}"#;
        let urls = preview_urls(m);
        assert_eq!(urls.len(), 3, "{urls:?}");
        assert!(urls[0].ends_with("X%20Y~small_1.jpg") && urls[2].ends_with("X%20Y~small_3.jpg"), "{urls:?}");
        assert!(preview_urls(b"nope").is_empty() && preview_urls(br#"{"collection":{}}"#).is_empty());
    }

    #[test]
    fn the_address_asks_for_the_kinds_nasa_has_and_names_are_checked() {
        let req = Request { text: "apollo moon landing".into(), kinds: vec![Kind::Video, Kind::Image, Kind::Audio], count: 5, page: 2, ..Request::default() };
        assert_eq!(search_url(&req).unwrap(), "https://images-api.nasa.gov/search?q=apollo%20moon%20landing&media_type=video%2Cimage&page_size=5&page=2");
        assert!(search_url(&Request { kinds: vec![Kind::Audio], ..req.clone() }).is_none(), "NASA has no sound here");
        assert!(search_url(&Request { text: " ".into(), ..req }).is_none());
        assert!(valid_id("NDTV000908 Apollo") && !valid_id("a/b") && !valid_id("..") && !valid_id("a%2Fb") && !valid_id(""));
        assert_eq!(manifest_url("a b").as_deref(), Some("https://images-api.nasa.gov/asset/a%20b"));
        assert!(manifest_url("../x").is_none());
    }
}
