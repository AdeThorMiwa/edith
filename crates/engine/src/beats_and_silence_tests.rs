//! Tests of `music_beats` and `silence_map` (fork roadmap F16.2).

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::Session;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("edith-beats-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

/// A mono 16-bit WAV, 16 kHz: a short noise burst every `60 / bpm` seconds from `offset` (a click track), or
/// a steady tone when `bpm` is zero (no pulse to find).
fn wav(path: &std::path::Path, bpm: f64, offset: f64, seconds: f64) {
    let rate = 16_000u32;
    let n = (f64::from(rate) * seconds) as usize;
    let mut seed = 3u32;
    let mut data = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f64 / f64::from(rate);
        let v = if bpm > 0.0 {
            let since = (t - offset).rem_euclid(60.0 / bpm);
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (seed >> 16) as f64 / 32768.0 - 1.0;
            if t >= offset && since < 0.02 { noise * 0.6 * (1.0 - since / 0.02) } else { 0.0 }
        } else {
            0.2 * (2.0 * std::f64::consts::PI * 300.0 * t).sin()
        };
        data.extend(((v * 32767.0) as i16).to_le_bytes());
    }
    let mut out = Vec::new();
    for part in [
        &b"RIFF"[..],
        &(36 + data.len() as u32).to_le_bytes(),
        b"WAVEfmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        &rate.to_le_bytes(),
        &(rate * 2).to_le_bytes(),
        &2u16.to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &(data.len() as u32).to_le_bytes(),
    ] {
        out.extend_from_slice(part);
    }
    out.extend(data);
    std::fs::write(path, out).unwrap();
}

/// Imports a WAV, places it on a fresh audio track at `at` seconds and returns (item, clip).
fn place(s: &mut Session, path: &std::path::Path, at: f64) -> (u64, u64) {
    let r = s.execute("file.import", json!({"paths": [path.to_string_lossy()]})).unwrap();
    let item = r["items"][0].as_u64().unwrap();
    s.execute("sequence.addTracks", json!({"video": 0, "audio": 1, "audioAfter": "last"})).unwrap();
    let tracks = s.active_sequence().unwrap().audio_tracks.len();
    s.execute("timeline.place", json!({"item": item, "audioTrack": format!("A{tracks}"), "seconds": at})).unwrap();
    let clip = s.active_sequence().unwrap().audio_tracks.last().unwrap().items.last().unwrap().id.0;
    (item, clip)
}

fn measure(s: &mut Session, item: u64) {
    s.execute("analysis.run", json!({"item": item, "wait": true})).unwrap();
}

fn beats(s: &Session, input: Value) -> edith_ai::ToolOutput {
    s.run_assistant_tool("music_beats", &input)
}

fn silences(s: &Session, input: Value) -> edith_ai::ToolOutput {
    s.run_assistant_tool("silence_map", &input)
}

fn numbers(text: &str) -> Vec<f64> {
    text.lines().last().unwrap_or("").split_whitespace().filter_map(|w| w.parse().ok()).collect()
}

#[test]
fn a_click_track_gives_its_tempo_and_beats_on_the_timeline() {
    let dir = scratch("clicks");
    let mut s = demo();
    let path = dir.join("score.wav");
    wav(&path, 120.0, 0.13, 20.0);
    let (item, clip) = place(&mut s, &path, 40.0);
    // before it is measured, the tool says what to do
    let early = beats(&s, json!({"clip": clip}));
    assert!(early.is_error && early.content.contains(&format!("analysis_run with item {item}")), "{}", early.content);
    measure(&mut s, item);
    let out = beats(&s, json!({"clip": clip}));
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("near 119") || out.content.contains("near 120"), "{}", out.content);
    assert!(out.content.contains("good to roughly a tenth of a second"), "{}", out.content);
    let times = numbers(&out.content);
    assert!(times.len() >= 30, "{} beats of 20 s at 120 BPM", times.len());
    // on the timeline: the clip starts at 40 s, the first click is 0.13 s into it, then every half second; the
    // measured grid is good to about a tenth of a second
    let first = times[0];
    assert!((first - 40.13).abs() < 0.15, "{first}");
    assert!(times.windows(2).all(|w| (w[1] - w[0] - 0.5).abs() < 0.03), "{times:?}");
    let last = times[times.len() - 1];
    assert!((last - 59.63).abs() < 0.15 || (last - 59.13).abs() < 0.15, "{last}");
    // with one measured music clip, `clip` can be left out
    assert!(!beats(&s, json!({})).is_error);
    // thinned and windowed
    let bars = numbers(&beats(&s, json!({"clip": clip, "every": 4})).content);
    assert!(bars.len() >= 9 && bars.windows(2).all(|w| (w[1] - w[0] - 2.0).abs() < 0.1), "{bars:?}");
    let window = numbers(&beats(&s, json!({"clip": clip, "from": 45, "to": "0:50"})).content);
    assert!(window.iter().all(|t| (45.0..50.0).contains(t)) && window.len() >= 8, "{window:?}");
    let none = beats(&s, json!({"clip": clip, "from": 100}));
    assert!(none.content.contains("No beats fall inside"), "{}", none.content);
}

#[test]
fn trimming_the_clip_keeps_only_the_beats_that_are_heard_and_moves_them() {
    let dir = scratch("trim");
    let mut s = demo();
    let path = dir.join("score.wav");
    wav(&path, 120.0, 0.0, 20.0);
    let (item, clip) = place(&mut s, &path, 60.0);
    measure(&mut s, item);
    // hear media seconds 4 to 8 (4 s) starting at 70 s on the timeline
    s.edit("trim", |pr, _| {
        let seq_id = pr.sequences().next().map(|q| q.id).unwrap();
        let seq = pr.sequence_mut(seq_id).unwrap();
        let (_, it) = seq.find_item_mut(filmcraft_project::ClipId(clip)).unwrap();
        it.start = filmcraft_time::Tick(70 * filmcraft_time::TICKS_PER_SECOND);
        it.source_in = filmcraft_time::Tick(4 * filmcraft_time::TICKS_PER_SECOND);
        it.duration = filmcraft_time::Tick(4 * filmcraft_time::TICKS_PER_SECOND);
        Ok(())
    })
    .unwrap();
    let times = numbers(&beats(&s, json!({"clip": clip})).content);
    assert!(times.iter().all(|t| (70.0..74.0).contains(t)), "{times:?}");
    assert!(times.len() >= 7 && (times[0] - 70.0) < 0.55, "the first beat heard is within half a second of the clip's start: {times:?}");
}

#[test]
fn music_with_no_steady_beat_says_so_instead_of_inventing_one() {
    let dir = scratch("tone");
    let mut s = demo();
    let path = dir.join("drone.wav");
    wav(&path, 0.0, 0.0, 20.0);
    let (item, clip) = place(&mut s, &path, 0.0);
    measure(&mut s, item);
    let out = beats(&s, json!({"clip": clip}));
    assert!(out.is_error && out.content.contains("no steady beat") && out.content.contains("Cut on phrases"), "{}", out.content);
}

#[test]
fn bad_input_and_odd_projects_are_explained_never_fatal() {
    let mut s = demo();
    assert!(beats(&Session::default(), json!({})).is_error);
    for bad in [
        json!({"clip": 999_999}),
        json!({"clip": "x"}),
        json!({"clip": null, "from": "banana"}),
        json!(null),
        json!([1]),
        json!("x"),
        json!({"every": 0}),
        json!({"every": 1e9}),
    ] {
        let r = beats(&s, bad.clone());
        assert!(r.is_error || r.content.contains("Beats of"), "{bad}: {}", r.content);
    }
    // the demo's own sound has not been measured: it says so, or lists what to measure
    let r = beats(&s, json!({}));
    assert!(r.is_error, "{}", r.content);
    // silence_map on nothing
    assert!(silences(&Session::default(), json!({})).is_error);
    let _ = &mut s;
}

#[test]
fn the_silence_map_lists_the_gaps_where_no_clip_sounds_and_marks_planned_ones() {
    // clips snap to frames, so the gaps are a hundredth of a second off the round numbers: match on lengths
    let (mut s, _) = tones(&[(1, 0.0, 10.0), (1, 14.0, 16.0), (1, 32.0, 4.0), (1, 36.2, 2.0)]);
    let out = silences(&s, json!({}));
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.content.matches("unplanned").count(), 2, "{}", out.content);
    assert!(out.content.contains("(4.00 s) unplanned") && out.content.contains("(2.00 s) unplanned"), "{}", out.content);
    let all = silences(&s, json!({"minSeconds": 0.1}));
    assert_eq!(all.content.matches("unplanned").count(), 3, "the 0.2 s gap is listed when asked for: {}", all.content);
    // the plan asks for the first of them
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 1})).unwrap();
    let beat = crate::planner::get(&s.project).unwrap().beats().next().unwrap().id;
    s.execute("planner.addCue", json!({"beat": beat, "kind": "silence", "text": "after the name", "atSeconds": 10, "lengthSeconds": 4})).unwrap();
    let out = silences(&s, json!({}));
    assert!(out.content.contains("(4.00 s) planned") && out.content.contains("(2.00 s) unplanned"), "{}", out.content);
    // a window narrows it, and a stretch with sound the whole way has none
    let only = silences(&s, json!({"from": 29, "to": 33}));
    assert!(only.content.contains("(2.00 s)") && !only.content.contains("(4.00 s)"), "{}", only.content);
    assert!(silences(&s, json!({"from": 0, "to": 5})).content.contains("None: something is sounding"));
    // a muted track does not count as sound
    s.execute("timeline.setTrack", json!({"track": "A1", "muted": true})).unwrap();
    let muted = silences(&s, json!({}));
    assert!(muted.content.contains("no sound on the timeline at all") || muted.content.contains("0.00–"), "{}", muted.content);
    for bad in
        [json!({"from": "x"}), json!({"to": -5}), json!({"minSeconds": "a"}), json!({"minSeconds": -1}), json!({"minSeconds": 1e300}), json!(null), json!([1])]
    {
        let _ = silences(&s, bad);
    }
}

/// A new sequence with a tone of each `(audio track, start, seconds)` placed on it.
fn tones(list: &[(usize, f64, f64)]) -> (Session, Vec<u64>) {
    let mut s = Session::default();
    let tracks = list.iter().map(|g| g.0).max().unwrap_or(1).max(2);
    s.execute("file.newSequence", json!({"name": "gaps", "audio": tracks, "video": 1})).unwrap();
    let mut clips = Vec::new();
    for (k, (track, start, seconds)) in list.iter().enumerate() {
        let samples: Vec<f32> = (0..(seconds * 48_000.0) as usize).flat_map(|i| [(0.1 * (i as f32 * 0.058).sin()), (0.1 * (i as f32 * 0.058).sin())]).collect();
        let bytes: std::sync::Arc<[u8]> = crate::previews::write_wav_f32(&samples, 48_000).into();
        let item = crate::commands::import_bytes(&mut s, &format!("/gap{k}.wav"), bytes, None).unwrap();
        let r = s.execute("timeline.place", json!({"item": item.0, "audioTrack": format!("A{track}"), "seconds": start})).unwrap();
        clips.push(r["clips"][0].as_u64().unwrap());
    }
    (s, clips)
}

#[test]
fn the_tools_are_read_only_and_in_the_list() {
    for name in ["music_beats", "silence_map"] {
        let t = crate::assistant_tools::tools().into_iter().find(|t| t.spec.name == name).unwrap_or_else(|| panic!("no {name}"));
        assert!(t.read_only);
    }
}
