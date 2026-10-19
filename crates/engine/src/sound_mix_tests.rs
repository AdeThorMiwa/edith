//! Tests of `sound.autoMix` (fork roadmap F17.2): the narration is dialogue, each lane is matched to its loudness, the
//! music and ambience duck under the voice, the mix is measured before and after, and it is one undo step that gives
//! the same result when asked again.

use std::path::PathBuf;

use filmcraft_project::essential::AudioType;
use serde_json::{Value, json};

use crate::Session;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("edith-soundmix-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A mono 16-bit WAV: a tone of `hz`, `amp` loud, with a long pause after every half second of it, so the music has time to come back between phrases to the
/// ducking detector (a bed would be steady).
fn wav(path: &std::path::Path, seconds: f64, hz: f64, amp: f64, gaps: bool) {
    let rate = 16_000u32;
    let n = (f64::from(rate) * seconds) as usize;
    let data: Vec<u8> = (0..n)
        .flat_map(|i| {
            let t = i as f64 / f64::from(rate);
            let on = !gaps || (t % 3.0) < 0.5;
            let v = if on { amp * (2.0 * std::f64::consts::PI * hz * t).sin() } else { 0.0 };
            ((v * 32767.0) as i16).to_le_bytes()
        })
        .collect();
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

/// A sequence with a voice (a loud tone with gaps) on the first audio track, ten seconds, and two sounds in the project.
/// Returns the session and the ids of the score and the bed.
fn edit(name: &str) -> (Session, u64, u64) {
    let dir = scratch(name);
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name": "Mix", "audio": 1, "video": 1})).unwrap();
    let (voice, score, bed) = (dir.join("voice.wav"), dir.join("score.wav"), dir.join("bed.wav"));
    wav(&voice, 10.0, 220.0, 0.5, true);
    wav(&score, 30.0, 330.0, 0.05, false);
    wav(&bed, 6.0, 120.0, 0.02, false);
    let r = s.execute("file.import", json!({"paths": [voice.to_string_lossy(), score.to_string_lossy(), bed.to_string_lossy()]})).unwrap();
    let ids: Vec<u64> = r["items"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
    s.execute("timeline.place", json!({"item": ids[0], "audioTrack": "A1", "seconds": 0})).unwrap();
    let voice_clip = s.active_sequence().unwrap().audio_tracks[0].items[0].id.0;
    s.execute("essentialSound.setType", json!({"clips": [voice_clip], "type": "dialogue"})).unwrap();
    (s, ids[1], ids[2])
}

fn lane<'a>(s: &'a Session, name: &str) -> &'a filmcraft_project::Track {
    s.active_sequence().unwrap().audio_tracks.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no {name} track"))
}

/// A clip's own loudness now (gain and effects included).
fn loudness(s: &Session, clip: &filmcraft_project::TrackItem) -> f64 {
    let sr = s.active_sequence().unwrap().settings.sample_rate;
    let provider = s.media.full_res_provider(s.project.clone(), s.services.clone());
    crate::essential_sound::clip_loudness(clip, sr, &provider).unwrap()
}

#[test]
fn the_voice_leads_the_score_and_bed_sit_under_it_and_duck_while_it_speaks() {
    let (mut s, score, bed) = edit("mix");
    s.execute(
        "timeline.placeSoundList",
        json!({"entries": [
            {"start": 0, "end": 10, "item": score, "lane": "music", "fadeInSeconds": 0, "fadeOutSeconds": 0},
            {"start": 0, "end": 10, "item": bed, "lane": "ambience", "loop": true, "fadeInSeconds": 0, "fadeOutSeconds": 0},
        ]}),
    )
    .unwrap();
    let before = s.active_sequence().unwrap().clone();
    let r = s.execute("sound.autoMix", json!({})).unwrap();
    assert!(r["beforeLufs"].is_number() && r["afterLufs"].is_number(), "{r}");
    let summary = r["summary"].as_str().unwrap();
    assert!(summary.contains("narration to -16 LUFS") && summary.contains("music to -22 LUFS") && summary.contains("ambience to -28 LUFS"), "{summary}");
    assert!(summary.contains("dip under the voice") && summary.contains("before") && summary.contains("after"), "{summary}");
    // each type sits at its target (the match gain brings the clip itself to it)
    let voice = &s.active_sequence().unwrap().audio_tracks[0].items[0];
    assert!((loudness(&s, voice) - -16.0).abs() < 0.6, "voice at {}", loudness(&s, voice));
    let music = &lane(&s, "Music").items[0];
    assert!((loudness(&s, music) - -22.0).abs() < 0.6, "music at {}", loudness(&s, music));
    for c in &lane(&s, "Ambience").items {
        assert!((loudness(&s, c) - -28.0).abs() < 1.5, "bed piece at {}", loudness(&s, c));
    }
    // the score dips while the voice speaks: its level has keyframes that go down and come back
    let level = music.effect("volume").and_then(|e| e.param("level")).unwrap();
    assert!(
        level.is_animated() && level.keyframes.len() >= 4,
        "{:?}",
        level.keyframes.iter().map(|k| (k.time.0 as f64 / 254016000000.0, k.value.as_f64())).collect::<Vec<_>>()
    );
    let values: Vec<f64> = level.keyframes.iter().filter_map(|k| k.value.as_f64()).collect();
    let (lo, hi) = (values.iter().cloned().fold(f64::INFINITY, f64::min), values.iter().cloned().fold(f64::NEG_INFINITY, f64::max));
    assert!(hi - lo >= 10.0, "ducks by about 15 dB: {lo} to {hi}");
    // one undo step takes it all back
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(*s.active_sequence().unwrap(), before);
}

#[test]
fn asked_again_it_gives_the_same_mix_and_a_target_and_ducking_can_be_chosen() {
    let (mut s, score, _) = edit("again");
    s.execute(
        "timeline.placeSoundList",
        json!({"entries": [{"start": 0, "end": 10, "item": score, "lane": "music", "fadeInSeconds": 0, "fadeOutSeconds": 0}]}),
    )
    .unwrap();
    let first = s.execute("sound.autoMix", json!({})).unwrap();
    let music = lane(&s, "Music").items[0].clone();
    let (gain, level) = (music.essential.as_ref().unwrap().loudness.gain_db, music.effect("volume").unwrap().param("level").unwrap().keyframes.len());
    let second = s.execute("sound.autoMix", json!({})).unwrap();
    let again = &lane(&s, "Music").items[0];
    assert!((again.essential.as_ref().unwrap().loudness.gain_db - gain).abs() < 0.05, "matching is recomputed, not stacked");
    assert_eq!(again.effect("volume").unwrap().param("level").unwrap().keyframes.len(), level);
    assert!((first["afterLufs"].as_f64().unwrap() - second["afterLufs"].as_f64().unwrap()).abs() < 0.1);
    // a different target and no ducking
    let r = s.execute("sound.autoMix", json!({"musicLufs": -30, "duck": false})).unwrap();
    assert!(r["summary"].as_str().unwrap().contains("music to -30 LUFS") && !r["summary"].as_str().unwrap().contains("dip under"), "{r}");
    assert!((loudness(&s, &lane(&s, "Music").items[0]) - -30.0).abs() < 0.6);
    assert_eq!(lane(&s, "Music").items[0].essential.as_ref().map(|e| e.kind), Some(AudioType::Music));
}

#[test]
fn untyped_clips_in_a_lane_are_typed_by_it_and_a_mix_of_nothing_or_a_bad_target_is_refused() {
    let (mut s, score, _) = edit("typed");
    // a score the person put on a track named Music by hand, with no type
    s.execute("sequence.addTracks", json!({"video": 0, "audio": 1, "audioAfter": "last"})).unwrap();
    let seq = s.state.active_sequence.unwrap();
    s.edit("Name Track", |p, _| {
        p.sequence_mut(seq).unwrap().audio_tracks[1].name = "Music".into();
        Ok(())
    })
    .unwrap();
    s.execute("timeline.place", json!({"item": score, "audioTrack": "A2", "seconds": 0})).unwrap();
    assert!(lane(&s, "Music").items[0].essential.is_none());
    s.execute("sound.autoMix", json!({})).unwrap();
    assert_eq!(lane(&s, "Music").items[0].essential.as_ref().map(|e| e.kind), Some(AudioType::Music), "typed by its lane");
    assert!(s.execute("sound.autoMix", json!({"musicLufs": 5})).unwrap_err().to_string().contains("-60 to 0"));
    assert!(s.execute("sound.autoMix", json!({"dialogueLufs": "loud"})).unwrap_err().to_string().contains("-60 to 0"));
    // an empty sequence has nothing to mix
    s.execute("file.newSequence", json!({"name": "Empty", "audio": 1, "video": 1})).unwrap();
    assert!(s.execute("sound.autoMix", json!({})).unwrap_err().to_string().contains("no narration"));
}
