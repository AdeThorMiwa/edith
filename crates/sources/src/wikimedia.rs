//! Wikimedia Commons (`https://commons.wikimedia.org/w/api.php`): millions of freely licensed
//! pictures, plus some video and audio. No key. Every file has its own licence (public domain, CC0,
//! CC BY, CC BY-SA, …) in its metadata, read here into what it allows; most ask for a credit, and
//! the author's name is taken from the file's page.

use crate::types::{Candidate, Download, Kind, Page, Request, Rights, SourceError};
use crate::util::{dim, seconds, text, uint, url, with_query};
use serde_json::Value;

pub const ID: &str = "wikimedia";

/// The width asked of Commons for pictures and video posters. A picture is downloaded at this
/// size (a small, ready-to-use file) when the original is larger.
const WIDE: u32 = 1920;

/// The address to ask for `kind`.
pub fn search_url(req: &Request, kind: Kind) -> Option<String> {
    let query = crate::query::keywords(&req.text, 8, 100);
    if query.is_empty() {
        return None;
    }
    let filter = match kind {
        Kind::Image => "filetype:bitmap|drawing",
        Kind::Video => "filetype:video",
        Kind::Audio => "filetype:audio",
    };
    let n = req.count.clamp(1, 40);
    Some(with_query(
        "https://commons.wikimedia.org/w/api.php",
        &[
            ("action", "query".into()),
            ("format", "json".into()),
            ("formatversion", "2".into()),
            ("generator", "search".into()),
            ("gsrnamespace", "6".into()),
            ("gsrsearch", format!("{query} {filter}")),
            ("gsrlimit", n.to_string()),
            ("gsroffset", (req.page.saturating_sub(1).saturating_mul(n)).to_string()),
            ("prop", "imageinfo".into()),
            ("iiprop", "url|size|mime|mediatype|extmetadata".into()),
            ("iiurlwidth", WIDE.to_string()),
            ("iiextmetadatafilter", "LicenseShortName|LicenseUrl|Artist|AttributionRequired|UsageTerms|Copyrighted".into()),
        ],
    ))
}

/// Text with the HTML Commons puts in author and credit fields taken out.
pub fn plain(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            _ => out.push(c),
        }
    }
    let out = out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&nbsp;", " ");
    out.split_whitespace().collect::<Vec<_>>().join(" ").chars().filter(|c| !c.is_control()).take(150).collect()
}

fn meta(info: &Value, key: &str) -> String {
    info.get("extmetadata").and_then(|m| m.get(key)).map(|v| text(v, "value", 1000)).unwrap_or_default()
}

fn rights(info: &Value) -> Rights {
    let short = plain(&meta(info, "LicenseShortName"));
    let addr = url(info.get("extmetadata").and_then(|m| m.get("LicenseUrl")).unwrap_or(&Value::Null), "value");
    let mut r = crate::cc::rights(if addr.is_empty() { &short } else { &addr });
    if r.commercial.is_none() {
        // the short name alone ("CC BY-SA 4.0"), when the address was missing or odd
        r = crate::cc::rights(&short);
    }
    if !short.is_empty() && r.commercial.is_some() {
        r.name = short.clone();
    }
    // Commons says outright whether a credit is required
    match meta(info, "AttributionRequired").to_ascii_lowercase().as_str() {
        "true" => r.attribution_required = Some(true),
        "false" => r.attribution_required = Some(false),
        _ => {}
    }
    if r.commercial.is_none() && !short.is_empty() {
        r.name = short;
    }
    r
}

/// A smaller picture from the same server: the grid shows 320 px wide thumbnails, not the 1920 px one.
fn small(thumb: &str) -> String {
    let marker = format!("/{WIDE}px-");
    if thumb.contains(&marker) { thumb.replacen(&marker, "/320px-", 1) } else { thumb.to_string() }
}

fn extension(u: &str) -> String {
    let path = u.split(['?', '#']).next().unwrap_or("");
    path.rsplit('.').next().unwrap_or("").to_ascii_lowercase()
}

fn one(page: &Value) -> Option<Candidate> {
    let pageid = uint(page, "pageid")?;
    let info = page.get("imageinfo").and_then(Value::as_array).and_then(|a| a.first())?;
    let kind = match info.get("mediatype").and_then(Value::as_str)? {
        "BITMAP" | "DRAWING" => Kind::Image,
        "VIDEO" => Kind::Video,
        "AUDIO" => Kind::Audio,
        _ => return None,
    };
    let (original, thumb) = (url(info, "url"), url(info, "thumburl"));
    // pictures come at a ready-to-use size; video and sound as they are on Commons
    let file = if kind == Kind::Image && !thumb.is_empty() { thumb.clone() } else { original.clone() };
    let ext = extension(&file);
    let allowed: &[&str] = match kind {
        Kind::Image => &["jpg", "jpeg", "png", "gif", "webp"],
        Kind::Video => &["webm", "mp4", "mov", "mkv"],
        Kind::Audio => &["ogg", "oga", "opus", "mp3", "wav", "flac"],
    };
    if file.is_empty() || !allowed.contains(&ext.as_str()) {
        return None;
    }
    let title = {
        let t = text(page, "title", 200);
        let t = t.strip_prefix("File:").unwrap_or(&t).replace('_', " ");
        match t.rsplit_once('.') {
            Some((stem, e)) if e.len() <= 5 => stem.to_string(),
            _ => t,
        }
    };
    let artist = plain(&meta(info, "Artist"));
    let (w, h) =
        if kind == Kind::Image && !thumb.is_empty() { (dim(info, "thumbwidth"), dim(info, "thumbheight")) } else { (dim(info, "width"), dim(info, "height")) };
    Some(Candidate {
        id: format!("{ID}:{pageid}"),
        source: ID.into(),
        kind,
        title,
        author: artist,
        author_url: String::new(),
        source_url: url(info, "descriptionurl"),
        thumb: if kind == Kind::Audio || thumb.is_empty() { String::new() } else { small(&thumb) },
        width: dim(info, "width"),
        height: dim(info, "height"),
        seconds: seconds(info, "duration"),
        tags: Vec::new(),
        rights: rights(info),
        download: Some(Download {
            url: file,
            ext: if ext == "jpeg" { "jpg".into() } else { ext },
            width: w,
            height: h,
            bytes: if kind == Kind::Image { None } else { uint(info, "size") },
            note: String::new(),
            resolve: false,
        }),
    })
}

pub fn parse(body: &[u8]) -> Result<Page, SourceError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| SourceError::bad("Wikimedia Commons answered with something that is not JSON"))?;
    // a search with no matches has no `query` at all
    let Some(pages) = v.get("query").and_then(|q| q.get("pages")).and_then(Value::as_array) else {
        return if v.get("error").is_some() { Err(SourceError::bad("Wikimedia Commons reported an error")) } else { Ok(Page::default()) };
    };
    // the pages come in an unspecified order; `index` is the search rank
    let mut ranked: Vec<(u64, &Value)> = pages.iter().map(|p| (uint(p, "index").unwrap_or(u64::MAX), p)).collect();
    ranked.sort_by_key(|(i, _)| *i);
    let candidates: Vec<Candidate> = ranked.into_iter().take(50).filter_map(|(_, p)| one(p)).collect();
    Ok(Page { total: candidates.len() as u64, candidates })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = r#"{"batchcomplete":true,"query":{"pages":[
      {"pageid":222,"ns":6,"title":"File:Fog on the lake.jpg","index":2,"imagerepository":"local","imageinfo":[{
        "size":9000000,"width":6000,"height":4000,"url":"https://upload.wikimedia.org/wikipedia/commons/a/ab/Fog_on_the_lake.jpg",
        "thumburl":"https://upload.wikimedia.org/wikipedia/commons/thumb/a/ab/Fog_on_the_lake.jpg/1920px-Fog_on_the_lake.jpg","thumbwidth":1920,"thumbheight":1280,
        "descriptionurl":"https://commons.wikimedia.org/wiki/File:Fog_on_the_lake.jpg","mime":"image/jpeg","mediatype":"BITMAP",
        "extmetadata":{"LicenseShortName":{"value":"CC BY-SA 4.0"},"LicenseUrl":{"value":"https://creativecommons.org/licenses/by-sa/4.0"},
                       "Artist":{"value":"<a href=\"//commons.wikimedia.org/wiki/User:J\" title=\"User:J\">Jane &amp; Co</a>"},"AttributionRequired":{"value":"true"}}}]},
      {"pageid":111,"ns":6,"title":"File:Tule Fog California - 2005.jpg","index":1,"imageinfo":[{
        "size":1800000,"width":2400,"height":3200,"url":"https://upload.wikimedia.org/wikipedia/commons/9/92/Tule_Fog.jpg",
        "thumburl":"https://upload.wikimedia.org/wikipedia/commons/thumb/9/92/Tule_Fog.jpg/1920px-Tule_Fog.jpg","thumbwidth":1440,"thumbheight":1920,
        "descriptionurl":"https://commons.wikimedia.org/wiki/File:Tule_Fog.jpg","mime":"image/jpeg","mediatype":"BITMAP",
        "extmetadata":{"LicenseShortName":{"value":"Public domain"},"Artist":{"value":"Jeff Schmaltz; NASA"},"AttributionRequired":{"value":"false"},"Copyrighted":{"value":"False"}}}]},
      {"pageid":333,"ns":6,"title":"File:Morning mist.webm","index":3,"imageinfo":[{
        "size":22000000,"width":1920,"height":1080,"duration":31.5,"url":"https://upload.wikimedia.org/wikipedia/commons/1/1a/Morning_mist.webm",
        "thumburl":"https://upload.wikimedia.org/wikipedia/commons/thumb/1/1a/Morning_mist.webm/1920px--Morning_mist.webm.jpg","descriptionurl":"https://commons.wikimedia.org/wiki/File:Morning_mist.webm",
        "mime":"video/webm","mediatype":"VIDEO","extmetadata":{"LicenseShortName":{"value":"CC0"},"Artist":{"value":"A"}}}]},
      {"pageid":444,"ns":6,"title":"File:Foghorn.ogg","index":4,"imageinfo":[{"size":900000,"url":"https://upload.wikimedia.org/wikipedia/commons/4/44/Foghorn.ogg","duration":8,
        "descriptionurl":"https://commons.wikimedia.org/wiki/File:Foghorn.ogg","mime":"application/ogg","mediatype":"AUDIO","extmetadata":{"LicenseShortName":{"value":"CC BY 3.0"}}}]},
      {"pageid":555,"title":"File:Diagram.svg","index":5,"imageinfo":[{"url":"https://upload.wikimedia.org/x.svg","thumburl":"https://upload.wikimedia.org/thumb/x.svg/1920px-x.svg.png","mediatype":"DRAWING","extmetadata":{"LicenseShortName":{"value":"GFDL"}}}]},
      {"pageid":666,"title":"File:Scan.tif","index":6,"imageinfo":[{"url":"https://upload.wikimedia.org/x.tif","mediatype":"BITMAP"}]},
      {"pageid":777,"title":"File:Doc.pdf","index":7,"imageinfo":[{"url":"https://upload.wikimedia.org/x.pdf","mediatype":"OFFICE"}]},
      {"nopageid":1}]}}"#;

    #[test]
    fn results_are_put_in_search_order_with_licence_author_and_file_chosen_per_kind() {
        let p = parse(ANSWER.as_bytes()).unwrap();
        let ids: Vec<&str> = p.candidates.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["wikimedia:111", "wikimedia:222", "wikimedia:333", "wikimedia:444", "wikimedia:555"], "ranked by the search, tiff and pdf left out");
        let pd = &p.candidates[0];
        assert_eq!((pd.title.as_str(), pd.author.as_str(), pd.rights.name.as_str()), ("Tule Fog California - 2005", "Jeff Schmaltz; NASA", "Public domain"));
        assert_eq!((pd.rights.commercial, pd.rights.attribution_required), (Some(true), Some(false)));
        let sa = &p.candidates[1];
        assert_eq!(
            (sa.author.as_str(), sa.rights.name.as_str(), sa.rights.commercial, sa.rights.attribution_required),
            ("Jane & Co", "CC BY-SA 4.0", Some(true), Some(true))
        );
        assert_eq!(sa.rights.url, "https://creativecommons.org/licenses/by-sa/4.0");
        assert!(sa.download.as_ref().unwrap().url.contains("/1920px-"), "a big picture is fetched at 1920 px, not at 9 MB");
        assert!(sa.thumb.contains("/320px-"), "{}", sa.thumb);
        assert_eq!(sa.source_url, "https://commons.wikimedia.org/wiki/File:Fog_on_the_lake.jpg");
    }

    #[test]
    fn video_and_sound_keep_their_originals_and_odd_kinds_are_skipped() {
        let p = parse(ANSWER.as_bytes()).unwrap();
        let v = &p.candidates[2];
        assert_eq!(
            (v.kind, v.seconds, v.download.as_ref().unwrap().ext.as_str(), v.download.as_ref().unwrap().bytes),
            (Kind::Video, Some(31.5), "webm", Some(22_000_000))
        );
        assert!(v.download.as_ref().unwrap().url.ends_with("Morning_mist.webm") && v.thumb.contains("/320px-"));
        assert_eq!(v.rights.name, "CC0 1.0".replace(" 1.0", ""), "the site's own short name is shown");
        let a = &p.candidates[3];
        assert_eq!((a.kind, a.download.as_ref().unwrap().ext.as_str(), a.thumb.as_str()), (Kind::Audio, "ogg", ""));
        // a drawing comes as a png, and a licence we do not model claims nothing
        let d = &p.candidates[4];
        assert_eq!(d.download.as_ref().unwrap().ext, "png");
        assert_eq!((d.rights.name.as_str(), d.rights.commercial), ("GFDL", None));
    }

    #[test]
    fn no_matches_and_errors_and_garbage() {
        assert!(parse(br#"{"batchcomplete":true}"#).unwrap().candidates.is_empty());
        assert!(parse(br#"{"error":{"code":"x"}}"#).is_err());
        assert!(parse(b"nope").is_err());
        assert!(parse(br#"{"query":{"pages":[5,null,{"pageid":1}]}}"#).unwrap().candidates.is_empty());
    }

    #[test]
    fn html_in_names_is_removed_and_the_address_asks_for_the_right_kind() {
        assert_eq!(plain("<a href=\"x\">A &amp; B</a> <b>and</b>\n  C&nbsp;D"), "A & B and C D");
        assert_eq!(plain(&"x".repeat(1000)).len(), 150);
        let req = Request { text: "fog over a valley".into(), count: 5, page: 3, ..Request::default() };
        let u = search_url(&req, Kind::Video).unwrap();
        assert!(
            u.contains("gsrsearch=fog%20valley%20filetype%3Avideo") && u.contains("gsrlimit=5") && u.contains("gsroffset=10") && u.contains("iiurlwidth=1920"),
            "{u}"
        );
        assert!(search_url(&req, Kind::Image).unwrap().contains("filetype%3Abitmap%7Cdrawing"));
        assert!(search_url(&Request { text: " ".into(), ..Request::default() }, Kind::Image).is_none());
    }
}
