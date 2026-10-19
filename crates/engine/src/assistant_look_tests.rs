//! Seeing and hearing a stock result before it is offered (fork roadmap F29): the pictures reach a model that can see
//! and only that one, a sound is measured and the copy made for it is deleted, and the words for what a sound is like
//! follow the numbers.

use std::sync::mpsc::sync_channel;
use std::time::Duration;

use edith_ai::Part;
use serde_json::json;

use crate::Session;
use crate::assistant_look::PendingListen as Pending;
use crate::assistant_tests::{scripted, text_reply, tool_reply};

const SR: u32 = 22_050;

/// A mono 16-bit WAV of `samples`.
fn wav(samples: &[f32]) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|v| ((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&SR.to_le_bytes());
    out.extend_from_slice(&(SR * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    out
}

/// A steady tone, `seconds` long, optionally fading in over the first and out over the last two seconds, with a soft
/// kick every half second when `pulse` is set.
fn tone(seconds: f64, fade: bool, pulse: bool) -> Vec<f32> {
    let n = (seconds * f64::from(SR)) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(SR);
            let mut v = 0.2 * (2.0 * std::f64::consts::PI * 220.0 * t).sin();
            if pulse {
                let since = t % 0.5;
                v += 0.7 * (-since * 30.0).exp() * (2.0 * std::f64::consts::PI * 70.0 * t).sin();
            }
            if fade {
                v *= (t / 2.0).min(1.0).min(((seconds - t) / 2.0).clamp(0.0, 1.0));
            }
            v as f32
        })
        .collect()
}

fn describe(samples: &[f32]) -> String {
    let st = edith_analysis::audio::analyze_audio(SR, 1, samples).unwrap();
    let seconds = samples.len() as f64 / f64::from(SR);
    crate::assistant_look::describe_sound(&st, samples, 1, SR, seconds, seconds)
}

#[test]
fn a_sound_is_described_by_its_kind_loudness_pulse_and_how_it_starts_and_ends() {
    let held = describe(&tone(16.0, false, false));
    assert!(held.contains("LUFS") && held.contains("Tone:") && held.contains("measured, not heard"), "{held}");
    assert!(held.contains("no fade at either end") && held.contains("repeated or crossfaded"), "{held}");
    assert!(held.contains("very steady"), "{held}");
    let faded = describe(&tone(16.0, true, false));
    assert!(faded.contains("fades in and fades out") && faded.contains("will not repeat cleanly"), "{faded}");
    // a kick every half second is 120 beats a minute
    let beat = describe(&tone(24.0, false, true));
    assert!(beat.contains("steady pulse at about 12"), "{beat}");
    // a short sound says so, and a long one says how much of it was measured
    let st = edith_analysis::audio::analyze_audio(SR, 1, &tone(20.0, false, false)).unwrap();
    assert!(crate::assistant_look::describe_sound(&st, &tone(20.0, false, false), 1, SR, 20.0, 200.0).contains("its first 20 s of 200 s"));
}

fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("edith-look-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

fn pending(
    s: &mut Session,
    rx: std::sync::mpsc::Receiver<Result<std::path::PathBuf, String>>,
    delete: bool,
    age: u64,
) -> std::sync::mpsc::Receiver<edith_ai::ToolOutput> {
    let (reply, answer) = sync_channel(1);
    let since = web_time::Instant::now().checked_sub(Duration::from_secs(age)).unwrap();
    s.assistant.cards.looks.push_listen_for_tests(Pending { reply, since, line: "freesound:77 | “Hum” by A".into(), rx, delete });
    answer
}

#[test]
fn a_fetched_sound_is_measured_answered_and_its_copy_deleted() {
    let mut s = Session::default();
    let path = temp_file("hum.wav", &wav(&tone(14.0, false, false)));
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(Ok(path.clone())).unwrap();
    let answer = pending(&mut s, rx, true, 0);
    crate::assistant_look::pump(&mut s);
    let out = answer.try_recv().unwrap();
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.starts_with("freesound:77 | “Hum” by A\n") && out.content.contains("LUFS") && out.content.contains("all 14 s"), "{}", out.content);
    assert!(!path.exists(), "the copy made for listening is deleted");
    // a sound in the person's own folders is measured where it is, and left alone
    let own = temp_file("own.wav", &wav(&tone(10.0, false, false)));
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(Ok(own.clone())).unwrap();
    let answer = pending(&mut s, rx, false, 0);
    crate::assistant_look::pump(&mut s);
    assert!(!answer.try_recv().unwrap().is_error);
    assert!(own.exists(), "the person's own file is never deleted");
    let _ = std::fs::remove_file(own);
}

#[test]
fn a_sound_that_cannot_be_fetched_or_opened_or_is_too_slow_says_so_and_never_hangs() {
    let mut s = Session::default();
    // the fetch failed
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(Err("it is 40 MB, over the 12 MB limit".to_string())).unwrap();
    let answer = pending(&mut s, rx, true, 0);
    // the file is not sound
    let junk = temp_file("junk.mp3", b"this is not audio at all");
    let (tx2, rx2) = std::sync::mpsc::channel();
    tx2.send(Ok(junk.clone())).unwrap();
    let junk_answer = pending(&mut s, rx2, true, 0);
    // nothing has arrived yet and it has been too long
    let (_tx3, rx3) = std::sync::mpsc::channel::<Result<std::path::PathBuf, String>>();
    let slow = pending(&mut s, rx3, true, 200);
    // nothing has arrived yet and it has not been long: it waits
    let (_tx4, rx4) = std::sync::mpsc::channel::<Result<std::path::PathBuf, String>>();
    let waiting = pending(&mut s, rx4, true, 1);
    crate::assistant_look::pump(&mut s);
    let a = answer.try_recv().unwrap();
    assert!(a.is_error && a.content.contains("over the 12 MB limit"), "{}", a.content);
    let j = junk_answer.try_recv().unwrap();
    assert!(j.is_error && j.content.contains("could not be opened"), "{}", j.content);
    assert!(!junk.exists());
    let t = slow.try_recv().unwrap();
    assert!(t.is_error && t.content.contains("too long"), "{}", t.content);
    assert!(waiting.try_recv().is_err(), "a sound still on its way is waited for");
    assert_eq!(s.assistant.cards.looks.waiting_for_tests(), 1);
}

#[test]
fn the_new_tools_are_refused_cleanly_when_asked_badly() {
    let mut s = Session::default();
    let model = scripted(vec![
        tool_reply(&[
            ("a", "media_look", json!({})),
            ("b", "media_look", json!({"candidates": ["pexels:never-seen"]})),
            ("c", "media_look", json!({"candidates": ["a", "b", "c", "d", "e", "f", "g"]})),
            ("d", "media_listen", json!({})),
            ("e", "media_listen", json!({"candidate": "freesound:never-seen"})),
            ("f", "media_listen", json!({"candidate": 7})),
            ("g", "media_look", json!("pexels:1")),
        ]),
        text_reply("none worked"),
    ]);
    s.assistant.use_model(model.clone());
    s.execute("assistant.send", json!({"text": "look", "wait": true})).unwrap();
    let results: Vec<(String, bool)> = model.requests()[1]
        .messages
        .iter()
        .flat_map(|m| m.parts.iter())
        .filter_map(|p| if let Part::ToolResult { content, is_error, .. } = p { Some((content.clone(), *is_error)) } else { None })
        .collect();
    assert_eq!(results.len(), 7);
    assert!(results.iter().all(|(_, e)| *e), "{results:?}");
    assert!(results[0].0.contains("1 to 6 result ids"), "{results:?}");
    assert!(results[1].0.contains("search with media_search first") || results[1].0.contains("cannot be reached"), "{results:?}");
    assert!(results[2].0.contains("at most 6"), "{results:?}");
}

#[cfg(feature = "net")]
mod with_the_sites {
    use super::*;

    fn tool_results(model: &edith_ai::scripted::ScriptedModel, request: usize) -> Vec<(String, bool, usize, Vec<String>)> {
        model.requests()[request]
            .messages
            .iter()
            .flat_map(|m| m.parts.iter())
            .filter_map(|p| match p {
                Part::ToolResult { content, is_error, images, .. } => {
                    Some((content.clone(), *is_error, images.len(), images.iter().map(|i| i.media_type.clone()).collect()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn results_are_seen_as_pictures_before_they_are_offered_and_nothing_is_downloaded() {
        let (mut s, _rig) = crate::hub_tests::rig(None);
        let model = scripted(vec![
            tool_reply(&[("m", "media_search", json!({"text": "pier at dawn", "kind": "image", "count": 3}))]),
            tool_reply(&[("l", "media_look", json!({"candidates": ["pexels:photo-1", "pixabay:image-9", "pexels:never-seen"]}))]),
            tool_reply(&[("l2", "media_look", json!({"candidates": ["pexels:photo-1", "pixabay:image-9"], "frames": 3}))]),
            text_reply("The first suits the opening."),
        ]);
        s.assistant.use_model(model.clone());
        let items = s.project.items.len();
        s.execute("assistant.send", json!({"text": "find the harbour shot", "wait": true})).unwrap();
        // an id she never saw is refused by name, and nothing is fetched for the others
        let refused = tool_results(&model, 2);
        let refused = &refused.last().unwrap();
        assert!(refused.1 && refused.0.contains("pexels:never-seen") && refused.2 == 0, "{refused:?}");
        let seen = tool_results(&model, 3);
        let seen = seen.last().unwrap();
        assert!(!seen.1, "{seen:?}");
        assert_eq!((seen.2, seen.3.clone()), (2, vec!["image/png".to_string(), "image/png".to_string()]), "one picture each (a photo has one), as png");
        assert!(seen.0.contains("Picture 1") && seen.0.contains("Picture 2") && seen.0.contains("A pier at dawn") && seen.0.contains("licence:"), "{}", seen.0);
        assert!(seen.0.contains("Nothing has been downloaded"), "{}", seen.0);
        assert_eq!(s.project.items.len(), items, "looking adds nothing to the project");
        assert!(s.hub.acquired("pexels:photo-1").is_none());
    }

    #[test]
    fn listening_reports_what_cannot_be_opened_and_a_picture_is_not_a_sound() {
        let (mut s, _rig) = crate::hub_tests::rig(None);
        // a search the session has made, with a sound and pictures among its results
        s.execute("hub.search", json!({"text": "fog horn pier", "kinds": ["audio", "image"], "wait": true})).unwrap();
        let model = scripted(vec![
            tool_reply(&[("l", "media_listen", json!({"candidate": "freesound:77"})), ("i", "media_listen", json!({"candidate": "pexels:photo-1"}))]),
            text_reply("I could not measure it."),
        ]);
        s.assistant.use_model(model.clone());
        s.execute("assistant.send", json!({"text": "find a horn", "wait": true})).unwrap();
        let heard = tool_results(&model, 1);
        // the rig serves a picture where the sound should be: it is fetched, found not to be sound, said so, and deleted
        let horn = heard.iter().find(|r| r.0.contains("could not be opened")).unwrap_or_else(|| panic!("{heard:?}"));
        assert!(horn.1, "{horn:?}");
        let not_a_sound = heard.iter().find(|r| r.0.contains("not a sound")).unwrap_or_else(|| panic!("{heard:?}"));
        assert!(not_a_sound.1 && not_a_sound.0.contains("media_look"), "{not_a_sound:?}");
        let left = std::fs::read_dir(std::env::temp_dir().join(format!("edith-listen-{}", std::process::id()))).map(|d| d.count()).unwrap_or(0);
        assert_eq!(left, 0, "the copy made for listening was deleted");
    }
}

/// Asks the real Openverse, NASA and Wikimedia Commons (no key), looks at what comes and listens to a real track.
/// `cargo test -p filmcraft-engine --features net --lib -- --ignored real_sites_are_seen_and_heard --nocapture`
#[cfg(feature = "net")]
#[test]
#[ignore = "uses the real network (Openverse, NASA, Wikimedia Commons; no key)"]
fn real_sites_are_seen_and_heard() {
    use crate::assistant::ToolRequest;
    use serde_json::Value;
    let mut s = Session::default();
    let run = |s: &mut Session, name: &str, input: Value| -> edith_ai::ToolOutput {
        let (reply, answer) = sync_channel(1);
        if let Some(out) = crate::assistant_look::serve(s, &ToolRequest { thread: 0, name: name.into(), input, reply: reply.clone() }) {
            return out;
        }
        let started = std::time::Instant::now();
        loop {
            crate::assistant_look::pump(s);
            if let Ok(out) = answer.try_recv() {
                return out;
            }
            assert!(started.elapsed() < Duration::from_secs(120), "{name} took too long");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let found = s
        .execute(
            "hub.search",
            json!({"text": "apollo moon", "kinds": ["video", "image"], "sources": ["nasa", "wikimedia", "openverse"], "wait": true, "count": 6}),
        )
        .unwrap();
    let ids: Vec<String> = found["candidates"].as_array().unwrap().iter().filter_map(|c| c["id"].as_str().map(str::to_string)).take(6).collect();
    assert!(!ids.is_empty(), "{found}");
    let out = run(&mut s, "media_look", json!({"candidates": ids, "frames": 3}));
    eprintln!("{}\n({} pictures, {} bytes)", out.content, out.images.len(), out.images.iter().map(|i| i.data.len()).sum::<usize>());
    assert!(!out.is_error && !out.images.is_empty(), "{}", out.content);
    let music = s.execute("hub.search", json!({"text": "ambient piano", "kinds": ["audio"], "sources": ["openverse"], "wait": true, "count": 3})).unwrap();
    let id = music["candidates"][0]["id"].as_str().unwrap().to_string();
    let heard = run(&mut s, "media_listen", json!({"candidate": id}));
    eprintln!("{}", heard.content);
    assert!(!heard.is_error && heard.content.contains("LUFS"), "{}", heard.content);
}
