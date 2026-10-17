//! Tests of [`crate::analysis`]: measuring footage, sound and an edit as jobs, the cache and when it
//! goes stale, and hostile input.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use filmcraft_project::ItemKind;
use serde_json::{Value, json};

use crate::Session;
use crate::analysis::{Cache, valid_key};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("edith-analysis-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
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

fn import(s: &mut Session, path: &Path) -> u64 {
    let r = s.execute("file.import", json!({"paths": [path.to_string_lossy()]})).unwrap();
    r["items"][0].as_u64().unwrap()
}

fn footage(s: &Session) -> u64 {
    s.project.items.values().find(|i| matches!(&i.kind, ItemKind::Media(m) if m.info.video.is_some())).map(|i| i.id.0).expect("footage in the demo project")
}

fn analyse(s: &mut Session, params: Value) -> Value {
    let mut p = params;
    p["wait"] = json!(true);
    s.execute("analysis.run", p).unwrap()
}

fn list(s: &mut Session) -> Vec<Value> {
    s.execute("analysis.list", json!({})).unwrap()["reports"].as_array().unwrap().clone()
}

#[test]
fn the_commands_exist_and_do_not_touch_the_project() {
    let all = crate::commands::command_specs();
    for id in ["analysis.run", "analysis.get", "analysis.list", "analysis.clear"] {
        let spec = all.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("no {id}"));
        assert!(!spec.journal, "{id} changes no project");
    }
    let mut s = demo();
    let (rev, undo) = (s.revision, s.history.undo.len());
    let item = footage(&s);
    analyse(&mut s, json!({"item": item, "samples": 2}));
    s.execute("analysis.clear", json!({"all": true})).unwrap();
    assert_eq!((s.revision, s.history.undo.len()), (rev, undo), "analysing is not an edit");
}

#[test]
fn footage_is_measured_in_pictures_with_the_motion_around_each() {
    let mut s = demo();
    let item = footage(&s);
    let r = analyse(&mut s, json!({"item": item, "samples": 5}));
    assert_eq!(r["cached"], false);
    let report = &r["report"];
    assert_eq!((report["kind"].as_str(), report["reportVersion"].as_u64()), (Some("item"), Some(1)));
    assert!(report["algorithms"]["image"].is_u64() && report["algorithms"]["audio"].is_u64());
    let v = &report["video"];
    assert_eq!(v["samples"].as_array().unwrap().len(), 5);
    assert_eq!(v["failed"], 0);
    for sample in v["samples"].as_array().unwrap() {
        let st = &sample["stats"];
        assert!(st["lumaMean"].as_f64().unwrap() > 0.0 && !st["palette"].as_array().unwrap().is_empty(), "{sample}");
    }
    let sum = &v["summary"];
    let luma = sum["lumaMean"].as_f64().unwrap();
    assert!((0.0..=1.0).contains(&luma) && sum["contrast"].as_f64().unwrap() > 0.0);
    assert!(sum["palette"].as_array().unwrap().len() <= 5);
    assert!(sum["motion"]["camera"].is_string());
    // the times of the samples are spread over the clip, in order
    let times: Vec<f64> = v["samples"].as_array().unwrap().iter().map(|x| x["time"].as_f64().unwrap()).collect();
    assert!(times.windows(2).all(|w| w[1] > w[0]), "{times:?}");
}

#[test]
fn sound_is_measured_in_segments_and_a_report_is_cached_until_the_file_changes() {
    let dir = scratch("wav");
    let path = dir.join("tone.wav");
    wav(&path, 440.0, 70.0);
    let mut s = Session::default();
    let item = import(&mut s, &path);
    let first = analyse(&mut s, json!({"item": item}));
    assert_eq!(first["cached"], false);
    let report = &first["report"];
    assert!(report["video"].is_null());
    let audio = &report["audio"];
    assert!(audio["stats"]["integratedLufs"].as_f64().unwrap() < -10.0);
    assert!(audio["stats"]["spectral"]["mid"].as_f64().unwrap() > 0.95, "440 Hz is mid: {}", audio["stats"]["spectral"]);
    assert_eq!(audio["segments"].as_array().unwrap().len(), 3, "70 s in 30 s segments: 30, 30, 10");
    assert_eq!(audio["truncated"], false);
    let key = first["key"].as_str().unwrap().to_string();

    // the same again is served from the cache, with no job
    let again = s.execute("analysis.run", json!({"item": item, "wait": true})).unwrap();
    assert_eq!((again["cached"].clone(), again["job"].clone(), again["key"].as_str()), (json!(true), Value::Null, Some(key.as_str())));
    assert_eq!(s.execute("analysis.get", json!({"item": item})).unwrap()["key"], json!(key));
    // read back from the cache's text, so floats may differ in the last digit
    let cached = s.execute("analysis.get", json!({"key": key})).unwrap();
    assert!((cached["audio"]["stats"]["integratedLufs"].as_f64().unwrap() - audio["stats"]["integratedLufs"].as_f64().unwrap()).abs() < 1e-9);
    assert_eq!(cached["audio"]["segments"].as_array().unwrap().len(), 3);
    let l = list(&mut s);
    assert_eq!(l.len(), 1);
    assert_eq!((l[0]["current"].clone(), l[0]["kind"].clone(), l[0]["target"]["id"].clone()), (json!(true), json!("item"), json!(item)));
    assert!(l[0]["bytes"].as_u64().unwrap() > 100);

    // another number of samples is another report; `force` measures again under the same key
    let forced = s.execute("analysis.run", json!({"item": item, "wait": true, "force": true})).unwrap();
    assert_eq!((forced["cached"].clone(), forced["key"].as_str()), (json!(false), Some(key.as_str())));

    // the file changes (same size, other sound): the old report is no longer current and is not returned
    wav(&path, 1760.0, 70.0);
    let l = list(&mut s);
    assert_eq!(l[0]["current"], false, "{l:?}");
    assert!(s.execute("analysis.get", json!({"item": item})).is_err(), "never an old report for new content");
    let fresh = analyse(&mut s, json!({"item": item}));
    assert_ne!(fresh["key"], first["key"]);
    assert_eq!(list(&mut s).len(), 2);
    let cleared = s.execute("analysis.clear", json!({"stale": true})).unwrap();
    assert_eq!(cleared["removed"], 1);
    let l = list(&mut s);
    assert_eq!((l.len(), l[0]["current"].clone()), (1, json!(true)));
    assert_eq!(s.execute("analysis.clear", json!({"item": item})).unwrap()["removed"], 1);
    assert!(list(&mut s).is_empty());
}

#[test]
fn a_sequence_is_measured_from_its_layout_and_each_shot_is_looked_at() {
    let mut s = demo();
    let r = analyse(&mut s, json!({"sequence": true, "maxShots": 4}));
    let report = &r["report"];
    assert_eq!(report["kind"], "sequence");
    let q = &report["sequence"];
    assert!(q["shots"]["count"].as_u64().unwrap() > 0, "{q}");
    assert!(q["durationSeconds"].as_f64().unwrap() > 0.0);
    let analysed = report["shotsAnalysed"].as_u64().unwrap();
    assert!(analysed <= 4 && analysed > 0);
    assert_eq!(report["shotsTotal"], q["shots"]["count"]);
    let shots = report["shots"].as_array().unwrap();
    assert_eq!(shots.len() as u64, analysed);
    assert!(shots.iter().any(|x| x["stats"]["lumaMean"].is_f64()), "{shots:?}");
    assert!(shots.windows(2).all(|w| w[1]["start"].as_f64() >= w[0]["start"].as_f64()));

    // without the pictures it is instant and needs no media
    let bare = analyse(&mut s, json!({"sequence": true, "shots": false}));
    assert_eq!(bare["report"]["shots"].as_array().unwrap().len(), 0);
    assert_eq!(bare["report"]["sequence"], report["sequence"]);
    assert_ne!(bare["key"], r["key"], "other options, other report");

    // an edit changes the key: the old report is stale, and clearing stale removes only that
    let key = r["key"].as_str().unwrap().to_string();
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("timeline.select", json!({"clips": [clip.0]})).unwrap();
    s.execute("edit.clear", json!({})).unwrap();
    let l = list(&mut s);
    let old = l.iter().find(|e| e["key"] == json!(key)).unwrap();
    assert_eq!(old["current"], false);
    assert!(
        s.execute("analysis.get", json!({"sequence": true})).is_err() || s.execute("analysis.get", json!({"sequence": true})).unwrap()["key"] != json!(key)
    );
    let removed = s.execute("analysis.clear", json!({"stale": true})).unwrap()["removed"].as_u64().unwrap();
    assert!(removed >= 2, "both reports of the edit before the delete are stale: removed {removed}");
    // undo brings the old layout back, and so a measurement of it is current again once repeated
    s.execute("edit.undo", json!({})).unwrap();
    let again = analyse(&mut s, json!({"sequence": true, "maxShots": 4}));
    assert_eq!(again["key"], json!(key), "the same edit has the same key");
}

#[test]
fn narration_and_transitions_reach_the_sequence_numbers() {
    let mut s = demo();
    let none = analyse(&mut s, json!({"sequence": true, "shots": false}));
    assert_eq!(none["report"]["sequence"]["narration"]["words"], 0);
    let transitions_before = none["report"]["sequence"]["transitions"]["count"].as_u64().unwrap();
    assert!(transitions_before > 0, "the demo has transitions: {}", none["report"]["sequence"]["transitions"]);
    // a narration (its script is kept with the item the voice made) on the first audio clip
    let seq = s.active_sequence().unwrap();
    let a = seq.audio_tracks.iter().flat_map(|t| &t.items).next().expect("an audio clip in the demo").clone();
    let narration = filmcraft_project::Narration {
        text: "one two three [pause 1s] four five six".into(),
        language: "en-US".into(),
        voice: "test".into(),
        pitch: Default::default(),
        pace: 1.0,
        take: 0,
        speech_duration: a.duration,
    };
    Arc::make_mut(&mut s.project).narrations.insert(a.item, narration);
    let r = analyse(&mut s, json!({"sequence": true, "shots": false}));
    assert_ne!(r["key"], none["key"], "a narration is part of what is measured, so it changes the key");
    let q = &r["report"]["sequence"];
    assert_eq!(q["narration"]["words"], 6, "the pause marker is not a word: {}", q["narration"]);
    let wpm = q["narration"]["wordsPerMinute"].as_f64().unwrap();
    let expected = 6.0 / (a.duration.0 as f64 / filmcraft_time::TICKS_PER_SECOND as f64 / 60.0);
    assert!((wpm - expected).abs() < 1e-6, "{wpm} vs {expected}");
    let in_project: usize = s.active_sequence().unwrap().video_tracks.iter().map(|t| t.transitions.len()).sum();
    assert_eq!((q["transitions"]["count"].as_u64().unwrap(), transitions_before), (in_project as u64, in_project as u64), "{}", q["transitions"]);
    assert!(q["transitions"]["byKind"].as_object().is_some_and(|k| !k.is_empty()));
}

#[test]
fn hostile_parameters_are_refused_or_clamped() {
    let mut s = demo();
    let item = footage(&s);
    assert!(s.execute("analysis.run", json!({})).is_err(), "nothing to analyse");
    assert!(s.execute("analysis.run", json!({"item": 999_999})).is_err());
    assert!(s.execute("analysis.run", json!({"sequence": 999_999})).is_err());
    assert!(s.execute("analysis.run", json!({"item": "x"})).is_err());
    assert!(s.execute("analysis.run", json!({"sequence": false})).is_err());
    let seq = s.state.active_sequence.unwrap();
    assert!(s.execute("analysis.run", json!({"item": seq.0})).is_err(), "a sequence is not footage");
    assert!(s.execute("analysis.run", json!({"sequence": item})).is_err(), "footage is not a sequence");
    // the number of samples is cut to 1..=600 (checked on the parameter, so no 600-picture job runs here)
    for (given, want) in
        [(json!(0), 1), (json!(-5), 1), (json!(u64::MAX), 600), (json!(1e30), 600), (json!("many"), 60), (json!(null), 60), (json!(7), 7), (json!(601), 600)]
    {
        assert_eq!(crate::analysis::samples_param(&json!({"samples": given})), want, "{given}");
    }
    for (given, want) in [(json!(0), 1), (json!(u64::MAX), 2000), (json!(-1), 1), (json!("x"), 300), (json!(12), 12)] {
        assert_eq!(crate::analysis::max_shots_param(&json!({"maxShots": given})), want, "{given}");
    }
    for samples in [json!(0), json!("many"), json!(null)] {
        let r = s.execute("analysis.run", json!({"item": item, "samples": samples, "wait": true, "force": true})).unwrap();
        let n = r["report"]["video"]["samples"].as_array().unwrap().len();
        assert!(n == 1 || n == 60, "{samples}: {n}");
    }
    for p in [json!({"key": "../../etc/passwd"}), json!({"key": ""}), json!({"key": "a/b"}), json!({"key": ".hidden"}), json!({"key": 7})] {
        assert!(s.execute("analysis.get", p.clone()).is_err(), "{p}");
    }
    assert!(s.execute("analysis.clear", json!({})).is_err(), "say what to clear");
    assert_eq!(s.execute("analysis.clear", json!({"key": "../x"})).unwrap()["removed"], 0);
    assert!(s.execute("analysis.clear", json!({"item": "x"})).is_err());
    assert_eq!(s.execute("analysis.clear", json!({"item": 424_242})).unwrap()["removed"], 0);
}

#[test]
fn a_job_that_is_stopped_keeps_nothing() {
    let mut s = demo();
    let item = footage(&s);
    // start it asynchronously and stop it at once; whichever wins, a stopped job caches nothing
    let r = s.execute("analysis.run", json!({"item": item, "samples": 600})).unwrap();
    s.execute("jobs.cancel", json!({"job": r["job"]})).unwrap();
    let t0 = std::time::Instant::now();
    loop {
        let jobs = s.execute("jobs.list", json!({})).unwrap();
        let j = jobs.as_array().unwrap().iter().find(|x| x["id"] == r["job"]).unwrap().clone();
        if j["finished"] == true {
            let stopped = j["result"]["error"].is_string();
            assert_eq!(list(&mut s).len(), usize::from(!stopped), "{j}");
            break;
        }
        assert!(t0.elapsed().as_secs() < 120, "the job never finished");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn keys_cannot_name_a_path_and_the_disk_cache_round_trips_and_shrugs_off_junk() {
    for bad in ["", ".", "..", "../x", "a/b", "a\\b", ".hidden", "a b", "é", &"k".repeat(201)] {
        assert!(!valid_key(bad), "{bad:?}");
    }
    assert!(valid_key("item-12-34-n60-i1a1m1s1r1") && valid_key("sequence-0123abcd.v2"));
    let dir = scratch("cache");
    let memory = Arc::new(Mutex::new(Default::default()));
    let cache = Cache { dir: Some(dir.join("Analysis")), memory: memory.clone() };
    cache.write("one", &json!({"a": 1}));
    cache.write("../escape", &json!({"a": 2}));
    assert!(!dir.join("escape.json").exists() && !dir.join("Analysis").join("..").join("escape.json").exists());
    assert_eq!(cache.read("one"), Some(json!({"a": 1})));
    // a second cache over the same folder (a new session) finds it on disk
    let other = Cache { dir: Some(dir.join("Analysis")), memory: Arc::new(Mutex::new(Default::default())) };
    assert_eq!(other.read("one"), Some(json!({"a": 1})));
    assert_eq!(other.keys(), ["one"]);
    // junk in the folder: not JSON, a directory, a file with another extension, a hostile name
    std::fs::write(dir.join("Analysis").join("broken.json"), "{ not json").unwrap();
    std::fs::create_dir_all(dir.join("Analysis").join("folder.json")).unwrap();
    std::fs::write(dir.join("Analysis").join("notes.txt"), "x").unwrap();
    std::fs::write(dir.join("Analysis").join(".hidden.json"), "{}").unwrap();
    assert_eq!(other.read("broken"), None);
    assert!(!other.keys().contains(&".hidden".to_string()));
    assert!(other.remove("one") && other.read("one").is_none());
    assert!(!other.remove("one"));
    // a folder that cannot be written leaves the report in memory only
    let blocked = scratch("blocked").join("file");
    std::fs::write(&blocked, "x").unwrap();
    let c = Cache { dir: Some(blocked.join("Analysis")), memory };
    c.write("kept", &json!({"b": 1}));
    assert_eq!(c.read("kept"), Some(json!({"b": 1})));
}
