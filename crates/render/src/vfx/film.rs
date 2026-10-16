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
    matches!(id, "film_grain" | "halation" | "diffusion")
}

pub(super) fn apply(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    match e.effect.as_str() {
        "film_grain" => film_grain(img, e, cx),
        "halation" => halation(img, e, cx),
        "diffusion" => diffusion(img, e, cx),
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
