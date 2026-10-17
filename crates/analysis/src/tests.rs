//! Tests of the picture measures on synthetic pictures with known properties, and on hostile input.

use crate::color::{colour_temperature, lab_distance, srgb_to_lab, srgb_to_linear};
use crate::image::{MAX_DIMENSION, analyze_rgba};
use crate::{AnalysisError, FrameStats};

fn picture(w: u32, h: u32, mut f: impl FnMut(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let [r, g, b] = f(x, y);
            v.extend([r, g, b, 255]);
        }
    }
    v
}

fn measure(w: u32, h: u32, f: impl FnMut(u32, u32) -> [u8; 3]) -> FrameStats {
    analyze_rgba(w, h, &picture(w, h, f)).unwrap()
}

/// A repeatable stream of noise.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f32 / (1u64 << 53) as f32
    }

    /// Roughly normal with deviation 1 (sum of twelve uniforms).
    fn gauss(&mut self) -> f32 {
        (0..12).map(|_| self.next()).sum::<f32>() - 6.0
    }
}

#[test]
fn a_flat_grey_picture_has_no_contrast_colour_detail_or_grain() {
    let s = measure(64, 36, |_, _| [128, 128, 128]);
    assert!((s.luma_mean - 0.5).abs() < 0.01, "{}", s.luma_mean);
    assert_eq!((s.contrast, s.saturation, s.sharpness, s.grain), (0.0, 0.0, 0.0, 0.0));
    assert!(!s.letterbox.any());
    assert_eq!(s.palette.len(), 1);
    assert!((s.palette[0].share - 1.0).abs() < 1e-6);
    let cct = s.colour_temperature.unwrap();
    assert!((6200.0..6800.0).contains(&cct), "neutral grey is about 6500 K, got {cct}");
    assert!((s.frame_aspect - 64.0 / 36.0).abs() < 1e-5 && (s.active_aspect - s.frame_aspect).abs() < 1e-4);
}

#[test]
fn a_black_picture_has_no_colour_temperature_and_no_bars() {
    let s = measure(48, 27, |_, _| [0, 0, 0]);
    assert_eq!((s.luma_mean, s.colour_temperature), (0.0, None));
    assert!(!s.letterbox.any(), "a wholly dark picture is dark, not letterboxed");
    assert_eq!(s.palette.len(), 1);
}

#[test]
fn black_and_white_halves_have_full_contrast_and_a_two_colour_palette() {
    let s = measure(100, 50, |x, _| if x < 50 { [0, 0, 0] } else { [255, 255, 255] });
    assert!(s.contrast > 0.99, "{}", s.contrast);
    assert!((s.luma_mean - 0.5).abs() < 0.01);
    assert_eq!(s.palette.len(), 2);
    assert!(s.palette.iter().all(|c| (c.share - 0.5).abs() < 0.02), "{:?}", s.palette);
    let l: Vec<f32> = s.palette.iter().map(|c| c.lab[0]).collect();
    assert!(l.iter().any(|v| *v < 1.0) && l.iter().any(|v| *v > 99.0), "{l:?}");
}

#[test]
fn red_green_and_blue_stripes_give_three_equal_colours_with_known_lab_values() {
    let s = measure(90, 30, |x, _| [[255, 0, 0], [0, 255, 0], [0, 0, 255]][(x / 30) as usize]);
    assert_eq!(s.palette.len(), 3);
    assert!(s.palette.iter().all(|c| (c.share - 1.0 / 3.0).abs() < 0.02), "{:?}", s.palette);
    assert!((s.saturation - 1.0).abs() < 1e-4);
    // published sRGB primaries in L*a*b* (D65): red 53.2, 80.1, 67.2; green 87.7, -86.2, 83.2; blue 32.3, 79.2, -107.9
    for (want, name) in [([53.24, 80.09, 67.20], "red"), ([87.73, -86.18, 83.18], "green"), ([32.30, 79.19, -107.86], "blue")] {
        assert!(s.palette.iter().any(|c| lab_distance(c.lab, want) < 2.0), "no {name} in {:?}", s.palette);
    }
    assert!(lab_distance(srgb_to_lab([1.0, 0.0, 0.0]), [53.24, 80.09, 67.20]) < 0.5);
}

#[test]
fn the_palette_lists_the_biggest_colour_first_and_adds_up_to_one() {
    let s = measure(100, 100, |x, _| {
        if x < 70 {
            [200, 40, 40]
        } else if x < 90 {
            [30, 90, 200]
        } else {
            [240, 220, 60]
        }
    });
    assert_eq!(s.palette.len(), 3);
    assert!(s.palette.windows(2).all(|w| w[0].share >= w[1].share));
    assert!((s.palette[0].share - 0.7).abs() < 0.03);
    assert!((s.palette.iter().map(|c| c.share).sum::<f32>() - 1.0).abs() < 1e-4);
    assert!(s.palette[0].rgb[0] > 0.7 && s.palette[0].rgb[1] < 0.3, "the biggest colour is the red: {:?}", s.palette[0].rgb);
}

#[test]
fn a_noisy_busy_picture_never_gets_more_than_five_colours() {
    let mut n = Noise(7);
    let s = measure(200, 120, |_, _| [(n.next() * 255.0) as u8, (n.next() * 255.0) as u8, (n.next() * 255.0) as u8]);
    assert!(!s.palette.is_empty() && s.palette.len() <= 5);
    assert!((s.palette.iter().map(|c| c.share).sum::<f32>() - 1.0).abs() < 1e-3);
}

#[test]
fn warm_light_reads_low_and_cool_light_high() {
    let temp = |rgb: [u8; 3]| measure(16, 16, |_, _| rgb).colour_temperature.unwrap();
    let (warm, neutral, cool) = (temp([230, 180, 120]), temp([160, 160, 160]), temp([120, 160, 235]));
    assert!(warm < 4800.0 && warm > 2000.0, "{warm}");
    assert!((6000.0..7000.0).contains(&neutral), "{neutral}");
    assert!(cool > 8500.0, "{cool}");
    assert!(warm < neutral && neutral < cool);
    // the maths on its own: D65 white is 6504 K, and black has none
    let d65 = colour_temperature([1.0, 1.0, 1.0]).unwrap();
    assert!((d65 - 6504.0).abs() < 60.0, "{d65}");
    assert_eq!(colour_temperature([0.0, 0.0, 0.0]), None);
    assert_eq!(colour_temperature([f32::NAN, 1.0, 1.0]), None);
    assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6 && srgb_to_linear(f32::NAN) == 0.0 && srgb_to_linear(-3.0) == 0.0 && srgb_to_linear(9.0) == 1.0);
}

#[test]
fn letterbox_bars_are_found_on_each_side_and_the_active_aspect_follows() {
    // 192x108 (16:9) with 14 black rows above and below: the picture inside is 192x80, 2.4:1
    let s = measure(192, 108, |_, y| if !(14..94).contains(&y) { [0, 0, 0] } else { [120, 110, 100] });
    assert!((s.letterbox.top - 14.0 / 108.0).abs() < 1e-5 && (s.letterbox.bottom - 14.0 / 108.0).abs() < 1e-5, "{:?}", s.letterbox);
    assert_eq!((s.letterbox.left, s.letterbox.right), (0.0, 0.0));
    assert!((s.active_aspect - 2.4).abs() < 0.01, "{}", s.active_aspect);
    assert!((s.frame_aspect - 16.0 / 9.0).abs() < 1e-4);
    // pillarbox: 24 black columns each side of a 144x108 picture, 4:3
    let p = measure(192, 108, |x, _| if !(24..168).contains(&x) { [0, 0, 0] } else { [120, 110, 100] });
    assert!((p.letterbox.left - 24.0 / 192.0).abs() < 1e-5 && (p.letterbox.right - 24.0 / 192.0).abs() < 1e-5, "{:?}", p.letterbox);
    assert!((p.active_aspect - 4.0 / 3.0).abs() < 0.01, "{}", p.active_aspect);
    // a single dark edge row is not a letterbox, and a dark scene with a bright patch is not either
    let one = measure(64, 36, |_, y| if y == 0 { [0, 0, 0] } else { [120, 110, 100] });
    assert!(!one.letterbox.any());
    let dark = measure(64, 36, |x, y| if x > 28 && x < 34 && y > 14 && y < 20 { [200, 200, 200] } else { [3, 3, 3] });
    assert!(dark.letterbox.top > 0.3, "a dark frame with a small bright patch has black above it: {:?}", dark.letterbox);
}

#[test]
fn a_sharp_picture_scores_higher_than_a_blurred_one() {
    let sharp = measure(320, 180, |x, y| if (x / 8 + y / 8) % 2 == 0 { [20, 20, 20] } else { [235, 235, 235] });
    let blurred = measure(320, 180, |x, y| {
        let v = 128.0 + 100.0 * ((x as f32 / 8.0 * std::f32::consts::PI * 0.1).sin() * (y as f32 / 8.0 * std::f32::consts::PI * 0.1).sin());
        [v as u8; 3]
    });
    let flat = measure(320, 180, |_, _| [90, 90, 90]);
    assert!(sharp.sharpness > blurred.sharpness * 20.0, "{} vs {}", sharp.sharpness, blurred.sharpness);
    assert!(blurred.sharpness > flat.sharpness);
    assert_eq!(flat.sharpness, 0.0);
}

#[test]
fn grain_tracks_the_noise_that_was_added() {
    let grainy = |sigma: f32| {
        let mut n = Noise(99);
        measure(256, 256, |_, _| [(128.0 + n.gauss() * sigma * 255.0).clamp(0.0, 255.0) as u8; 3]).grain
    };
    let (clean, light, heavy) = (grainy(0.0), grainy(0.01), grainy(0.04));
    assert_eq!(clean, 0.0);
    assert!((light - 0.01).abs() < 0.004, "{light}");
    assert!((heavy - 0.04).abs() < 0.012, "{heavy}");
    assert!(clean < light && light < heavy);
    // a smooth ramp has next to no grain, and a hard edge does not read as grain
    let ramp = measure(256, 256, |x, _| [x as u8; 3]).grain;
    let edge = measure(256, 256, |x, _| if x < 128 { [30; 3] } else { [220; 3] }).grain;
    assert!(ramp < 0.003 && edge < 0.003, "{ramp} {edge}");
}

#[test]
fn the_measures_do_not_depend_on_how_many_pixels_the_picture_has() {
    let scene = |x: f32, y: f32| -> [u8; 3] {
        let v = 0.5 + 0.4 * (x * 6.0).sin() * (y * 4.0).cos();
        [(v * 255.0) as u8, (v * 200.0) as u8, (v * 150.0) as u8]
    };
    let small = measure(160, 90, |x, y| scene(x as f32 / 160.0, y as f32 / 90.0));
    let large = measure(640, 360, |x, y| scene(x as f32 / 640.0, y as f32 / 360.0));
    assert!((small.luma_mean - large.luma_mean).abs() < 0.01);
    assert!((small.saturation - large.saturation).abs() < 0.02);
    assert!((small.contrast - large.contrast).abs() < 0.03);
    let (a, b) = (small.colour_temperature.unwrap(), large.colour_temperature.unwrap());
    assert!((a - b).abs() / a < 0.03, "{a} {b}");
}

#[test]
fn the_same_picture_always_gives_the_same_numbers_and_they_round_trip_as_json() {
    let mut n = Noise(5);
    let px = picture(120, 70, |_, _| [(n.next() * 255.0) as u8, (n.next() * 120.0) as u8, (n.next() * 255.0) as u8]);
    let a = analyze_rgba(120, 70, &px).unwrap();
    assert_eq!(a, analyze_rgba(120, 70, &px).unwrap());
    let back: FrameStats = serde_json::from_value(serde_json::to_value(&a).unwrap()).unwrap();
    assert_eq!(a, back);
    assert!(serde_json::to_value(&a).unwrap().get("lumaMean").is_some(), "camelCase names");
}

#[test]
fn hostile_sizes_and_buffers_are_refused_before_anything_is_allocated() {
    assert_eq!(analyze_rgba(0, 10, &[]), Err(AnalysisError::Empty));
    assert_eq!(analyze_rgba(10, 0, &[0; 400]), Err(AnalysisError::Empty));
    assert_eq!(analyze_rgba(2, 2, &[0; 15]), Err(AnalysisError::ShortBuffer { need: 16, have: 15 }));
    assert!(matches!(analyze_rgba(u32::MAX, u32::MAX, &[0; 16]), Err(AnalysisError::TooLarge { .. })));
    assert!(matches!(analyze_rgba(MAX_DIMENSION + 1, 1, &[0; 16]), Err(AnalysisError::TooLarge { .. })));
    assert!(matches!(analyze_rgba(20_000, 20_000, &[0; 16]), Err(AnalysisError::TooLarge { .. })), "400 megapixels is over the limit");
    assert!(matches!(analyze_rgba(MAX_DIMENSION, MAX_DIMENSION, &[]), Err(AnalysisError::TooLarge { .. })));
    // bytes past the picture are ignored
    assert!(analyze_rgba(2, 2, &[7; 100]).is_ok());
}

#[test]
fn tiny_odd_and_random_pictures_give_finite_numbers() {
    let mut n = Noise(1234);
    for (w, h) in [(1, 1), (2, 2), (1, 50), (50, 1), (3, 3), (7, 5), (31, 17), (320, 3), (3, 320), (641, 359)] {
        let bytes: Vec<u8> = (0..w * h * 4).map(|_| (n.next() * 256.0) as u8).collect();
        let s = analyze_rgba(w, h, &bytes).unwrap_or_else(|e| panic!("{w}x{h}: {e}"));
        let all = [s.luma_mean, s.luma_p5, s.luma_p95, s.contrast, s.saturation, s.sharpness, s.grain, s.frame_aspect, s.active_aspect];
        assert!(all.iter().all(|v| v.is_finite() && *v >= 0.0), "{w}x{h}: {s:?}");
        assert!(s.palette.iter().all(|c| c.lab.iter().all(|v| v.is_finite()) && (0.0..=1.0).contains(&c.share)));
        assert!(s.luma_p5 <= s.luma_p95);
    }
}

#[test]
fn a_large_picture_is_measured_in_one_pass() {
    // 3840x2160 of a gradient: the work is one read of every pixel plus fixed-size work
    let w = 3840usize;
    let h = 2160usize;
    let mut px = vec![255u8; w * h * 4];
    for (i, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = (i % w) as f32 / w as f32;
        p[0] = (x * 255.0) as u8;
        p[1] = 120;
        p[2] = (255.0 - x * 255.0) as u8;
    }
    let s = analyze_rgba(w as u32, h as u32, &px).unwrap();
    assert!(s.saturation > 0.3 && s.palette.len() >= 2);
}
