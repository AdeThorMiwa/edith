//! Tests of the motion measures on synthetic pictures whose motion is known, and on hostile input.

use std::f64::consts::PI;

use crate::image::Frame;
use crate::motion::{CameraMove, MotionPair, analyze_motion, summarize};
use crate::{AnalysisError, MotionSummary};

/// A smooth texture with detail at several sizes, 0..1.
fn texture(u: f64, v: f64) -> f64 {
    0.5 + 0.2 * (2.0 * PI * (7.0 * u + 3.0 * v)).sin()
        + 0.15 * (2.0 * PI * (11.0 * v - 5.0 * u) + 1.0).sin()
        + 0.1 * (2.0 * PI * (17.0 * u + 13.0 * v) + 2.0).sin()
}

fn picture(w: u32, h: u32, f: impl Fn(f64, f64) -> f64) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let g = (f((f64::from(x) + 0.5) / f64::from(w), (f64::from(y) + 0.5) / f64::from(h)).clamp(0.0, 1.0) * 255.0) as u8;
            v.extend([g, g, g, 255]);
        }
    }
    v
}

fn pair(w: u32, h: u32, seconds: f64, a: impl Fn(f64, f64) -> f64, b: impl Fn(f64, f64) -> f64) -> MotionPair {
    let (pa, pb) = (picture(w, h, a), picture(w, h, b));
    analyze_motion(&Frame::new(w, h, &pa).unwrap(), &Frame::new(w, h, &pb).unwrap(), seconds).unwrap()
}

#[test]
fn identical_pictures_have_no_motion() {
    let m = pair(256, 144, 0.5, texture, texture);
    assert_eq!((m.energy, m.residual), (0.0, 0.0));
    assert!(m.dx.abs() < 1e-3 && m.dy.abs() < 1e-3 && m.zoom.abs() < 1e-3, "{m:?}");
    assert!(m.blocks > 40, "{}", m.blocks);
}

#[test]
fn a_shift_of_the_picture_is_recovered_in_size_and_direction() {
    // the picture moves right by 2% of its width in half a second: 0.04 per second
    let m = pair(256, 144, 0.5, texture, |u, v| texture(u - 0.02, v));
    assert!((m.dx - 0.04).abs() < 0.008, "{m:?}");
    assert!(m.dy.abs() < 0.008 && m.zoom.abs() < 0.01 && m.residual < 0.1, "{m:?}");
    let left = pair(256, 144, 0.5, texture, |u, v| texture(u + 0.03, v));
    assert!((left.dx + 0.06).abs() < 0.01, "{left:?}");
    // 3% of the height down in one second
    let down = pair(256, 144, 1.0, texture, |u, v| texture(u, v - 0.03));
    assert!((down.dy - 0.03).abs() < 0.008 && down.dx.abs() < 0.008, "{down:?}");
    // the time between the pictures scales the rate
    let slow = pair(256, 144, 1.0, texture, |u, v| texture(u - 0.02, v));
    assert!((slow.dx - 0.02).abs() < 0.006, "{slow:?}");
}

#[test]
fn a_zoom_is_recovered_with_its_sign() {
    let at = |s: f64| move |u: f64, v: f64| texture(0.5 + (u - 0.5) / (1.0 + s), 0.5 + (v - 0.5) / (1.0 + s));
    let zoom_in = pair(256, 144, 0.5, texture, at(0.04));
    assert!((zoom_in.zoom - 0.08).abs() < 0.02, "{zoom_in:?}");
    assert!(zoom_in.dx.abs() < 0.01 && zoom_in.dy.abs() < 0.01, "{zoom_in:?}");
    let zoom_out = pair(256, 144, 0.5, texture, at(-0.04));
    assert!(zoom_out.zoom < -0.05, "{zoom_out:?}");
}

#[test]
fn something_moving_on_its_own_is_residual_not_camera_motion() {
    let square = |x0: f64| move |u: f64, v: f64| if (x0..x0 + 0.16).contains(&u) && (0.4..0.75).contains(&v) { 1.0 } else { texture(u, v) };
    let m = pair(256, 144, 0.5, square(0.20), square(0.26));
    assert!(m.residual > 0.03, "{m:?}");
    assert!(m.dx.abs() < 0.01 && m.dy.abs() < 0.01, "the camera did not move: {m:?}");
    assert!(m.energy > 0.0);
}

#[test]
fn a_cut_changes_everything_and_a_small_move_does_not() {
    let other = |u: f64, v: f64| 0.5 + 0.4 * (2.0 * PI * (23.0 * u - 9.0 * v) + 0.5).cos();
    let cut = pair(256, 144, 0.5, texture, other);
    let drift = pair(256, 144, 0.5, texture, |u, v| texture(u - 0.01, v));
    assert!(cut.energy > 0.15, "{}", cut.energy);
    assert!(drift.energy < cut.energy / 2.0, "{} vs {}", drift.energy, cut.energy);
}

#[test]
fn the_numbers_do_not_depend_on_the_resolution() {
    let a = pair(256, 144, 0.5, texture, |u, v| texture(u - 0.02, v));
    let b = pair(512, 288, 0.5, texture, |u, v| texture(u - 0.02, v));
    assert!((a.dx - b.dx).abs() < 0.008, "{} vs {}", a.dx, b.dx);
}

#[test]
fn flat_pictures_have_nothing_to_match_and_say_so() {
    let m = pair(256, 144, 0.5, |_, _| 0.4, |_, _| 0.45);
    assert_eq!((m.blocks, m.dx, m.dy, m.zoom, m.residual), (0, 0.0, 0.0, 0.0, 0.0));
    assert!((m.energy - 0.05).abs() < 0.01, "{}", m.energy);
}

fn p(dx: f32, dy: f32, zoom: f32) -> MotionPair {
    MotionPair { energy: 0.02, dx, dy, zoom, residual: 0.0, blocks: 60 }
}

#[test]
fn a_run_of_pairs_adds_up_to_a_camera_move() {
    let still: Vec<MotionPair> = (0..12).map(|i| p(0.002 * (i % 2) as f32, 0.0, 0.0)).collect();
    assert_eq!(summarize(&still).camera, CameraMove::Static);
    let pan: Vec<MotionPair> = (0..12).map(|i| p(0.06 + 0.005 * (i % 3) as f32, 0.002, 0.0)).collect();
    let s = summarize(&pan);
    assert_eq!(s.camera, CameraMove::Pan);
    assert!(s.dx > 0.05);
    let tilt: Vec<MotionPair> = (0..12).map(|_| p(0.0, -0.05, 0.0)).collect();
    assert_eq!(summarize(&tilt).camera, CameraMove::Tilt);
    assert_eq!(summarize(&(0..12).map(|_| p(0.0, 0.0, 0.05)).collect::<Vec<_>>()).camera, CameraMove::ZoomIn);
    assert_eq!(summarize(&(0..12).map(|_| p(0.0, 0.0, -0.05)).collect::<Vec<_>>()).camera, CameraMove::ZoomOut);
    // shaking: it moves every time, and goes nowhere
    let shake: Vec<MotionPair> = (0..12).map(|i| p(if i % 2 == 0 { 0.08 } else { -0.08 }, if i % 3 == 0 { 0.05 } else { -0.03 }, 0.0)).collect();
    assert_eq!(summarize(&shake).camera, CameraMove::Handheld);
    // a push-in while panning is more than one thing
    let both: Vec<MotionPair> = (0..12).map(|_| p(0.06, 0.0, 0.05)).collect();
    assert_eq!(summarize(&both).camera, CameraMove::Mixed);
}

#[test]
fn a_summary_of_nothing_or_of_pictures_that_could_not_be_matched_is_unknown() {
    assert_eq!(summarize(&[]).camera, CameraMove::Unknown);
    let unmatched = MotionPair { blocks: 2, ..p(0.1, 0.0, 0.0) };
    assert_eq!(summarize(&[unmatched; 8]).camera, CameraMove::Unknown);
    // energy is still reported, and the 90th percentile finds the burst
    let mut pairs: Vec<MotionPair> = (0..9).map(|_| p(0.0, 0.0, 0.0)).collect();
    pairs.push(MotionPair { energy: 0.4, ..p(0.0, 0.0, 0.0) });
    let s = summarize(&pairs);
    assert!((s.energy - 0.058).abs() < 1e-4 && (s.energy_p90 - 0.02).abs() < 1e-6, "{s:?}");
    let burst = summarize(&pairs[5..]);
    assert!(burst.energy_p90 >= 0.4 - 1e-6, "{burst:?}");
}

#[test]
fn hostile_pictures_and_intervals_are_refused_or_survived() {
    let a = picture(64, 36, texture);
    let b = picture(32, 18, texture);
    let (fa, fb) = (Frame::new(64, 36, &a).unwrap(), Frame::new(32, 18, &b).unwrap());
    assert_eq!(analyze_motion(&fa, &fb, 0.5), Err(AnalysisError::MismatchedFrames));
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 60.1, f64::MIN_POSITIVE / 1e300] {
        let r = analyze_motion(&fa, &fa, bad);
        if bad == f64::MIN_POSITIVE / 1e300 {
            assert!(r.is_ok() || r == Err(AnalysisError::BadInterval));
        } else {
            assert_eq!(r, Err(AnalysisError::BadInterval), "{bad}");
        }
    }
    // tiny and odd sizes, and noise
    let mut seed = 1u64;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 24) as u8
    };
    for (w, h) in [(1u32, 1u32), (2, 2), (5, 3), (16, 16), (31, 200), (200, 31), (130, 75)] {
        let (x, y): (Vec<u8>, Vec<u8>) = ((0..w * h * 4).map(|_| noise()).collect(), (0..w * h * 4).map(|_| noise()).collect());
        let m = analyze_motion(&Frame::new(w, h, &x).unwrap(), &Frame::new(w, h, &y).unwrap(), 1.0).unwrap_or_else(|e| panic!("{w}x{h}: {e}"));
        assert!([m.energy, m.dx, m.dy, m.zoom, m.residual].iter().all(|v| v.is_finite()), "{w}x{h}: {m:?}");
        assert!((0.0..=1.0).contains(&m.residual));
    }
}

#[test]
fn the_same_pictures_always_give_the_same_numbers_and_they_round_trip_as_json() {
    let a = pair(256, 144, 0.5, texture, |u, v| texture(u - 0.02, v - 0.01));
    assert_eq!(a, pair(256, 144, 0.5, texture, |u, v| texture(u - 0.02, v - 0.01)));
    let back: MotionPair = serde_json::from_value(serde_json::to_value(a).unwrap()).unwrap();
    assert_eq!(a, back);
    let s = summarize(&[a; 4]);
    let back: MotionSummary = serde_json::from_value(serde_json::to_value(s).unwrap()).unwrap();
    assert_eq!(s, back);
    assert_eq!(serde_json::to_value(CameraMove::ZoomIn).unwrap(), serde_json::json!("zoomIn"));
}
