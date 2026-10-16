//! Tests of the Film Look pack ([`super::film`]).

use filmcraft_project::{EffectInstance, EffectKind, ParamKind, ParamValue, find_effect};
use filmcraft_time::Tick;

use crate::effects::{FxCtx, dec, enc};
use crate::image::Image;

const IDS: &[&str] = &["film_grain"];

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
