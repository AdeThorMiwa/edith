//! Freesound (`https://freesound.org/docs/api/`): sounds under Creative Commons licences. Each
//! sound has its own licence (CC0, CC BY, CC BY-NC, …), read here into what it allows. The key goes
//! in the `Authorization` header. Fetching a sound's **original file** needs the person to sign in
//! to Freesound (OAuth2); a key alone can fetch the **high-quality preview** (an MP3), which is
//! what is downloaded, and each candidate says so.

use crate::types::{Candidate, Download, Kind, Page, Request, Rights, SourceError};
use crate::util::{seconds, tags, text, uint, url, with_query};
use serde_json::Value;

pub const ID: &str = "freesound";

/// What a Freesound licence (a name or a Creative Commons address) allows.
pub fn rights_of(license: &str) -> Rights {
    crate::cc::rights(license)
}

/// The address to ask for sounds; `None` when the request does not want audio.
pub fn search_url(req: &Request) -> Option<String> {
    if !req.wants(Kind::Audio) {
        return None;
    }
    let query = crate::query::keywords(&req.text, 8, 150);
    if query.is_empty() {
        return None;
    }
    let mut p = vec![
        ("query", query),
        ("page_size", req.count.clamp(1, 40).to_string()),
        ("page", req.page.to_string()),
        ("fields", "id,name,username,license,url,previews,duration,tags,images".to_string()),
    ];
    if req.min_seconds.is_some() || req.max_seconds.is_some() {
        let f = |v: Option<f64>| v.map_or("*".to_string(), |s| format!("{s:.0}"));
        p.push(("filter", format!("duration:[{} TO {}]", f(req.min_seconds), f(req.max_seconds))));
    }
    Some(with_query("https://freesound.org/apiv2/search/", &p))
}

fn one(v: &Value) -> Option<Candidate> {
    let id = uint(v, "id")?;
    let previews = v.get("previews").unwrap_or(&Value::Null);
    let preview = ["preview-hq-mp3", "preview-lq-mp3"].iter().map(|k| url(previews, k)).find(|u| !u.is_empty());
    let images = v.get("images").unwrap_or(&Value::Null);
    let license = v.get("license").and_then(Value::as_str).unwrap_or("");
    let author = text(v, "username", 100);
    Some(Candidate {
        id: format!("{ID}:{id}"),
        source: ID.into(),
        kind: Kind::Audio,
        title: text(v, "name", 150),
        author_url: if author.is_empty() { String::new() } else { format!("https://freesound.org/people/{}/", crate::util::encode(&author)) },
        author,
        source_url: url(v, "url"),
        thumb: ["waveform_m", "waveform_l", "spectral_m"].iter().map(|k| url(images, k)).find(|u| !u.is_empty()).unwrap_or_default(),
        width: None,
        height: None,
        seconds: seconds(v, "duration"),
        tags: v
            .get("tags")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(|t| t.to_lowercase()).take(12).collect())
            .unwrap_or_else(|| tags(&text(v, "tags", 300))),
        rights: rights_of(license),
        download: preview.map(|u| Download {
            url: u,
            ext: "mp3".into(),
            width: None,
            height: None,
            bytes: None,
            note: "Preview quality (MP3). The original file needs a Freesound sign-in.".into(),
            resolve: false,
        }),
    })
}

pub fn parse(body: &[u8]) -> Result<Page, SourceError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| SourceError::bad("Freesound answered with something that is not JSON"))?;
    let list = v.get("results").and_then(Value::as_array).ok_or_else(|| SourceError::bad("Freesound's answer had no results list"))?;
    Ok(Page { candidates: list.iter().take(150).filter_map(one).collect(), total: uint(&v, "count").unwrap_or(0) })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = r#"{"count":321,"next":null,"results":[
      {"id":1234,"name":"Fog horn at dawn.wav","username":"sound man","license":"https://creativecommons.org/licenses/by/4.0/","url":"https://freesound.org/people/soundman/sounds/1234/",
       "duration":8.5,"tags":["fog","Horn","harbour"],"previews":{"preview-hq-mp3":"https://cdn.freesound.org/previews/1/1234_hq.mp3","preview-lq-mp3":"https://cdn.freesound.org/previews/1/1234_lq.mp3"},
       "images":{"waveform_m":"https://cdn.freesound.org/displays/1/1234_wave_M.png"}},
      {"id":5,"name":"Zero","username":"z","license":"http://creativecommons.org/publicdomain/zero/1.0/","duration":1,"previews":{"preview-lq-mp3":"https://cdn.freesound.org/q.mp3"}},
      {"id":6,"name":"NC","username":"n","license":"http://creativecommons.org/licenses/by-nc/3.0/","duration":2,"previews":{}},
      {"id":7,"name":"Odd","license":"Sampling+","duration":"x"},
      {"nope":1}]}"#;

    #[test]
    fn licences_are_read_into_what_they_allow() {
        let r = |s: &str| {
            let r = rights_of(s);
            (r.name, r.commercial, r.attribution_required)
        };
        assert_eq!(r("https://creativecommons.org/licenses/by/4.0/"), ("CC BY 4.0".into(), Some(true), Some(true)));
        assert_eq!(r("http://creativecommons.org/publicdomain/zero/1.0/"), ("CC0 1.0".into(), Some(true), Some(false)));
        assert_eq!(r("http://creativecommons.org/licenses/by-nc/3.0/"), ("CC BY-NC 3.0".into(), Some(false), Some(true)));
        assert_eq!(r("Attribution"), ("CC BY".into(), Some(true), Some(true)));
        assert_eq!(r("Creative Commons 0"), ("CC0 1.0".into(), Some(true), Some(false)));
        assert_eq!(r("Attribution NonCommercial"), ("CC BY-NC".into(), Some(false), Some(true)));
        // anything not understood claims nothing: the audit will call it a risk
        assert_eq!(r("Sampling+"), ("Sampling+".into(), None, None));
        assert_eq!(r(""), (String::new(), None, None));
    }

    #[test]
    fn an_answer_becomes_audio_candidates_downloading_the_preview() {
        let p = parse(ANSWER.as_bytes()).unwrap();
        assert_eq!((p.total, p.candidates.len()), (321, 4));
        let c = &p.candidates[0];
        assert_eq!((c.id.as_str(), c.title.as_str(), c.author.as_str(), c.seconds), ("freesound:1234", "Fog horn at dawn.wav", "sound man", Some(8.5)));
        assert_eq!(c.rights.name, "CC BY 4.0");
        assert_eq!(c.author_url, "https://freesound.org/people/sound%20man/");
        assert_eq!(c.tags, ["fog", "horn", "harbour"]);
        let d = c.download.as_ref().unwrap();
        assert_eq!((d.url.as_str(), d.ext.as_str()), ("https://cdn.freesound.org/previews/1/1234_hq.mp3", "mp3"));
        assert!(d.note.contains("Preview quality") && c.thumb.ends_with("_wave_M.png"));
        // the low-quality preview is the fallback; no preview means nothing to download
        assert!(p.candidates[1].download.as_ref().unwrap().url.ends_with("q.mp3"));
        assert!(p.candidates[2].download.is_none());
        assert_eq!((p.candidates[3].rights.commercial, p.candidates[3].seconds), (None, None));
        for bad in ["nope", "{}", r#"{"results":3}"#] {
            assert!(parse(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_address_asks_for_the_fields_needed_and_the_length_range() {
        let req = Request {
            text: "fog horn at dawn".into(),
            kinds: vec![Kind::Audio],
            count: 5,
            min_seconds: Some(3.0),
            max_seconds: Some(30.0),
            ..Request::default()
        };
        let u = search_url(&req).unwrap();
        assert!(u.starts_with("https://freesound.org/apiv2/search/?query=fog%20horn%20dawn&page_size=5&page=1&fields=id%2Cname"), "{u}");
        assert!(u.contains("filter=duration%3A%5B3%20TO%2030%5D"), "{u}");
        let open = search_url(&Request { min_seconds: Some(3.0), max_seconds: None, ..req.clone() }.sanitized()).unwrap();
        assert!(open.contains("%5B3%20TO%20%2A%5D"), "{open}");
        assert!(search_url(&Request { kinds: vec![Kind::Video], ..req }).is_none(), "no audio wanted, no request");
    }
}
