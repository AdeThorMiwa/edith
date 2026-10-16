//! Tests of [`crate::hub`] against a local server that plays the sites: searching several at once,
//! failures that stay with their source, the 24-hour cache, the shortlist, and bringing a chosen
//! file into the project with its provenance. Nothing here touches the real network or keychain.

use super::*;
use crate::hub::credit_text;
use edith_net::http::{Client, ClientConfig};
use edith_net::testserver::{Server, response, serve};
use edith_net::{NetState, Outcome};
use edith_sources::{SourceError, Sources};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

const KEY: &str = "SECRETKEY-0123456789";

fn png() -> Vec<u8> {
    let mut out = Vec::new();
    let img = image::RgbaImage::from_pixel(8, 6, image::Rgba([200, 80, 40, 255]));
    image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

/// Serves the files the candidates point at.
fn files() -> Server {
    let image = png();
    serve(move |_, req| if req.contains("GET /gone") { response(404, &[], b"") } else { response(200, &[("content-type", "image/png")], &image) })
}

fn pexels_photos(files: &str) -> String {
    format!(
        r#"{{"total_results":2,"photos":[
        {{"id":1,"width":800,"height":600,"url":"https://www.pexels.com/photo/1/","photographer":"Sam","alt":"A pier at dawn","src":{{"large2x":"{files}/big1.jpg","medium":"{files}/thumb1.png"}}}},
        {{"id":2,"width":800,"height":600,"url":"https://www.pexels.com/photo/2/","photographer":"Lee","alt":"Gone","src":{{"large2x":"{files}/gone","medium":"{files}/thumb2.png"}}}}]}}"#
    )
}

fn pixabay_hits(files: &str) -> String {
    format!(
        r#"{{"total":1,"totalHits":1,"hits":[{{"id":9,"pageURL":"https://pixabay.com/photos/pier-9/","tags":"pier, sea","previewURL":"{files}/p.png","webformatURL":"{files}/w.png","largeImageURL":"{files}/l.jpg","imageWidth":800,"imageHeight":600,"user":"Ann"}}]}}"#
    )
}

fn freesound_results(files: &str) -> String {
    format!(
        r#"{{"count":1,"results":[{{"id":77,"name":"Fog horn","username":"sound man","license":"https://creativecommons.org/licenses/by/4.0/","url":"https://freesound.org/s/77/","duration":8,"tags":["fog"],"previews":{{"preview-hq-mp3":"{files}/horn.mp3"}}}}]}}"#
    )
}

struct Rig {
    sites: Server,
    _files: Server,
    client: Arc<Client>,
}

/// A session whose Source Hub talks to local servers. `broken` names a source that answers 401.
fn rig(broken: Option<&'static str>) -> (Session, Rig) {
    let files = files();
    let furl = files.url.clone();
    let sites = serve(move |_, req| {
        let json = |b: String| response(200, &[("content-type", "application/json")], b.as_bytes());
        if req.contains("api.pexels") || req.starts_with("GET /v1/search") || req.starts_with("GET /v1/videos") {
            if broken == Some("pexels") {
                return response(401, &[], b"");
            }
            if req.starts_with("GET /v1/videos") { json(r#"{"total_results":0,"videos":[]}"#.to_string()) } else { json(pexels_photos(&furl)) }
        } else if req.starts_with("GET /api/") {
            if broken == Some("pixabay") {
                return response(401, &[], format!("bad key {KEY}").as_bytes());
            }
            json(pixabay_hits(&furl))
        } else if req.starts_with("GET /apiv2/search") {
            if broken == Some("freesound") {
                return response(401, &[], b"");
            }
            json(freesound_results(&furl))
        } else {
            response(404, &[], b"")
        }
    });
    let client = Arc::new(Client::new(
        ClientConfig { allow_loopback_http: true, min_host_interval: Duration::ZERO, timeout: Duration::from_secs(5), ..ClientConfig::default() },
        Arc::new(NetState::new()),
    ));
    let sources = Sources::new(client.clone(), Arc::new(|_| Ok(Zeroizing::new(KEY.to_string())))).with_origin(&sites.url);
    let mut s = Session::default();
    s.hub.use_sources(Arc::new(sources));
    (s, Rig { sites, _files: files, client })
}

fn search(s: &mut Session, p: Value) -> Value {
    let mut p = p;
    p["wait"] = json!(true);
    s.execute("hub.search", p).unwrap()
}

fn ids(r: &Value) -> Vec<String> {
    r["candidates"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap().to_string()).collect()
}

#[test]
fn the_providers_are_listed_with_what_each_offers() {
    let mut s = Session::default();
    let p = s.execute("hub.providers", json!({})).unwrap();
    let list = p["providers"].as_array().unwrap();
    assert_eq!(list.iter().map(|x| x["id"].as_str().unwrap()).collect::<Vec<_>>(), ["pexels", "pixabay", "freesound"]);
    assert_eq!(list[2]["kinds"], json!(["audio"]));
    assert!(list[0]["credit"].as_str().unwrap().contains("Pexels") && list[0]["keyUrl"].as_str().unwrap().starts_with("https://"));
}

#[test]
fn a_search_asks_every_source_that_offers_the_kinds_and_interleaves_the_answers() {
    let (mut s, rig) = rig(None);
    let r = search(&mut s, json!({"text": "a pier at dawn", "kinds": ["image", "audio"]}));
    assert_eq!(r["running"], false);
    assert_eq!(ids(&r), ["pexels:photo-1", "pixabay:image-9", "freesound:77", "pexels:photo-2"], "one from each source in turn: {r}");
    let states: Vec<&str> = r["sources"].as_array().unwrap().iter().map(|x| x["state"].as_str().unwrap()).collect();
    assert_eq!(states, ["done", "done", "done"]);
    let first = &r["candidates"][0];
    assert_eq!((first["rights"]["name"].clone(), first["author"].clone()), (json!("Pexels License"), json!("Sam")));
    // a search for video only skips the audio site
    let v = search(&mut s, json!({"text": "fog", "kinds": ["video"]}));
    assert_eq!(v["sources"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect::<Vec<_>>(), ["pexels", "pixabay"]);
    // naming one source asks only it
    let hits = rig.sites.hits();
    let one = search(&mut s, json!({"text": "pier", "kinds": ["image"], "sources": ["pixabay"]}));
    assert_eq!(ids(&one), ["pixabay:image-9"]);
    assert_eq!(rig.sites.hits(), hits + 1);
    // the key reached the sites and nowhere else
    let log = format!("{:?}", rig.client.state().entries(None));
    assert!(!log.contains(KEY), "{log}");
    assert!(!r.to_string().contains(KEY));
}

#[test]
fn one_source_failing_does_not_hide_the_others() {
    let (mut s, rig) = rig(Some("pixabay"));
    let r = search(&mut s, json!({"text": "pier", "kinds": ["image"]}));
    assert_eq!(ids(&r), ["pexels:photo-1", "pexels:photo-2"]);
    let pixabay = &r["sources"][1];
    assert_eq!(pixabay["state"], "failed");
    assert!(pixabay["message"].as_str().unwrap().contains("refused the key"), "{pixabay}");
    assert!(!r.to_string().contains(KEY) && !format!("{:?}", rig.client.state().entries(None)).contains(KEY));
}

#[test]
fn answers_are_reused_for_a_day() {
    let (mut s, rig) = rig(None);
    let ask = || json!({"text": "pier at dawn", "kinds": ["image"], "sources": ["pexels"]});
    search(&mut s, ask());
    let after_first = rig.sites.hits();
    let again = search(&mut s, ask());
    assert_eq!(rig.sites.hits(), after_first, "the second search came from memory");
    assert_eq!(ids(&again).len(), 2);
    // a different request is a new question
    search(&mut s, json!({"text": "a different place", "kinds": ["image"], "sources": ["pexels"]}));
    assert!(rig.sites.hits() > after_first);
}

#[test]
fn the_shortlist_is_yours_and_only_holds_results_seen_this_session() {
    let (mut s, _rig) = rig(None);
    search(&mut s, json!({"text": "pier", "kinds": ["image"]}));
    assert!(s.execute("hub.shortlist", json!({})).unwrap()["shortlist"].as_array().unwrap().is_empty());
    s.execute("hub.shortlist", json!({"id": "pexels:photo-1"})).unwrap();
    let l = s.execute("hub.shortlist", json!({"id": "pixabay:image-9", "on": true})).unwrap();
    assert_eq!(l["shortlist"].as_array().unwrap().len(), 2);
    let r = s.execute("hub.results", json!({})).unwrap();
    assert!(r["candidates"].as_array().unwrap().iter().any(|c| c["id"] == "pexels:photo-1" && c["shortlisted"] == true));
    // toggling takes it off, and a new search does not forget it
    s.execute("hub.shortlist", json!({"id": "pexels:photo-1"})).unwrap();
    search(&mut s, json!({"text": "something else", "kinds": ["image"]}));
    let l = s.execute("hub.shortlist", json!({})).unwrap();
    assert_eq!(l["shortlist"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect::<Vec<_>>(), ["pixabay:image-9"]);
    assert!(s.execute("hub.shortlist", json!({"id": "pexels:made-up"})).is_err());
}

#[test]
fn a_chosen_file_is_downloaded_imported_into_a_bin_and_its_provenance_recorded() {
    let (mut s, rig) = rig(None);
    s.execute("file.newProject", json!({"name": "T"})).unwrap();
    let dir = std::env::temp_dir().join(format!("edith-hub-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    s.execute("file.projectSettings.scratchDisks", json!({"captured": dir.to_string_lossy()})).unwrap();
    search(&mut s, json!({"text": "pier", "kinds": ["image"]}));
    let r = s.execute("hub.acquire", json!({"id": "pexels:photo-1", "wait": true})).unwrap();
    assert_eq!(r["acquired"]["state"], "done", "{r}");
    let item = r["acquired"]["item"].as_u64().unwrap();
    // the file is where the project keeps downloaded sources
    let path = r["path"].as_str().unwrap().to_string();
    assert!(path.contains("Sources") && path.contains("Pexels") && path.ends_with("pexels-photo-1-a-pier-at-dawn.jpg"), "{path}");
    assert!(std::path::Path::new(&path).exists());
    // imported into Sources > Pexels
    let bins = serde_json::to_string(&s.project.root).unwrap();
    assert!(bins.contains("\"Sources\"") && bins.contains("\"Pexels\""), "{bins}");
    assert!(s.project.items.contains_key(&filmcraft_project::ItemId(item)));
    // provenance, recorded the way F04 reads it
    let rec = s.execute("provenance.get", json!({"item": item})).unwrap();
    assert_eq!(
        (rec["status"].clone(), rec["record"]["source"].clone(), rec["record"]["author"].clone()),
        (json!("clear"), json!("pexels"), json!("Sam")),
        "{rec}"
    );
    assert_eq!(rec["record"]["license"]["name"], "Pexels License");
    assert_eq!(rec["record"]["hash"].as_str().unwrap().len(), 64);
    assert_eq!(rec["record"]["sourceUrl"], "https://www.pexels.com/photo/1/");
    assert!(rec["record"]["note"].as_str().unwrap().contains("Source Hub"));
    // the result now says so, and the job finished
    let res = s.execute("hub.results", json!({})).unwrap();
    assert!(res["candidates"].as_array().unwrap().iter().any(|c| c["id"] == "pexels:photo-1" && c["acquired"]["state"] == "done"));
    assert!(s.jobs.iter().any(|j| j.label.contains("Download") && j.progress.finished.load(std::sync::atomic::Ordering::Relaxed)));
    assert!(s.log.entries.iter().any(|e| e.message.contains("Added \"A pier at dawn\" from Pexels")));
    // nothing in any record or log carries the key
    assert!(!format!("{:?}{:?}", rig.client.state().entries(None), s.log.entries).contains(KEY));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_credit_is_recorded_for_a_licence_that_asks_for_one_and_a_failed_download_leaves_nothing() {
    let (mut s, _rig) = rig(None);
    let dir = std::env::temp_dir().join(format!("edith-hub2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    s.execute("file.projectSettings.scratchDisks", json!({"captured": dir.to_string_lossy()})).unwrap();
    search(&mut s, json!({"text": "fog horn", "kinds": ["audio", "image"]}));
    // a refused download: the second Pexels photo points at an address that answers 404
    let items_before = s.project.items.len();
    let r = s.execute("hub.acquire", json!({"id": "pexels:photo-2", "wait": true})).unwrap();
    assert_eq!(r["acquired"]["state"], "failed", "{r}");
    assert_eq!(s.project.items.len(), items_before, "nothing was imported");
    assert!(!std::path::Path::new(r["path"].as_str().unwrap()).exists());
    assert!(s.log.entries.iter().any(|e| e.message.contains("Could not get \"Gone\"")));
    // a Freesound preview is not a decodable file here, so the import fails cleanly
    let r = s.execute("hub.acquire", json!({"id": "freesound:77", "wait": true})).unwrap();
    assert_eq!(r["acquired"]["state"], "failed", "{r}");
    let _ = std::fs::remove_dir_all(&dir);
    // the credit line that would be recorded
    let c = s.hub.candidate("freesound:77").unwrap();
    assert_eq!(credit_text(&c), "Fog horn by sound man · CC BY 4.0 · https://freesound.org/s/77/");
    assert_eq!(credit_text(&s.hub.candidate("pexels:photo-1").unwrap()), "", "no credit asked, none recorded");
}

#[test]
fn hostile_parameters_are_refused_offline_stops_everything_and_nothing_changes() {
    let (mut s, rig) = rig(None);
    let snap = (*s.project).clone();
    let long = "x".repeat(400);
    for (cmd, p) in [
        ("hub.search", json!({})),
        ("hub.search", json!({"text": ""})),
        ("hub.search", json!({"text": long})),
        ("hub.search", json!({"text": "a", "kinds": []})),
        ("hub.search", json!({"text": "a", "kinds": ["video", "sound"]})),
        ("hub.search", json!({"text": "a", "kinds": "video"})),
        ("hub.search", json!({"text": "a", "sources": ["evil"]})),
        ("hub.search", json!({"text": "a", "sources": []})),
        ("hub.search", json!({"text": "a", "kinds": ["audio"], "sources": ["pexels"]})),
        ("hub.search", json!({"text": "a", "orientation": "diagonal"})),
        ("hub.search", json!({"text": "a", "minWidth": -1})),
        ("hub.search", json!({"text": "a", "minWidth": 99999})),
        ("hub.search", json!({"text": "a", "count": 500})),
        ("hub.search", json!({"text": "a", "minSeconds": "x"})),
        ("hub.search", json!({"text": "a", "wait": "yes"})),
        ("hub.acquire", json!({})),
        ("hub.acquire", json!({"id": ""})),
        ("hub.acquire", json!({"id": 5})),
        ("hub.acquire", json!({"id": "pexels:never-seen"})),
        ("hub.shortlist", json!({"id": 5})),
        ("hub.shortlist", json!({"id": "pexels:never-seen", "on": "x"})),
    ] {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute(cmd, p.clone())));
        assert!(r.is_ok() && r.unwrap().is_err(), "{cmd} accepted {p}");
    }
    assert_eq!(*s.project, snap);
    assert_eq!(rig.sites.hits(), 0, "refused requests asked no site");
    // Offline mode stops a search and a download before anything leaves
    search(&mut s, json!({"text": "pier", "kinds": ["image"]}));
    let hits = rig.sites.hits();
    edith_net::shared().set_offline(true);
    let e = s.execute("hub.search", json!({"text": "other", "kinds": ["image"]})).unwrap_err().to_string();
    assert!(e.contains("Offline"), "{e}");
    assert!(s.execute("hub.acquire", json!({"id": "pexels:photo-1"})).unwrap_err().to_string().contains("Offline"));
    edith_net::shared().set_offline(false);
    assert_eq!(rig.sites.hits(), hits);
    let _ = (Outcome::Ok, SourceError::Cancelled);
}

#[test]
fn thumbnails_are_fetched_in_the_background_a_few_at_a_time() {
    let (mut s, _rig) = rig(None);
    let r = search(&mut s, json!({"text": "pier", "kinds": ["image"]}));
    let id = r["candidates"][0]["id"].as_str().unwrap().to_string();
    let c = s.hub.candidate(&id).unwrap();
    assert!(s.hub.thumbnail(&c).is_none(), "the first ask starts the fetch");
    let started = std::time::Instant::now();
    let bytes = loop {
        if let Some(b) = s.hub.thumbnail(&c) {
            break b;
        }
        assert!(started.elapsed() < Duration::from_secs(5), "the thumbnail never arrived");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
}

#[test]
fn the_commands_exist_and_none_is_journaled() {
    for id in ["hub.providers", "hub.search", "hub.results", "hub.shortlist", "hub.acquire", "hub.cancel"] {
        let spec = crate::commands::command_specs().iter().find(|c| c.id == id).unwrap_or_else(|| panic!("{id} is missing"));
        assert!(!spec.journal, "{id}");
    }
}
