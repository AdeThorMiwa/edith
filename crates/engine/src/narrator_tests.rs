//! Tests of [`crate::narrator`]: saving a narrator voice from a narration, applying it to another,
//! the built-ins, A/B, persistence, and hostile input.

use super::*;
use crate::narrator::{FILE, NarratorVoice, builtins, find};
use serde_json::json;

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("edith-narrator-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

/// A narration placed after `after` seconds, on its own track; its clip id and track strip name.
fn narrate(s: &mut Session, dir: &std::path::Path, after: f64, extra: Value) -> (u64, String) {
    s.execute("playhead.set", json!({"seconds": after})).unwrap();
    let mut p = json!({"text": "The fog rolled in.", "dir": dir.to_string_lossy()});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        p[k] = v;
    }
    let r = s.execute("tts.create", p).unwrap();
    (r["clip"].as_u64().unwrap(), r["track"].as_str().unwrap().to_string())
}

fn inserts(s: &Session, strip: &str) -> Vec<(String, bool)> {
    let seq = s.active_sequence().unwrap();
    let id = crate::mixer::strip_ref(seq, &json!(strip)).unwrap();
    seq.strip(id).unwrap().effects.iter().map(|e| (e.effect.clone(), e.enabled)).collect()
}

fn param(s: &Session, strip: &str, slot: usize, k: &str) -> f64 {
    let seq = s.active_sequence().unwrap();
    let id = crate::mixer::strip_ref(seq, &json!(strip)).unwrap();
    seq.strip(id).unwrap().effects[slot].param(k).unwrap().value.as_f64().unwrap()
}

#[test]
fn the_built_ins_are_listed_real_and_cannot_be_changed() {
    let mut s = demo();
    let r = s.execute("tts.voicePresets", json!({})).unwrap();
    let names: Vec<&str> = r["voices"].as_array().unwrap().iter().map(|v| v["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Documentary", "Clean"]);
    for b in builtins() {
        // every effect is a real audio effect, every parameter exists, and nothing is dropped as invalid
        let def = crate::narrator::find(&s, &b.name).unwrap();
        assert_eq!(def, b);
        assert!(!b.inserts.is_empty() && b.inserts.len() <= filmcraft_project::mixer::MAX_INSERTS);
        for i in &b.inserts {
            let d = filmcraft_project::find_effect(&i.effect).unwrap_or_else(|| panic!("{}", i.effect));
            for k in i.params.keys() {
                assert!(d.param(k).is_some(), "{} has no {k}", i.effect);
            }
        }
    }
    assert!(s.execute("tts.deleteVoicePreset", json!({"name": "documentary"})).is_err());
    assert!(s.execute("tts.saveVoicePreset", json!({"name": "Documentary", "voice": "basic-male"})).is_err());
}

#[test]
fn a_voice_saved_from_one_narration_is_applied_to_another_as_one_undo_step() {
    let dir = scratch("apply");
    let mut s = demo();
    let (a, strip_a) = narrate(&mut s, &dir, 1.0, json!({"voice": "basic-female", "pace": 1.2, "pitch": "low"}));
    // build a chain on the first narration's track by applying a built-in, then change a value
    s.execute("tts.applyVoicePreset", json!({"name": "Documentary", "clip": a, "dir": dir.to_string_lossy()})).unwrap();
    assert_eq!(inserts(&s, &strip_a).iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["parametric_eq", "dynamics", "deesser", "studio_reverb", "amplify"]);
    s.execute("mixer.setInsert", json!({"strip": strip_a, "slot": 1, "params": {"ratio": 5.0}})).unwrap();
    let saved = s.execute("tts.saveVoicePreset", json!({"name": "  My narrator ", "clip": a})).unwrap();
    assert_eq!((saved["saved"].clone(), saved["replaced"].clone()), (json!("My narrator"), json!(false)));
    let mine = find(&s, "my narrator").unwrap();
    assert_eq!((mine.voice.as_deref(), mine.pace, mine.pitch.as_deref()), (Some("basic-female"), Some(1.2), Some("low")));
    assert_eq!(mine.inserts.len(), 5);
    // a second narration gets the voice: its settings, its chain, one undo step
    let (b, strip_b) = narrate(&mut s, &dir, 1.0, json!({"voice": "basic-male", "pace": 0.8}));
    assert_ne!(strip_a, strip_b);
    let undo = s.history.undo.len();
    let r = s.execute("tts.applyVoicePreset", json!({"name": "my narrator", "clip": b, "dir": dir.to_string_lossy()})).unwrap();
    assert_eq!((r["applied"].clone(), r["inserts"].clone()), (json!("My narrator"), json!(5)));
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    assert_eq!(inserts(&s, &strip_b), inserts(&s, &strip_a));
    assert_eq!(param(&s, &strip_b, 1, "ratio"), 5.0);
    let item = s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(b)).unwrap().1.item;
    let n = &s.project.narrations[&item];
    assert_eq!((n.voice.as_str(), n.pace, n.pitch), ("basic-female", 1.2, filmcraft_project::VocalPitch::Low));
    // undo takes the whole thing back, redo brings it again
    s.undo().unwrap();
    assert!(inserts(&s, &strip_b).is_empty());
    let item = s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(b)).unwrap().1.item;
    assert_eq!(s.project.narrations[&item].voice, "basic-male");
    s.redo().unwrap();
    assert_eq!(inserts(&s, &strip_b).len(), 5);
    // saving under the same name replaces it
    assert_eq!(s.execute("tts.saveVoicePreset", json!({"name": "MY NARRATOR", "clip": b})).unwrap()["replaced"], true);
    assert_eq!(s.execute("tts.voicePresets", json!({})).unwrap()["voices"].as_array().unwrap().len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_new_narration_can_start_from_a_voice_and_gets_its_chain_on_its_track() {
    let dir = scratch("create");
    let mut s = demo();
    s.execute("tts.saveVoicePreset", json!({"name": "Low and slow", "voice": "basic-male", "pace": 0.9, "pitch": "low"})).unwrap();
    let undo = s.history.undo.len();
    let (clip, strip) = narrate(&mut s, &dir, 1.0, json!({"preset": "Documentary", "voice": "basic-female"}));
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step for the narration and its chain");
    assert_eq!(inserts(&s, &strip).len(), 5);
    // a built-in has no settings of its own, so what was asked for stays; a saved voice fills the gaps
    let item = s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(clip)).unwrap().1.item;
    assert_eq!(s.project.narrations[&item].voice, "basic-female");
    let (c2, strip2) = narrate(&mut s, &dir, 30.0, json!({"preset": "low and slow", "pace": 1.5}));
    let item = s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(c2)).unwrap().1.item;
    let n = &s.project.narrations[&item];
    assert_eq!((n.voice.as_str(), n.pace, n.pitch), ("basic-male", 1.5, filmcraft_project::VocalPitch::Low));
    assert!(inserts(&s, &strip2).is_empty(), "this voice has no effects");
    s.undo().unwrap();
    assert!(all_have_no_narration_at(&s, 30.0));
    // an unknown or mistyped voice is refused and adds nothing
    let before = (*s.project).clone();
    for p in [json!({"preset": "nope"}), json!({"preset": 5}), json!({"preset": ["Clean"]})] {
        let mut q = json!({"text": "Hi.", "voice": "basic-female", "dir": dir.to_string_lossy()});
        q["preset"] = p["preset"].clone();
        assert!(s.execute("tts.create", q).is_err());
    }
    assert_eq!(*s.project, before);
    let _ = std::fs::remove_dir_all(&dir);
}

fn all_have_no_narration_at(s: &Session, at: f64) -> bool {
    let t = filmcraft_time::Tick::from_seconds_f64(at);
    !s.active_sequence().unwrap().audio_tracks.iter().any(|tr| tr.items.iter().any(|it| it.start == t))
}

#[test]
fn hearing_the_plain_voice_switches_the_effects_off_and_back_on() {
    let dir = scratch("ab");
    let mut s = demo();
    let (a, strip) = narrate(&mut s, &dir, 1.0, json!({"preset": "Clean"}));
    assert!(inserts(&s, &strip).iter().all(|x| x.1));
    let track = s.execute("tts.voicePresets", json!({"clip": a})).unwrap()["track"].clone();
    assert_eq!((track["inserts"].clone(), track["plain"].clone(), track["effects"].as_array().unwrap().len()), (json!(4), json!(false), 4));
    let undo = s.history.undo.len();
    assert_eq!(s.execute("tts.compareVoice", json!({"clip": a})).unwrap()["plain"], true, "without `plain` it flips");
    assert_eq!(s.history.undo.len(), undo + 1);
    assert!(inserts(&s, &strip).iter().all(|x| !x.1));
    assert_eq!(s.execute("tts.voicePresets", json!({"clip": a})).unwrap()["track"]["plain"], true);
    // asking for plain again changes nothing; asking for effects turns them back on
    assert_eq!(s.execute("tts.compareVoice", json!({"clip": a, "plain": true})).unwrap()["plain"], true);
    assert_eq!(s.execute("tts.compareVoice", json!({"clip": a, "plain": false})).unwrap()["plain"], false);
    assert!(inserts(&s, &strip).iter().all(|x| x.1));
    // one effect already off by hand stays off when the voice is plain, and the rest follow
    s.execute("mixer.setInsert", json!({"strip": strip, "slot": 1, "enabled": false})).unwrap();
    s.execute("tts.compareVoice", json!({"clip": a, "plain": true})).unwrap();
    assert!(inserts(&s, &strip).iter().all(|x| !x.1));
    // errors
    assert!(s.execute("tts.compareVoice", json!({"clip": a, "plain": "yes"})).is_err());
    let (b, _) = narrate(&mut s, &dir, 30.0, json!({}));
    assert!(s.execute("tts.compareVoice", json!({"clip": b})).is_err(), "a track with no effects has nothing to switch");
    assert!(s.execute("tts.compareVoice", json!({"clip": 999_999})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saved_voices_are_remembered_between_sessions_and_a_hostile_file_is_cleaned_entry_by_entry() {
    let data = scratch("file");
    let prefs = data.join("preferences.json");
    let mut s = Session { prefs_path: Some(prefs.clone()), ..Default::default() };
    s.execute("tts.saveVoicePreset", json!({"name": "Mine", "voice": "basic-male", "pace": 1.1})).unwrap();
    let again = Session { prefs_path: Some(prefs.clone()), ..Default::default() };
    assert_eq!(find(&again, "mine").unwrap().voice.as_deref(), Some("basic-male"));
    let mut again = again;
    again.execute("tts.deleteVoicePreset", json!({"name": "MINE"})).unwrap();
    assert!(find(&Session { prefs_path: Some(prefs.clone()), ..Default::default() }, "mine").is_none());
    assert!(again.execute("tts.deleteVoicePreset", json!({"name": "mine"})).is_err());
    // damaged files are no voices
    let file = data.join(FILE);
    for text in ["{ not json", "[1,2,3]", "", "{}"] {
        std::fs::write(&file, text).unwrap();
        assert!(find(&Session { prefs_path: Some(prefs.clone()), ..Default::default() }, "x").is_none(), "{text}");
    }
    // hostile entries: bad names, copies of a built-in's name, unknown effects, NaN-like values, huge lists
    let big: Vec<Value> = (0..10_000).map(|i| json!({"name": format!("v{i}")})).collect();
    let mut list = vec![
        json!({"name": "", "voice": "basic-male"}),
        json!({"name": "documentary"}),
        json!({"name": "bad\u{7}name"}),
        json!({"name": "x".repeat(200)}),
        json!({"name": "wrong voice", "voice": "nobody"}),
        json!({"name": "wrong pace", "pace": 99.0}),
        json!({"name": "wrong pitch", "pitch": "shrill"}),
        json!({"name": "Chain", "inserts": [
            {"effect": "no_such_effect"},
            {"effect": "volume"},
            {"effect": "highpass", "params": {"cutoff": 1e30, "bogus": 3, "x": "text"}},
            {"effect": "highpass", "enabled": false, "postFader": true, "params": {"cutoff": -5}},
            {"effect": "deesser"}, {"effect": "deesser"}, {"effect": "deesser"}, {"effect": "deesser"}
        ]}),
    ];
    list.extend(big);
    std::fs::write(&file, serde_json::to_string(&list).unwrap()).unwrap();
    let s = Session { prefs_path: Some(prefs), ..Default::default() };
    let all = crate::narrator::mine(&s);
    assert!(all.len() <= crate::narrator::MAX_VOICES, "{}", all.len());
    let chain: NarratorVoice = all.iter().find(|v| v.name == "Chain").unwrap().clone();
    assert_eq!(chain.inserts.len(), filmcraft_project::mixer::MAX_INSERTS);
    assert_eq!(chain.inserts[0].params["cutoff"], 20000.0, "clamped to the effect's own range");
    assert_eq!(chain.inserts[0].params.len(), 1, "unknown or mistyped parameters are dropped");
    assert_eq!(chain.inserts[1].params["cutoff"], 20.0);
    assert!(!chain.inserts[1].enabled && chain.inserts[1].post_fader);
    for bad in ["wrong voice", "wrong pace", "wrong pitch", "documentary"] {
        assert!(all.iter().all(|v| v.name.to_lowercase() != bad), "{bad}");
    }
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn hostile_parameters_are_refused_and_change_nothing() {
    let dir = scratch("hostile");
    let mut s = demo();
    let (a, _) = narrate(&mut s, &dir, 1.0, json!({"preset": "Clean"}));
    let before = (*s.project).clone();
    let long = "x".repeat(300);
    for (cmd, p) in [
        ("tts.saveVoicePreset", json!({})),
        ("tts.saveVoicePreset", json!({"name": ""})),
        ("tts.saveVoicePreset", json!({"name": 5})),
        ("tts.saveVoicePreset", json!({"name": long})),
        ("tts.saveVoicePreset", json!({"name": "a\u{7}b", "clip": a})),
        ("tts.saveVoicePreset", json!({"name": "ok", "voice": 3})),
        ("tts.saveVoicePreset", json!({"name": "ok", "voice": "nobody"})),
        ("tts.saveVoicePreset", json!({"name": "ok", "pace": "fast"})),
        ("tts.saveVoicePreset", json!({"name": "ok", "pace": 9.0})),
        ("tts.saveVoicePreset", json!({"name": "ok", "pitch": "shrill"})),
        ("tts.saveVoicePreset", json!({"name": "ok", "strip": "A99", "voice": "basic-male"})),
        ("tts.applyVoicePreset", json!({})),
        ("tts.applyVoicePreset", json!({"name": 5})),
        ("tts.applyVoicePreset", json!({"name": "nope", "clip": a})),
        ("tts.applyVoicePreset", json!({"name": "Clean", "clip": 999_999})),
        ("tts.deleteVoicePreset", json!({})),
        ("tts.deleteVoicePreset", json!({"name": 5})),
        ("tts.compareVoice", json!({"plain": []})),
    ] {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute(cmd, p.clone())));
        assert!(r.is_ok() && r.unwrap().is_err(), "{cmd} accepted {p}");
    }
    assert_eq!(*s.project, before);
    assert!(s.execute("tts.voicePresets", json!({})).unwrap()["voices"].as_array().unwrap().len() == 2, "nothing was saved");
    // with nothing selected there is nothing to save or apply
    let mut empty = demo();
    assert!(empty.execute("tts.saveVoicePreset", json!({"name": "ok"})).is_err());
    assert!(empty.execute("tts.applyVoicePreset", json!({"name": "Clean"})).is_err());
    // a locked track is refused, and nothing changes
    let seq_id = s.state.active_sequence.unwrap();
    let track = s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(a)).unwrap().0;
    std::sync::Arc::make_mut(&mut s.project).sequence_mut(seq_id).unwrap().audio_tracks.iter_mut().filter(|t| t.id == track).for_each(|t| t.locked = true);
    let before = (*s.project).clone();
    assert!(s.execute("tts.applyVoicePreset", json!({"name": "Documentary", "clip": a})).is_err());
    assert_eq!(*s.project, before);
    let _ = std::fs::remove_dir_all(&dir);
}

/// With the natural voices installed on this machine: the same line, plain and through each
/// built-in chain, mixed down with the editor's own export, for listening (set `EDITH_SAMPLES_DIR`
/// to keep the files). Run with `cargo test -p filmcraft-engine --features neural-voices --lib -- --ignored real_voices_through`.
#[test]
#[ignore = "needs the downloaded natural voices"]
fn real_voices_through_the_built_in_chains() {
    let Some(dir) = crate::transcript::models_dir().filter(|d| filmcraft_tts::catalog::installed(d)) else {
        eprintln!("natural voices are not installed: skipped");
        return;
    };
    let _ = dir;
    let out = std::env::var("EDITH_SAMPLES_DIR").map(std::path::PathBuf::from).unwrap_or_else(|_| scratch("through"));
    std::fs::create_dir_all(&out).unwrap();
    let voice = filmcraft_tts::catalog::VOICES.first().unwrap().info.id;
    let line = "The fog rolled in over the valley before anyone woke. Nobody had walked that road in forty years.";
    let mut hashes = Vec::new();
    for (name, preset) in [("plain", None), ("documentary", Some("Documentary")), ("clean", Some("Clean"))] {
        let mut s = demo();
        let wavs = scratch(&format!("through-{name}"));
        let mut p = json!({"text": line, "voice": voice, "time": 40 * 254_016_000_000i64, "dir": wavs.to_string_lossy()});
        if let Some(pr) = preset {
            p["preset"] = json!(pr);
        }
        let r = s.execute("tts.create", p).unwrap();
        let seconds = r["duration"].as_i64().unwrap() as f64 / 254_016_000_000.0;
        let path = out.join(format!("{name}.wav"));
        s.execute(
            "file.exportMedia",
            json!({"path": path.to_string_lossy(), "format": "wav", "range": "custom", "startSeconds": 40.0, "endSeconds": 40.0 + seconds + 0.5, "wait": true}),
        )
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() > 100_000, "{name}: {} bytes", bytes.len());
        hashes.push(bytes);
        let _ = std::fs::remove_dir_all(&wavs);
    }
    assert!(hashes[0] != hashes[1] && hashes[0] != hashes[2] && hashes[1] != hashes[2], "each chain changes the sound");
    // the panel's Preview: the same voice, plain and through each chain, with how much they differ
    let mut s = demo();
    let mut stats = Vec::new();
    for (name, through) in [("preview-plain", None), ("preview-documentary", Some("Documentary")), ("preview-clean", Some("Clean"))] {
        let path = out.join(format!("{name}.wav"));
        let mut p = json!({"text": line, "voice": voice, "path": path.to_string_lossy()});
        if let Some(t) = through {
            p["through"] = json!(t);
        }
        s.execute("tts.preview", p).unwrap();
        let x = s.tts_preview.as_ref().unwrap().samples.clone();
        let rms = (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt();
        let peak = x.iter().fold(0f32, |m, v| m.max(v.abs()));
        // energy in the changes between samples: a rough measure of the treble
        let hf = (x.windows(2).map(|w| f64::from(w[1] - w[0]).powi(2)).sum::<f64>() / x.len() as f64).sqrt();
        println!("{name}: rms {rms:.4} peak {peak:.3} treble {hf:.4} seconds {:.2}", x.len() as f64 / 24_000.0);
        stats.push((rms, hf));
    }
    assert!((stats[1].1 / stats[0].1 - 1.0).abs() > 0.05 || (stats[1].0 / stats[0].0 - 1.0).abs() > 0.02, "Documentary is measurably different: {stats:?}");
    // a fair comparison: the processed voice is about as loud as the plain one (within about 1 dB)
    for (i, name) in [(1, "Documentary"), (2, "Clean")] {
        let ratio = stats[i].0 / stats[0].0;
        assert!((0.89..=1.12).contains(&ratio), "{name} level is {ratio:.2} of the plain voice: {stats:?}");
    }
}

fn preview(s: &mut Session, extra: Value) -> Value {
    let mut p = json!({"text": "The fog rolled in over the valley. Nobody woke.", "voice": "basic-female"});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        p[k] = v;
    }
    s.execute("tts.preview", p).unwrap()
}

fn previewed(s: &Session) -> Vec<f32> {
    s.tts_preview.as_ref().unwrap().samples.clone()
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

#[test]
fn a_preview_can_be_played_through_a_narrator_voice_or_the_narrations_track() {
    let dir = scratch("through");
    let mut s = demo();
    preview(&mut s, json!({}));
    let plain = previewed(&s);
    let r = preview(&mut s, json!({"through": "Documentary"}));
    assert_eq!(r["through"], "Documentary");
    let doc = previewed(&s);
    assert!(doc != plain, "the effects change the sound");
    assert!(
        doc.len() >= plain.len() && doc.len() as f64 <= plain.len() as f64 + 24_000.0,
        "the room adds at most a short tail: {} vs {}",
        doc.len(),
        plain.len()
    );
    assert!(doc.iter().all(|x| x.is_finite() && x.abs() <= 0.98 + 1e-6), "never clips");
    assert!(
        doc == {
            preview(&mut s, json!({"through": "documentary"}));
            previewed(&s)
        },
        "repeatable, and the name is not case-sensitive"
    );
    // the cleaner chain differs from the documentary one
    preview(&mut s, json!({"through": "Clean"}));
    assert!(previewed(&s) != doc);
    // a voice with no effects, and no `through` at all, play the voice as it is
    s.execute("tts.saveVoicePreset", json!({"name": "Bare", "voice": "basic-male"})).unwrap();
    assert_eq!(preview(&mut s, json!({"through": "Bare"}))["through"], Value::Null);
    assert!(previewed(&s) == plain);
    // "track": the narration's own effects, honouring an effect switched off
    let (clip, strip) =
        narrate(&mut s, &dir, 1.0, json!({"voice": "basic-female", "text": "The fog rolled in over the valley. Nobody woke.", "preset": "Documentary"}));
    preview(&mut s, json!({"through": "track", "clip": clip}));
    let on_track = previewed(&s);
    assert!(on_track != plain && on_track.len() > plain.len() - 1);
    s.execute("tts.compareVoice", json!({"clip": clip, "plain": true})).unwrap();
    assert_eq!(preview(&mut s, json!({"through": "track", "clip": clip}))["through"], Value::Null, "all effects off: the plain voice");
    assert!(previewed(&s).len() == plain.len() || previewed(&s) != on_track);
    s.execute("tts.compareVoice", json!({"clip": clip, "plain": false})).unwrap();
    s.execute("mixer.setInsert", json!({"strip": strip, "slot": 4, "enabled": false})).unwrap();
    preview(&mut s, json!({"through": "track", "clip": clip}));
    assert!(previewed(&s) != on_track, "one effect off changes the sound");
    // errors change nothing
    for p in [json!({"through": "nope"}), json!({"through": 5}), json!({"through": ["Clean"]}), json!({"through": "track"})] {
        let mut empty = demo();
        assert!(
            empty
                .execute("tts.preview", {
                    let mut q = json!({"text": "Hi there.", "voice": "basic-female"});
                    q["through"] = p["through"].clone();
                    q
                })
                .is_err(),
            "{p}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn running_audio_through_effects_is_safe_for_tiny_silent_and_long_audio() {
    use crate::narrator::run_effects;
    let chain: Vec<filmcraft_project::EffectInstance> =
        builtins()[0].inserts.iter().filter_map(|i| filmcraft_project::find_effect(&i.effect)).map(|d| d.instance()).collect();
    for samples in [vec![], vec![0.5f32], vec![0.0; 24_000], vec![f32::NAN, 1.0, -1.0, 50.0]] {
        let out = run_effects(&filmcraft_tts::Audio { samples: samples.clone(), sample_rate: 24_000 }, &chain);
        assert!(out.samples.iter().all(|x| x.is_finite() && x.abs() <= 0.98 + 1e-6), "{:?}", samples.len());
        assert_eq!(out.sample_rate, 24_000);
    }
    // an empty chain returns the same audio (apart from the headroom rule)
    let a = filmcraft_tts::Audio { samples: (0..2400).map(|i| (i as f32 * 0.05).sin() * 0.5).collect(), sample_rate: 24_000 };
    assert_eq!(run_effects(&a, &[]).samples, a.samples);
    // sixty seconds of speech-like audio is processed in a few seconds at most
    let long =
        filmcraft_tts::Audio { samples: (0..24_000 * 60).map(|i| (i as f32 * 0.03).sin() * 0.4 * ((i / 6000) % 2) as f32).collect(), sample_rate: 24_000 };
    let t = std::time::Instant::now();
    let out = run_effects(&long, &chain);
    assert!(t.elapsed().as_secs() < 20, "{:?}", t.elapsed());
    assert!(out.samples.len() >= long.samples.len());
}

#[test]
fn documentary_is_audibly_different_from_the_plain_voice_and_applying_says_what_it_did() {
    let mut s = demo();
    // measured, since it cannot be heard here: the chain adds body and presence, so the level moves
    preview(&mut s, json!({}));
    let plain = previewed(&s);
    preview(&mut s, json!({"through": "Documentary"}));
    let doc = previewed(&s);
    let n = plain.len().min(doc.len());
    let diff: Vec<f32> = (0..n).map(|i| doc[i] - plain[i]).collect();
    assert!(rms(&diff) > rms(&plain) * 0.15, "the difference is at least 15% of the voice: {} vs {}", rms(&diff), rms(&plain));
    // applying to a narration reports it
    let dir = scratch("summary");
    let (clip, strip) = narrate(&mut s, &dir, 1.0, json!({"voice": "basic-female"}));
    let r = s.execute("tts.applyVoicePreset", json!({"name": "Documentary", "clip": clip, "dir": dir.to_string_lossy()})).unwrap();
    assert_eq!(r["summary"], format!("Documentary applied: 5 effects on {strip}, narration rendered again"));
    let listed = s.execute("tts.voicePresets", json!({"clip": clip})).unwrap();
    assert_eq!(listed["track"]["effects"].as_array().unwrap().len(), 5);
    assert_eq!(listed["track"]["effects"][0], json!({"name": "Parametric Equalizer", "enabled": true}));
    let _ = std::fs::remove_dir_all(&dir);
}
