//! Motion between two pictures ([`analyze_motion`] → [`MotionPair`]) and a rough camera-move class
//! for a run of them ([`summarize`] → [`MotionSummary`]).
//!
//! Both pictures are reduced to at most 128×72 luma. Blocks of 8×8 with some texture are matched
//! against the other picture over ±6 pixels (smallest sum of absolute differences, refined to a
//! fraction of a pixel). The vectors are fitted with a global model of the camera: a shift plus a
//! zoom about the centre, `v = t + s·(p − centre)`, refitted without the blocks that disagree. The
//! blocks that still disagree are things moving on their own (`residual`, their share).
//!
//! Directions are those of the *picture*: a positive `dx` means the picture moved right, which is a
//! camera pan to the left. All rates are per second, as a fraction of the picture's width (`dx`),
//! height (`dy`) or size (`zoom`, the fractional change of scale), so they do not depend on the
//! resolution or on how far apart in time the two pictures were.

use serde::{Deserialize, Serialize};

use crate::image::Frame;
use crate::{AnalysisError, Result};

/// Bumped whenever a number in [`MotionPair`] or [`MotionSummary`] could change.
pub const MOTION_VERSION: u32 = 1;
const MAX_W: usize = 128;
const MAX_H: usize = 72;
const BLOCK: usize = 8;
const RANGE: i32 = 6;
/// A block needs at least this much luma variance (0..1 scale) to be matched.
const MIN_VARIANCE: f32 = 0.0004;
/// Frames further apart than this say little about camera motion.
const MAX_INTERVAL: f64 = 60.0;

/// Motion between two pictures.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MotionPair {
    /// The mean absolute difference of the two pictures' luma, 0..1: how much changed, whatever the cause.
    pub energy: f32,
    /// Fraction of the width the picture moved right, per second.
    pub dx: f32,
    /// Fraction of the height the picture moved down, per second.
    pub dy: f32,
    /// Fractional growth of the picture's scale per second (positive: zooming in or moving forward).
    pub zoom: f32,
    /// The share of the matched blocks that move on their own rather than with the camera, 0..1.
    pub residual: f32,
    /// How many blocks had enough texture to be matched (of at most about 90); few means the numbers are shaky.
    pub blocks: u32,
}

/// The luma of a picture box-averaged down to fit [`MAX_W`]×[`MAX_H`].
struct Plane {
    w: usize,
    h: usize,
    luma: Vec<f32>,
}

impl Plane {
    fn new(f: &Frame) -> Plane {
        let (w, h) = (f.width() as usize, f.height() as usize);
        let step = w.div_ceil(MAX_W).max(h.div_ceil(MAX_H)).max(1);
        let (pw, ph) = (w.div_ceil(step), h.div_ceil(step));
        let mut sums = vec![0f32; pw * ph];
        let mut counts = vec![0u32; pw * ph];
        for (y, row) in f.rgba().chunks_exact(w * 4).enumerate() {
            let base = (y / step) * pw;
            for (x, &[r, g, b, _]) in row.as_chunks::<4>().0.iter().enumerate() {
                let i = base + x / step;
                if let (Some(s), Some(c)) = (sums.get_mut(i), counts.get_mut(i)) {
                    *s += 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
                    *c += 1;
                }
            }
        }
        let luma = sums.iter().zip(&counts).map(|(s, c)| s / (*c).max(1) as f32 / 255.0).collect();
        Plane { w: pw, h: ph, luma }
    }

    fn at(&self, x: usize, y: usize) -> f32 {
        self.luma.get(y * self.w + x).copied().unwrap_or(0.0)
    }

    fn variance(&self, x0: usize, y0: usize) -> f32 {
        let (mut s, mut s2) = (0f32, 0f32);
        for y in y0..y0 + BLOCK {
            for x in x0..x0 + BLOCK {
                let v = self.at(x, y);
                s += v;
                s2 += v * v;
            }
        }
        let n = (BLOCK * BLOCK) as f32;
        (s2 / n - (s / n) * (s / n)).max(0.0)
    }

    fn sad(&self, other: &Plane, x0: usize, y0: usize, dx: i32, dy: i32) -> f32 {
        let mut s = 0f32;
        for y in 0..BLOCK {
            for x in 0..BLOCK {
                let (ox, oy) = ((x0 + x) as i32 + dx, (y0 + y) as i32 + dy);
                s += (self.at(x0 + x, y0 + y) - other.at(ox.max(0) as usize, oy.max(0) as usize)).abs();
            }
        }
        s
    }
}

/// A block's centre and where its content went, in plane pixels.
struct Vector {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
}

/// The offset of the lowest of three costs at -1, 0, +1, as a fraction of a pixel.
fn parabola(minus: f32, centre: f32, plus: f32) -> f32 {
    let denom = minus - 2.0 * centre + plus;
    if denom.abs() < 1e-9 { 0.0 } else { (0.5 * (minus - plus) / denom).clamp(-0.5, 0.5) }
}

fn vectors(a: &Plane, b: &Plane) -> Vec<Vector> {
    let r = RANGE as usize;
    let mut out = Vec::new();
    if a.w < BLOCK + 2 * r || a.h < BLOCK + 2 * r {
        return out;
    }
    let mut y0 = r;
    while y0 + BLOCK + r <= a.h {
        let mut x0 = r;
        while x0 + BLOCK + r <= a.w {
            if a.variance(x0, y0) >= MIN_VARIANCE {
                let mut best = (f32::INFINITY, 0i32, 0i32);
                for dy in -RANGE..=RANGE {
                    for dx in -RANGE..=RANGE {
                        let cost = a.sad(b, x0, y0, dx, dy);
                        // on a tie the smaller move wins: a flat area stays put
                        if cost < best.0 - 1e-6 || (cost - best.0).abs() <= 1e-6 && dx.abs() + dy.abs() < best.1.abs() + best.2.abs() {
                            best = (cost, dx, dy);
                        }
                    }
                }
                let (_, bx, by) = best;
                let fx = if bx.abs() < RANGE { parabola(a.sad(b, x0, y0, bx - 1, by), best.0, a.sad(b, x0, y0, bx + 1, by)) } else { 0.0 };
                let fy = if by.abs() < RANGE { parabola(a.sad(b, x0, y0, bx, by - 1), best.0, a.sad(b, x0, y0, bx, by + 1)) } else { 0.0 };
                out.push(Vector { x: x0 as f32 + BLOCK as f32 / 2.0, y: y0 as f32 + BLOCK as f32 / 2.0, vx: bx as f32 + fx, vy: by as f32 + fy });
            }
            x0 += BLOCK;
        }
        y0 += BLOCK;
    }
    out
}

/// Least squares for `v = t + s·(p − c)` over the vectors picked by `keep`: (tx, ty, s).
fn fit(vs: &[Vector], c: (f32, f32), keep: impl Fn(&Vector) -> bool) -> Option<(f32, f32, f32)> {
    // unknowns (tx, ty, s); each vector gives two equations
    let (mut n, mut sx, mut sy, mut sxx, mut syy) = (0f64, 0f64, 0f64, 0f64, 0f64);
    let (mut bx, mut by, mut bs) = (0f64, 0f64, 0f64);
    for v in vs.iter().filter(|v| keep(v)) {
        let (px, py) = (f64::from(v.x - c.0), f64::from(v.y - c.1));
        let (vx, vy) = (f64::from(v.vx), f64::from(v.vy));
        n += 1.0;
        sx += px;
        sy += py;
        sxx += px * px;
        syy += py * py;
        bx += vx;
        by += vy;
        bs += vx * px + vy * py;
    }
    if n < 3.0 {
        return None;
    }
    // normal equations of [1 0 px; 0 1 py] t = v:
    //   [ n   0   sx ] [tx]   [bx]
    //   [ 0   n   sy ] [ty] = [by]
    //   [ sx  sy  q  ] [s ]   [bs]      with q = sxx + syy
    let q = sxx + syy;
    // Cramer's rule
    let d3 = |m: [[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let a = [[n, 0.0, sx], [0.0, n, sy], [sx, sy, q]];
    let det = d3(a);
    if det.abs() < 1e-9 {
        return None;
    }
    let col = |k: usize| {
        let mut m = a;
        for (row, b) in m.iter_mut().zip([bx, by, bs]) {
            row[k] = b;
        }
        d3(m) / det
    };
    let (tx, ty, s) = (col(0), col(1), col(2));
    (tx.is_finite() && ty.is_finite() && s.is_finite()).then_some((tx as f32, ty as f32, s as f32))
}

/// Motion between `a` and `b`, which were `seconds` apart (0 < seconds ≤ 60). The pictures must have
/// the same size.
pub fn analyze_motion(a: &Frame, b: &Frame, seconds: f64) -> Result<MotionPair> {
    if (a.width(), a.height()) != (b.width(), b.height()) {
        return Err(AnalysisError::MismatchedFrames);
    }
    if !(seconds.is_finite() && seconds > 0.0 && seconds <= MAX_INTERVAL) {
        return Err(AnalysisError::BadInterval);
    }
    let (pa, pb) = (Plane::new(a), Plane::new(b));
    let energy = pa.luma.iter().zip(&pb.luma).map(|(x, y)| (x - y).abs()).sum::<f32>() / pa.luma.len().max(1) as f32;
    let vs = vectors(&pa, &pb);
    let centre = (pa.w as f32 / 2.0, pa.h as f32 / 2.0);
    let per = |step: f32, whole: usize| (f64::from(step) / whole.max(1) as f64 / seconds) as f32;
    let Some(first) = fit(&vs, centre, |_| true) else {
        return Ok(MotionPair { energy, dx: 0.0, dy: 0.0, zoom: 0.0, residual: 0.0, blocks: vs.len() as u32 });
    };
    // refit without the blocks that move on their own
    let off = |m: (f32, f32, f32), v: &Vector| {
        let (ex, ey) = (m.0 + m.2 * (v.x - centre.0) - v.vx, m.1 + m.2 * (v.y - centre.1) - v.vy);
        (ex * ex + ey * ey).sqrt()
    };
    let model = fit(&vs, centre, |v| off(first, v) <= 1.5).unwrap_or(first);
    let moving = vs.iter().filter(|v| off(model, v) > 1.0).count();
    Ok(MotionPair {
        energy,
        dx: per(model.0, pa.w),
        dy: per(model.1, pa.h),
        zoom: (f64::from(model.2) / seconds) as f32,
        residual: moving as f32 / vs.len().max(1) as f32,
        blocks: vs.len() as u32,
    })
}

/// A rough class of what the camera did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CameraMove {
    /// No camera motion worth the name.
    Static,
    /// A steady sideways move.
    Pan,
    /// A steady up or down move.
    Tilt,
    ZoomIn,
    ZoomOut,
    /// Shaking: movement with no direction to it.
    Handheld,
    /// More than one of these, or none clearly.
    Mixed,
    /// Too few matched blocks to say.
    Unknown,
}

/// What a run of pairs (one shot) adds up to.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MotionSummary {
    /// The mean of the pairs' `energy`.
    pub energy: f32,
    /// The 90th percentile of the pairs' `energy`: a burst of action shows here.
    pub energy_p90: f32,
    /// The mean of the pairs' `residual`.
    pub residual: f32,
    /// Mean rates, signed, per second (see [`MotionPair`]).
    pub dx: f32,
    pub dy: f32,
    pub zoom: f32,
    pub camera: CameraMove,
}

/// Shift or zoom per second below which the camera counts as still.
const STILL_SHIFT: f32 = 0.01;
const STILL_ZOOM: f32 = 0.012;

fn mean(v: impl Iterator<Item = f32>) -> f32 {
    let (s, n) = v.fold((0f64, 0u32), |(s, n), x| (s + f64::from(x), n + 1));
    if n == 0 { 0.0 } else { (s / f64::from(n)) as f32 }
}

/// The share of `values` that has the sign of `of`.
fn agreement(values: &[f32], of: f32) -> f32 {
    values.iter().filter(|v| **v * of > 0.0).count() as f32 / values.len().max(1) as f32
}

/// Sums up the pairs of one shot. Empty input is [`CameraMove::Unknown`].
pub fn summarize(pairs: &[MotionPair]) -> MotionSummary {
    let usable: Vec<&MotionPair> = pairs.iter().filter(|p| p.blocks >= 6).collect();
    let mut energies: Vec<f32> = pairs.iter().map(|p| p.energy).collect();
    energies.sort_by(f32::total_cmp);
    let p90 = energies.get(((energies.len() as f32 * 0.9).ceil() as usize).saturating_sub(1)).copied().unwrap_or(0.0);
    let (dxs, dys, zs): (Vec<f32>, Vec<f32>, Vec<f32>) =
        (usable.iter().map(|p| p.dx).collect(), usable.iter().map(|p| p.dy).collect(), usable.iter().map(|p| p.zoom).collect());
    let (dx, dy, zoom) = (mean(dxs.iter().copied()), mean(dys.iter().copied()), mean(zs.iter().copied()));
    let base = MotionSummary {
        energy: mean(pairs.iter().map(|p| p.energy)),
        energy_p90: p90,
        residual: mean(pairs.iter().map(|p| p.residual)),
        dx,
        dy,
        zoom,
        camera: CameraMove::Unknown,
    };
    if usable.is_empty() || usable.len() * 3 < pairs.len() {
        return base;
    }
    // how big the movement is each time, and how much of it is net travel
    let (travel_x, travel_y, travel_z) = (mean(dxs.iter().map(|v| v.abs())), mean(dys.iter().map(|v| v.abs())), mean(zs.iter().map(|v| v.abs())));
    let consistent = |values: &[f32], net: f32| agreement(values, net) >= 0.75;
    let (pan, tilt, zoom_move) =
        (dx.abs() >= STILL_SHIFT && consistent(&dxs, dx), dy.abs() >= STILL_SHIFT && consistent(&dys, dy), zoom.abs() >= STILL_ZOOM && consistent(&zs, zoom));
    let camera = match (pan, tilt, zoom_move) {
        (true, false, false) => CameraMove::Pan,
        (false, true, false) => CameraMove::Tilt,
        (false, false, true) if zoom > 0.0 => CameraMove::ZoomIn,
        (false, false, true) => CameraMove::ZoomOut,
        (false, false, false) => {
            if travel_x.max(travel_y) >= STILL_SHIFT || travel_z >= STILL_ZOOM {
                // moving about without going anywhere: shaking
                CameraMove::Handheld
            } else {
                CameraMove::Static
            }
        }
        _ => CameraMove::Mixed,
    };
    MotionSummary { camera, ..base }
}
