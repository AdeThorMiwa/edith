//! Tests of the fingerprint on synthetic films whose properties are known, and on hostile input.

use crate::audio::{AudioKind, Mix};
use crate::fingerprint::{CURVE_POINTS, Fingerprint, PaceArc, PictureSample, ReferenceInput, SoundInput, SoundSegment, fingerprint};
use crate::image::analyze_rgba;
use crate::motion::CameraMove;
use crate::{AnalysisError, FrameStats};

fn solid(rgb: [u8; 3]) -> FrameStats {
    let px: Vec<u8> = (0..32 * 18).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect();
    analyze_rgba(32, 18, &px).unwrap()
}

fn film(duration: f64, cuts: Vec<f64>) -> ReferenceInput {
    ReferenceInput { name: "Test film".into(), duration_seconds: duration, width: 1920, height: 1080, cuts, pictures: vec![], sound: None }
}

fn every(duration: f64, step: f64) -> Vec<f64> {
    (1..).map(|i| f64::from(i) * step).take_while(|t| *t < duration).collect()
}

#[test]
fn a_film_cut_every_ten_seconds_has_sixty_shots_of_ten_and_a_flat_rhythm() {
    let f = fingerprint(&film(600.0, every(600.0, 10.0))).unwrap();
    assert_eq!((f.version, f.shots.count), (1, 60));
    assert_eq!(
        (f.shots.mean_seconds, f.shots.median_seconds, f.shots.p10_seconds, f.shots.p90_seconds, f.shots.min_seconds, f.shots.max_seconds),
        (10.0, 10.0, 10.0, 10.0, 10.0, 10.0)
    );
    assert!((f.shots.cuts_per_minute - 5.9).abs() < 1e-9, "{}", f.shots.cuts_per_minute);
    let counts: Vec<u32> = f.shots.histogram.iter().map(|b| b.count).collect();
    assert_eq!(counts, [0, 0, 0, 0, 60, 0], "ten seconds is under 16 and not under 8");
    assert_eq!(f.rhythm.curve.len(), CURVE_POINTS);
    // 30 s per twentieth, 3 cuts in each (the first has 2: the cut at 30 s starts the second)
    assert!(f.rhythm.curve.iter().all(|c| (*c - 6.0).abs() < 2.1), "{:?}", f.rhythm.curve);
    let cuts: f64 = f.rhythm.curve.iter().map(|c| f64::from(*c) * 30.0 / 60.0).sum();
    assert!((cuts - 59.0).abs() < 1e-3, "every cut is in exactly one twentieth: {cuts}");
    assert_eq!(f.rhythm.arc, PaceArc::Steady);
    assert!((f.frame_aspect - 16.0 / 9.0).abs() < 1e-5);
}

#[test]
fn the_arc_says_whether_the_shots_get_shorter_or_longer() {
    // 900 s: first third in 20 s shots, last third in 4 s shots
    let mut quick = every(300.0, 20.0);
    quick.extend((0..).map(|i| 300.0 + 10.0 + f64::from(i) * 10.0).take_while(|t| *t < 600.0));
    quick.extend((0..).map(|i| 600.0 + 4.0 + f64::from(i) * 4.0).take_while(|t| *t < 900.0));
    let f = fingerprint(&film(900.0, quick.clone())).unwrap();
    assert_eq!(f.rhythm.arc, PaceArc::Quickening);
    let (o, m, c) = (f.rhythm.opening_mean_seconds.unwrap(), f.rhythm.middle_mean_seconds.unwrap(), f.rhythm.closing_mean_seconds.unwrap());
    assert!(o > m && m > c && (o - 20.0).abs() < 1.5 && (c - 4.0).abs() < 0.5, "{o} {m} {c}");
    // the same film backwards slows down
    let back: Vec<f64> = quick.iter().rev().map(|t| 900.0 - t).collect();
    assert_eq!(fingerprint(&film(900.0, back)).unwrap().rhythm.arc, PaceArc::Slowing);
    // a change of a few per cent is steady
    assert_eq!(fingerprint(&film(900.0, every(900.0, 10.0))).unwrap().rhythm.arc, PaceArc::Steady);
}

#[test]
fn a_film_with_no_cuts_is_one_shot_and_hostile_cuts_are_ignored() {
    let one = fingerprint(&film(120.0, vec![])).unwrap();
    assert_eq!((one.shots.count, one.shots.mean_seconds, one.shots.cuts_per_minute), (1, 120.0, 0.0));
    assert!(one.rhythm.curve.iter().all(|c| *c == 0.0));
    // out of range, repeated, unsorted, not numbers: only the real cuts count
    let messy = vec![50.0, 25.0, 25.0, -3.0, 0.0, 120.0, 500.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 75.0];
    let f = fingerprint(&film(120.0, messy)).unwrap();
    assert_eq!(f.shots.count, 4, "cuts at 25, 50 and 75 s");
    assert_eq!((f.shots.min_seconds, f.shots.max_seconds), (25.0, 45.0), "three shots of 25 s and the last 45 s");
}

#[test]
fn films_that_cannot_be_measured_are_refused() {
    for d in [0.0, -5.0, f64::NAN, f64::INFINITY, 49.0 * 3600.0] {
        assert_eq!(fingerprint(&film(d, vec![])), Err(AnalysisError::BadInterval), "{d}");
    }
    let many = film(1000.0, (0..200_001).map(|i| f64::from(i) * 0.004 + 0.001).collect());
    assert!(matches!(fingerprint(&many), Err(AnalysisError::TooMany { what: "cuts", .. })));
    let mut pics = film(100.0, vec![]);
    pics.pictures =
        (0..5_001).map(|i| PictureSample { time_seconds: f64::from(i) * 0.01, stats: solid([100, 100, 100]), camera: None, motion_energy: None }).collect();
    assert!(matches!(fingerprint(&pics), Err(AnalysisError::TooMany { what: "pictures", .. })));
    let mut snd = film(100.0, vec![]);
    snd.sound = Some(SoundInput {
        mix: Mix::default(),
        segments: vec![SoundSegment { start_seconds: 0.0, duration_seconds: 1.0, kind: AudioKind::Music, loudness_lufs: None }; 20_001],
    });
    assert!(matches!(fingerprint(&snd), Err(AnalysisError::TooMany { what: "pieces of sound", .. })));
}

#[test]
fn the_picture_is_averaged_and_followed_through_the_film() {
    let mut f = film(1000.0, every(1000.0, 10.0));
    // 40 pictures, getting brighter and warmer through the film
    f.pictures = (0..40)
        .map(|i| {
            let level = 40 + i * 5;
            let mut stats = solid([level as u8, level as u8, (level as u8).saturating_sub(20)]);
            stats.letterbox.top = 0.1;
            stats.letterbox.bottom = 0.1;
            stats.active_aspect = 2.35;
            PictureSample { time_seconds: f64::from(i) * 25.0 + 5.0, stats, camera: None, motion_energy: None }
        })
        .collect();
    let p = fingerprint(&f).unwrap().picture.expect("a picture");
    assert!(p.luma_mean > 0.25 && p.luma_mean < 0.7, "{}", p.luma_mean);
    assert_eq!(p.timeline.len(), CURVE_POINTS);
    assert!(p.timeline.windows(2).all(|w| w[1].luma > w[0].luma && w[1].at > w[0].at), "brighter through the film: {:?}", p.timeline);
    assert!(p.timeline.iter().all(|t| (0.0..=1.0).contains(&t.at)));
    assert_eq!((p.letterbox.top, p.letterbox.bottom, p.letterbox.left), (0.1, 0.1, 0.0));
    assert_eq!(p.active_aspect, 2.35);
    assert!(!p.palette.is_empty() && p.palette.len() <= 5);
    assert!((p.palette.iter().map(|c| c.share).sum::<f32>() - 1.0).abs() < 1e-4);
    assert!(p.colour_temperature.is_some());
    // no pictures, no picture; pictures that are not numbers are left out
    assert!(fingerprint(&film(100.0, vec![])).unwrap().picture.is_none());
    let mut bad = film(100.0, vec![]);
    let mut nan = solid([90, 90, 90]);
    nan.luma_mean = f32::NAN;
    bad.pictures = vec![
        PictureSample { time_seconds: 10.0, stats: nan, camera: None, motion_energy: None },
        PictureSample { time_seconds: f64::NAN, stats: solid([90, 90, 90]), camera: None, motion_energy: None },
        PictureSample { time_seconds: 20.0, stats: solid([90, 90, 90]), camera: None, motion_energy: None },
    ];
    let one = fingerprint(&bad).unwrap().picture.unwrap();
    assert!(one.luma_mean.is_finite() && one.timeline.len() == 1);
}

#[test]
fn motion_and_camera_shares_are_measured_when_they_were() {
    let mut f = film(100.0, vec![]);
    let cams = [CameraMove::Static, CameraMove::Static, CameraMove::Pan, CameraMove::Unknown];
    f.pictures = cams
        .iter()
        .enumerate()
        .map(|(i, c)| PictureSample { time_seconds: i as f64 * 20.0, stats: solid([80, 80, 80]), camera: Some(*c), motion_energy: Some(0.02 * (i + 1) as f32) })
        .collect();
    let m = fingerprint(&f).unwrap().motion.unwrap();
    assert!((m.energy - 0.05).abs() < 1e-6);
    assert!((m.camera["static"] - 2.0 / 3.0).abs() < 1e-6 && (m.camera["pan"] - 1.0 / 3.0).abs() < 1e-6, "unknown is left out: {:?}", m.camera);
    assert!((m.camera.values().sum::<f32>() - 1.0).abs() < 1e-5);
    assert!(fingerprint(&film(100.0, vec![])).unwrap().motion.is_none());
}

#[test]
fn the_sound_is_summed_up_and_followed_through_the_film() {
    let mut f = film(600.0, every(600.0, 10.0));
    let segments: Vec<SoundSegment> = (0..20)
        .map(|i| SoundSegment {
            start_seconds: f64::from(i) * 30.0,
            duration_seconds: 30.0,
            kind: if i % 2 == 0 { AudioKind::Speech } else { AudioKind::Music },
            loudness_lufs: Some(-30.0 + f64::from(i)),
        })
        .collect();
    f.sound = Some(SoundInput {
        mix: Mix { duration_seconds: 600.0, integrated_lufs: Some(-18.0), loudness_range_lu: Some(9.0), true_peak_dbtp: Some(-2.0), silence_share: 0.1 },
        segments,
    });
    let s = fingerprint(&f).unwrap().sound.unwrap();
    assert_eq!((s.integrated_lufs, s.loudness_range_lu, s.true_peak_dbtp), (Some(-18.0), Some(9.0), Some(-2.0)));
    assert!((s.speech_share - 0.5).abs() < 1e-6 && (s.music_share - 0.5).abs() < 1e-6 && s.ambience_share == 0.0 && s.ambience_share.is_sign_positive());
    assert!((s.silence_share - 0.1).abs() < 1e-6);
    assert_eq!(s.loudness_curve.len(), CURVE_POINTS);
    assert!((s.loudness_curve[0] + 30.0).abs() < 1e-4 && (s.loudness_curve[19] + 11.0).abs() < 1e-4, "{:?}", s.loudness_curve);
    // a stretch that was silent is at the floor, and a stretch with no piece takes the one before
    let mut gap = f.clone();
    if let Some(snd) = gap.sound.as_mut() {
        snd.segments.truncate(5);
        snd.segments[2].loudness_lufs = None;
    }
    let c = fingerprint(&gap).unwrap().sound.unwrap().loudness_curve;
    assert_eq!(c[2], -70.0, "silent");
    assert_eq!(c[10], c[4], "past the last piece it holds the last value");
    // pieces that begin partway in: the stretch before them is taken to be as loud as the first piece, not silent
    let mut late = film(600.0, vec![]);
    late.sound = Some(SoundInput {
        mix: Mix { duration_seconds: 600.0, integrated_lufs: Some(-20.0), loudness_range_lu: None, true_peak_dbtp: None, silence_share: 0.0 },
        segments: vec![SoundSegment { start_seconds: 300.0, duration_seconds: 30.0, kind: AudioKind::Music, loudness_lufs: Some(-22.0) }],
    });
    let c = fingerprint(&late).unwrap().sound.unwrap().loudness_curve;
    assert!(c.iter().all(|v| (*v + 22.0).abs() < 1e-4), "{c:?}");
    // no sound at all
    assert!(fingerprint(&film(100.0, vec![])).unwrap().sound.is_none());
    // pieces that are not numbers, a mix that is not
    let mut bad = film(100.0, vec![]);
    bad.sound = Some(SoundInput {
        mix: Mix {
            duration_seconds: f64::NAN,
            integrated_lufs: Some(f64::NAN),
            loudness_range_lu: Some(f64::INFINITY),
            true_peak_dbtp: None,
            silence_share: f32::NAN,
        },
        segments: vec![
            SoundSegment { start_seconds: f64::NAN, duration_seconds: 5.0, kind: AudioKind::Speech, loudness_lufs: Some(-20.0) },
            SoundSegment { start_seconds: 0.0, duration_seconds: -5.0, kind: AudioKind::Speech, loudness_lufs: Some(-20.0) },
            SoundSegment { start_seconds: 10.0, duration_seconds: 5.0, kind: AudioKind::Music, loudness_lufs: Some(f64::NAN) },
        ],
    });
    let s = fingerprint(&bad).unwrap().sound.unwrap();
    assert_eq!((s.integrated_lufs, s.loudness_range_lu, s.silence_share), (None, None, 0.0));
    assert!(s.loudness_curve.iter().all(|v| v.is_finite()) && s.music_share == 1.0 && s.speech_share == 0.0);
}

#[test]
fn a_feature_length_fingerprint_is_small_deterministic_and_round_trips_as_json() {
    let mut f = film(7200.0, every(7200.0, 2.4));
    f.pictures = (0..120)
        .map(|i| PictureSample {
            time_seconds: f64::from(i) * 60.0,
            stats: solid([(i % 200) as u8 + 20, 90, 120]),
            camera: Some(CameraMove::Static),
            motion_energy: Some(0.03),
        })
        .collect();
    f.sound = Some(SoundInput {
        mix: Mix { duration_seconds: 7200.0, integrated_lufs: Some(-23.0), loudness_range_lu: Some(14.0), true_peak_dbtp: Some(-1.0), silence_share: 0.2 },
        segments: (0..240)
            .map(|i| SoundSegment { start_seconds: f64::from(i) * 30.0, duration_seconds: 30.0, kind: AudioKind::Ambience, loudness_lufs: Some(-25.0) })
            .collect(),
    });
    let a = fingerprint(&f).unwrap();
    assert_eq!(a, fingerprint(&f).unwrap());
    let text = serde_json::to_string(&a).unwrap();
    assert!(text.len() < 12_000, "a whole film in {} bytes", text.len());
    let back: Fingerprint = serde_json::from_str(&text).unwrap();
    assert_eq!(a.shots, back.shots);
    assert_eq!(a.rhythm, back.rhythm);
    let v = serde_json::to_value(&a).unwrap();
    assert!(v["shots"]["cutsPerMinute"].is_f64() && v["rhythm"]["arc"] == "steady" && v["sound"]["loudnessCurve"].is_array());
    // a name is cleaned and cut
    let mut n = film(10.0, vec![]);
    n.name = format!("a\u{7}b{}", "x".repeat(500));
    let name = fingerprint(&n).unwrap().name;
    assert!(name.chars().count() <= 120 && !name.contains('\u{7}'));
}

#[test]
fn a_fingerprint_made_here_passes_the_check_and_damaged_ones_do_not() {
    let mut f = film(900.0, every(900.0, 7.0));
    f.pictures = (0..30)
        .map(|i| PictureSample { time_seconds: f64::from(i) * 30.0, stats: solid([90, 100, 110]), camera: Some(CameraMove::Pan), motion_energy: Some(0.05) })
        .collect();
    f.sound = Some(SoundInput {
        mix: Mix { duration_seconds: 900.0, integrated_lufs: Some(-20.0), loudness_range_lu: Some(8.0), true_peak_dbtp: Some(-3.0), silence_share: 0.1 },
        segments: (0..30)
            .map(|i| SoundSegment { start_seconds: f64::from(i) * 30.0, duration_seconds: 30.0, kind: AudioKind::Speech, loudness_lufs: Some(-21.0) })
            .collect(),
    });
    let good = fingerprint(&f).unwrap();
    assert_eq!(good.check(), Ok(()));
    assert_eq!(fingerprint(&film(10.0, vec![])).unwrap().check(), Ok(()), "even a bare one");
    let damaged: Vec<(&str, Box<dyn Fn(&mut Fingerprint)>)> = vec![
        ("version", Box::new(|f| f.version = 99)),
        ("duration", Box::new(|f| f.duration_seconds = f64::NAN)),
        ("no length", Box::new(|f| f.duration_seconds = 0.0)),
        ("name", Box::new(|f| f.name = "x".repeat(500))),
        ("name control", Box::new(|f| f.name = "a\u{7}b".into())),
        ("size", Box::new(|f| f.width = u32::MAX)),
        ("shots", Box::new(|f| f.shots.mean_seconds = f64::INFINITY)),
        ("negative shots", Box::new(|f| f.shots.min_seconds = -1.0)),
        ("histogram", Box::new(|f| f.shots.histogram = vec![Default::default(); 100])),
        ("curve short", Box::new(|f| f.rhythm.curve.truncate(5))),
        ("curve long", Box::new(|f| f.rhythm.curve = vec![1.0; 10_000])),
        ("curve nan", Box::new(|f| f.rhythm.curve[3] = f32::NAN)),
        ("thirds", Box::new(|f| f.rhythm.opening_mean_seconds = Some(f64::NAN))),
        ("picture", Box::new(|f| f.picture.as_mut().unwrap().luma_mean = f32::NAN)),
        ("bars", Box::new(|f| f.picture.as_mut().unwrap().letterbox.top = 5.0)),
        ("palette", Box::new(|f| f.picture.as_mut().unwrap().palette = vec![f.picture.as_ref().unwrap().palette[0]; 50])),
        ("timeline", Box::new(|f| f.picture.as_mut().unwrap().timeline = vec![f.picture.as_ref().unwrap().timeline[0]; 50])),
        ("motion", Box::new(|f| f.motion.as_mut().unwrap().energy = f32::INFINITY)),
        ("camera", Box::new(|f| f.motion.as_mut().unwrap().camera = (0..50).map(|i| (format!("c{i}"), 0.02)).collect())),
        ("camera share", Box::new(|f| f.motion.as_mut().unwrap().camera.insert("pan".into(), 9.0).map(drop).unwrap_or(()))),
        ("sound", Box::new(|f| f.sound.as_mut().unwrap().integrated_lufs = Some(f64::NAN))),
        ("share", Box::new(|f| f.sound.as_mut().unwrap().music_share = -0.5)),
        ("sound curve", Box::new(|f| f.sound.as_mut().unwrap().loudness_curve = vec![0.0; 7])),
    ];
    for (what, break_it) in damaged {
        let mut f = good.clone();
        break_it(&mut f);
        assert!(f.check().is_err(), "{what} should be refused");
    }
}

#[test]
fn readings_say_what_the_numbers_suggest_and_only_what_is_there() {
    // a patient film: 14 s shots, mostly silent, still camera, dark and widescreen
    let mut f = film(1200.0, every(1200.0, 14.0));
    f.pictures = (0..30)
        .map(|i| {
            let mut stats = solid([40, 36, 30]);
            stats.active_aspect = 2.39;
            stats.grain = 0.03;
            PictureSample { time_seconds: f64::from(i) * 40.0, stats, camera: Some(CameraMove::Static), motion_energy: Some(0.01) }
        })
        .collect();
    f.sound = Some(SoundInput {
        mix: Mix { duration_seconds: 1200.0, integrated_lufs: Some(-25.0), loudness_range_lu: Some(15.0), true_peak_dbtp: Some(-3.0), silence_share: 0.35 },
        segments: (0..40)
            .map(|i| SoundSegment { start_seconds: f64::from(i) * 30.0, duration_seconds: 30.0, kind: AudioKind::Ambience, loudness_lufs: Some(-25.0) })
            .collect(),
    });
    let r = fingerprint(&f).unwrap().readings();
    let all = r.join(" ");
    for want in [
        "a patient pace",
        "holds steady",
        "dark",
        "visibly grainy",
        "widescreen, about 2.39:1",
        "mostly still",
        "About 35% of it is silent",
        "atmosphere and ambience",
        "dynamics are wide",
        "It is quiet",
        "contemplative",
    ] {
        assert!(all.contains(want), "{want} not in {r:?}");
    }
    // a fast film: 1.5 s shots
    let fast = fingerprint(&film(600.0, every(600.0, 1.5))).unwrap().readings();
    assert!(fast[0].contains("rapid, energetic cutting"), "{fast:?}");
    // with no picture, sound or motion, nothing is said about them
    let bare = fingerprint(&film(600.0, every(600.0, 4.0))).unwrap().readings();
    assert!(bare.iter().all(|s| !s.contains("picture") && !s.contains("sound") && !s.contains("silent") && !s.contains("camera")), "{bare:?}");
    assert!(bare[0].contains("brisk cutting"), "{bare:?}");
    // one shot has no pace to read; the readings never panic on odd fingerprints
    let one = fingerprint(&film(10.0, vec![])).unwrap().readings();
    assert!(one.iter().all(|s| !s.contains("Shots average")), "{one:?}");
    // a pace that moves
    let mut moving: Vec<f64> = (1..30).map(|i| f64::from(i) * 4.0).collect();
    moving.extend((1..40).map(|i| 120.0 + f64::from(i) * 12.0));
    let m = fingerprint(&film(600.0, moving)).unwrap().readings();
    assert!(m.iter().any(|s| s.contains("slows toward the end")), "{m:?}");
    // bursts
    let mut burst: Vec<f64> = (1..9).map(|i| 300.0 + f64::from(i) * 2.0).collect();
    burst.extend((1..5).map(|i| f64::from(i) * 60.0));
    let b = fingerprint(&film(600.0, burst)).unwrap().readings();
    assert!(b.iter().any(|s| s.contains("bursts")), "{b:?}");
}
