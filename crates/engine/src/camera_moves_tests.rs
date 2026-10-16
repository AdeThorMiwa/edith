//! Tests of [`crate::camera_moves`]: the plan of each move, that no move shows the frame's edges,
//! where the keyframes land on a clip, that a move undoes in one step, and hostile input.

use filmcraft_media::{DemoScene, Generator, generators::GeneratorSource};
use filmcraft_project::Label;
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};
use serde_json::json;
use std::sync::Arc;

use crate::Session;
use crate::camera_moves::{MOVES, plan};

const W: u32 = 320;
const H: u32 = 180;

fn session(seconds: i64, generator: Generator) -> (Session, u64) {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"width": W, "height": H, "fps": 24})).unwrap();
    let item = crate::demo::add_generator(
        Arc::make_mut(&mut s.project),
        &s.media,
        GeneratorSource::new(generator, W, H, FrameRate::FPS_24, Tick(seconds * TICKS_PER_SECOND)),
        "Source clip",
        Label::Iris,
        None,
    );
    s.execute("timeline.place", json!({"item": item.0, "track": "V1"})).unwrap();
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    (s, clip)
}

fn demo(seconds: i64) -> (Session, u64) {
    session(seconds, Generator::Demo(DemoScene::OceanSunset))
}

fn motion(s: &Session, clip: u64) -> filmcraft_project::EffectInstance {
    s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(clip)).unwrap().1.effect("motion").unwrap().clone()
}

fn frame(s: &mut Session, name: &str, at: Tick) -> Vec<u8> {
    let dir = std::env::var("EDITH_SAMPLES_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join(format!("edith-camera-{}", std::process::id())));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.png"));
    s.set_playhead(at);
    s.execute("file.exportFrame", json!({"target": "program", "path": path.to_string_lossy()})).unwrap();
    image::load_from_memory(&std::fs::read(path).unwrap()).unwrap().to_rgba8().into_raw()
}

#[test]
fn each_move_starts_and_ends_where_it_says_and_never_shows_the_frames_edges() {
    for mv in MOVES {
        for amount in [0.0, 1.0, 25.0, 50.0, 100.0] {
            for secs in [0.2, 3.0, 400.0] {
                let keys = plan(mv, amount, 7, secs).unwrap_or_else(|| panic!("{mv}"));
                assert!(keys.len() >= 2 && keys.len() <= 61, "{mv}: {} keys", keys.len());
                assert_eq!((keys[0].at, keys[keys.len() - 1].at), (0.0, 1.0), "{mv} spans the whole clip");
                assert!(keys.windows(2).all(|w| w[0].at < w[1].at), "{mv}: in order");
                for k in &keys {
                    // the picture is enlarged enough that its edges stay outside the frame
                    let cover = k.scale / 100.0;
                    assert!(cover >= 1.0 + 2.0 * k.dx.abs() - 1e-9, "{mv} {amount}: scale {} for offset {}", k.scale, k.dx);
                    assert!(cover >= 1.0 + 2.0 * k.dy.abs() - 1e-9, "{mv} {amount}: scale {} for offset {}", k.scale, k.dy);
                    assert!(k.scale >= 100.0 - 1e-9 && k.scale <= 200.0);
                }
            }
        }
    }
    let push = plan("pushIn", 50.0, 0, 5.0).unwrap();
    assert_eq!((push[0].scale, push[1].scale), (100.0, 115.0));
    let pull = plan("pullOut", 50.0, 0, 5.0).unwrap();
    assert_eq!((pull[0].scale, pull[1].scale), (115.0, 100.0));
    // amount 0 is no move at all
    for mv in MOVES {
        assert!(plan(mv, 0.0, 3, 5.0).unwrap().iter().all(|k| k.scale == 100.0 && k.dx == 0.0 && k.dy == 0.0), "{mv}");
    }
    // drifts go opposite ways
    let (l, r) = (plan("driftLeft", 100.0, 0, 5.0).unwrap(), plan("driftRight", 100.0, 0, 5.0).unwrap());
    assert!(l[1].dx < l[0].dx && r[1].dx > r[0].dx);
    assert!(plan("spin", 50.0, 0, 5.0).is_none());
    assert!(plan("pushIn", f64::NAN, 0, 5.0).unwrap().iter().all(|k| k.scale == 100.0));
}

#[test]
fn a_sway_is_the_same_for_the_same_seed_and_different_for_another() {
    let a = plan("sway", 60.0, 1, 9.0).unwrap();
    assert_eq!(a, plan("sway", 60.0, 1, 9.0).unwrap());
    assert_ne!(a, plan("sway", 60.0, 2, 9.0).unwrap());
    assert!(a.len() >= 10, "about one point every 0.6 s: {}", a.len());
    assert!(a.iter().any(|k| k.dx != 0.0) && a.iter().any(|k| k.dy != 0.0), "it moves both ways");
    assert!(plan("sway", 60.0, 1, 100_000.0).unwrap().len() <= 61, "a very long clip is not given thousands of keys");
}

#[test]
fn push_in_puts_keyframes_on_the_first_and_last_frame_and_the_picture_really_zooms() {
    let (mut s, clip) = demo(4);
    let before = frame(&mut s, "40-push-start", Tick::ZERO);
    let last = FrameRate::FPS_24.tick_of(4 * 24 - 1);
    let before_last = frame(&mut s, "41-plain-end", last);
    let undo0 = s.history.undo.len();
    let r = s.execute("motion.cameraMove", json!({"clip": clip, "move": "pushIn", "amount": 80})).unwrap();
    assert_eq!((r["keyframes"].clone(), r["move"].clone()), (json!(2), json!("pushIn")));
    assert_eq!(s.history.undo.len(), undo0 + 1, "one undo step");
    let m = motion(&s, clip);
    let scale = &m.param("scale").unwrap().keyframes;
    assert_eq!(scale.len(), 2);
    let it = s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(clip)).unwrap().1.clone();
    assert_eq!(scale[0].time, it.effect_time_at(it.start), "first key at the clip's first frame");
    assert_eq!(scale[1].time, it.effect_time_at(it.end()), "last key at its end");
    assert_eq!(scale[0].value.as_f64(), Some(100.0));
    assert_eq!(scale[1].value.as_f64(), Some(124.0));
    assert_eq!(m.param("position").unwrap().keyframes.len(), 2);
    // the first frame is the unmoved picture; later frames are closer in
    assert_eq!(before, frame(&mut s, "start-after", Tick::ZERO), "the first frame is untouched");
    assert_ne!(before_last, frame(&mut s, "42-push-end", last), "by the end the picture has moved in");
    // a second move replaces the first (same number of keys, not more)
    s.execute("motion.cameraMove", json!({"clip": clip, "move": "driftLeft", "amount": 40})).unwrap();
    assert_eq!(motion(&s, clip).param("scale").unwrap().keyframes.len(), 2);
    assert_eq!(motion(&s, clip).param("scale").unwrap().keyframes[0].value.as_f64(), Some(100.0 * (1.0 + 2.0 * 0.08 * 0.4)));
    // undo takes each away
    s.undo().unwrap();
    s.undo().unwrap();
    assert!(motion(&s, clip).param("scale").unwrap().keyframes.is_empty());
    assert_eq!(before_last, frame(&mut s, "undone", last), "undo gives back the plain picture");
}

#[test]
fn it_follows_a_trimmed_clip_and_works_on_a_still_like_matte() {
    let (mut s, clip) = demo(6);
    // trim the clip: it starts 2 s into the media and plays 3 s
    s.execute("clip.trim", json!({"clip": clip, "edge": "out", "time": 3.0})).ok();
    let dur = s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(clip)).unwrap().1.duration;
    assert!(dur > Tick::ZERO);
    s.execute("motion.cameraMove", json!({"clip": clip, "move": "sway", "amount": 60, "seed": 3})).unwrap();
    let m = motion(&s, clip);
    let keys = &m.param("position").unwrap().keyframes;
    assert!(keys.len() >= 3);
    assert!(keys.windows(2).all(|w| w[0].time < w[1].time), "sorted");
    let (mut s2, c2) = session(5, Generator::ColorMatte { color: [0.2, 0.3, 0.5, 1.0] });
    s2.execute("motion.cameraMove", json!({"clip": c2, "move": "pullOut"})).unwrap();
    assert_eq!(motion(&s2, c2).param("scale").unwrap().keyframes.len(), 2);
}

#[test]
fn a_selected_clip_is_used_when_none_is_given_and_hostile_input_is_refused() {
    let (mut s, clip) = demo(3);
    s.state.selection.clear();
    assert!(s.execute("motion.cameraMove", json!({"move": "pushIn"})).is_err(), "nothing selected");
    s.state.selection = vec![filmcraft_project::ClipId(clip)];
    assert!(s.execute("motion.cameraMove", json!({"move": "pushIn"})).is_ok(), "the selected clip");
    let before = (*s.project).clone();
    let big = "x".repeat(10_000);
    for p in [
        json!({}),
        json!({"move": 5}),
        json!({"move": "spin"}),
        json!({"move": big}),
        json!({"move": "pushIn", "amount": -1}),
        json!({"move": "pushIn", "amount": 101}),
        json!({"move": "pushIn", "amount": "lots"}),
        json!({"move": "pushIn", "seed": -1}),
        json!({"move": "pushIn", "seed": "a"}),
        json!({"move": "pushIn", "clip": 999_999}),
        json!({"move": "pushIn", "clip": "abc"}),
        json!({"move": "pushIn", "clip": [1]}),
    ] {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute("motion.cameraMove", p.clone())));
        assert!(r.is_ok() && r.unwrap().is_err(), "accepted {p}");
    }
    assert_eq!(*s.project, before);
    // a locked track is refused
    let seq_id = s.state.active_sequence.unwrap();
    std::sync::Arc::make_mut(&mut s.project).sequence_mut(seq_id).unwrap().video_tracks[0].locked = true;
    let locked = (*s.project).clone();
    assert!(s.execute("motion.cameraMove", json!({"move": "sway", "clip": clip})).is_err());
    assert_eq!(*s.project, locked);
}
