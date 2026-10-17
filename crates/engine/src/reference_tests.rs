//! Tests of [`crate::reference`]: measuring real film files (made here with our own ProRes encoder, so
//! no outside tool is needed) into fingerprints, and hostile files and parameters.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use filmcraft_media::generators::GeneratorSource;
use filmcraft_media::{DemoScene, Generator, MediaSource};
use filmcraft_project::{ItemId, ItemKind, Label, MediaClip, MediaRef, Project, SequenceSettings, TrackKind};
use filmcraft_time::{FrameRate, Tick, TimeRange};
use serde_json::{Value, json};

use crate::Session;
use crate::media_test_util::tmp_dir;

fn clip_of(g: Generator, src: &GeneratorSource) -> MediaClip {
    MediaClip {
        media: MediaRef::Generator(g),
        info: src.info().clone(),
        interpret: Default::default(),
        mark_in: None,
        mark_out: None,
        markers: vec![],
        offline: false,
        proxy: None,
        identity: None,
    }
}

/// A 24 fps ProRes movie of demo scenes played one after the other (`shots` = scene and frames), with a
/// tone under it when `tone` is set. The cuts are at the running sums of the frame counts.
fn make_film(path: &Path, shots: &[(DemoScene, i64)], tone: bool) {
    let (w, h, rate) = (160u32, 90u32, FrameRate::FPS_24);
    let mut p = Project::new("fixture");
    let seq = p.new_sequence("s", SequenceSettings { width: w, height: h, frame_rate: rate, ..Default::default() }, 1, 1, None);
    let mut sources: Vec<(ItemId, filmcraft_media::SharedSource)> = Vec::new();
    let mut at = Tick::ZERO;
    for (k, (scene, frames)) in shots.iter().enumerate() {
        let dur = rate.tick_of(*frames);
        let g = Generator::Demo(*scene);
        let src = GeneratorSource::new(g.clone(), w, h, rate, rate.tick_of(200));
        let item = p.add_item(&format!("shot{k}"), Label::Iris, ItemKind::Media(clip_of(g, &src)), None);
        let v = p.make_track_item(item, TrackKind::Video, at, TimeRange::new(rate.tick_of(10 * k as i64), dur), rate).unwrap();
        p.sequence_mut(seq).unwrap().video_tracks[0].items.push(v);
        sources.push((item, Arc::new(src)));
        at += dur;
    }
    if tone {
        let g = Generator::Tone { hz: 440.0, db: -20.0 };
        let src = GeneratorSource::new(g.clone(), w, h, rate, at);
        let item = p.add_item("tone", Label::Iris, ItemKind::Media(clip_of(g, &src)), None);
        let a = p.make_track_item(item, TrackKind::Audio, Tick::ZERO, TimeRange::new(Tick::ZERO, at), rate).unwrap();
        p.sequence_mut(seq).unwrap().audio_tracks[0].items.push(a);
        sources.push((item, Arc::new(src)));
    }
    let settings = filmcraft_export::ExportSettings {
        format: filmcraft_export::Format::ProRes,
        path: path.to_string_lossy().into_owned(),
        include_audio: tone,
        ..Default::default()
    };
    let provider = move |id: ItemId| sources.iter().find(|(i, _)| *i == id).map(|(_, s)| s.clone());
    filmcraft_export::export(&Arc::new(p), seq, &settings, &provider, &Default::default()).unwrap();
}

fn measure(s: &mut Session, params: Value) -> Value {
    let mut p = params;
    p["wait"] = json!(true);
    s.execute("reference.measure", p).unwrap()["fingerprint"].clone()
}

/// A mono 16-bit WAV of a sine.
fn wav(path: &Path, hz: f64, seconds: f64) {
    let rate = 16_000u32;
    let n = (f64::from(rate) * seconds) as usize;
    let mut data = Vec::with_capacity(n * 2);
    for i in 0..n {
        let v = (0.2 * (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate)).sin() * 32767.0) as i16;
        data.extend(v.to_le_bytes());
    }
    let mut out = Vec::new();
    out.extend(b"RIFF");
    out.extend((36 + data.len() as u32).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(rate.to_le_bytes());
    out.extend((rate * 2).to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(data);
    std::fs::write(path, out).unwrap();
}

#[test]
fn the_commands_exist_and_change_nothing_in_the_project() {
    let all = crate::commands::command_specs();
    for id in ["reference.measure", "reference.result"] {
        assert!(all.iter().any(|c| c.id == id && !c.journal), "{id}");
    }
}

#[test]
fn a_film_with_two_cuts_is_measured_to_three_shots_with_the_cuts_where_they_are() {
    let dir = tmp_dir("ref-cuts");
    let path = dir.join("film.mov");
    // Ocean Sunset 2 s, City Night 3 s, Forest 2 s: cuts at 2 s and 5 s
    make_film(&path, &[(DemoScene::OceanSunset, 48), (DemoScene::CityNight, 72), (DemoScene::Forest, 48)], false);
    let mut s = Session::default();
    let rev = s.revision;
    let fp = measure(&mut s, json!({"path": path.to_string_lossy(), "name": "Test film", "sampleFps": 12, "pictures": 8}));
    assert_eq!(s.revision, rev, "measuring is not an edit");
    assert_eq!(fp["name"], "Test film");
    assert_eq!(fp["shots"]["count"], 3, "{}", fp["shots"]);
    assert!((fp["durationSeconds"].as_f64().unwrap() - 7.0).abs() < 0.2);
    assert!(
        (fp["shots"]["maxSeconds"].as_f64().unwrap() - 3.0).abs() < 0.2 && (fp["shots"]["minSeconds"].as_f64().unwrap() - 2.0).abs() < 0.2,
        "{}",
        fp["shots"]
    );
    assert_eq!((fp["width"].as_u64(), fp["height"].as_u64()), (Some(160), Some(90)));
    assert!(fp["picture"]["lumaMean"].as_f64().unwrap() > 0.0 && fp["picture"]["palette"].as_array().unwrap().len() <= 5);
    assert!(fp["motion"]["energy"].is_f64() && fp["motion"]["camera"].is_object());
    assert!(fp["sound"].is_null(), "a silent film has no sound numbers");
    assert_eq!(fp["version"], 1);
}

#[test]
fn a_film_with_sound_has_its_loudness_and_pieces_measured() {
    let dir = tmp_dir("ref-sound");
    let path = dir.join("film.mov");
    make_film(&path, &[(DemoScene::Dunes, 96), (DemoScene::Plasma, 96)], true);
    let mut s = Session::default();
    let fp = measure(&mut s, json!({"path": path.to_string_lossy(), "sampleFps": 8, "pictures": 8}));
    let snd = &fp["sound"];
    assert!(snd.is_object(), "{fp}");
    let lufs = snd["integratedLufs"].as_f64().unwrap();
    assert!((-30.0..-10.0).contains(&lufs), "a -20 dB tone: {lufs}");
    assert!(snd["silenceShare"].as_f64().unwrap() < 0.1);
    let shares: f64 = ["speechShare", "musicShare", "ambienceShare"].iter().map(|k| snd[*k].as_f64().unwrap()).sum();
    assert!((shares - 1.0).abs() < 1e-4 || shares == 0.0, "{snd}");
    assert_eq!(snd["loudnessCurve"].as_array().unwrap().len(), 20);
}

#[test]
fn a_sound_only_file_is_measured_without_a_picture() {
    let dir = tmp_dir("ref-wav");
    let path = dir.join("tone.wav");
    wav(&path, 440.0, 65.0);
    let mut s = Session::default();
    let fp = measure(&mut s, json!({"path": path.to_string_lossy()}));
    assert_eq!(fp["name"], "tone", "the file's name without its extension");
    assert_eq!(fp["shots"]["count"], 1);
    assert!(fp["picture"].is_null() && fp["motion"].is_null());
    assert!((fp["sound"]["integratedLufs"].as_f64().unwrap() + 17.6).abs() < 1.5, "{}", fp["sound"]);
    assert!((fp["durationSeconds"].as_f64().unwrap() - 65.0).abs() < 0.2);
}

#[test]
fn a_measuring_job_runs_in_the_background_and_its_answer_is_asked_for_by_number() {
    let dir = tmp_dir("ref-job");
    let path = dir.join("film.mov");
    make_film(&path, &[(DemoScene::Dunes, 24), (DemoScene::Plasma, 24)], false);
    let mut s = Session::default();
    let r = s.execute("reference.measure", json!({"path": path.to_string_lossy(), "pictures": 8})).unwrap();
    let job = r["job"].as_u64().unwrap();
    assert!(r.get("fingerprint").is_none(), "not waited for");
    let t0 = std::time::Instant::now();
    let done = loop {
        let v = s.execute("reference.result", json!({"job": job})).unwrap();
        if v["state"] != "running" {
            break v;
        }
        assert!(t0.elapsed().as_secs() < 120, "the job never finished");
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["fingerprint"]["shots"]["count"], 2);
    let j = s.execute("jobs.list", json!({})).unwrap();
    let j = j.as_array().unwrap().iter().find(|x| x["id"] == job).unwrap().clone();
    assert_eq!(j["finished"], true);
    assert!(j["label"].as_str().unwrap().starts_with("Measure reference"));
}

#[test]
fn files_that_cannot_be_measured_say_why_and_leave_the_session_alone() {
    let dir = tmp_dir("ref-bad");
    let mut s = Session::default();
    let text = dir.join("notes.mov");
    std::fs::write(&text, "this is not a film").unwrap();
    let truncated = dir.join("cut-short.mov");
    let good = dir.join("good.mov");
    make_film(&good, &[(DemoScene::Dunes, 24), (DemoScene::Plasma, 24)], false);
    let bytes = std::fs::read(&good).unwrap();
    std::fs::write(&truncated, &bytes[..bytes.len() / 3]).unwrap();
    for (path, what) in [
        (dir.join("missing.mov").to_string_lossy().to_string(), "missing"),
        (dir.to_string_lossy().to_string(), "a folder"),
        (text.to_string_lossy().to_string(), "not a film"),
    ] {
        let e = s.execute("reference.measure", json!({"path": path, "wait": true})).unwrap_err().to_string();
        assert!(e.starts_with("could not open") || !e.is_empty(), "{what}: {e}");
    }
    // a file cut short is measured as far as it goes or refused: never a panic
    let _ = s.execute("reference.measure", json!({"path": truncated.to_string_lossy(), "wait": true}));
    for bad in [json!({}), json!({"path": ""}), json!({"path": 7}), json!({"path": "a\u{0}b"}), json!({"path": "x".repeat(5000)}), json!({"path": null})] {
        assert!(s.execute("reference.measure", bad.clone()).is_err(), "{bad}");
    }
    assert!(s.execute("reference.result", json!({})).is_err() && s.execute("reference.result", json!({"job": 999_999})).is_err());
}

#[test]
fn the_options_are_held_to_sensible_ranges() {
    use crate::reference::Options;
    let o = Options::from_params(&json!({}));
    assert_eq!((o.sample_fps, o.pictures, o.sensitivity), (8.0, 120, 50.0));
    let hi = Options::from_params(&json!({"sampleFps": 1e9, "pictures": u64::MAX, "sensitivity": 1e9}));
    assert_eq!((hi.sample_fps, hi.pictures, hi.sensitivity), (30.0, 600, 100.0));
    let lo = Options::from_params(&json!({"sampleFps": -3, "pictures": 0, "sensitivity": -9}));
    assert_eq!((lo.sample_fps, lo.pictures, lo.sensitivity), (1.0, 8, 0.0));
    let junk = Options::from_params(&json!({"sampleFps": "fast", "pictures": "many", "sensitivity": null}));
    assert_eq!((junk.sample_fps, junk.pictures, junk.sensitivity), (8.0, 120, 50.0));
    let _ = Options::from_params(&json!({"sampleFps": f64::NAN}));
}

#[test]
fn a_stopped_measurement_keeps_nothing() {
    use crate::reference::{Options, measure};
    let dir = tmp_dir("ref-stop");
    let path = dir.join("film.mov");
    make_film(&path, &[(DemoScene::Dunes, 48), (DemoScene::Plasma, 48)], false);
    let s = Session::default();
    let src = s.media.open_file(&path.to_string_lossy(), &*s.services).unwrap();
    let prog = filmcraft_export::Progress::default();
    prog.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    let r = measure(&src, "x", &Options::from_params(&json!({})), &prog);
    assert_eq!(r.err().as_deref(), Some("stopped"));
    let _ = PathBuf::new();
}

fn small_film(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    make_film(&path, &[(DemoScene::Dunes, 48), (DemoScene::Plasma, 48)], false);
    path
}

fn session_in(dir: &Path) -> Session {
    Session { prefs_path: Some(dir.join("preferences.json")), ..Default::default() }
}

fn add(s: &mut Session, path: &Path, rights: &str) -> Value {
    s.execute("reference.add", json!({"path": path.to_string_lossy(), "rights": rights, "wait": true, "pictures": 8, "sampleFps": 8})).unwrap()
}

#[test]
fn a_film_is_added_to_the_library_with_its_note_and_is_there_for_the_next_session() {
    let dir = tmp_dir("ref-lib");
    let film = small_film(&dir, "Dune Walk.mov");
    let mut s = session_in(&dir);
    assert!(s.execute("reference.list", json!({})).unwrap()["references"].as_array().unwrap().is_empty());
    let r = add(&mut s, &film, "Public domain (made for this test)");
    let id = r["reference"].as_str().unwrap().to_string();
    assert_eq!(r["fingerprint"]["name"], "Dune Walk");
    let list = s.execute("reference.list", json!({})).unwrap();
    let row = &list["references"][0];
    assert_eq!(
        (row["id"].as_str(), row["name"].as_str(), row["rights"].as_str()),
        (Some(id.as_str()), Some("Dune Walk"), Some("Public domain (made for this test)"))
    );
    assert_eq!(row["shots"]["count"], 2);
    assert!(row["durationSeconds"].as_f64().unwrap() > 3.0 && row["arc"].is_string() && row["addedUnix"].as_i64().unwrap() > 0);
    // the whole fingerprint comes from `get`
    let got = s.execute("reference.get", json!({"id": id})).unwrap();
    assert_eq!(got["fingerprint"]["shots"]["count"], 2);
    // another session (another run of the app) finds it in the global file, and the file holds numbers only
    let mut later = session_in(&dir);
    assert_eq!(later.execute("reference.list", json!({})).unwrap()["references"][0]["name"], "Dune Walk");
    let text = std::fs::read_to_string(dir.join("edith-references.json")).unwrap();
    assert!(text.len() < 30_000 && !text.contains(&*dir.to_string_lossy()), "no path, no footage: {} bytes", text.len());
    // a project does not need to be open: the library is not in any project
    assert!(later.project.ext.is_empty() || !later.project.ext.contains_key("edith.references"));
    // rename and remove, and they persist
    later.execute("reference.rename", json!({"id": id, "name": "  A slow walk  "})).unwrap();
    assert_eq!(session_in(&dir).execute("reference.get", json!({"id": id})).unwrap()["name"], "A slow walk");
    later.execute("reference.remove", json!({"id": id})).unwrap();
    assert!(session_in(&dir).execute("reference.list", json!({})).unwrap()["references"].as_array().unwrap().is_empty());
}

#[test]
fn a_note_of_why_the_film_may_be_used_is_required_and_nothing_is_measured_without_it() {
    let dir = tmp_dir("ref-rights");
    let film = small_film(&dir, "film.mov");
    let mut s = session_in(&dir);
    for bad in [
        json!({}),
        json!({"rights": ""}),
        json!({"rights": "   "}),
        json!({"rights": 5}),
        json!({"rights": null}),
        json!({"rights": "x".repeat(301)}),
        json!({"rights": "\u{7}\u{7}"}),
    ] {
        let mut p = json!({"path": film.to_string_lossy(), "wait": true});
        p.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
        let e = s.execute("reference.add", p).unwrap_err().to_string();
        assert!(e.contains("rights"), "{bad}: {e}");
    }
    assert!(s.jobs.iter().all(|j| !j.label.starts_with("Add reference")), "no job was started");
    assert!(s.execute("reference.list", json!({})).unwrap()["references"].as_array().unwrap().is_empty());
    // the note is cleaned of control characters and kept
    let r = add(&mut s, &film, "CC-BY\u{7} 4.0, from the archive");
    assert_eq!(s.execute("reference.get", json!({"id": r["reference"]})).unwrap()["rights"], "CC-BY 4.0, from the archive");
}

#[test]
fn a_fingerprint_is_exported_without_footage_and_imported_back_and_hostile_files_are_refused() {
    let dir = tmp_dir("ref-share");
    let film = small_film(&dir, "film.mov");
    let mut s = session_in(&dir);
    let id = add(&mut s, &film, "Own export")["reference"].as_str().unwrap().to_string();
    let out = dir.join("film.fingerprint.json");
    let r = s.execute("reference.export", json!({"id": id, "path": out.to_string_lossy()})).unwrap();
    assert!(r["bytes"].as_u64().unwrap() < 30_000);
    let text = std::fs::read_to_string(&out).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!((v["format"].as_str(), v["rights"].as_str()), (Some("edith-fingerprint"), Some("Own export")));
    // into another library: the same numbers, with the note that came with it
    let other_dir = tmp_dir("ref-share-other");
    let mut other = session_in(&other_dir);
    let imported = other.execute("reference.import", json!({"path": out.to_string_lossy()})).unwrap();
    let got = other.execute("reference.get", json!({"id": imported["id"]})).unwrap();
    assert_eq!(got["fingerprint"], s.execute("reference.get", json!({"id": id})).unwrap()["fingerprint"]);
    assert_eq!(got["rights"], "Own export");
    // an import with no note says so
    let mut bare = v.clone();
    bare.as_object_mut().unwrap().remove("rights");
    let p = dir.join("bare.json");
    std::fs::write(&p, bare.to_string()).unwrap();
    let r = other.execute("reference.import", json!({"path": p.to_string_lossy()})).unwrap();
    assert!(other.execute("reference.get", json!({"id": r["id"]})).unwrap()["rights"].as_str().unwrap().contains("no note"));
    // hostile files
    let write = |name: &str, body: &str| {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    };
    let mut nan = v.clone();
    nan["fingerprint"]["rhythm"]["curve"] = json!([1.0, 2.0]);
    let mut huge = v.clone();
    huge["fingerprint"]["picture"]["palette"] = json!(vec![json!({"lab": [0.0, 0.0, 0.0], "rgb": [0.0, 0.0, 0.0], "share": 0.1}); 500]);
    let mut wrong_version = v.clone();
    wrong_version["fingerprint"]["version"] = json!(99);
    let big = write("big.json", &" ".repeat(3 * 1024 * 1024));
    for (what, path) in [
        ("not json", write("a.json", "{ nope")),
        ("an array", write("b.json", "[1, 2, 3]")),
        ("other format", write("c.json", r#"{"format": "something-else"}"#)),
        ("no fingerprint", write("d.json", r#"{"format": "edith-fingerprint"}"#)),
        ("short curve", write("e.json", &nan.to_string())),
        ("huge palette", write("f.json", &huge.to_string())),
        ("another version", write("g.json", &wrong_version.to_string())),
        ("too big", big),
        ("missing", dir.join("missing.json")),
    ] {
        assert!(other.execute("reference.import", json!({"path": path.to_string_lossy()})).is_err(), "{what} should be refused");
    }
    let n = other.execute("reference.list", json!({})).unwrap()["references"].as_array().unwrap().len();
    assert_eq!(n, 2, "only the two good imports are in the library");
    // exporting what is not there, or where it cannot go
    assert!(s.execute("reference.export", json!({"id": "nope", "path": out.to_string_lossy()})).is_err());
    assert!(s.execute("reference.export", json!({"id": id, "path": dir.join("no/such/folder/x.json").to_string_lossy()})).is_err());
}

#[test]
fn the_library_is_capped_and_a_damaged_file_is_an_empty_library_not_a_crash() {
    let dir = tmp_dir("ref-cap");
    let film = small_film(&dir, "film.mov");
    let mut s = session_in(&dir);
    let id = add(&mut s, &film, "Own")["reference"].as_str().unwrap().to_string();
    let out = dir.join("fp.json");
    s.execute("reference.export", json!({"id": id, "path": out.to_string_lossy()})).unwrap();
    for _ in 0..49 {
        s.execute("reference.import", json!({"path": out.to_string_lossy()})).unwrap();
    }
    assert_eq!(s.execute("reference.list", json!({})).unwrap()["references"].as_array().unwrap().len(), 50);
    let e = s.execute("reference.import", json!({"path": out.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("50"), "{e}");
    let e = s.execute("reference.add", json!({"path": film.to_string_lossy(), "rights": "Own", "wait": true})).unwrap_err().to_string();
    assert!(e.contains("50"), "refused before anything is measured: {e}");
    // ids are never reused, even after a removal
    s.execute("reference.remove", json!({"id": id})).unwrap();
    let again = s.execute("reference.import", json!({"path": out.to_string_lossy()})).unwrap();
    assert_ne!(again["id"], json!(id));
    // a damaged or hostile file on disk: an empty library, and a later add writes a good one
    let bad_dir = tmp_dir("ref-damaged");
    let file = bad_dir.join("edith-references.json");
    for junk in ["{ not json", "[]", "null", "{\"references\": 5}", "\u{0}\u{1}", &"x".repeat(9 * 1024 * 1024)] {
        std::fs::write(&file, junk).unwrap();
        let mut d = session_in(&bad_dir);
        assert!(d.execute("reference.list", json!({})).unwrap()["references"].as_array().unwrap().is_empty(), "{}", junk.chars().take(20).collect::<String>());
    }
    // a file with one good entry, one with a bad fingerprint, one repeated id, one with a hostile id
    let good = s.execute("reference.get", json!({"id": "r2"})).unwrap();
    let entry = |id: &str, fp: Value| json!({"id": id, "name": "n", "rights": "r", "addedUnix": 1, "fingerprint": fp});
    let mut broken = good["fingerprint"].clone();
    broken["rhythm"]["curve"] = json!([]);
    let lib = json!({"version": 1, "nextId": 9, "references": [entry("a1", good["fingerprint"].clone()), entry("a2", broken), entry("a1", good["fingerprint"].clone()), entry("../x", good["fingerprint"].clone())]});
    std::fs::write(&file, lib.to_string()).unwrap();
    let mut d = session_in(&bad_dir);
    let refs = d.execute("reference.list", json!({})).unwrap();
    assert_eq!(refs["references"].as_array().unwrap().len(), 1, "{refs}");
    assert_eq!(refs["references"][0]["id"], "a1");
}

#[test]
fn hostile_library_parameters_are_refused() {
    let mut s = Session::default();
    for cmd in ["reference.get", "reference.remove"] {
        for bad in [json!({}), json!({"id": ""}), json!({"id": 5}), json!({"id": "x".repeat(50)}), json!({"id": "nope"}), json!({"id": null})] {
            assert!(s.execute(cmd, bad.clone()).is_err(), "{cmd} {bad}");
        }
    }
    for bad in [
        json!({"id": "r1"}),
        json!({"id": "r1", "name": ""}),
        json!({"id": "r1", "name": "x".repeat(500)}),
        json!({"id": "r1", "name": 4}),
        json!({"name": "ok"}),
    ] {
        assert!(s.execute("reference.rename", bad.clone()).is_err(), "{bad}");
    }
    for bad in [json!({}), json!({"path": ""}), json!({"path": "a\u{0}b"}), json!({"path": 3})] {
        assert!(s.execute("reference.import", bad.clone()).is_err(), "{bad}");
        assert!(s.execute("reference.export", json!({"id": "r1", "path": bad["path"].clone()})).is_err());
    }
}

fn with_reference(dir: &Path, shots: &[(DemoScene, i64)], tone: bool, sess: Session) -> (Session, String) {
    let film = dir.join("ref.mov");
    make_film(&film, shots, tone);
    let mut s = Session { prefs_path: Some(dir.join("preferences.json")), ..sess };
    let r = s
        .execute(
            "reference.add",
            json!({"path": film.to_string_lossy(), "rights": "Own export", "name": "Slow film", "wait": true, "pictures": 8, "sampleFps": 8}),
        )
        .unwrap();
    (s, r["reference"].as_str().unwrap().to_string())
}

#[test]
fn your_edit_is_compared_with_a_reference_in_words_and_more_rows_appear_as_you_measure() {
    let dir = tmp_dir("ref-compare");
    // the reference: shots of 2 s, with sound
    let (mut s, id) = with_reference(
        &dir,
        &[(DemoScene::Dunes, 48), (DemoScene::Plasma, 48), (DemoScene::Forest, 48)],
        true,
        crate::analysis_tests::edit_of(&[6.0, 6.0, 6.0, 6.0, 6.0, 6.0]),
    );
    let r = s.execute("reference.compare", json!({"id": id})).unwrap();
    assert_eq!((r["reference"]["name"].as_str(), r["sequence"]["name"].as_str()), (Some("Slow film"), Some("Coach test")));
    let rows = r["comparison"]["rows"].as_array().unwrap();
    let mean = rows.iter().find(|x| x["metric"] == "meanShot").unwrap();
    assert_eq!(mean["verdict"], "higher", "{mean}");
    assert!(mean["words"].as_str().unwrap().contains("yours 6.0 s") && mean["words"].as_str().unwrap().contains("times as long"), "{mean}");
    assert_eq!(r["measured"], json!({"pictures": false, "mix": false}));
    let missing: Vec<&str> = r["comparison"]["missing"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(missing.iter().any(|m| m.contains("measure your shots")) && missing.iter().any(|m| m.contains("measure your mix")), "{missing:?}");
    assert!(rows.iter().all(|x| x["group"] == "shots" || x["group"] == "rhythm"));
    // measure the shots and the mix, and the picture and sound rows appear
    s.execute("analysis.run", json!({"sequence": true, "wait": true})).unwrap();
    s.execute("analysis.run", json!({"mix": true, "wait": true})).unwrap();
    let r = s.execute("reference.compare", json!({"id": id})).unwrap();
    assert_eq!(r["measured"], json!({"pictures": true, "mix": true}));
    let metrics: Vec<&str> = r["comparison"]["rows"].as_array().unwrap().iter().filter_map(|x| x["metric"].as_str()).collect();
    for m in ["brightness", "contrast", "loudness", "silence"] {
        assert!(metrics.contains(&m), "{m} in {metrics:?}");
    }
    // errors: no such reference, none adopted, no such sequence, not a sequence
    assert!(s.execute("reference.compare", json!({"id": "nope"})).is_err());
    assert!(s.execute("reference.compare", json!({})).is_err(), "nothing adopted and no id");
    assert!(s.execute("reference.compare", json!({"id": id, "sequence": 999_999})).is_err());
    assert!(s.execute("reference.compare", json!({"id": id, "sequence": "x"})).is_err());
    assert!(Session::default().execute("reference.compare", json!({"id": id})).is_err());
}

#[test]
fn an_adopted_reference_becomes_the_coachs_target_for_this_project_and_is_one_undo_step() {
    let dir = tmp_dir("ref-adopt");
    let (mut s, id) = with_reference(
        &dir,
        &[(DemoScene::Dunes, 48), (DemoScene::Plasma, 48), (DemoScene::Forest, 48)],
        true,
        crate::analysis_tests::edit_of(&[6.0, 6.0, 6.0, 6.0, 6.0, 6.0]),
    );
    assert!(s.execute("reference.adopted", json!({})).unwrap()["adopted"].is_null());
    let before = s.execute("coach.findings", json!({})).unwrap();
    assert!(before["targets"]["reference"].is_null() && !before["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "paceVsPlan"));
    let undo = s.history.undo.len();
    let r = s.execute("reference.adopt", json!({"id": id})).unwrap();
    assert_eq!(r["adopted"]["name"], "Slow film");
    assert!((r["adopted"]["targets"]["averageShotSeconds"].as_f64().unwrap() - 2.0).abs() < 0.3, "{r}");
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    // the coach now measures against it, and says so
    let after = s.execute("coach.findings", json!({})).unwrap();
    assert_eq!(after["targets"]["reference"], "Slow film");
    assert!((after["targets"]["averageShotSeconds"].as_f64().unwrap() - 2.0).abs() < 0.3);
    let pace = after["findings"].as_array().unwrap().iter().find(|f| f["kind"] == "paceVsPlan").unwrap_or_else(|| panic!("{}", after["findings"]));
    assert!(pace["title"].as_str().unwrap().contains("slower than the reference's"), "{pace}");
    let text = crate::assistant_tools::tools().into_iter().find(|d| d.spec.name == "coach_findings").map(|d| (d.run)(&s, &json!({})).content).unwrap();
    assert!(text.contains("Targets from the reference “Slow film”"), "{text}");
    // the project keeps the numbers: the library entry can go and the adoption stays
    s.execute("reference.remove", json!({"id": id})).unwrap();
    assert_eq!(s.execute("reference.adopted", json!({})).unwrap()["adopted"]["name"], "Slow film");
    assert_eq!(s.execute("coach.findings", json!({})).unwrap()["targets"]["reference"], "Slow film");
    // compare with no id uses the adopted one
    assert!(s.execute("reference.compare", json!({})).is_ok());
    // undo, redo
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.execute("reference.adopted", json!({})).unwrap()["adopted"].is_null());
    s.execute("edit.redo", json!({})).unwrap();
    assert!(s.execute("reference.adopted", json!({})).unwrap()["adopted"].is_object());
    // stop using it: one more undo step, and clearing when nothing is adopted changes nothing
    let n = s.history.undo.len();
    s.execute("reference.adopt", json!({"clear": true})).unwrap();
    assert_eq!(s.history.undo.len(), n + 1);
    assert!(s.execute("coach.findings", json!({})).unwrap()["targets"]["reference"].is_null());
    s.execute("reference.adopt", json!({"clear": true})).unwrap();
    assert_eq!(s.history.undo.len(), n + 1, "nothing to clear");
    // hostile
    for bad in [json!({}), json!({"id": "nope"}), json!({"id": 5}), json!({"id": ""})] {
        assert!(s.execute("reference.adopt", bad.clone()).is_err(), "{bad}");
    }
}

#[test]
fn a_damaged_adoption_in_a_project_is_ignored_and_the_coach_carries_on() {
    let mut s = crate::analysis_tests::edit_of(&[6.0, 6.0, 6.0, 6.0, 6.0, 6.0]);
    for junk in
        [json!(null), json!(5), json!("x"), json!({"id": "r1"}), json!({"id": "r1", "name": "n", "rights": "r", "fingerprint": {"version": 1}}), json!([1, 2])]
    {
        std::sync::Arc::make_mut(&mut s.project).ext.insert("edith.reference".into(), junk.clone());
        assert!(s.execute("reference.adopted", json!({})).unwrap()["adopted"].is_null(), "{junk}");
        let c = s.execute("coach.findings", json!({})).unwrap();
        assert!(c["targets"]["reference"].is_null(), "{junk}");
    }
}

fn tool(s: &Session, name: &str, input: Value) -> edith_ai::ToolOutput {
    s.run_assistant_tool(name, &input)
}

#[test]
fn the_assistant_can_read_the_library_one_reference_and_a_comparison_and_cannot_change_any_of_it() {
    let tools = crate::assistant_tools::tools();
    for name in ["reference_list", "reference_get", "reference_compare"] {
        assert!(tools.iter().any(|d| d.spec.name == name && d.read_only), "{name}");
    }
    let dir = tmp_dir("ref-tools");
    let (mut s, id) = with_reference(
        &dir,
        &[(DemoScene::Dunes, 48), (DemoScene::Plasma, 48), (DemoScene::Forest, 48)],
        true,
        crate::analysis_tests::edit_of(&[6.0, 6.0, 6.0, 6.0, 6.0, 6.0]),
    );
    let rev = s.revision;
    // list
    let list = tool(&s, "reference_list", json!({})).content;
    assert!(list.contains(&id) && list.contains("“Slow film”") && list.contains("rights note: Own export") && !list.contains("adopted"), "{list}");
    assert!(tool(&Session::default(), "reference_list", json!({})).content.contains("There are no references yet"));
    // get: by id, and the readings that go with it
    let got = tool(&s, "reference_get", json!({"id": id}));
    assert!(!got.is_error, "{}", got.content);
    for want in ["“Slow film”", "Shots: 3 in all", "Pace by thirds", "Picture: brightness", "Sound: loudness", "What the numbers suggest", "cutting"] {
        assert!(got.content.contains(want), "{want} in {}", got.content);
    }
    assert!(tool(&s, "reference_get", json!({})).is_error, "nothing adopted and no id");
    assert!(tool(&s, "reference_get", json!({"id": "nope"})).is_error);
    // compare names what differs and what could not be compared
    let c = tool(&s, "reference_compare", json!({"id": id}));
    assert!(!c.is_error, "{}", c.content);
    assert!(c.content.contains("“Coach test” against the reference “Slow film”") && c.content.contains("[higher] Average shot: yours 6.0 s"), "{}", c.content);
    assert!(c.content.contains("Not compared:") && c.content.contains("analysis_run with sequence true"), "{}", c.content);
    assert!(c.content.contains("What the reference's numbers suggest"), "{}", c.content);
    // once adopted the others say so, and the default reference is the adopted one
    s.execute("reference.adopt", json!({"id": id})).unwrap();
    let rev_after_adopt = s.revision;
    assert!(tool(&s, "reference_list", json!({})).content.contains("(adopted as this project's target)"));
    assert!(tool(&s, "reference_get", json!({})).content.contains("“Slow film”"));
    assert!(tool(&s, "reference_compare", json!({})).content.contains("adopted as this project's target"));
    // reading changes nothing, and hostile input is an error or text, never a panic
    assert_eq!(s.revision, rev_after_adopt);
    assert!(rev_after_adopt >= rev);
    for name in ["reference_list", "reference_get", "reference_compare"] {
        for odd in [
            json!(null),
            json!([]),
            json!("x"),
            json!(7),
            json!({"id": 5}),
            json!({"id": ""}),
            json!({"sequence": "x"}),
            json!({"sequence": -1}),
            json!({"id": "r1", "sequence": 1e30}),
        ] {
            let _ = tool(&s, name, odd);
        }
    }
    assert!(tool(&Session::default(), "reference_compare", json!({"id": "r1"})).is_error);
}

#[test]
fn the_compare_view_holds_the_rows_that_differ_most_or_the_ones_asked_for() {
    let dir = tmp_dir("ref-view-compare");
    let (mut s, id) = with_reference(
        &dir,
        &[(DemoScene::Dunes, 48), (DemoScene::Plasma, 48), (DemoScene::Forest, 48)],
        true,
        crate::analysis_tests::edit_of(&[6.0, 6.0, 6.0, 6.0, 6.0, 6.0]),
    );
    let show = |s: &Session, p: Value| tool(s, "show", p);
    // no reference named and none adopted
    assert!(show(&s, json!({"view": "compare"})).is_error);
    let r = show(&s, json!({"view": "compare", "reference": id, "caption": "Against the slow film"}));
    assert!(!r.is_error, "{}", r.content);
    let v = r.view.clone().unwrap();
    assert_eq!((v["view"].as_str(), v["reference"].as_str(), v["caption"].as_str()), (Some("compare"), Some("Slow film"), Some("Against the slow film")));
    let rows = v["rows"].as_array().unwrap();
    assert!(!rows.is_empty() && rows.len() <= 8);
    assert!(rows.iter().any(|x| x["metric"] == "meanShot" && x["verdict"] == "higher"), "{rows:?}");
    assert!(rows.iter().all(|x| x["yoursText"].is_string() && x["theirsText"].is_string() && x["words"].is_string()));
    assert!(v["missing"].as_array().is_some_and(|m| !m.is_empty()), "the unmeasured parts are named");
    assert!(r.content.starts_with("Shown to the person: ") && !r.content.contains("yoursText"), "{}", r.content);
    // chosen rows, in the order asked
    let picked = show(&s, json!({"view": "compare", "reference": id, "rows": ["cutsPerMinute", "meanShot"]})).view.unwrap();
    let metrics: Vec<&str> = picked["rows"].as_array().unwrap().iter().filter_map(|x| x["metric"].as_str()).collect();
    assert_eq!(metrics, ["cutsPerMinute", "meanShot"]);
    // a row that is not there says which ones are
    let bad = show(&s, json!({"view": "compare", "reference": id, "rows": ["mood"]}));
    assert!(bad.is_error && bad.content.contains("meanShot"), "{}", bad.content);
    for odd in [
        json!({"view": "compare", "reference": 5}),
        json!({"view": "compare", "reference": "nope"}),
        json!({"view": "compare", "reference": id, "rows": "x"}),
        json!({"view": "compare", "reference": id, "rows": [1]}),
        json!({"view": "compare", "reference": id, "rows": vec!["meanShot"; 50]}),
    ] {
        assert!(show(&s, odd.clone()).is_error, "{odd}");
    }
    // the adopted reference is the default
    s.execute("reference.adopt", json!({"id": id})).unwrap();
    assert!(show(&s, json!({"view": "compare"})).view.is_some());
    assert!(show(&Session::default(), json!({"view": "compare"})).is_error);
}

#[test]
fn the_rhythm_view_sets_the_cutting_beside_the_planned_intensity_and_a_references_rhythm() {
    let dir = tmp_dir("ref-view-rhythm");
    let (mut s, id) = with_reference(
        &dir,
        &[(DemoScene::Dunes, 48), (DemoScene::Plasma, 48), (DemoScene::Forest, 48)],
        false,
        crate::analysis_tests::edit_of(&[2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 12.0, 12.0, 12.0, 12.0]),
    );
    let show = |s: &Session, p: Value| tool(s, "show", p);
    // no plan and no reference: the cutting alone
    let r = show(&s, json!({"view": "rhythm"}));
    assert!(!r.is_error, "{}", r.content);
    let v = r.view.clone().unwrap();
    assert!(v["intensity"].as_array().unwrap().is_empty() && v["reference"].is_null());
    let cut: Vec<f64> = v["cut"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
    assert!(cut.iter().all(|c| (0.0..=1.0).contains(c)) && cut.iter().any(|c| (*c - 1.0).abs() < 1e-9), "{cut:?}");
    assert!(r.content.contains("there is no plan"), "{}", r.content);
    // with a plan and a reference named
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 1})).unwrap();
    let v = show(&s, json!({"view": "rhythm", "reference": id})).view.unwrap();
    let beats = v["intensity"].as_array().unwrap();
    assert!(
        !beats.is_empty() && beats.iter().all(|b| (0.0..=1.0).contains(&b["intensity"].as_f64().unwrap()) && b["start"].as_i64() <= b["end"].as_i64()),
        "{beats:?}"
    );
    assert_eq!(v["reference"]["name"], "Slow film");
    let curve = v["reference"]["curve"].as_array().unwrap();
    assert_eq!(curve.len(), 20);
    assert!(curve.iter().all(|c| (0.0..=1.0 + 1e-9).contains(&c.as_f64().unwrap())));
    // an unknown reference is refused; with none adopted and none named there is simply no reference line
    assert!(show(&s, json!({"view": "rhythm", "reference": "nope"})).is_error);
    assert!(show(&s, json!({"view": "rhythm"})).view.unwrap()["reference"].is_null());
    s.execute("reference.adopt", json!({"id": id})).unwrap();
    assert!(show(&s, json!({"view": "rhythm"})).view.unwrap()["reference"].is_object(), "the adopted one is used");
    // one shot has no rhythm
    assert!(show(&crate::analysis_tests::edit_of(&[5.0]), json!({"view": "rhythm"})).is_error);
}
