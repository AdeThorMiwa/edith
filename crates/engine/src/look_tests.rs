//! Tests of [`crate::look`]: pictures kept from files and from the Program, their numbers, the
//! comparison with a frame, Match to Look, the library file, and hostile input.

use std::path::{Path, PathBuf};

use filmcraft_media::DemoScene;
use serde_json::{Value, json};

use crate::Session;
use crate::media_test_util::tmp_dir;
use crate::reference_tests::make_film;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn session_in(dir: &Path) -> Session {
    Session { prefs_path: Some(dir.join("preferences.json")), ..Default::default() }
}

/// A PNG of one colour, `w` by `h`.
fn png(dir: &Path, name: &str, w: u32, h: u32, rgb: [u8; 3]) -> PathBuf {
    let path = dir.join(name);
    let img = image::RgbaImage::from_fn(w, h, |_, _| image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
    img.save(&path).unwrap();
    path
}

fn new_look(s: &mut Session, name: &str) -> String {
    s.execute("look.create", json!({"name": name})).unwrap()["look"].as_str().unwrap().to_string()
}

fn add_file(s: &mut Session, look: &str, path: &Path) -> Value {
    s.execute("look.addStill", json!({"look": look, "path": path.to_string_lossy(), "rights": "my own picture"})).unwrap()
}

fn pick_clip(s: &mut Session) -> filmcraft_project::ClipId {
    let q = s.active_sequence().unwrap();
    let t = s.playhead();
    let id = q.video_tracks[0].item_at(t).unwrap().id;
    s.state.selection = vec![id];
    id
}

#[test]
fn the_commands_exist() {
    let all = crate::commands::command_specs();
    for id in [
        "look.create",
        "look.list",
        "look.get",
        "look.set",
        "look.remove",
        "look.addStill",
        "look.setStill",
        "look.removeStill",
        "look.picture",
        "look.compare",
        "look.match",
    ] {
        assert!(all.iter().any(|c| c.id == id), "{id}");
    }
}

#[test]
fn a_look_is_made_edited_listed_and_removed() {
    let mut s = Session::default();
    let id = new_look(&mut s, "  Cold \u{7}harbour  ");
    let l = s.execute("look.get", json!({"id": id})).unwrap();
    assert_eq!(l["name"], "Cold harbour", "control characters and spaces are cleaned");
    assert!(l["measured"].is_null(), "no pictures, no numbers");
    let set =
        s.execute("look.set", json!({"id": id, "notes": "grey and wet\nlow sun", "aspect": "2.39:1", "grain": "heavy 35 mm", "lut": "Kodak 2383"})).unwrap();
    assert_eq!((set["aspect"].as_str(), set["grain"].as_str(), set["lut"].as_str()), (Some("2.39:1"), Some("heavy 35 mm"), Some("Kodak 2383")));
    assert_eq!(set["notes"], "grey and wet\nlow sun", "a new line in the notes is kept");
    s.execute("look.set", json!({"id": id, "name": "Harbour"})).unwrap();
    assert_eq!(s.execute("look.list", json!({})).unwrap()["looks"][0]["name"], "Harbour");
    assert!(s.execute("look.set", json!({"id": id, "name": ""})).is_err(), "a look needs a name");
    assert!(s.execute("look.set", json!({"id": id, "grain": 4})).is_err());
    assert!(s.execute("look.create", json!({"name": "   "})).is_err());
    s.execute("look.remove", json!({"id": id})).unwrap();
    assert!(s.execute("look.get", json!({"id": id})).is_err());
    assert!(s.execute("look.remove", json!({"id": id})).is_err());
    assert_eq!(s.execute("look.list", json!({})).unwrap()["looks"].as_array().unwrap().len(), 0);
}

#[test]
fn a_picture_from_a_file_is_kept_small_with_its_note_and_measured() {
    let dir = tmp_dir("look-file");
    let warm = png(&dir, "dusk.png", 1920, 1080, [200, 120, 60]);
    let mut s = Session::default();
    let l = new_look(&mut s, "Dusk");
    let r = add_file(&mut s, &l, &warm);
    let sid = r["still"].as_str().unwrap().to_string();
    let get = s.execute("look.get", json!({"id": l})).unwrap();
    let p = &get["pictures"][0];
    assert_eq!(p["id"], json!(sid));
    assert_eq!(p["source"], "dusk.png", "the file's name, never its folder");
    assert_eq!(p["rights"], "my own picture");
    assert!(p["width"].as_u64().unwrap() <= 640 && p["height"].as_u64().unwrap() <= 640, "{p}");
    assert_eq!(p["width"], 640);
    assert!(p["warmthKelvin"].as_f64().unwrap() < 4500.0, "orange is warm: {p}");
    assert!(get["measured"]["lumaMean"].as_f64().unwrap() > 0.3);
    assert!(get["measured"]["palette"].as_array().is_some_and(|a| !a.is_empty()), "{}", get["measured"]);
    // the picture itself comes back as a JPEG
    let pic = s.execute("look.picture", json!({"look": l, "still": sid})).unwrap();
    let bytes = filmcraft_project::gtemplate::base64_decode(pic["jpegBase64"].as_str().unwrap()).unwrap();
    assert!(bytes.starts_with(&[0xFF, 0xD8, 0xFF]));
    assert!(bytes.len() < 200_000, "a kept picture is small: {} bytes", bytes.len());
    // caption, then remove
    s.execute("look.setStill", json!({"look": l, "still": sid, "caption": "the light I want"})).unwrap();
    assert_eq!(s.execute("look.get", json!({"id": l})).unwrap()["pictures"][0]["caption"], "the light I want");
    s.execute("look.removeStill", json!({"look": l, "still": sid})).unwrap();
    assert_eq!(s.execute("look.get", json!({"id": l})).unwrap()["pictures"].as_array().unwrap().len(), 0);
    assert!(s.execute("look.removeStill", json!({"look": l, "still": sid})).is_err());
}

#[test]
fn a_file_needs_a_note_of_rights_and_must_be_a_picture() {
    let dir = tmp_dir("look-rights");
    let ok = png(&dir, "a.png", 64, 64, [10, 10, 10]);
    let mut s = Session::default();
    let l = new_look(&mut s, "L");
    let no_note = s.execute("look.addStill", json!({"look": l, "path": ok.to_string_lossy()}));
    assert!(no_note.unwrap_err().to_string().contains("rights"));
    assert!(s.execute("look.addStill", json!({"look": l, "path": ok.to_string_lossy(), "rights": "  "})).is_err());
    let text = dir.join("notes.png");
    std::fs::write(&text, "this is not a picture").unwrap();
    for path in [text, dir.join("missing.png"), dir.clone()] {
        let e = s.execute("look.addStill", json!({"look": l, "path": path.to_string_lossy(), "rights": "x"}));
        assert!(e.is_err(), "{}", path.display());
    }
    assert!(s.execute("look.addStill", json!({"look": l, "path": "bad\u{0}path", "rights": "x"})).is_err());
    assert!(s.execute("look.addStill", json!({"look": "nope", "path": ok.to_string_lossy(), "rights": "x"})).is_err());
    assert_eq!(s.execute("look.get", json!({"id": l})).unwrap()["pictures"].as_array().unwrap().len(), 0, "nothing was kept");
}

#[test]
fn odd_shaped_pictures_are_kept_without_trouble() {
    let dir = tmp_dir("look-odd");
    let mut s = Session::default();
    let l = new_look(&mut s, "Odd");
    for (name, w, h) in [("dot.png", 1, 1), ("thin.png", 3000, 3), ("tall.png", 5, 2500), ("small.png", 40, 30)] {
        let p = png(&dir, name, w, h, [90, 120, 160]);
        add_file(&mut s, &l, &p);
    }
    let get = s.execute("look.get", json!({"id": l})).unwrap();
    for p in get["pictures"].as_array().unwrap() {
        assert!(p["width"].as_u64().unwrap() >= 1 && p["width"].as_u64().unwrap() <= 640, "{p}");
        assert!(p["height"].as_u64().unwrap() >= 1 && p["height"].as_u64().unwrap() <= 640, "{p}");
    }
    assert_eq!(get["pictures"].as_array().unwrap().len(), 4);
}

#[test]
fn a_frame_of_a_film_is_taken_at_the_time_asked() {
    let dir = tmp_dir("look-film");
    let film = dir.join("film.mov");
    make_film(&film, &[(DemoScene::Dunes, 48), (DemoScene::Plasma, 48)], false);
    let mut s = Session::default();
    let l = new_look(&mut s, "Film");
    let early = s.execute("look.addStill", json!({"look": l, "path": film.to_string_lossy(), "rights": "my own", "seconds": 0.5})).unwrap()["still"]
        .as_str()
        .unwrap()
        .to_string();
    let late = s.execute("look.addStill", json!({"look": l, "path": film.to_string_lossy(), "rights": "my own", "seconds": 3.0})).unwrap()["still"]
        .as_str()
        .unwrap()
        .to_string();
    let middle =
        s.execute("look.addStill", json!({"look": l, "path": film.to_string_lossy(), "rights": "my own"})).unwrap()["still"].as_str().unwrap().to_string();
    let get = s.execute("look.get", json!({"id": l})).unwrap();
    let by = |id: &str| get["pictures"].as_array().unwrap().iter().find(|p| p["id"] == json!(id)).cloned().unwrap();
    assert_eq!(by(&early)["source"], "film.mov");
    assert!((by(&early)["timeSeconds"].as_f64().unwrap() - 0.5).abs() < 0.1);
    assert!((by(&late)["timeSeconds"].as_f64().unwrap() - 3.0).abs() < 0.1);
    assert!((by(&middle)["timeSeconds"].as_f64().unwrap() - 2.0).abs() < 0.1, "the middle of a 4 s film");
    assert_ne!(by(&early)["brightness"], by(&late)["brightness"], "two different scenes");
    assert!(s.execute("look.addStill", json!({"look": l, "path": film.to_string_lossy(), "rights": "x", "seconds": -1})).is_err());
    // past the end is held to the last frame, not an error
    assert!(s.execute("look.addStill", json!({"look": l, "path": film.to_string_lossy(), "rights": "x", "seconds": 9999.0})).is_ok());
}

#[test]
fn the_program_frame_is_kept_with_its_time_and_a_default_note() {
    let mut s = demo();
    let l = new_look(&mut s, "Mine");
    s.execute("look.addStill", json!({"look": l, "caption": "opening"})).unwrap();
    let p = s.execute("look.get", json!({"id": l})).unwrap()["pictures"][0].clone();
    assert_eq!(p["source"], "Program frame");
    assert_eq!(p["rights"], "A frame of my own edit");
    assert_eq!(p["caption"], "opening");
    assert!(p["timeSeconds"].as_f64().is_some());
    assert!(s.execute("look.addStill", json!({"look": l, "seconds": 100000.0})).is_err(), "a moment outside the edit");
    // with no edit there is no Program
    let mut empty = Session::default();
    let e = new_look(&mut empty, "E");
    assert!(empty.execute("look.addStill", json!({"look": e})).is_err());
}

#[test]
fn a_frame_is_compared_with_a_look_in_plain_words() {
    let dir = tmp_dir("look-compare");
    let mut s = demo();
    // a look made from the frame itself is close to it
    let same = new_look(&mut s, "Same");
    s.execute("look.addStill", json!({"look": same})).unwrap();
    let c = s.execute("look.compare", json!({"look": same})).unwrap();
    assert_eq!(c["differ"], 0, "{c}");
    assert!(c["rows"].as_array().unwrap().len() >= 4);
    // a very different one is not
    let other = new_look(&mut s, "Night");
    add_file(&mut s, &other, &png(&dir, "black.png", 320, 180, [4, 6, 30]));
    let c = s.execute("look.compare", json!({"look": other})).unwrap();
    assert!(c["differ"].as_u64().unwrap() >= 2, "{c}");
    let words: Vec<&str> = c["rows"].as_array().unwrap().iter().filter_map(|r| r["words"].as_str()).collect();
    assert!(words.iter().any(|w| w.starts_with("Brightness")), "{words:?}");
    assert!(c["rows"].as_array().unwrap().iter().all(|r| r["group"] == "picture"));
    // no pictures, no comparison; no look, no comparison
    let empty = new_look(&mut s, "Empty");
    assert!(s.execute("look.compare", json!({"look": empty})).is_err());
    assert!(s.execute("look.compare", json!({"look": "zzz"})).is_err());
    assert!(s.execute("look.compare", json!({"look": other, "seconds": 100000.0})).is_err());
}

#[test]
fn match_to_look_grades_the_clip_towards_a_picture_in_one_undo_step() {
    let dir = tmp_dir("look-match");
    let mut s = demo();
    let clip = pick_clip(&mut s);
    let l = new_look(&mut s, "Teal and orange");
    let a = add_file(&mut s, &l, &png(&dir, "teal.png", 320, 180, [20, 90, 110]))["still"].as_str().unwrap().to_string();
    let r = s.execute("look.match", json!({"look": l})).unwrap();
    assert_eq!((r["look"].as_str(), r["still"].as_str()), (Some(l.as_str()), Some(a.as_str())));
    let (before, after) = (r["distanceBefore"].as_f64().unwrap(), r["distanceAfter"].as_f64().unwrap());
    assert!(after < before * 0.8, "the grade moves the clip towards the picture: {r}");
    let q = s.active_sequence().unwrap();
    let e = q.find_item(clip).unwrap().1.effects.iter().find(|e| e.effect == "lumetri").unwrap().clone();
    let moved = ["wheel_shadows", "wheel_midtones", "wheel_highlights"]
        .iter()
        .any(|w| e.param(w).and_then(|p| p.value.as_vec2()).is_some_and(|v| v.x.abs() + v.y.abs() > 1e-3));
    assert!(moved, "{r}");
    s.execute("edit.undo", json!({})).unwrap();
    let q = s.active_sequence().unwrap();
    let e = q.find_item(clip).unwrap().1.effects.iter().find(|e| e.effect == "lumetri").unwrap();
    assert_eq!(e.param("wheel_midtones").and_then(|p| p.value.as_vec2()).map(|v| v.x), Some(0.0), "one undo undoes the whole match");
    // several pictures: it will not guess
    add_file(&mut s, &l, &png(&dir, "warm.png", 320, 180, [200, 140, 80]));
    let e = s.execute("look.match", json!({"look": l})).unwrap_err().to_string();
    assert!(e.contains("still"), "{e}");
    assert!(s.execute("look.match", json!({"look": l, "still": a})).is_ok());
    assert!(s.execute("look.match", json!({"look": l, "still": "s999"})).is_err());
    let empty = new_look(&mut s, "Empty");
    assert!(s.execute("look.match", json!({"look": empty})).is_err());
    assert!(s.execute("look.match", json!({"look": "nope"})).is_err());
}

#[test]
fn counts_are_held_to_their_limits() {
    let dir = tmp_dir("look-caps");
    let dot = png(&dir, "dot.png", 8, 8, [50, 60, 70]);
    let mut s = Session::default();
    let l = new_look(&mut s, "Full");
    for _ in 0..crate::look::MAX_STILLS {
        add_file(&mut s, &l, &dot);
    }
    let e = s.execute("look.addStill", json!({"look": l, "path": dot.to_string_lossy(), "rights": "x"})).unwrap_err().to_string();
    assert!(e.contains("at most"), "{e}");
    for i in 1..crate::look::MAX_LOOKS {
        s.execute("look.create", json!({"name": format!("L{i}")})).unwrap();
    }
    assert!(s.execute("look.create", json!({"name": "one too many"})).is_err());
    // the ids never repeat, even after removals
    let ids: Vec<String> =
        s.execute("look.get", json!({"id": l})).unwrap()["pictures"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap().to_string()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len());
}

#[test]
fn the_library_is_a_file_every_session_finds_and_a_damaged_one_is_empty() {
    let dir = tmp_dir("look-file-lib");
    let mut a = session_in(&dir);
    let l = new_look(&mut a, "Kept");
    add_file(&mut a, &l, &png(&dir, "p.png", 100, 60, [120, 60, 40]));
    assert!(dir.join(crate::look::FILE).exists());
    let mut b = session_in(&dir);
    let got = b.execute("look.get", json!({"id": l})).unwrap();
    assert_eq!(got["name"], "Kept");
    assert_eq!(got["pictures"].as_array().unwrap().len(), 1);
    let pic = b.execute("look.picture", json!({"look": l, "still": got["pictures"][0]["id"]})).unwrap();
    assert!(pic["jpegBase64"].as_str().unwrap().starts_with("/9j/"));
    // reading does not write the file
    let before = std::fs::metadata(dir.join(crate::look::FILE)).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(30));
    for _ in 0..5 {
        b.execute("look.list", json!({})).unwrap();
        b.execute("look.get", json!({"id": l})).unwrap();
    }
    assert_eq!(std::fs::metadata(dir.join(crate::look::FILE)).unwrap().modified().unwrap(), before);
    // removing the look leaves a file without it
    b.execute("look.remove", json!({"id": l})).unwrap();
    assert_eq!(session_in(&dir).execute("look.list", json!({})).unwrap()["looks"].as_array().unwrap().len(), 0);
    // damaged files are an empty library
    for junk in ["not json", "{\"looks\": 5}", "", "[]"] {
        let d = tmp_dir("look-junk");
        std::fs::write(d.join(crate::look::FILE), junk).unwrap();
        assert_eq!(session_in(&d).execute("look.list", json!({})).unwrap()["looks"].as_array().unwrap().len(), 0, "{junk}");
    }
}

#[test]
fn what_is_read_from_the_file_is_not_trusted() {
    let dir = tmp_dir("look-hostile");
    let mut a = session_in(&dir);
    let l = new_look(&mut a, "Real");
    add_file(&mut a, &l, &png(&dir, "p.png", 100, 60, [120, 60, 40]));
    let text = std::fs::read_to_string(dir.join(crate::look::FILE)).unwrap();
    let mut v: Value = serde_json::from_str(&text).unwrap();
    let good = v["looks"][0]["stills"][0].clone();
    let mut huge = good.clone();
    huge["id"] = json!("s900");
    huge["width"] = json!(100_000);
    let mut empty_jpeg = good.clone();
    empty_jpeg["id"] = json!("s901");
    empty_jpeg["jpeg"] = json!("");
    let mut bad_id = good.clone();
    bad_id["id"] = json!("../../etc");
    let mut dup = good.clone();
    dup["caption"] = json!("duplicate id");
    let mut long = good.clone();
    long["id"] = json!("s902");
    long["caption"] = json!("x".repeat(5000));
    v["looks"][0]["stills"] = json!([good, huge, empty_jpeg, bad_id, dup, long]);
    v["looks"][0]["name"] = json!(format!("{}\u{1}", "n".repeat(500)));
    let mut twin = v["looks"][0].clone();
    twin["name"] = json!("same id twice");
    v["looks"].as_array_mut().unwrap().push(twin);
    v["looks"].as_array_mut().unwrap().push(json!({"id": "bad id!", "name": "x"}));
    std::fs::write(dir.join(crate::look::FILE), v.to_string()).unwrap();
    let mut b = session_in(&dir);
    let list = b.execute("look.list", json!({})).unwrap();
    assert_eq!(list["looks"].as_array().unwrap().len(), 1, "a repeated id and a bad id are left out: {list}");
    let got = b.execute("look.get", json!({"id": l})).unwrap();
    assert_eq!(got["name"].as_str().unwrap().chars().count(), 80);
    let ids: Vec<&str> = got["pictures"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), 2, "the real one and the one with the long caption: {ids:?}");
    assert!(got["pictures"][1]["caption"].as_str().unwrap().chars().count() <= 120);
    // an oversized file is an empty library
    let d = tmp_dir("look-big");
    std::fs::write(d.join(crate::look::FILE), vec![b' '; 41 * 1024 * 1024]).unwrap();
    assert_eq!(session_in(&d).execute("look.list", json!({})).unwrap()["looks"].as_array().unwrap().len(), 0);
}

#[test]
fn a_damaged_kept_picture_is_reported_and_does_not_crash_a_match() {
    let dir = tmp_dir("look-damaged");
    let mut a = session_in(&dir);
    let l = new_look(&mut a, "D");
    let sid = add_file(&mut a, &l, &png(&dir, "p.png", 64, 64, [120, 60, 40]))["still"].as_str().unwrap().to_string();
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(crate::look::FILE)).unwrap()).unwrap();
    v["looks"][0]["stills"][0]["jpeg"] = json!("AAAA");
    std::fs::write(dir.join(crate::look::FILE), v.to_string()).unwrap();
    let mut b = session_in(&dir);
    b.execute("file.openDemoProject", json!({})).unwrap();
    pick_clip(&mut b);
    let e = b.execute("look.match", json!({"look": l, "still": sid}));
    assert!(e.is_err());
}
