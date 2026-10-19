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

/// One picture from Commons, only when images were asked for (the answer to a video or sound
/// search is empty, as Commons's would be for a search with no match).
fn commons_pages(files: &str) -> String {
    format!(
        r#"{{"batchcomplete":true,"query":{{"pages":[{{"pageid":300,"title":"File:Misty pier.jpg","index":1,"imageinfo":[{{"size":5000,"width":3000,"height":2000,
        "url":"{files}/orig.jpg","thumburl":"{files}/thumb/1920px-Misty_pier.jpg","thumbwidth":1920,"thumbheight":1280,"descriptionurl":"https://commons.wikimedia.org/wiki/File:Misty_pier.jpg",
        "mime":"image/jpeg","mediatype":"BITMAP","extmetadata":{{"LicenseShortName":{{"value":"CC BY 4.0"}},"LicenseUrl":{{"value":"https://creativecommons.org/licenses/by/4.0/"}},"Artist":{{"value":"<a href=\"x\">Wiki Person</a>"}},"AttributionRequired":{{"value":"true"}}}}}}]}}]}}}}"#
    )
}

fn freesound_results(files: &str) -> String {
    format!(
        r#"{{"count":1,"results":[{{"id":77,"name":"Fog horn","username":"sound man","license":"https://creativecommons.org/licenses/by/4.0/","url":"https://freesound.org/s/77/","duration":8,"tags":["fog"],"previews":{{"preview-hq-mp3":"{files}/horn.mp3"}}}}]}}"#
    )
}

pub(crate) struct Rig {
    sites: Server,
    _files: Server,
    client: Arc<Client>,
    _hold: Box<dyn std::any::Any>,
}

/// Offline mode is one switch for the whole process. Every rig holds this for reading; the one
/// test that turns the switch on holds it for writing, so no other test sees it on.
static OFFLINE: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// A session whose Source Hub talks to local servers. `broken` names a source that answers 401.
pub(crate) fn rig(broken: Option<&'static str>) -> (Session, Rig) {
    rig_holding(broken, Box::new(OFFLINE.read().unwrap_or_else(std::sync::PoisonError::into_inner)))
}

fn rig_holding(broken: Option<&'static str>, hold: Box<dyn std::any::Any>) -> (Session, Rig) {
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
        } else if req.starts_with("GET /w/api.php") {
            if broken == Some("wikimedia") {
                return response(500, &[], b"");
            }
            // only a search for pictures finds the picture
            json(if req.contains("bitmap") { commons_pages(&furl) } else { r#"{"batchcomplete":true}"#.to_string() })
        } else if req.starts_with("GET /advancedsearch.php") {
            json(r#"{"response":{"numFound":0,"docs":[]}}"#.to_string())
        } else if req.starts_with("GET /v1/images/") || req.starts_with("GET /v1/audio/") {
            json(r#"{"result_count":0,"results":[]}"#.to_string())
        } else if req.starts_with("GET /search?") {
            json(r#"{"collection":{"items":[],"metadata":{"total_hits":0}}}"#.to_string())
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
    (s, Rig { sites, _files: files, client, _hold: hold })
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
    assert_eq!(
        list.iter().map(|x| x["id"].as_str().unwrap()).collect::<Vec<_>>(),
        ["pexels", "pixabay", "wikimedia", "archive", "freesound", "openverse", "nasa", "local"]
    );
    assert_eq!(list[4]["kinds"], json!(["audio"]));
    assert_eq!((list[5]["kinds"].clone(), list[5]["needsKey"].clone(), list[5]["connected"].clone()), (json!(["image", "audio"]), json!(false), json!(true)));
    assert_eq!((list[6]["kinds"].clone(), list[6]["needsKey"].clone()), (json!(["video", "image"]), json!(false)));
    // the open sources need no key and are always ready; the others say whether a key is stored
    assert_eq!((list[2]["needsKey"].clone(), list[2]["connected"].clone(), list[3]["connected"].clone()), (json!(false), json!(true), json!(true)));
    assert_eq!(list[0]["needsKey"], true);
    assert!(list[0]["credit"].as_str().unwrap().contains("Pexels") && list[0]["keyUrl"].as_str().unwrap().starts_with("https://"));
}

#[test]
fn a_search_asks_every_source_that_offers_the_kinds_and_interleaves_the_answers() {
    let (mut s, rig) = rig(None);
    let r = search(&mut s, json!({"text": "a pier at dawn", "kinds": ["image", "audio"]}));
    assert_eq!(r["running"], false);
    assert_eq!(ids(&r), ["pexels:photo-1", "pixabay:image-9", "wikimedia:300", "freesound:77", "pexels:photo-2"], "one from each source in turn: {r}");
    let states: Vec<&str> = r["sources"].as_array().unwrap().iter().map(|x| x["state"].as_str().unwrap()).collect();
    assert_eq!(states, ["done"; 7], "the open sources answered too; the Archive, Openverse and NASA had nothing");
    let first = &r["candidates"][0];
    assert_eq!((first["rights"]["name"].clone(), first["author"].clone()), (json!("Pexels License"), json!("Sam")));
    // a search for video only skips the audio site
    let v = search(&mut s, json!({"text": "fog", "kinds": ["video"]}));
    assert_eq!(
        v["sources"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect::<Vec<_>>(),
        ["pexels", "pixabay", "wikimedia", "archive", "nasa"]
    );
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
    assert_eq!(ids(&r), ["pexels:photo-1", "wikimedia:300", "pexels:photo-2"]);
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
    let (mut s, rig) = rig_holding(None, Box::new(OFFLINE.write().unwrap_or_else(std::sync::PoisonError::into_inner)));
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
    // your own folders can still be searched while offline, and no site is asked
    let dir = std::env::temp_dir().join(format!("edith-hub-offline-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("fog valley.mp4"), b"x").unwrap();
    let dir = dir.canonicalize().unwrap();
    s.execute("hub.addFolder", json!({"path": dir.to_string_lossy(), "tag": "own"})).unwrap();
    let r = search(&mut s, json!({"text": "fog", "kinds": ["video"]}));
    edith_net::shared().set_offline(false);
    assert_eq!(r["sources"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect::<Vec<_>>(), ["local"]);
    assert_eq!(r["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(rig.sites.hits(), hits);
    let _ = std::fs::remove_dir_all(&dir);
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

// ---- your own folders (F05.5) -----------------------------------------------------------------

mod folders {
    use super::*;
    use std::path::{Path, PathBuf};

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("edith-hub-folders-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    fn put(dir: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p
    }

    /// A folder of footage, pictures and sound, with names a person would give them.
    fn library(name: &str) -> PathBuf {
        let d = scratch(name);
        put(&d, "Iceland/fog_over_valley_01.mp4", b"not really a video");
        put(&d, "Iceland/fog_over_valley_02.mov", b"x");
        put(&d, "Iceland/black sand beach.mp4", b"x");
        put(&d, "Stills/Valley fog dawn.jpg", &png());
        put(&d, "Stills/city.png", &png());
        put(&d, "Sound/Fog horn.wav", b"RIFF");
        put(&d, "Sound/notes.txt", b"text is not media");
        put(&d, ".hidden/fog.mp4", b"x");
        d
    }

    fn add(s: &mut Session, dir: &Path, tag: &str) -> Value {
        s.execute("hub.addFolder", json!({"path": dir.to_string_lossy(), "tag": tag})).unwrap()
    }

    #[test]
    fn a_search_looks_through_the_names_in_a_folder_and_ranks_by_how_many_words_match() {
        let dir = library("find");
        let (mut s, _rig) = rig(None);
        add(&mut s, &dir, "own");
        let r = search(&mut s, json!({"text": "fog over a valley", "kinds": ["video", "image", "audio"], "sources": ["local"]}));
        let titles: Vec<String> = r["candidates"].as_array().unwrap().iter().map(|c| c["title"].as_str().unwrap().to_string()).collect();
        assert_eq!(titles.len(), 4, "{titles:?}");
        assert!(titles[..3].iter().all(|t| t.to_lowercase().contains("fog")), "the best matches first: {titles:?}");
        assert!(titles[..2].contains(&"fog_over_valley_01".to_string()) || titles[..2].contains(&"Valley fog dawn".to_string()), "{titles:?}");
        assert!(!titles.iter().any(|t| t.contains("notes") || t == "city" || t.contains("beach")), "only files that match, and only media: {titles:?}");
        assert!(!r.to_string().contains(".hidden"), "hidden folders are left alone");
        let c = &r["candidates"][0];
        assert_eq!(
            (c["source"].clone(), c["rights"]["name"].clone(), c["rights"]["attributionRequired"].clone()),
            (json!("local"), json!("Own work"), json!(false))
        );
        // asking for video only
        let v = search(&mut s, json!({"text": "fog", "kinds": ["video"], "sources": ["local"]}));
        assert!(v["candidates"].as_array().unwrap().iter().all(|c| c["kind"] == "video"));
        assert_eq!(v["candidates"].as_array().unwrap().len(), 2);
        // the folders are part of a plain search, and the sites are asked too
        let all = search(&mut s, json!({"text": "fog", "kinds": ["image"]}));
        assert!(all["sources"].as_array().unwrap().iter().any(|x| x["id"] == "local"));
        assert!(ids(&all).iter().any(|i| i.starts_with("local:")) && ids(&all).iter().any(|i| i.starts_with("pexels:")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_chosen_file_is_imported_where_it_is_and_recorded_as_yours_or_licensed() {
        let dir = library("import");
        let (mut s, _rig) = rig(None);
        add(&mut s, &dir, "licensed");
        let r = search(&mut s, json!({"text": "valley fog dawn", "kinds": ["image"], "sources": ["local"]}));
        let id = r["candidates"][0]["id"].as_str().unwrap().to_string();
        let got = s.execute("hub.acquire", json!({"id": id})).unwrap();
        assert_eq!(got["acquired"]["state"], "done", "{got}");
        let item = got["acquired"]["item"].as_u64().unwrap();
        let original = dir.join("Stills/Valley fog dawn.jpg");
        assert!(original.exists(), "nothing was moved or copied");
        let src = &s.project.items[&filmcraft_project::ItemId(item)];
        assert!(format!("{:?}", src.kind).contains("Valley fog dawn"), "imported from where it is");
        let rec = s.execute("provenance.get", json!({"item": item})).unwrap();
        assert_eq!(
            (rec["status"].clone(), rec["record"]["source"].clone(), rec["record"]["license"]["name"].clone()),
            (json!("clear"), json!("licensed"), json!("Licensed to you")),
            "{rec}"
        );
        assert!(rec["record"]["note"].as_str().unwrap().contains("folder you added"));
        assert!(serde_json::to_string(&s.project.root).unwrap().contains("My folders"));
        // an "own" folder records its files as own work
        let own = scratch("own");
        put(&own, "clips/ocean sunset.png", &png());
        add(&mut s, &own, "own");
        let r = search(&mut s, json!({"text": "ocean sunset", "kinds": ["image"], "sources": ["local"]}));
        let id = r["candidates"][0]["id"].as_str().unwrap().to_string();
        let item = s.execute("hub.acquire", json!({"id": id})).unwrap()["acquired"]["item"].as_u64().unwrap();
        assert_eq!(s.execute("provenance.get", json!({"item": item})).unwrap()["record"]["source"], "own");
        let _ = (std::fs::remove_dir_all(&dir), std::fs::remove_dir_all(&own));
    }

    #[test]
    fn a_file_that_left_its_folder_is_not_imported() {
        let dir = library("moved");
        let outside = scratch("outside");
        let (mut s, _rig) = rig(None);
        add(&mut s, &dir, "own");
        let r = search(&mut s, json!({"text": "city", "kinds": ["image"], "sources": ["local"]}));
        assert_eq!(r["candidates"].as_array().unwrap().len(), 1);
        let id = r["candidates"][0]["id"].as_str().unwrap().to_string();
        // the folder is taken away from the list after the search: the result may no longer be used
        s.execute("hub.removeFolder", json!({"path": dir.to_string_lossy()})).unwrap();
        let e = s.execute("hub.acquire", json!({"id": id})).unwrap_err().to_string();
        assert!(e.contains("not in one of your folders"), "{e}");
        assert!(!crate::hub_local::inside(&[crate::hub_local::Folder { path: dir.to_string_lossy().into(), tag: "own".into() }], &outside.to_string_lossy()));
        let _ = (std::fs::remove_dir_all(&dir), std::fs::remove_dir_all(&outside));
    }

    #[cfg(unix)]
    #[test]
    fn links_are_not_followed_so_a_search_cannot_wander_out_of_the_folder_or_loop() {
        let dir = library("links");
        let secret = scratch("secret");
        put(&secret, "fog secret.mp4", b"x");
        std::os::unix::fs::symlink(&secret, dir.join("escape")).unwrap();
        std::os::unix::fs::symlink(&dir, dir.join("Iceland/loop")).unwrap();
        let found = crate::hub_local::find(
            &[crate::hub_local::Folder { path: dir.to_string_lossy().into(), tag: "own".into() }],
            &edith_sources::Request { text: "fog".into(), kinds: vec![edith_sources::Kind::Video], count: 40, ..Default::default() },
        );
        assert!(found.candidates.iter().all(|c| !c.title.contains("secret")), "{:?}", found.candidates.iter().map(|c| &c.title).collect::<Vec<_>>());
        assert_eq!(found.candidates.len(), 2);
        let _ = (std::fs::remove_dir_all(&dir), std::fs::remove_dir_all(&secret));
    }

    #[test]
    fn the_folder_list_is_validated_remembered_and_forgotten_on_request() {
        let dir = library("list");
        let data = scratch("data");
        let (mut s, _rig) = rig(None);
        s.prefs_path = Some(data.join("preferences.json"));
        let l = add(&mut s, &dir, "own");
        assert_eq!(l["folders"][0]["tag"], "own");
        assert_eq!(l["folders"][0]["exists"], true);
        // adding it again changes its tag rather than listing it twice
        let l = add(&mut s, &dir, "licensed");
        assert_eq!((l["folders"].as_array().unwrap().len(), l["folders"][0]["tag"].clone()), (1, json!("licensed")));
        // a new session on the same data folder finds it
        let mut again = Session { prefs_path: Some(data.join("preferences.json")), ..Default::default() };
        assert_eq!(again.execute("hub.folders", json!({})).unwrap()["folders"][0]["path"], dir.to_string_lossy().as_ref());
        // the providers list says whether there is anything to look in
        let prov = s.execute("hub.providers", json!({})).unwrap();
        let local = prov["providers"].as_array().unwrap().iter().find(|p| p["id"] == "local").unwrap().clone();
        assert_eq!((local["connected"].clone(), local["folders"].clone()), (json!(true), json!(1)));
        // removing it
        assert_eq!(s.execute("hub.removeFolder", json!({"path": dir.to_string_lossy()})).unwrap()["folders"].as_array().unwrap().len(), 0);
        assert!(s.execute("hub.removeFolder", json!({"path": dir.to_string_lossy()})).is_err());
        assert!(dir.exists(), "nothing on disk is touched");
        // a search naming local with no folder says what to do
        assert!(s.execute("hub.search", json!({"text": "fog", "sources": ["local"]})).unwrap_err().to_string().contains("not added a folder"));
        let _ = (std::fs::remove_dir_all(&dir), std::fs::remove_dir_all(&data));
    }

    #[test]
    fn hostile_folder_parameters_are_refused_and_a_damaged_list_is_ignored() {
        let file = scratch("file");
        let f = put(&file, "a.txt", b"x");
        let (mut s, _rig) = rig(None);
        for p in [
            json!({}),
            json!({"path": ""}),
            json!({"path": 5, "tag": "own"}),
            json!({"path": file.to_string_lossy(), "tag": "mine"}),
            json!({"path": file.to_string_lossy()}),
            json!({"path": "/no/such/folder/anywhere", "tag": "own"}),
            json!({"path": f.to_string_lossy(), "tag": "own"}),
            json!({"path": "/", "tag": "own"}),
            json!({"path": "x".repeat(2000), "tag": "own"}),
        ] {
            assert!(s.execute("hub.addFolder", p.clone()).is_err(), "{p}");
        }
        for p in [json!({}), json!({"path": 5}), json!({"path": "/not/mine"})] {
            assert!(s.execute("hub.removeFolder", p.clone()).is_err(), "{p}");
        }
        assert!(s.execute("hub.folders", json!({})).unwrap()["folders"].as_array().unwrap().is_empty());
        // at most sixteen
        for i in 0..16 {
            add(&mut s, &scratch(&format!("many{i}")), "own");
        }
        assert!(s.execute("hub.addFolder", json!({"path": scratch("many-extra").to_string_lossy(), "tag": "own"})).is_err());
        // a damaged list on disk is no folders, not a crash
        let data = scratch("damaged");
        std::fs::write(data.join("edith-media-folders.json"), "{ not json").unwrap();
        let mut d = Session { prefs_path: Some(data.join("preferences.json")), ..Default::default() };
        assert!(d.execute("hub.folders", json!({})).unwrap()["folders"].as_array().unwrap().is_empty());
        std::fs::write(data.join("edith-media-folders.json"), r#"[{"path":"/x","tag":"evil"},{"path":"","tag":"own"},{"path":"/ok","tag":"own"}]"#).unwrap();
        let mut d = Session { prefs_path: Some(data.join("preferences.json")), ..Default::default() };
        assert_eq!(d.execute("hub.folders", json!({})).unwrap()["folders"].as_array().unwrap().len(), 1, "only the valid entry is kept");
        let _ = std::fs::remove_dir_all(&file);
    }

    #[test]
    fn a_picture_in_a_folder_gets_a_thumbnail() {
        let dir = library("thumb");
        let (mut s, _rig) = rig(None);
        add(&mut s, &dir, "own");
        let r = search(&mut s, json!({"text": "city", "kinds": ["image"], "sources": ["local"]}));
        let c = s.hub.candidate(r["candidates"][0]["id"].as_str().unwrap()).unwrap();
        assert!(c.thumb.starts_with("file://"));
        let started = std::time::Instant::now();
        let bytes = loop {
            if let Some(b) = s.hub.thumbnail(&c) {
                break b;
            }
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
