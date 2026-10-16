//! Tests of the Film Look pack ([`super::film`]).

use filmcraft_project::{EffectInstance, EffectKind, ParamKind, ParamValue, find_effect};
use filmcraft_time::Tick;

use crate::effects::{FxCtx, dec, enc};
use crate::image::Image;

const IDS: &[&str] = &["film_grain", "halation", "diffusion", "film_gate", "anamorphic_streak", "lens_fringing"];

fn cx_at(t: i64) -> FxCtx<'static> {
    FxCtx {
        t: Tick(t),
        px_scale: 1.0,
        seconds: 0.0,
        timecode: "01:00:00:00",
        clip_name: "clip",
        project: None,
        env: None,
        working: filmcraft_color::WorkingSpace::default(),
    }
}

fn inst(id: &str) -> EffectInstance {
    find_effect(id).unwrap_or_else(|| panic!("no effect {id}")).instance()
}

fn setf(e: &mut EffectInstance, k: &str, v: f64) {
    e.params.get_mut(k).unwrap_or_else(|| panic!("{}: no param {k}", e.effect)).value = ParamValue::Float(v);
}

fn run(e: &EffectInstance, img: &Image, cx: &FxCtx) -> Image {
    let mut o = img.clone();
    crate::effects::apply(&mut o, e, cx);
    o
}

/// A flat picture at display brightness `v` (grey), opaque.
fn flat(w: usize, h: usize, v: f32) -> Image {
    let l = dec([v; 3]);
    Image::filled(w, h, [l[0], l[1], l[2], 1.0])
}

/// A textured picture: gradients and a little detail, opaque.
fn picture(w: usize, h: usize) -> Image {
    let mut img = Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let c = dec([0.15 + 0.7 * u, 0.25 + 0.5 * v, 0.4 + 0.4 * (u - v).abs()]);
            img.px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[c[0], c[1], c[2], 1.0]);
        }
    }
    img
}

fn display_luma(img: &Image) -> Vec<f32> {
    img.px.as_chunks::<4>().0.iter().map(|p| enc([p[0], p[1], p[2]])).map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]).collect()
}

fn mean(x: &[f32]) -> f64 {
    x.iter().map(|v| f64::from(*v)).sum::<f64>() / x.len().max(1) as f64
}

fn std(x: &[f32]) -> f64 {
    let m = mean(x);
    (x.iter().map(|v| (f64::from(*v) - m).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

/// Energy of the pixel-to-pixel changes (a measure of how fine the texture is).
fn fine_energy(x: &[f32], w: usize) -> f64 {
    let mut s = 0.0f64;
    let mut n = 0.0f64;
    for row in x.chunks(w) {
        for p in row.windows(2) {
            s += f64::from(p[1] - p[0]).powi(2);
            n += 1.0;
        }
    }
    (s / n.max(1.0)).sqrt()
}

#[test]
fn every_effect_of_the_pack_is_defined_as_video_in_the_film_look_folder_with_sane_defaults() {
    for id in IDS {
        let d = find_effect(id).unwrap_or_else(|| panic!("{id}"));
        assert_eq!(d.kind, EffectKind::Video, "{id}");
        assert_eq!(d.category, ["Video Effects", "Film Look"], "{id}");
        for p in &d.params {
            if let ParamKind::Float { min, max, soft_min, soft_max, .. } = p.kind {
                let v = p.default.as_f64().unwrap();
                assert!(v >= min && v <= max, "{id}.{} default {v} outside {min}..{max}", p.id);
                assert!(soft_min >= min && soft_max <= max, "{id}.{}", p.id);
            }
        }
    }
}

#[test]
fn every_effect_is_finite_deterministic_and_keeps_the_size_at_its_extremes() {
    let img = picture(40, 28);
    for id in IDS {
        let d = find_effect(id).unwrap();
        let mut extremes = vec![inst(id)];
        for pick in [|min: f64, _max: f64| min, |_min: f64, max: f64| max] {
            let mut e = inst(id);
            for p in &d.params {
                if let ParamKind::Float { min, max, .. } = p.kind {
                    setf(&mut e, p.id, pick(min, max));
                }
            }
            extremes.push(e);
        }
        for e in &extremes {
            for t in [0, 254_016_000_000] {
                let a = run(e, &img, &cx_at(t));
                let b = run(e, &img, &cx_at(t));
                assert_eq!(a.px, b.px, "{id} not deterministic");
                assert_eq!((a.w, a.h), (img.w, img.h), "{id} changed size");
                for p in a.px.as_chunks::<4>().0 {
                    assert!(p.iter().all(|v| v.is_finite()), "{id}: {p:?}");
                    assert!((-1e-4..=1.0 + 1e-3).contains(&p[3]), "{id}: alpha {}", p[3]);
                    assert!(p[..3].iter().all(|v| *v >= -1e-3), "{id}: negative {p:?}");
                }
            }
        }
    }
}

#[test]
fn every_choice_value_runs_and_tiny_or_empty_images_are_left_alone() {
    for id in IDS {
        let d = find_effect(id).unwrap();
        for p in &d.params {
            if let ParamKind::Choice(opts) = p.kind {
                for i in 0..opts.len() {
                    let mut e = inst(id);
                    e.params.get_mut(p.id).unwrap().value = ParamValue::Choice(i as u32);
                    let o = run(&e, &picture(20, 12), &cx_at(0));
                    assert!(o.px.iter().all(|v| v.is_finite()), "{id}.{}={i}", p.id);
                }
            }
        }
        for (w, h) in [(0, 0), (1, 1), (1, 9), (9, 1), (2, 2)] {
            let e = inst(id);
            let o = run(&e, &Image::filled(w, h, [0.4, 0.4, 0.4, 1.0]), &cx_at(0));
            assert_eq!((o.w, o.h), (w, h));
            assert!(o.px.iter().all(|v| v.is_finite()));
        }
    }
}

// ---------------------------------------------------------------------------- film grain

fn grain(amount: f64) -> EffectInstance {
    let mut e = inst("film_grain");
    setf(&mut e, "amount", amount);
    e
}

#[test]
fn grain_at_zero_is_the_identity_and_keeps_the_average_brightness_otherwise() {
    let img = flat(96, 64, 0.5);
    assert_eq!(run(&grain(0.0), &img, &cx_at(0)).px, img.px);
    let o = run(&grain(60.0), &img, &cx_at(0));
    assert!(o.px != img.px);
    let (a, b) = (mean(&display_luma(&img)), mean(&display_luma(&o)));
    assert!((a - b).abs() < 0.01, "mean {a} vs {b}");
    // the grain is real texture: a flat picture is no longer flat
    assert!(std(&display_luma(&o)) > 0.02, "{}", std(&display_luma(&o)));
}

#[test]
fn more_amount_is_stronger_grain_and_it_never_touches_alpha() {
    let img = flat(96, 64, 0.5);
    let weak = std(&display_luma(&run(&grain(20.0), &img, &cx_at(0))));
    let strong = std(&display_luma(&run(&grain(80.0), &img, &cx_at(0))));
    assert!(strong > weak * 2.5, "{weak} vs {strong}");
    let mut half = img.clone();
    half.px.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 0.0);
    let o = run(&grain(80.0), &half, &cx_at(0));
    assert_eq!(o.px, half.px, "transparent pixels are left alone");
}

#[test]
fn bigger_grain_is_coarser() {
    let img = flat(128, 96, 0.5);
    let energy = |size: f64| {
        let mut e = grain(60.0);
        setf(&mut e, "size", size);
        setf(&mut e, "softness", 0.0);
        fine_energy(&display_luma(&run(&e, &img, &cx_at(0))), 128)
    };
    let (fine, coarse) = (energy(0.8), energy(5.0));
    assert!(coarse < fine * 0.6, "fine {fine}, coarse {coarse}");
    // the strength stays the same whatever the size
    let spread = |size: f64| {
        let mut e = grain(60.0);
        setf(&mut e, "size", size);
        std(&display_luma(&run(&e, &img, &cx_at(0))))
    };
    let (a, b) = (spread(0.8), spread(5.0));
    assert!((a / b - 1.0).abs() < 0.25, "{a} vs {b}");
}

#[test]
fn grain_is_strongest_in_the_midtones_and_follows_the_response_sliders() {
    let spread = |v: f32, e: &EffectInstance| std(&display_luma(&run(e, &flat(96, 64, v), &cx_at(0))));
    let e = grain(70.0);
    let (dark, mid, bright) = (spread(0.05, &e), spread(0.5, &e), spread(0.97, &e));
    assert!(mid > dark * 1.15 && mid > bright * 1.4, "{dark} {mid} {bright}");
    let mut none = grain(70.0);
    setf(&mut none, "shadows", 0.0);
    setf(&mut none, "highlights", 0.0);
    assert!(spread(0.97, &none) < 0.002 && spread(0.05, &none) < 0.002, "no grain where the response is zero");
    assert!(spread(0.5, &none) > 0.02);
}

#[test]
fn colour_grain_varies_between_channels_and_mono_grain_does_not() {
    let img = flat(96, 64, 0.5);
    let channels = |color: f64| {
        let mut e = grain(70.0);
        setf(&mut e, "color", color);
        let o = run(&e, &img, &cx_at(0));
        let (mut a, mut b) = (Vec::new(), Vec::new());
        for p in o.px.as_chunks::<4>().0 {
            let c = enc([p[0], p[1], p[2]]);
            a.push(c[0] - c[1]);
            b.push(c[1] - c[2]);
        }
        std(&a).max(std(&b))
    };
    assert!(channels(0.0) < 1e-4, "mono: the channels move together");
    assert!(channels(100.0) > 0.02, "colour: they do not");
}

#[test]
fn the_pattern_changes_every_frame_and_with_the_seed_but_repeats_for_the_same_ones() {
    let img = flat(64, 48, 0.5);
    let e = grain(50.0);
    let f0 = run(&e, &img, &cx_at(0));
    assert_eq!(f0.px, run(&e, &img, &cx_at(0)).px, "same frame, same grain");
    assert!(f0.px != run(&e, &img, &cx_at(10_584_000_000)).px, "next frame, new grain");
    let mut other = grain(50.0);
    setf(&mut other, "seed", 7.0);
    assert!(f0.px != run(&other, &img, &cx_at(0)).px, "other seed, other grain");
    // consecutive frames are unrelated (no frozen pattern): their grain is uncorrelated
    let (a, b) = (display_luma(&f0), display_luma(&run(&e, &img, &cx_at(10_584_000_000))));
    let (ma, mb) = (mean(&a), mean(&b));
    let cov = a.iter().zip(&b).map(|(x, y)| (f64::from(*x) - ma) * (f64::from(*y) - mb)).sum::<f64>() / a.len() as f64;
    assert!((cov / (std(&a) * std(&b))).abs() < 0.1, "correlation {}", cov / (std(&a) * std(&b)));
}

#[test]
fn a_stock_overrides_the_controls_and_heavier_stocks_are_coarser_and_stronger() {
    let img = flat(128, 96, 0.5);
    let with = |stock: u32| {
        let mut e = grain(60.0);
        e.params.get_mut("stock").unwrap().value = ParamValue::Choice(stock);
        // controls that a stock must ignore
        setf(&mut e, "size", 12.0);
        run(&e, &img, &cx_at(0))
    };
    let (fine, heavy) = (display_luma(&with(1)), display_luma(&with(3)));
    assert!(fine_energy(&heavy, 128) < fine_energy(&fine, 128), "heavy grain is coarser");
    assert!(std(&heavy) >= std(&fine) * 0.95, "and at least as strong: {} vs {}", std(&fine), std(&heavy));
    assert!(with(1).px != with(0).px, "a stock is not the same as Custom");
}

#[test]
fn half_resolution_grain_looks_the_same_size_as_full_resolution_grain() {
    let e = grain(60.0);
    let full = run(&e, &flat(192, 128, 0.5), &cx_at(0));
    let mut half_cx = cx_at(0);
    half_cx.px_scale = 0.5;
    let half = run(&e, &flat(96, 64, 0.5), &half_cx);
    // grain is about 1.6 px at full size, so about 0.8 px at half: compare in grain-sized units
    let (f, h) = (fine_energy(&display_luma(&full), 192), fine_energy(&display_luma(&half), 96));
    assert!(h > f * 0.6 && h < f * 2.5, "full {f}, half {h}");
    assert!((std(&display_luma(&full)) / std(&display_luma(&half)) - 1.0).abs() < 0.3);
}

#[test]
fn hostile_parameter_values_cannot_crash_or_blow_up_grain() {
    let img = picture(24, 16);
    for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -5.0, 1e30, 1e-30] {
        let mut e = inst("film_grain");
        for k in ["amount", "size", "softness", "color", "shadows", "midtones", "highlights", "seed"] {
            setf(&mut e, k, v);
        }
        let o = run(&e, &img, &cx_at(i64::MAX));
        assert!(o.px.iter().all(|x| x.is_finite() && *x >= -1e-3), "{v}");
        let o = run(&e, &img, &cx_at(i64::MIN));
        assert!(o.px.iter().all(|x| x.is_finite()), "{v}");
    }
}

/// `cargo test -p filmcraft-render --lib film_tests::speed -- --ignored --nocapture`
#[test]
#[ignore = "timing, not a check"]
fn speed() {
    let img = picture(1920, 1080);
    for id in IDS {
        let e = inst(id);
        let mut best = f64::MAX;
        for t in 0..3 {
            let t0 = std::time::Instant::now();
            let _ = run(&e, &img, &cx_at(t));
            best = best.min(t0.elapsed().as_secs_f64() * 1000.0);
        }
        println!("{id}: {best:.0} ms per 1080p frame");
    }
}

// ---------------------------------------------------------------------- halation and diffusion

/// A dark picture with one bright square in the middle (display grey `v` outside, white inside).
fn spot(w: usize, h: usize, outside: f32, half: usize) -> Image {
    let mut img = flat(w, h, outside);
    for y in h / 2 - half..h / 2 + half {
        for x in w / 2 - half..w / 2 + half {
            img.px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[1.0, 1.0, 1.0, 1.0]);
        }
    }
    img
}

fn px(img: &Image, x: usize, y: usize) -> [f32; 4] {
    let i = (y * img.w + x) * 4;
    [img.px[i], img.px[i + 1], img.px[i + 2], img.px[i + 3]]
}

fn halo(strength: f64) -> EffectInstance {
    let mut e = inst("halation");
    setf(&mut e, "strength", strength);
    e
}

#[test]
fn halation_at_zero_is_the_identity_and_only_highlights_make_a_glow() {
    let img = spot(128, 96, 0.02, 4);
    assert_eq!(run(&halo(0.0), &img, &cx_at(0)).px, img.px);
    let o = run(&halo(80.0), &img, &cx_at(0));
    // the bright square itself stays at least as bright, and the glow reaches outside it
    let (inside, near) = (px(&o, 64, 48), px(&o, 64 + 4 + 6, 48));
    assert!(inside[0] >= px(&img, 64, 48)[0] - 1e-5);
    let before = px(&img, 64 + 4 + 6, 48);
    assert!(near[0] > before[0] + 0.01, "glow next to the square: {near:?} vs {before:?}");
    // and it is red-orange: red exceeds green exceeds blue in the added light
    let added = [near[0] - before[0], near[1] - before[1], near[2] - before[2]];
    assert!(added[0] > added[1] * 1.5 && added[1] > added[2] * 1.2, "{added:?}");
    // far from any highlight (many radii away) nothing changes
    let mut small = halo(80.0);
    setf(&mut small, "radius", 8.0);
    let o = run(&small, &img, &cx_at(0));
    let far = px(&o, 3, 3);
    assert!((far[0] - px(&img, 3, 3)[0]).abs() < 1e-4, "{far:?}");
}

#[test]
fn a_picture_with_no_highlights_gets_no_halation_and_a_white_tint_makes_neutral_glow() {
    let dim = flat(64, 48, 0.3);
    assert_eq!(run(&halo(100.0), &dim, &cx_at(0)).px, dim.px);
    let mut e = halo(80.0);
    e.params.get_mut("tint").unwrap().value = ParamValue::Color([1.0, 1.0, 1.0, 1.0]);
    let o = run(&e, &spot(128, 96, 0.02, 4), &cx_at(0));
    let near = px(&o, 64 + 10, 48);
    assert!((near[0] - near[1]).abs() < 0.01 && (near[1] - near[2]).abs() < 0.01, "{near:?}");
}

#[test]
fn halation_gets_wider_with_the_radius_and_keeps_its_size_at_half_resolution() {
    let reach = |radius: f64, scale: f32, w: usize, h: usize, half: usize| {
        let mut e = halo(90.0);
        setf(&mut e, "radius", radius);
        let mut c = cx_at(0);
        c.px_scale = scale;
        let img = spot(w, h, 0.0, half);
        let o = run(&e, &img, &c);
        // how far from the square's edge the glow still adds 1% (in working pixels)
        (1..w / 2 - half).take_while(|d| px(&o, w / 2 + half + d, h / 2)[0] > 0.01).count() as f32
    };
    let (small, large) = (reach(10.0, 1.0, 256, 128, 6), reach(60.0, 1.0, 256, 128, 6));
    assert!(large > small * 2.0, "{small} vs {large}");
    // the same picture at half size (and half-size working scale) glows to about half the distance
    let (full, half) = (reach(40.0, 1.0, 256, 128, 8), reach(40.0, 0.5, 128, 64, 4));
    assert!(half > full * 0.35 && half < full * 0.7, "full {full}, half {half}");
}

fn diff(strength: f64, lift: f64) -> EffectInstance {
    let mut e = inst("diffusion");
    setf(&mut e, "strength", strength);
    setf(&mut e, "black_lift", lift);
    e
}

#[test]
fn diffusion_at_zero_is_the_identity_and_blooms_bright_parts_into_dark_ones() {
    let img = spot(128, 96, 0.02, 4);
    assert_eq!(run(&diff(0.0, 0.0), &img, &cx_at(0)).px, img.px);
    let mut e = diff(70.0, 0.0);
    setf(&mut e, "highlight_bias", 100.0);
    let o = run(&e, &img, &cx_at(0));
    let near = px(&o, 64 + 4 + 5, 48)[0];
    assert!(near > px(&img, 64 + 4 + 5, 48)[0] + 0.01, "bloom spills out of the highlight: {near}");
    // the bloom adds light: the picture does not get darker anywhere
    assert!(o.px.iter().zip(&img.px).all(|(a, b)| *a >= *b - 1e-4));
}

#[test]
fn diffusion_with_no_bloom_softens_edges_without_adding_light() {
    let img = spot(128, 96, 0.02, 12);
    let mut e = diff(80.0, 0.0);
    setf(&mut e, "highlight_bias", 0.0);
    let o = run(&e, &img, &cx_at(0));
    let edge = |i: &Image| (px(i, 64 + 12, 48)[0] - px(i, 64 + 11, 48)[0]).abs() + (px(i, 64 + 13, 48)[0] - px(i, 64 + 12, 48)[0]).abs();
    assert!(edge(&o) < edge(&img) * 0.8, "softer edge");
    // the total light is about the same (a blur moves light, it does not make it)
    let total = |i: &Image| i.px.iter().step_by(4).map(|v| f64::from(*v)).sum::<f64>();
    assert!((total(&o) / total(&img) - 1.0).abs() < 0.02);
}

#[test]
fn black_lift_raises_only_the_blacks() {
    let lifted = |v: f32| display_luma(&run(&diff(0.0, 80.0), &flat(8, 8, v), &cx_at(0)))[0];
    assert!(lifted(0.0) > 0.05, "black becomes a faint haze: {}", lifted(0.0));
    assert!(lifted(0.0) > lifted(0.2) - 0.2 + 0.0 && (lifted(1.0) - 1.0).abs() < 1e-4, "white stays white: {}", lifted(1.0));
    assert!(lifted(0.5) - 0.5 < (lifted(0.0) - 0.0) * 0.45, "mid greys move far less than black");
    let mut prev = -1.0;
    for v in [0.0, 0.1, 0.3, 0.6, 0.9, 1.0] {
        let o = lifted(v);
        assert!(o >= v - 1e-4, "never darker: {v} -> {o}");
        assert!(o > prev, "still monotonic");
        prev = o;
    }
}

#[test]
fn hostile_values_cannot_break_halation_or_diffusion() {
    let img = spot(40, 30, 0.1, 5);
    for id in ["halation", "diffusion"] {
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -5.0, 1e30, 1e-30] {
            let mut e = inst(id);
            for p in find_effect(id).unwrap().params.iter() {
                if matches!(p.kind, ParamKind::Float { .. }) {
                    setf(&mut e, p.id, v);
                }
            }
            for scale in [1.0, 0.0, f32::NAN, 1e9] {
                let mut c = cx_at(0);
                c.px_scale = scale;
                let o = run(&e, &img, &c);
                assert!(o.px.iter().all(|x| x.is_finite() && *x >= -1e-3), "{id} {v} {scale}");
            }
        }
    }
}

// ---------------------------------------------------------------- film gate, streak, fringing

fn secs(s: f64) -> i64 {
    (s * filmcraft_time::TICKS_PER_SECOND as f64) as i64
}

fn gate(weave: f64, flicker: f64, dust: f64, scratches: f64) -> EffectInstance {
    let mut e = inst("film_gate");
    setf(&mut e, "weave", weave);
    setf(&mut e, "flicker", flicker);
    setf(&mut e, "dust", dust);
    setf(&mut e, "scratches", scratches);
    e
}

#[test]
fn a_gate_with_everything_at_zero_is_the_identity() {
    let img = picture(64, 48);
    assert_eq!(run(&gate(0.0, 0.0, 0.0, 0.0), &img, &cx_at(secs(1.3))).px, img.px);
}

#[test]
fn weave_moves_the_picture_a_little_and_never_further_than_set_and_varies_over_time() {
    // a vertical stripe: where it lands shows the horizontal shift
    let mut img = flat(200, 40, 0.1);
    for y in 0..40 {
        for x in 98..102 {
            img.px[(y * 200 + x) * 4..(y * 200 + x) * 4 + 4].copy_from_slice(&[1.0, 1.0, 1.0, 1.0]);
        }
    }
    let centre = |o: &Image| {
        let row: Vec<f32> = (0..200).map(|x| px(o, x, 20)[0]).collect();
        let total: f32 = row.iter().sum();
        let sum_x: f32 = row.iter().enumerate().map(|(x, v)| x as f32 * v).sum();
        sum_x / total
    };
    let base = centre(&img);
    let weave = gate(6.0, 0.0, 0.0, 0.0);
    let mut seen = Vec::new();
    for k in 0..40 {
        let o = run(&weave, &img, &cx_at(secs(k as f64 * 0.37)));
        let d = centre(&o) - base;
        assert!(d.abs() <= 6.0 + 0.6, "moved {d} pixels, more than the setting");
        seen.push(d);
    }
    let (lo, hi) = (seen.iter().cloned().fold(f32::MAX, f32::min), seen.iter().cloned().fold(f32::MIN, f32::max));
    assert!(hi - lo > 1.5, "it wanders: {lo} to {hi}");
    // smooth: neighbouring frames are close
    let a = run(&weave, &img, &cx_at(secs(2.0)));
    let b = run(&weave, &img, &cx_at(secs(2.0) + secs(1.0 / 24.0)));
    assert!((centre(&a) - centre(&b)).abs() < 1.5);
    // the picture's edges are not exposed as black borders
    let o = run(&weave, &img, &cx_at(secs(3.1)));
    assert!(px(&o, 0, 0)[0] > 0.005 && px(&o, 199, 39)[0] > 0.005, "the background (linear 0.01) reaches the corners");
}

#[test]
fn flicker_changes_the_exposure_from_frame_to_frame_within_its_range() {
    let img = flat(32, 24, 0.4);
    let level = |t: f64, flicker: f64| mean(&display_luma(&run(&gate(0.0, flicker, 0.0, 0.0), &img, &cx_at(secs(t)))));
    let base = level(0.0, 0.0);
    let samples: Vec<f64> = (0..60).map(|k| level(k as f64 / 24.0, 100.0)).collect();
    let (lo, hi) = (samples.iter().cloned().fold(f64::MAX, f64::min), samples.iter().cloned().fold(f64::MIN, f64::max));
    assert!(hi - lo > 0.05, "it flickers: {lo} to {hi}");
    // half a stop either way at most: a factor of about 1.41 in linear light
    let lin = |v: f64| dec([v as f32; 3])[0] as f64;
    assert!(lin(hi) / lin(base) < 1.45 && lin(lo) / lin(base) > 1.0 / 1.45, "{lo} {hi} {base}");
    assert!(level(0.5, 20.0) != level(0.5 + 1.0 / 24.0, 20.0));
}

#[test]
fn dust_and_scratches_add_a_few_marks_that_change_between_frames() {
    let img = flat(240, 160, 0.4);
    let marks = |o: &Image| o.px.as_chunks::<4>().0.iter().zip(img.px.as_chunks::<4>().0).filter(|(a, b)| (a[0] - b[0]).abs() > 0.01).count();
    let dusty = run(&gate(0.0, 0.0, 80.0, 0.0), &img, &cx_at(secs(1.0)));
    let n = marks(&dusty);
    assert!(n > 10 && n < 240 * 160 / 10, "a few specks, not a haze: {n}");
    assert!(dusty.px != run(&gate(0.0, 0.0, 80.0, 0.0), &img, &cx_at(secs(1.0) + secs(1.0 / 24.0))).px, "dust is different on the next frame");
    // frames 21 and 22 share one scratch pattern (it changes every five frames)
    let f21 = secs(21.0 / 24.0);
    let scratched = run(&gate(0.0, 0.0, 0.0, 100.0), &img, &cx_at(f21));
    // scratches are vertical lines: some column differs along many rows
    let by_col: Vec<usize> = (0..240).map(|x| (0..160).filter(|&y| (px(&scratched, x, y)[0] - px(&img, x, y)[0]).abs() > 0.01).count()).collect();
    assert!(by_col.iter().any(|&c| c > 40), "a long thin line: {:?}", by_col.iter().max());
    assert!(by_col.iter().filter(|&&c| c > 0).count() <= 12, "only a few columns");
    // a scratch stays for a few frames
    let next = run(&gate(0.0, 0.0, 0.0, 100.0), &img, &cx_at(f21 + secs(1.0 / 24.0)));
    assert_eq!(scratched.px, next.px);
}

fn streak(strength: f64) -> EffectInstance {
    let mut e = inst("anamorphic_streak");
    setf(&mut e, "strength", strength);
    e
}

#[test]
fn a_streak_runs_sideways_from_a_highlight_and_not_up_and_down() {
    let img = spot(256, 160, 0.02, 3);
    assert_eq!(run(&streak(0.0), &img, &cx_at(0)).px, img.px);
    let o = run(&streak(80.0), &img, &cx_at(0));
    let (side, above) = (px(&o, 128 + 3 + 40, 80)[0], px(&o, 128, 80 - 3 - 40)[0]);
    let base = px(&img, 128, 5)[0];
    assert!(side > base + 0.008, "light along the row: {side}");
    assert!(above < base + 0.003, "none above the light: {above} vs {base}");
    // blue-ish by default
    let p = px(&o, 128 + 3 + 40, 80);
    assert!(p[2] > p[0] * 1.3, "blue tint: {p:?}");
    // a picture with no highlights gets no streaks
    let dim = flat(64, 48, 0.3);
    assert_eq!(run(&streak(100.0), &dim, &cx_at(0)).px, dim.px);
}

#[test]
fn a_longer_streak_reaches_further() {
    let reach = |len: f64| {
        let mut e = streak(90.0);
        setf(&mut e, "length", len);
        let o = run(&e, &spot(400, 100, 0.0, 3), &cx_at(0));
        (1..190).take_while(|d| px(&o, 200 + 3 + d, 50)[0] > 0.003).count()
    };
    assert!(reach(300.0) as f32 > reach(60.0) as f32 * 1.6, "{} vs {}", reach(60.0), reach(300.0));
}

fn fringe(amount: f64) -> EffectInstance {
    let mut e = inst("lens_fringing");
    setf(&mut e, "amount", amount);
    e
}

#[test]
fn fringing_is_zero_at_the_centre_and_grows_toward_the_corners() {
    // a white vertical bar on black, at the centre and near the right edge
    let make = |x0: usize| {
        let mut img = flat(240, 120, 0.0);
        for y in 0..120 {
            for x in x0..x0 + 6 {
                img.px[(y * 240 + x) * 4..(y * 240 + x) * 4 + 4].copy_from_slice(&[1.0, 1.0, 1.0, 1.0]);
            }
        }
        img
    };
    assert_eq!(run(&fringe(0.0), &make(117), &cx_at(0)).px, make(117).px);
    let spread = |x0: usize| {
        let o = run(&fringe(8.0), &make(x0), &cx_at(0));
        // how far apart red's and blue's centres of mass are along the middle row
        let com = |c: usize| {
            let total: f32 = (0..240).map(|x| px(&o, x, 60)[c]).sum();
            (0..240).map(|x| x as f32 * px(&o, x, 60)[c]).sum::<f32>() / total
        };
        (com(0) - com(2)).abs()
    };
    let (centre, edge) = (spread(117), spread(228));
    assert!(centre < 0.2, "no fringe at the centre: {centre}");
    assert!(edge > 1.0, "a fringe near the edge: {edge}");
    // red is pushed outward: at the right edge red lies to the right of blue
    let o = run(&fringe(8.0), &make(228), &cx_at(0));
    let com = |c: usize| {
        let total: f32 = (0..240).map(|x| px(&o, x, 60)[c]).sum();
        (0..240).map(|x| x as f32 * px(&o, x, 60)[c]).sum::<f32>() / total
    };
    assert!(com(0) > com(2), "red out, blue in");
    // green and alpha are never moved
    let g_in = (0..240).map(|x| px(&make(228), x, 60)[1]).collect::<Vec<_>>();
    let g_out = (0..240).map(|x| px(&o, x, 60)[1]).collect::<Vec<_>>();
    assert_eq!(g_in, g_out);
}

#[test]
fn hostile_values_cannot_break_gate_streak_or_fringing() {
    let img = spot(48, 32, 0.1, 5);
    for id in ["film_gate", "anamorphic_streak", "lens_fringing"] {
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -5.0, 1e30, 1e-30] {
            let mut e = inst(id);
            for p in find_effect(id).unwrap().params.iter() {
                if matches!(p.kind, ParamKind::Float { .. }) {
                    setf(&mut e, p.id, v);
                }
            }
            for (scale, t) in [(1.0, 0), (0.0, secs(3.0)), (f32::NAN, i64::MAX), (1e9, i64::MIN)] {
                let mut c = cx_at(t);
                c.px_scale = scale;
                let o = run(&e, &img, &c);
                assert!(o.px.iter().all(|x| x.is_finite() && *x >= -1e-3), "{id} {v} {scale}");
            }
        }
    }
}
