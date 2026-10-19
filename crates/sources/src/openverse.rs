//! Openverse (`https://api.openverse.org`): one search over openly licensed pictures and sound from
//! many collections (Flickr, Wikimedia, Jamendo's music, Freesound and more). No key. Only
//! commercial-use licences are asked for, and a result whose licence forbids **derivatives** is
//! dropped here as well: cutting, fading and grading a clip are derivatives, so such a file could
//! not be used in an edit. Share-alike and credit-required results are offered with the licence
//! named as the site gives it, so the person sees what the credit has to say.

use crate::types::{Candidate, Download, Kind, Page, Request, Rights, SourceError};
use crate::util::{text, uint, url, with_query};
use serde_json::Value;

pub const ID: &str = "openverse";

/// The address of a search for `kind` (pictures or sound; Openverse has no video).
pub fn search_url(req: &Request, kind: Kind) -> Option<String> {
    let path = match kind {
        Kind::Image => "images",
        Kind::Audio => "audio",
        Kind::Video => return None,
    };
    let query = crate::query::keywords(&req.text, 6, 80);
    if query.is_empty() {
        return None;
    }
    let mut p = vec![("q", query), ("license_type", "commercial".to_string()), ("mature", "false".to_string())];
    p.push(("page_size", req.count.clamp(1, 40).to_string()));
    p.push(("page", req.page.to_string()));
    Some(with_query(&format!("https://api.openverse.org/v1/{path}/"), &p))
}

/// What Openverse says the licence is, read into what it allows. `None` for a licence that cannot
/// be used in an edit (no derivatives, or non-commercial).
fn licence(v: &Value) -> Option<Rights> {
    let code = text(v, "license", 20).to_ascii_lowercase();
    let version = text(v, "license_version", 10);
    if code.split('-').any(|part| part == "nd" || part == "nc") {
        return None;
    }
    let address = url(v, "license_url");
    let address = if !address.is_empty() {
        address
    } else {
        match code.as_str() {
            "" => String::new(),
            "cc0" => "https://creativecommons.org/publicdomain/zero/1.0/".to_string(),
            "pdm" => "https://creativecommons.org/publicdomain/mark/1.0/".to_string(),
            c if c.starts_with("by") && !version.is_empty() && version.chars().all(|ch| ch.is_ascii_digit() || ch == '.') => {
                format!("https://creativecommons.org/licenses/{c}/{version}/")
            }
            _ => String::new(),
        }
    };
    let mut r = crate::cc::rights(&address);
    if r.name.is_empty() && !code.is_empty() {
        // an address we could not read: say what the site said, and claim nothing
        r = Rights { name: format!("CC {}", code.to_ascii_uppercase()), url: String::new(), commercial: None, attribution_required: None };
    }
    Some(r)
}

/// The file type a result is saved as: its stated type, else the end of its address.
fn extension(v: &Value, file: &str, kind: Kind) -> Option<String> {
    let stated = text(v, "filetype", 10).to_ascii_lowercase();
    let last = file.split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("");
    let tail = last.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let ext = if !stated.is_empty() { stated } else { tail };
    // Jamendo calls its streams `mp31` and `mp32`; they are mp3 files
    let ext = match ext.as_str() {
        "jpeg" => "jpg".to_string(),
        "mp31" | "mp32" => "mp3".to_string(),
        _ => ext,
    };
    let ok: &[&str] = match kind {
        Kind::Image => &["jpg", "png", "webp", "gif"],
        Kind::Audio => &["mp3", "wav", "ogg", "flac", "m4a"],
        Kind::Video => &[],
    };
    if ok.contains(&ext.as_str()) {
        Some(ext)
    } else if kind == Kind::Image && ext.is_empty() {
        Some("jpg".to_string())
    } else {
        None
    }
}

fn one(v: &Value, kind: Kind) -> Option<Candidate> {
    let id = text(v, "id", 80);
    // an Openverse id is a UUID; anything else is not one of its results
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return None;
    }
    if v.get("mature").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let rights = licence(v)?;
    let file = url(v, "url");
    if file.is_empty() {
        return None;
    }
    let ext = extension(v, &file, kind)?;
    let mut tags: Vec<String> = Vec::new();
    let provider = text(v, "provider", 40).to_ascii_lowercase();
    if !provider.is_empty() {
        tags.push(provider);
    }
    for g in v.get("genres").and_then(Value::as_array).into_iter().flatten().take(4) {
        if let Some(s) = g.as_str() {
            tags.push(s.chars().filter(|c| !c.is_control()).take(30).collect::<String>().to_lowercase());
        }
    }
    for t in v.get("tags").and_then(Value::as_array).into_iter().flatten().take(8) {
        let name = text(t, "name", 30).to_lowercase();
        if !name.is_empty() && !tags.contains(&name) {
            tags.push(name);
        }
    }
    tags.truncate(12);
    let title = text(v, "title", 200);
    Some(Candidate {
        id: format!("{ID}:{id}"),
        source: ID.into(),
        kind,
        title: if title.is_empty() { "Untitled".into() } else { title },
        author: text(v, "creator", 100),
        author_url: url(v, "creator_url"),
        source_url: url(v, "foreign_landing_url"),
        thumb: if kind == Kind::Image { url(v, "thumbnail") } else { String::new() },
        previews: Vec::new(),
        width: crate::util::dim(v, "width"),
        height: crate::util::dim(v, "height"),
        // Openverse gives a length in milliseconds
        seconds: uint(v, "duration").map(|ms| ms as f64 / 1000.0).filter(|s| *s < 86_400.0),
        tags,
        rights,
        download: Some(Download {
            url: file,
            ext,
            width: crate::util::dim(v, "width"),
            height: crate::util::dim(v, "height"),
            bytes: uint(v, "filesize"),
            note: String::new(),
            resolve: false,
        }),
    })
}

pub fn parse(kind: Kind, body: &[u8]) -> Result<Page, SourceError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| SourceError::bad("Openverse answered with something that is not JSON"))?;
    let results = v.get("results").and_then(Value::as_array).ok_or_else(|| SourceError::bad("Openverse's answer had no results list"))?;
    Ok(Page { candidates: results.iter().take(100).filter_map(|r| one(r, kind)).collect(), total: uint(&v, "result_count").unwrap_or(0) })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOUND: &str = r#"{"result_count":240,"page_count":12,"results":[
      {"id":"6b072076-066b-45b6-9695-367a6260c96d","title":"Rain, Moderate, C.wav","foreign_landing_url":"https://freesound.org/people/InspectorJ/sounds/401275",
       "url":"https://cdn.freesound.org/previews/401/401275_5121236-hq.mp3","creator":"InspectorJ","creator_url":"https://freesound.org/people/InspectorJ",
       "license":"by","license_version":"4.0","license_url":"https://creativecommons.org/licenses/by/4.0/","provider":"freesound","genres":null,"filesize":1379890,
       "filetype":"mp3","tags":[{"name":"Rain"},{"name":"Weather"}],"duration":60116,"thumbnail":null,"mature":false},
      {"id":"11111111-2222-3333-4444-555555555555","title":"Slow piano","url":"https://cdn.example/jamendo/1.mp3","creator":"Pia","license":"by-sa","license_version":"3.0",
       "provider":"jamendo","genres":["classical","ambient"],"filetype":"mp3","duration":180000,"tags":[]},
      {"id":"88888888-2222-3333-4444-555555555555","title":"Jamendo stream","url":"https://prod-1.storage.jamendo.com/?trackid=23557&format=mp32","license":"by","license_version":"3.0","provider":"jamendo","filetype":"mp32","duration":228000},
      {"id":"22222222-2222-3333-4444-555555555555","title":"No changes allowed","url":"https://cdn.example/nd.mp3","license":"by-nd","license_version":"4.0","filetype":"mp3"},
      {"id":"33333333-2222-3333-4444-555555555555","title":"Not for business","url":"https://cdn.example/nc.mp3","license":"by-nc-sa","license_version":"4.0","filetype":"mp3"},
      {"id":"44444444-2222-3333-4444-555555555555","title":"Zero","url":"https://cdn.example/z.ogg","license":"cc0","license_version":"1.0","filetype":"ogg","duration":4500},
      {"id":"55555555-2222-3333-4444-555555555555","title":"Adult","url":"https://cdn.example/a.mp3","license":"by","license_version":"4.0","filetype":"mp3","mature":true},
      {"id":"not a uuid","title":"x","url":"https://cdn.example/x.mp3","license":"by","filetype":"mp3"},
      {"id":"66666666-2222-3333-4444-555555555555","title":"Odd file","url":"https://cdn.example/x.exe","license":"by","license_version":"4.0","filetype":"exe"},
      {"id":"77777777-2222-3333-4444-555555555555","title":"Bad address","url":"javascript:alert(1)","license":"by","license_version":"4.0","filetype":"mp3"}]}"#;

    #[test]
    fn sound_results_carry_licence_credit_length_and_the_unusable_ones_are_dropped() {
        let p = parse(Kind::Audio, SOUND.as_bytes()).unwrap();
        let ids: Vec<&str> = p.candidates.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "openverse:6b072076-066b-45b6-9695-367a6260c96d",
                "openverse:11111111-2222-3333-4444-555555555555",
                "openverse:88888888-2222-3333-4444-555555555555",
                "openverse:44444444-2222-3333-4444-555555555555"
            ],
            "no-derivatives, non-commercial, adult, odd file types, bad ids and bad addresses are never offered"
        );
        assert_eq!(p.total, 240);
        let rain = &p.candidates[0];
        assert_eq!((rain.author.as_str(), rain.seconds, rain.kind), ("InspectorJ", Some(60.116), Kind::Audio));
        assert_eq!((rain.rights.name.as_str(), rain.rights.commercial, rain.rights.attribution_required), ("CC BY 4.0", Some(true), Some(true)));
        assert_eq!(rain.source_url, "https://freesound.org/people/InspectorJ/sounds/401275");
        assert_eq!(rain.tags, ["freesound", "rain", "weather"]);
        assert_eq!(rain.download.as_ref().map(|d| (d.ext.as_str(), d.bytes, d.resolve)), Some(("mp3", Some(1_379_890), false)));
        assert_eq!(rain.thumb, "", "sound has no picture");
        let piano = &p.candidates[1];
        assert_eq!((piano.rights.name.as_str(), piano.rights.attribution_required), ("CC BY-SA 3.0", Some(true)), "share-alike is offered, named as such");
        assert_eq!(piano.tags, ["jamendo", "classical", "ambient"]);
        let jamendo = &p.candidates[2];
        assert_eq!((jamendo.download.as_ref().map(|d| d.ext.as_str()), jamendo.seconds), (Some("mp3"), Some(228.0)), "Jamendo's mp32 is an mp3");
        let zero = &p.candidates[3];
        assert_eq!((zero.rights.name.as_str(), zero.rights.attribution_required, zero.seconds), ("CC0 1.0", Some(false), Some(4.5)));
    }

    #[test]
    fn pictures_keep_their_size_and_thumbnail_and_a_licence_without_an_address_is_built_or_claims_nothing() {
        let body = r#"{"result_count":3,"results":[
          {"id":"30c9a758-2838-422a-a206-a5eda5af6294","title":"Fog","foreign_landing_url":"https://www.flickr.com/photos/1/2","url":"https://live.staticflickr.com/3204/x_b.jpg",
           "creator":"misteraitch","license":"by","license_version":"2.0","license_url":"https://creativecommons.org/licenses/by/2.0/","provider":"flickr",
           "height":685,"width":1024,"thumbnail":"https://api.openverse.org/v1/images/30c9a758/thumb/","tags":[{"name":"fog"}]},
          {"id":"aaaaaaaa-2838-422a-a206-a5eda5af6294","title":"","url":"https://example.org/p","license":"by-sa","license_version":"4.0","width":10,"height":10},
          {"id":"bbbbbbbb-2838-422a-a206-a5eda5af6294","title":"Mystery","url":"https://example.org/q.png","license":"zzz"}]}"#;
        let p = parse(Kind::Image, body.as_bytes()).unwrap();
        assert_eq!(p.candidates.len(), 3);
        let fog = &p.candidates[0];
        assert_eq!((fog.width, fog.height, fog.thumb.as_str()), (Some(1024), Some(685), "https://api.openverse.org/v1/images/30c9a758/thumb/"));
        let built = &p.candidates[1];
        assert_eq!(
            (built.title.as_str(), built.rights.name.as_str(), built.rights.url.as_str()),
            ("Untitled", "CC BY-SA 4.0", "https://creativecommons.org/licenses/by-sa/4.0/")
        );
        assert_eq!(built.download.as_ref().map(|d| d.ext.as_str()), Some("jpg"), "a picture with no stated type is saved as jpg");
        let unknown = &p.candidates[2];
        assert_eq!((unknown.rights.name.as_str(), unknown.rights.commercial, unknown.rights.attribution_required), ("CC ZZZ", None, None));
    }

    #[test]
    fn empty_answers_and_garbage() {
        assert!(parse(Kind::Image, br#"{"result_count":0,"results":[]}"#).unwrap().candidates.is_empty());
        for bad in ["nope", "{}", r#"{"results":3}"#] {
            assert!(parse(Kind::Audio, bad.as_bytes()).is_err(), "{bad}");
        }
        assert!(parse(Kind::Audio, br#"{"results":[5,null,{}]}"#).unwrap().candidates.is_empty());
    }

    #[test]
    fn the_address_asks_for_the_right_kind_commercial_use_only_and_nothing_for_video() {
        let req = Request { text: "rain on a window".into(), count: 7, page: 2, ..Request::default() };
        let u = search_url(&req, Kind::Audio).unwrap();
        assert!(u.starts_with("https://api.openverse.org/v1/audio/?q=rain%20window&license_type=commercial&mature=false&page_size=7&page=2"), "{u}");
        assert!(search_url(&req, Kind::Image).unwrap().contains("/v1/images/"));
        assert!(search_url(&req, Kind::Video).is_none());
        assert!(search_url(&Request { text: "  ".into(), ..Request::default() }, Kind::Image).is_none());
    }
}
