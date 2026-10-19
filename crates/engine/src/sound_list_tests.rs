//! Tests of `timeline.placeSoundList` (fork roadmap F17.1): a score, a bed and effects in one call and one undo step,
//! each in a lane of its own, with fades, loops with crossfaded seams, and nothing changed when an entry is refused.

use std::path::PathBuf;

use filmcraft_project::essential::AudioType;
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use crate::Session;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("edith-soundlist-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A mono 16-bit WAV of a steady tone, 16 kHz.
fn wav(path: &std::path::Path, seconds: f64) {
    let rate = 16_000u32;
    let n = (f64::from(rate) * seconds) as usize;
    let data: Vec<u8> =
        (0..n).flat_map(|i| (((0.2 * (2.0 * std::f64::consts::PI * 300.0 * i as f64 / f64::from(rate)).sin()) * 32767.0) as i16).to_le_bytes()).collect();
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

/// A session with an empty sequence and two sounds in the project: a 40 s score and a 6 s bed. Returns the ids.
fn edit(name: &str) -> (Session, u64, u64) {
    let dir = scratch(name);
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name": "Sound", "audio": 1, "video": 1})).unwrap();
    let (score, bed) = (dir.join("score.wav"), dir.join("bed.wav"));
    wav(&score, 40.0);
    wav(&bed, 6.0);
    let r = s.execute("file.import", json!({"paths": [score.to_string_lossy(), bed.to_string_lossy()]})).unwrap();
    let ids: Vec<u64> = r["items"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
    (s, ids[0], ids[1])
}

fn place(s: &mut Session, entries: Value) -> crate::Result<Value> {
    s.execute("timeline.placeSoundList", json!({"entries": entries}))
}

fn secs(t: Tick) -> f64 {
    t.0 as f64 / TICKS_PER_SECOND as f64
}

/// Times land on frames (the sequence runs at about 29.97 a second), so they are a frame or so from what was asked.
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.05
}

fn lane<'a>(s: &'a Session, name: &str) -> &'a filmcraft_project::Track {
    s.active_sequence().unwrap().audio_tracks.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no {name} track"))
}

#[test]
fn a_score_is_placed_in_a_music_lane_typed_faded_and_explained_in_one_undo_step() {
    let (mut s, score, _) = edit("one");
    let before = s.active_sequence().unwrap().clone();
    let r = place(&mut s, json!([{"start": 2, "end": 22, "item": score, "lane": "music", "gainDb": -6, "reason": "Low strings under the opening question."}]))
        .unwrap();
    assert_eq!((r["sounds"].clone(), r["tracksMade"].clone(), r["looped"].clone()), (json!(1), json!(["Music"]), json!(0)), "{r}");
    let music = lane(&s, "Music");
    assert_eq!(music.items.len(), 1);
    let clip = &music.items[0];
    assert!(near(secs(clip.start), 2.0) && near(secs(clip.end()), 22.0), "{} {}", secs(clip.start), secs(clip.end()));
    assert_eq!(clip.essential.as_ref().map(|e| e.kind), Some(AudioType::Music));
    assert_eq!(clip.gain_db, -6.0);
    assert!(clip.markers.iter().any(|m| m.name == "Why this sound" && m.comment.contains("Low strings")));
    // music fades in over 1.5 s and out over 3 s unless told otherwise
    let fades: Vec<(f64, f64)> = music.transitions.iter().map(|t| (secs(t.start), secs(t.duration))).collect();
    assert_eq!(fades.len(), 2, "{fades:?}");
    assert!(fades.iter().any(|(a, d)| (*a - 2.0).abs() < 0.05 && (*d - 1.5).abs() < 0.05), "{fades:?}");
    assert!(fades.iter().any(|(a, d)| (*a - 19.0).abs() < 0.05 && (*d - 3.0).abs() < 0.05), "{fades:?}");
    // one step takes the lane, its clip and its fades away
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(*s.active_sequence().unwrap(), before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(lane(&s, "Music").items.len(), 1);
}

#[test]
fn a_short_bed_is_looped_with_crossfaded_seams_and_a_lane_is_found_again_by_its_name() {
    let (mut s, score, bed) = edit("loop");
    let r = place(
        &mut s,
        json!([
            {"start": 0, "end": 30, "item": bed, "lane": "ambience", "loop": true, "fadeInSeconds": 2, "fadeOutSeconds": 2},
            {"start": 5, "end": 9, "item": score, "lane": "effects", "fadeInSeconds": 0.5},
        ]),
    )
    .unwrap();
    assert_eq!((r["looped"].clone(), r["tracksMade"].clone()), (json!(1), json!(["Ambience", "Effects"])), "{r}");
    let amb = lane(&s, "Ambience");
    // 6 s of sound with a 1 s seam: pieces of about 5 s end to end, from 0 to 30 s
    assert!(amb.items.len() >= 5, "{}", amb.items.len());
    assert!(near(secs(amb.items[0].start), 0.0) && near(secs(amb.items.last().unwrap().end()), 30.0));
    for w in amb.items.windows(2) {
        assert_eq!(w[0].end(), w[1].start, "end to end");
        // every repeat after the first starts a little into the sound, which is what leaves room for the crossfade
        assert!(w[1].source_in > Tick::ZERO);
    }
    // a transition at every seam, plus the two fades
    assert_eq!(amb.transitions.len(), amb.items.len() - 1 + 2, "{:?}", amb.transitions.iter().map(|t| (secs(t.start), secs(t.duration))).collect::<Vec<_>>());
    assert!(amb.items.iter().all(|c| c.essential.as_ref().map(|e| e.kind) == Some(AudioType::Ambience)));
    let tracks = s.active_sequence().unwrap().audio_tracks.len();
    assert_eq!(tracks, 3, "narration track, Ambience and Effects");
    // a second call finds the lanes by name instead of making them again
    let again = place(&mut s, json!([{"start": 40, "end": 50, "item": score, "lane": "effects"}])).unwrap();
    assert_eq!(again["tracksMade"], json!([]));
    assert_eq!(s.active_sequence().unwrap().audio_tracks.len(), tracks);
    assert_eq!(lane(&s, "Effects").items.len(), 2);
}

#[test]
fn a_bad_entry_refuses_the_whole_call_and_names_it_and_nothing_changes() {
    let (mut s, score, bed) = edit("bad");
    let still =
        s.project.items.values().find(|i| i.name.to_ascii_lowercase().contains("still") || i.name.to_ascii_lowercase().contains("photo")).map(|i| i.id.0);
    let before = s.active_sequence().unwrap().clone();
    let refuse = |s: &mut Session, entries: Value, needle: &str| {
        let e = place(s, entries).unwrap_err().to_string();
        assert!(e.contains(needle), "wanted “{needle}” in: {e}");
    };
    refuse(&mut s, json!([{"start": 0, "end": 10, "item": score, "lane": "music"}, {"start": 0, "end": 10, "item": score, "lane": "hold music"}]), "entry 2");
    refuse(&mut s, json!([{"start": 0, "end": 60, "item": bed, "lane": "ambience"}]), "loop: true");
    refuse(&mut s, json!([{"start": 0, "end": 60, "item": bed, "lane": "ambience", "loop": true, "sourceIn": 5}]), "only 1.0 s of sound");
    refuse(
        &mut s,
        json!([{"start": 0, "end": 10, "item": score, "lane": "music"}, {"start": 8, "end": 20, "item": score, "lane": "music"}]),
        "entries 1 and 2 are both music and overlap",
    );
    refuse(&mut s, json!([{"start": 10, "end": 5, "item": score, "lane": "music"}]), "less than one frame");
    refuse(&mut s, json!([{"start": 0, "end": 5, "item": score, "lane": "music", "gainDb": 30}]), "-60 to +12");
    refuse(&mut s, json!([{"start": 0, "end": 5, "item": score, "lane": "music", "fadeInSeconds": -1}]), "0 to 60");
    refuse(&mut s, json!([{"start": 0, "end": 5, "item": 999_999, "lane": "music"}]), "there is no item");
    refuse(&mut s, json!([{"start": 0, "end": 5, "item": score}]), "music, ambience or effects");
    refuse(&mut s, json!([{"start": 0, "end": 5, "item": score, "lane": "music", "reason": 5}]), "must be text");
    refuse(&mut s, json!([]), "needs 1 to");
    if let Some(still) = still {
        refuse(&mut s, json!([{"start": 0, "end": 5, "item": still, "lane": "music"}]), "no sound in it");
    }
    assert_eq!(*s.active_sequence().unwrap(), before, "no refused call left a track or a clip");
}

#[test]
fn sound_already_in_a_lane_is_not_overwritten_unless_asked() {
    let (mut s, score, _) = edit("replace");
    place(&mut s, json!([{"start": 0, "end": 20, "item": score, "lane": "music"}])).unwrap();
    let e = place(&mut s, json!([{"start": 10, "end": 25, "item": score, "lane": "music"}])).unwrap_err().to_string();
    assert!(e.contains("already has") && e.contains("replace: true"), "{e}");
    assert_eq!(lane(&s, "Music").items.len(), 1);
    s.execute("timeline.placeSoundList", json!({"entries": [{"start": 10, "end": 25, "item": score, "lane": "music"}], "replace": true})).unwrap();
    let m = lane(&s, "Music");
    assert!(
        m.items.iter().any(|c| near(secs(c.start), 10.0) && near(secs(c.end()), 25.0)),
        "{:?}",
        m.items.iter().map(|c| (secs(c.start), secs(c.end()))).collect::<Vec<_>>()
    );
}

/// The loudness (RMS) of the mix between `from` and `to` seconds.
fn rms(s: &Session, from: f64, to: f64) -> f64 {
    let q = s.active_sequence().unwrap();
    let sr = q.settings.sample_rate.max(1);
    let provider = s.media.full_res_provider(s.project.clone(), s.services.clone());
    let (a, n) = ((from * f64::from(sr)) as i64, ((to - from) * f64::from(sr)) as usize);
    let b = filmcraft_render::audio::mix_sequence(&s.project, q, a, n, &provider);
    let sum: f64 = b.channels.iter().flat_map(|c| c.iter()).map(|v| f64::from(*v).powi(2)).sum();
    (sum / (b.channels.len().max(1) * n.max(1)) as f64).sqrt()
}

#[test]
fn the_repeats_of_a_looped_bed_are_heard_as_one_continuous_sound() {
    let (mut s, _, bed) = edit("heard");
    place(&mut s, json!([{"start": 0, "end": 30, "item": bed, "lane": "ambience", "loop": true, "fadeInSeconds": 0, "fadeOutSeconds": 0}])).unwrap();
    let amb = lane(&s, "Ambience");
    let seams: Vec<f64> = amb.items.iter().skip(1).map(|c| secs(c.start)).collect();
    assert!(seams.len() >= 4, "{seams:?}");
    let steady = rms(&s, 1.0, 3.0);
    assert!(steady > 0.01, "the bed is audible: {steady}");
    for at in seams {
        // the half second around each seam is as loud as the middle of a repeat, within a few dB (a crossfade of
        // two different parts of a tone is not exactly level, but it is never a gap)
        let around = rms(&s, at - 0.25, at + 0.25);
        assert!(around > steady * 0.6 && around < steady * 1.6, "seam at {at:.2} s: {around} against {steady}");
    }
    // and nothing sounds past the end of the stretch
    assert!(rms(&s, 30.5, 31.5) < 1e-6);
}
