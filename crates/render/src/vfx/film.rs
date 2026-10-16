//! The Film Look pack (fork roadmap F07): grain, halation, diffusion, film gate, anamorphic streak,
//! lens fringing and the one-slider Cinema Look. Written from public descriptions of how film
//! behaves; no code from other implementations. Definitions: `filmcraft_project::effect::film`.
//!
//! Everything is deterministic: random patterns are seeded by the exact frame time and the
//! effect's own seed, so a preview and an export of the same frame match. Pixel sizes are in
//! full-resolution clip pixels and are scaled by `FxCtx::px_scale`.

use filmcraft_project::EffectInstance;
use filmcraft_project::effect::{GRAIN_STOCK_VALUES, GRAIN_STOCKS};
use rayon::prelude::*;

use super::*;
use crate::effects::{dec, enc, gaussian, hash3, noise_frame_seed};

/// The ids this module implements.
pub(super) fn handles(id: &str) -> bool {
    matches!(id, "film_grain" | "halation" | "diffusion" | "film_gate" | "anamorphic_streak" | "lens_fringing")
}

pub(super) fn apply(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    match e.effect.as_str() {
        "film_grain" => film_grain(img, e, cx),
        "halation" => halation(img, e, cx),
        "diffusion" => diffusion(img, e, cx),
        "film_gate" => film_gate(img, e, cx),
        "anamorphic_streak" => anamorphic_streak(img, e, cx),
        "lens_fringing" => lens_fringing(img, e, cx),
        _ => {}
    }
}

// ------------------------------------------------------------------------------ film grain

/// Strength of the grain at display brightness `l` (0 to 1): the response sliders (0 to 1 each)
/// for the shadows (up to 0.4), midtones and highlights (from 0.6), blended without steps.
fn response(l: f32, shadows: f32, mids: f32, highs: f32) -> f32 {
    // pure shadows below 0.1, pure highlights above 0.9, a smooth blend in between
    let ws = ((0.4 - l) / 0.3).clamp(0.0, 1.0);
    let wh = ((l - 0.6) / 0.3).clamp(0.0, 1.0);
    let wm = (1.0 - ws - wh).max(0.0);
    ws * shadows + wm * mids + wh * highs
}

/// A uniform random number in [-0.5, 0.5) summed twice (a triangular distribution: closer to the
/// bell shape of real grain than a flat one).
#[inline]
fn tri(x: usize, y: usize, seed: u64) -> f32 {
    hash3(x, y, seed) + hash3(x, y, seed ^ 0xA5A5_5A5A_1234_5678) - 1.0
}

/// Four planes of seeded noise (luminance, red, green, blue) softened to the grain size and scaled
/// to unit standard deviation, so the amount control means the same at any size.
fn grain_planes(w: usize, h: usize, sigma: f32, softness: f32, seed: u64) -> Image {
    let mut n = Image::new(w, h);
    n.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            for c in 0..4 {
                row[x * 4 + c] = tri(x, y, seed.wrapping_add(c as u64 * 0x9E37_79B9));
            }
        }
    });
    let mut fine = n.clone();
    gaussian(&mut fine, sigma, sigma, true);
    if softness > 0.0 {
        // a second, wider layer mixed in: softer, cloudier grain
        let mut wide = n;
        gaussian(&mut wide, sigma * 2.2, sigma * 2.2, true);
        for (a, b) in fine.px.iter_mut().zip(&wide.px) {
            *a = *a * (1.0 - softness) + *b * softness * 2.0;
        }
    }
    // unit standard deviation per plane
    let count = (w * h).max(1) as f64;
    let mut mean = [0f64; 4];
    let mut sq = [0f64; 4];
    for p in fine.px.as_chunks::<4>().0 {
        for c in 0..4 {
            mean[c] += f64::from(p[c]);
            sq[c] += f64::from(p[c]) * f64::from(p[c]);
        }
    }
    let mut scale = [1f32; 4];
    for c in 0..4 {
        let m = mean[c] / count;
        let var = (sq[c] / count - m * m).max(1e-12);
        scale[c] = (1.0 / var.sqrt()) as f32;
        mean[c] = m;
    }
    fine.px.par_chunks_mut(4).for_each(|p| {
        for c in 0..4 {
            p[c] = (p[c] - mean[c] as f32) * scale[c];
        }
    });
    fine
}

/// `v` when it is a usable number, else `fallback` (a parameter read from a damaged project or a
/// careless command can be NaN or infinite).
fn num(v: f32, fallback: f32) -> f32 {
    if v.is_finite() { v } else { fallback }
}

/// A percentage control as a fraction in 0 to 1.
fn frac(v: f32) -> f32 {
    (num(v, 0.0) / 100.0).clamp(0.0, 1.0)
}

pub fn film_grain(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let amount = frac(fv(e, "amount", cx));
    if amount <= 0.0 || img.w == 0 || img.h == 0 {
        return;
    }
    // a stock sets size, softness and the brightness response; Custom uses the controls
    let stock = chv(e, "stock").min(GRAIN_STOCKS.len() as u32 - 1) as usize;
    let (size, softness, shadows, mids, highs) = match stock.checked_sub(1).and_then(|i| GRAIN_STOCK_VALUES.get(i)) {
        Some(&(a, b, c, d, f)) => (a as f32, b as f32, c as f32, d as f32, f as f32),
        None => (fv(e, "size", cx), fv(e, "softness", cx), fv(e, "shadows", cx), fv(e, "midtones", cx), fv(e, "highlights", cx)),
    };
    let size = num(size, 1.6).clamp(0.5, 12.0);
    let px_scale = num(cx.px_scale, 1.0).max(0.05);
    let sigma = (size * px_scale * 0.5).max(0.01);
    let softness = frac(softness);
    let color = frac(fv(e, "color", cx));
    let (shadows, mids, highs) = (frac(shadows), frac(mids), frac(highs));
    let seed = noise_frame_seed(cx) ^ (num(fv(e, "seed", cx), 0.0).max(0.0) as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let planes = grain_planes(img.w, img.h, sigma, softness, seed);
    // grain swings the display value by up to about this much at full amount
    let swing = 0.22 * amount;
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let p = &mut row[x * 4..x * 4 + 4];
            let a = p[3];
            if a <= 1e-6 {
                continue;
            }
            let n = &planes.px[(y * w + x) * 4..(y * w + x) * 4 + 4];
            let c = enc([p[0] / a, p[1] / a, p[2] / a]);
            let l = luma(c).clamp(0.0, 1.0);
            let k = swing * response(l, shadows, mids, highs);
            // luminance grain, with each colour channel drawing on its own noise by `color`
            let o = [0, 1, 2].map(|i| (c[i] + k * (n[0] * (1.0 - color) + n[1 + i] * color)).clamp(0.0, 1.0));
            let o = dec(o);
            p[0] = o[0] * a;
            p[1] = o[1] * a;
            p[2] = o[2] * a;
        }
    });
}

// ------------------------------------------------------------------ halation and diffusion

/// A blurred copy (sigma in working pixels).
fn blurred_copy(src: &Image, sigma: f32) -> Image {
    let mut g = src.clone();
    if sigma > 0.3 {
        gaussian(&mut g, sigma, sigma, true);
    }
    g
}

/// A radius control (full-resolution pixels) as a blur sigma in working pixels.
fn sigma_of(radius: f32, cx: &FxCtx) -> f32 {
    num(radius, 1.0).clamp(0.0, 600.0) * num(cx.px_scale, 1.0).clamp(0.05, 8.0) * 0.5
}

/// Halation: light from bright parts of the picture scatters back through the film and exposes
/// the layer behind it, which gives a soft glow, red-orange in colour, around highlights. Only the
/// highlights spread; the rest of the picture is untouched.
pub fn halation(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let strength = frac(fv(e, "strength", cx));
    if strength <= 0.0 || img.w == 0 || img.h == 0 {
        return;
    }
    let threshold = frac(fv(e, "threshold", cx));
    let soft = frac(fv(e, "softness", cx)).max(0.02);
    let sigma = sigma_of(fv(e, "radius", cx), cx);
    let tint_c = lin(cv(e, "tint", cx).map(|v| num(v, 1.0).clamp(0.0, 1.0)));
    let hi = highlights(img, threshold, soft);
    // a tight core and a broad skirt, as the real thing has
    let core = blurred_copy(&hi, sigma * 0.5);
    let skirt = blurred_copy(&hi, sigma * 1.6);
    // the tint's own brightness is divided out so the strength means the same for any colour
    let tl = luma(tint_c).max(1e-3);
    let gain = [tint_c[0] / tl, tint_c[1] / tl, tint_c[2] / tl].map(|g| g * strength * 2.2);
    let w = img.w;
    img.px.par_chunks_mut(w * 4).zip(core.px.par_chunks(w * 4).zip(skirt.px.par_chunks(w * 4))).for_each(|(row, (c, s))| {
        for x in 0..w {
            let (c, s) = (&c[x * 4..x * 4 + 4], &s[x * 4..x * 4 + 4]);
            // energy of the glow at this pixel, from the light that reached it
            let l = (luma([c[0], c[1], c[2]]) * 0.45 + luma([s[0], s[1], s[2]]) * 0.55).max(0.0);
            let p = &mut row[x * 4..x * 4 + 4];
            if p[3] > 1e-6 {
                add_light(p, [l * gain[0], l * gain[1], l * gain[2]]);
            }
        }
    });
}

/// Diffusion: a soft filter in front of the lens. The picture softens a little, bright parts
/// bloom, and the blacks lift to a faint haze.
pub fn diffusion(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let strength = frac(fv(e, "strength", cx));
    let lift = frac(fv(e, "black_lift", cx));
    if (strength <= 0.0 && lift <= 0.0) || img.w == 0 || img.h == 0 {
        return;
    }
    if strength > 0.0 {
        let bias = frac(fv(e, "highlight_bias", cx));
        let sigma = sigma_of(fv(e, "radius", cx), cx);
        let soften = blurred_copy(img, sigma * 0.6);
        let bloom = blurred_copy(&highlights(img, 0.55, 0.35), sigma * 1.4);
        let mix = strength * (1.0 - bias) * 0.6;
        let glow = strength * bias * 1.1;
        img.px.par_chunks_mut(4).zip(soften.px.par_chunks(4).zip(bloom.px.par_chunks(4))).for_each(|(p, (s, b))| {
            if p[3] <= 1e-6 {
                return;
            }
            for k in 0..3 {
                p[k] = (p[k] * (1.0 - mix) + s[k] * mix + b[k] * glow).max(0.0);
            }
        });
    }
    if lift > 0.0 {
        // blacks rise towards a faint haze, in display space; whites do not move
        img.px.par_chunks_mut(4).for_each(|p| {
            let a = p[3];
            if a <= 1e-6 {
                return;
            }
            let c = enc([p[0] / a, p[1] / a, p[2] / a]);
            let o = dec(c.map(|v| v + lift * 0.09 * (1.0 - v.clamp(0.0, 1.0)).powi(2)));
            p[0] = o[0] * a;
            p[1] = o[1] * a;
            p[2] = o[2] * a;
        });
    }
}

// ------------------------------------------------------------- film gate, streak, fringing

/// Seconds of media time at this frame, from the exact tick (never from a float accumulator).
fn clip_seconds(cx: &FxCtx) -> f64 {
    cx.t.0 as f64 / filmcraft_time::TICKS_PER_SECOND as f64
}

/// `img` as seen shifted by (`dx`, `dy`) pixels (bilinear, edges repeated so no black borders).
fn shifted(img: &Image, dx: f32, dy: f32) -> Image {
    let (w, h) = (img.w, img.h);
    let mut out = Image::new(w, h);
    out.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let (sx, sy) = (x as f32 - dx, y as f32 - dy);
            let (x0, y0) = (sx.floor(), sy.floor());
            let (fx, fy) = (sx - x0, sy - y0);
            let (x0, y0) = (x0 as isize, y0 as isize);
            let (a, b, c, d) = (img.get_clamped(x0, y0), img.get_clamped(x0 + 1, y0), img.get_clamped(x0, y0 + 1), img.get_clamped(x0 + 1, y0 + 1));
            for k in 0..4 {
                row[x * 4 + k] = (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy;
            }
        }
    });
    out
}

/// Film gate: the picture wanders a little as the film moves through the projector gate (weave),
/// exposure wobbles from frame to frame (flicker), and dust and scratches ride on the print.
pub fn film_gate(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    if img.w == 0 || img.h == 0 {
        return;
    }
    let scale = num(cx.px_scale, 1.0).clamp(0.05, 8.0);
    let weave = num(fv(e, "weave", cx), 0.0).clamp(0.0, 200.0) * scale;
    let speed = frac(fv(e, "weave_speed", cx));
    let flicker = frac(fv(e, "flicker", cx));
    let dust = frac(fv(e, "dust", cx));
    let scratches = frac(fv(e, "scratches", cx));
    let seed = (num(fv(e, "seed", cx), 0.0).max(0.0) as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let secs = clip_seconds(cx);
    let frame = noise_frame_seed(cx) ^ seed;
    // weave: slow smooth wander in both directions (up to `weave` pixels either way)
    if weave > 0.0 && speed > 0.0 {
        let t = secs * (0.2 + 2.8 * f64::from(speed));
        let (dx, dy) = (noise1(t, 1, seed) * weave, noise1(t, 2, seed) * weave * 0.6);
        if dx.abs() > 1e-3 || dy.abs() > 1e-3 {
            *img = shifted(img, dx, dy);
        }
    }
    // flicker: a different exposure every frame, up to half a stop at 100%
    if flicker > 0.0 {
        let n = hash3(7, 11, frame ^ 0xF11C_4E55) * 2.0 - 1.0;
        let gain = (2f32).powf(n * flicker * 0.5);
        img.px.par_chunks_mut(4).for_each(|p| {
            for v in &mut p[..3] {
                *v *= gain;
            }
        });
    }
    let (w, h) = (img.w, img.h);
    // dust: a few small light and dark specks that exist for a single frame
    if dust > 0.0 {
        let count = (dust * (w * h) as f32 / 30_000.0).round().clamp(0.0, 400.0) as usize;
        for i in 0..count {
            let r = |k: u64| hash3(i, k as usize, frame ^ 0xD057);
            let (cxp, cyp) = (r(1) * w as f32, r(2) * h as f32);
            let rad = (0.6 + r(3) * 1.8) * scale;
            let light = r(4) > 0.4;
            let strength = 0.35 + 0.5 * r(5);
            let reach = rad.ceil() as isize + 1;
            for y in (cyp as isize - reach).max(0)..(cyp as isize + reach + 1).min(h as isize) {
                for x in (cxp as isize - reach).max(0)..(cxp as isize + reach + 1).min(w as isize) {
                    let d = ((x as f32 + 0.5 - cxp).powi(2) + (y as f32 + 0.5 - cyp).powi(2)).sqrt();
                    let k = (1.0 - d / rad).clamp(0.0, 1.0) * strength;
                    if k > 0.0 {
                        let i = (y as usize * w + x as usize) * 4;
                        for c in 0..3 {
                            img.px[i + c] = if light { img.px[i + c] + (img.px[i + 3] - img.px[i + c]) * k } else { img.px[i + c] * (1.0 - k) };
                        }
                    }
                }
            }
        }
    }
    // scratches: thin vertical lines that stay for a few frames
    if scratches > 0.0 {
        let epoch = ((secs * 24.0) / 5.0).floor() as i64 as u64 ^ seed;
        let count = (scratches * 4.0).ceil() as usize;
        for i in 0..count {
            let r = |k: usize| hash3(i, k, epoch.wrapping_mul(0x9E37_79B9) ^ 0x5C4A);
            if r(1) > scratches * 1.4 + 0.15 {
                continue;
            }
            let x = (r(2) * w as f32) as isize;
            let (y0, len) = ((r(3) * 0.5 * h as f32) as usize, ((0.35 + 0.65 * r(4)) * h as f32) as usize);
            let light = r(5) > 0.3;
            let wide = (scale.round() as isize).max(1);
            for xx in x..x + wide {
                if xx < 0 || xx >= w as isize {
                    continue;
                }
                for y in y0..(y0 + len).min(h) {
                    let i = (y * w + xx as usize) * 4;
                    for c in 0..3 {
                        img.px[i + c] = if light { img.px[i + c] + (img.px[i + 3] - img.px[i + c]) * 0.35 } else { img.px[i + c] * 0.7 };
                    }
                }
            }
        }
    }
}

/// Anamorphic streak: bright lights smear into long horizontal lines, coloured by the lens coating
/// (blue by default), because the squeezed lens blurs sideways far more than up and down.
pub fn anamorphic_streak(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let strength = frac(fv(e, "strength", cx));
    if strength <= 0.0 || img.w == 0 || img.h == 0 {
        return;
    }
    let threshold = frac(fv(e, "threshold", cx));
    let length = num(fv(e, "length", cx), 1.0).clamp(0.0, 4000.0) * num(cx.px_scale, 1.0).clamp(0.05, 8.0);
    let tint_c = lin(cv(e, "tint", cx).map(|v| num(v, 1.0).clamp(0.0, 1.0)));
    let hi = highlights(img, threshold, 0.12);
    // a long soft tail and a shorter, brighter core, both only horizontal (a touch of height keeps it a line)
    let mut tail = hi.clone();
    gaussian(&mut tail, length * 0.5, 0.8 * num(cx.px_scale, 1.0).clamp(0.05, 8.0), true);
    let mut core = hi;
    gaussian(&mut core, length * 0.15, 0.6 * num(cx.px_scale, 1.0).clamp(0.05, 8.0), true);
    let tl = luma(tint_c).max(1e-3);
    let gain = tint_c.map(|g| g / tl * strength * 1.6);
    img.px.par_chunks_mut(4).zip(tail.px.par_chunks(4).zip(core.px.par_chunks(4))).for_each(|(p, (t, c))| {
        if p[3] <= 1e-6 {
            return;
        }
        let l = luma([t[0], t[1], t[2]]) * 0.6 + luma([c[0], c[1], c[2]]) * 0.4;
        add_light(p, [l * gain[0], l * gain[1], l * gain[2]]);
    });
}

/// Lens fringing: a real lens focuses red and blue slightly apart, which shows as coloured edges
/// that grow toward the corners. Red is pushed out and blue pulled in, by up to `amount` pixels at
/// the corners and not at all at the centre.
pub fn lens_fringing(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let amount = num(fv(e, "amount", cx), 0.0).clamp(0.0, 200.0) * num(cx.px_scale, 1.0).clamp(0.05, 8.0);
    if amount <= 0.0 || img.w == 0 || img.h == 0 {
        return;
    }
    let (w, h) = (img.w, img.h);
    let (mx, my) = (w as f32 / 2.0, h as f32 / 2.0);
    let rmax = (mx * mx + my * my).sqrt().max(1.0);
    let src = img.clone();
    // one channel sampled `k` pixel-fractions along the way out from the centre
    let sample = |x: f32, y: f32, c: usize| {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as isize, y0 as isize);
        let v = |dx: isize, dy: isize| src.get_clamped(x0 + dx, y0 + dy)[c];
        (v(0, 0) * (1.0 - fx) + v(1, 0) * fx) * (1.0 - fy) + (v(0, 1) * (1.0 - fx) + v(1, 1) * fx) * fy
    };
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let (px, py) = (x as f32 + 0.5 - mx, y as f32 + 0.5 - my);
            let r = (px * px + py * py).sqrt();
            let d = amount * (r / rmax).powi(2);
            let (ux, uy) = if r > 1e-6 { (px / r, py / r) } else { (0.0, 0.0) };
            // the sample for a pixel lies nearer the centre for red (so red lands further out)
            let red = sample(x as f32 - ux * d, y as f32 - uy * d, 0);
            let blue = sample(x as f32 + ux * d, y as f32 + uy * d, 2);
            row[x * 4] = red;
            row[x * 4 + 2] = blue;
        }
    });
}
