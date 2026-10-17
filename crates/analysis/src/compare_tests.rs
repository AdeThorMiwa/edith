//! Tests of comparing two fingerprints: each kind of row, its verdict and its words, what is missing,
//! and numbers that are not numbers.

use crate::audio::{AudioKind, Mix};
use crate::compare::{Comparison, Verdict, compare};
use crate::fingerprint::{Fingerprint, PictureSample, ReferenceInput, SoundInput, SoundSegment, fingerprint};
use crate::image::analyze_rgba;
use crate::motion::CameraMove;

fn stats(rgb: [u8; 3]) -> crate::FrameStats {
    let px: Vec<u8> = (0..32 * 18).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect();
    analyze_rgba(32, 18, &px).unwrap()
}

/// A 600 s film cut every `shot` seconds, with pictures and sound when asked.
fn film(shot: f64, rgb: Option<[u8; 3]>, lufs: Option<f64>, silence: f32) -> Fingerprint {
    let cuts: Vec<f64> = (1..).map(|i| f64::from(i) * shot).take_while(|t| *t < 600.0).collect();
    let pictures = rgb
        .map(|c| {
            (0..20)
                .map(|i| PictureSample { time_seconds: f64::from(i) * 30.0, stats: stats(c), camera: Some(CameraMove::Static), motion_energy: Some(0.02) })
                .collect()
        })
        .unwrap_or_default();
    let sound = lufs.map(|l| SoundInput {
        mix: Mix { duration_seconds: 600.0, integrated_lufs: Some(l), loudness_range_lu: Some(8.0), true_peak_dbtp: Some(-3.0), silence_share: silence },
        segments: (0..20)
            .map(|i| SoundSegment { start_seconds: f64::from(i) * 30.0, duration_seconds: 30.0, kind: AudioKind::Speech, loudness_lufs: Some(l) })
            .collect(),
    });
    fingerprint(&ReferenceInput { name: "Film".into(), duration_seconds: 600.0, width: 1920, height: 1080, cuts, pictures, sound }).unwrap()
}

fn find<'a>(c: &'a Comparison, metric: &str) -> &'a crate::compare::Row {
    c.rows.iter().find(|r| r.metric == metric).unwrap_or_else(|| panic!("no row {metric}: {:?}", c.rows.iter().map(|r| &r.metric).collect::<Vec<_>>()))
}

#[test]
fn the_same_film_is_close_in_every_row() {
    let f = film(10.0, Some([90, 100, 110]), Some(-18.0), 0.1);
    let c = compare(&f, &f);
    assert_eq!((c.differ, c.close as usize), (0, c.rows.len()), "{:?}", c.rows);
    assert!(c.missing.is_empty());
    assert!(c.rows.iter().all(|r| r.verdict == Verdict::Close && r.words.contains("close") || r.metric == "arc"));
    assert_eq!(find(&c, "arc").words, "How the pace moves: both steady.");
    assert_eq!(c.reference, "Film");
}

#[test]
fn lengths_are_said_as_half_twice_or_how_many_times() {
    let theirs = film(11.0, None, None, 0.0);
    // 5.4 against 11: about half
    let half = compare(&film(5.4, None, None, 0.0), &theirs);
    let r = find(&half, "meanShot");
    assert_eq!(r.verdict, Verdict::Lower);
    assert!(r.words.contains("yours 5.4 s") && r.words.contains("theirs 11 s") && r.words.contains("about half as long"), "{}", r.words);
    // cutting twice as often says the rate
    assert!(find(&half, "cutsPerMinute").words.contains("about twice as fast"), "{}", find(&half, "cutsPerMinute").words);
    // 22 against 11: twice as long
    let twice = compare(&film(22.0, None, None, 0.0), &theirs);
    assert_eq!(find(&twice, "meanShot").verdict, Verdict::Higher);
    assert!(find(&twice, "meanShot").words.contains("about twice as long"));
    // 33 s cuts leave a short last shot, so the ratio is 55 shots against 19: 2.9 times as long, and the other way 2.9 times shorter
    assert!(find(&compare(&film(33.0, None, None, 0.0), &theirs), "meanShot").words.contains("2.9 times as long"));
    assert!(find(&compare(&theirs, &film(33.0, None, None, 0.0)), "meanShot").words.contains("2.9 times shorter"));
    // within a fifth is close
    assert_eq!(find(&compare(&film(12.0, None, None, 0.0), &theirs), "meanShot").verdict, Verdict::Close);
    assert_eq!(find(&compare(&film(9.0, None, None, 0.0), &theirs), "meanShot").verdict, Verdict::Close, "under a fifth off is close");
}

#[test]
fn sound_rows_say_louder_or_quieter_in_lu_and_shares_in_points() {
    let theirs = film(10.0, None, Some(-14.0), 0.1);
    let c = compare(&film(10.0, None, Some(-20.0), 0.3), &theirs);
    let l = find(&c, "loudness");
    assert_eq!(l.verdict, Verdict::Lower);
    assert!(l.words.contains("yours -20.0") && l.words.contains("theirs -14.0") && l.words.contains("yours is 6.0 LU quieter"), "{}", l.words);
    let s = find(&c, "silence");
    assert_eq!(s.verdict, Verdict::Higher);
    assert!(s.words.contains("yours 30%") && s.words.contains("theirs 10%") && s.words.contains("20 points higher"), "{}", s.words);
    assert_eq!(find(&compare(&film(10.0, None, Some(-15.0), 0.1), &theirs), "loudness").verdict, Verdict::Close, "1 LU is close");
    assert!(find(&compare(&film(10.0, None, Some(-10.0), 0.1), &theirs), "loudness").words.contains("louder"));
}

#[test]
fn the_picture_rows_say_warmer_or_cooler_and_brighter_or_darker() {
    let theirs = film(10.0, Some([200, 150, 90]), None, 0.0);
    let c = compare(&film(10.0, Some([90, 120, 200]), None, 0.0), &theirs);
    let w = find(&c, "warmth");
    assert_eq!(w.verdict, Verdict::Higher, "bluer is a higher temperature");
    assert!(w.words.contains("cooler"), "{}", w.words);
    assert_eq!(find(&c, "brightness").verdict, Verdict::Lower);
    assert!(find(&c, "brightness").words.contains("lower"));
    assert!(c.missing.contains(&"the sound (measure your mix first)".to_string()) || c.missing.iter().any(|m| m.contains("sound")));
}

#[test]
fn what_cannot_be_compared_is_named_and_its_rows_are_left_out() {
    let theirs = film(10.0, Some([90, 100, 110]), Some(-18.0), 0.1);
    let yours = film(10.0, None, None, 0.0);
    let c = compare(&yours, &theirs);
    assert!(c.rows.iter().all(|r| r.group == "shots" || r.group == "rhythm"), "{:?}", c.rows.iter().map(|r| &r.metric).collect::<Vec<_>>());
    assert_eq!(c.missing.len(), 3, "{:?}", c.missing);
    assert!(c.missing.iter().any(|m| m.contains("measure your shots")) && c.missing.iter().any(|m| m.contains("measure your mix")));
    // the other way round says the reference has none
    let c2 = compare(&theirs, &yours);
    assert!(c2.missing.iter().any(|m| m.contains("the reference has none")));
    // speech and music rows need both sides to have pieces of sound
    let mut bare = theirs.clone();
    if let Some(s) = bare.sound.as_mut() {
        s.speech_share = 0.0;
        s.music_share = 0.0;
        s.ambience_share = 0.0;
    }
    assert!(compare(&bare, &theirs).rows.iter().all(|r| r.metric != "speech" && r.metric != "music"));
    assert!(compare(&theirs, &theirs).rows.iter().any(|r| r.metric == "speech"));
}

#[test]
fn a_different_pace_arc_is_said() {
    let quick_start: Vec<f64> = (1..30).map(|i| f64::from(i) * 4.0).chain((1..40).map(|i| 120.0 + f64::from(i) * 12.0)).collect();
    let input = |cuts: Vec<f64>| ReferenceInput { name: "A".into(), duration_seconds: 600.0, width: 1920, height: 1080, cuts, pictures: vec![], sound: None };
    let slowing = fingerprint(&input(quick_start)).unwrap();
    let steady = film(10.0, None, None, 0.0);
    let c = compare(&slowing, &steady);
    let a = find(&c, "arc");
    assert_eq!(a.verdict, Verdict::Different);
    assert!(a.words.contains("yours is slowing") && a.words.contains("theirs is steady"), "{}", a.words);
}

#[test]
fn numbers_that_are_not_numbers_or_a_zero_are_skipped_or_said_without_panic() {
    let theirs = film(10.0, Some([90, 100, 110]), Some(-18.0), 0.1);
    let mut yours = theirs.clone();
    yours.shots.mean_seconds = f64::NAN;
    yours.shots.median_seconds = f64::INFINITY;
    if let Some(s) = yours.sound.as_mut() {
        s.integrated_lufs = Some(f64::NAN);
    }
    let c = compare(&yours, &theirs);
    assert!(c.rows.iter().all(|r| r.metric != "meanShot" && r.metric != "medianShot" && r.metric != "loudness"));
    // a reference with no cuts (a zero rate) against one with cuts
    let still =
        fingerprint(&ReferenceInput { name: "Still".into(), duration_seconds: 600.0, width: 1, height: 1, cuts: vec![], pictures: vec![], sound: None })
            .unwrap();
    let c = compare(&theirs, &still);
    let r = find(&c, "cutsPerMinute");
    assert_eq!(r.verdict, Verdict::Higher);
    assert!(!r.words.contains("NaN") && !r.words.contains("inf"), "{}", r.words);
    assert_eq!(find(&compare(&still, &still), "cutsPerMinute").verdict, Verdict::Close, "nothing against nothing is close");
}

#[test]
fn a_comparison_serialises_with_its_verdicts_in_words() {
    let c = compare(&film(5.0, None, None, 0.0), &film(10.0, None, None, 0.0));
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(v["rows"][0]["verdict"], "lower");
    assert_eq!(v["rows"][0]["group"], "shots");
    assert!(v["close"].is_u64() && v["missing"].is_array());
    let back: Comparison = serde_json::from_value(v).unwrap();
    assert_eq!(back, c);
}

#[test]
fn each_row_carries_its_two_numbers_as_written() {
    let c = compare(&film(5.4, Some([200, 150, 90]), Some(-20.0), 0.3), &film(11.0, Some([90, 100, 110]), Some(-14.0), 0.1));
    let t = |m: &str| (find(&c, m).yours_text.clone(), find(&c, m).theirs_text.clone());
    assert_eq!(t("meanShot"), ("5.4 s".to_string(), "11 s".to_string()));
    assert_eq!(t("loudness"), ("-20.0".to_string(), "-14.0".to_string()));
    assert_eq!(t("silence"), ("30%".to_string(), "10%".to_string()));
    assert!(t("warmth").0.ends_with(" K") && t("brightness").0.contains('.'));
    assert_eq!(t("arc"), ("steady".to_string(), "steady".to_string()));
}
