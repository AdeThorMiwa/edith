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
