//! Measures of one picture ([`analyze_rgba`] → [`FrameStats`]).
//!
//! The input is an sRGB-encoded RGBA8 picture (alpha is ignored), as a decoder or the preview
//! hands it over. One pass over every pixel builds a luma histogram, the saturation and a reduced
//! copy (box-averaged blocks, at most 320×180) on which the colour measures run; so the numbers do
//! not depend on how many pixels the picture has, only on what it shows.
//!
//! | measure | what it is |
//! |---|---|
//! | `luma_mean`, `luma_p5`, `luma_p95` | Rec.709 luma of the encoded values, 0..1, and its 5th and 95th percentile |
//! | `contrast` | `luma_p95 − luma_p5`, 0..1: robust to a few specks |
//! | `saturation` | mean HSV saturation of the pixels that are not near black, 0..1 |
//! | `palette` | up to five colours (k-means in Lab) with the share of the picture each covers |
//! | `colour_temperature` | McCamy CCT of the average colour, kelvin: a warm cast is low, a cool one high |
//! | `sharpness` | variance of the Laplacian of the reduced luma: bigger is crisper |
//! | `grain` | noise level (σ, luma 0..1) from the median response of a noise-isolating 3×3 filter on a centre crop at full resolution; an upper bound, as fine texture counts too |
//! | `letterbox` | the black bars on each side as fractions of the picture, and the aspect of what is left |

use serde::{Deserialize, Serialize};

use crate::color::{colour_temperature, lab_distance, srgb_to_lab, srgb_to_linear};
use crate::{AnalysisError, Result};

/// Bumped whenever a number in [`FrameStats`] could change for the same picture.
pub const IMAGE_VERSION: u32 = 1;
/// The longest side accepted.
pub const MAX_DIMENSION: u32 = 1 << 16;
/// The most pixels accepted (16384 × 16384): keeps every size computation inside 32-bit `usize`.
pub const MAX_PIXELS: u64 = 1 << 28;

const REDUCE_W: usize = 320;
const REDUCE_H: usize = 180;
const GRAIN_CROP: usize = 512;
/// A channel value at or below this counts as black for the letterbox scan (video black is 16).
const BLACK: u8 = 20;
/// A bar must be at least this many pixels to count (one dark edge row is not a letterbox).
const MIN_BAR_PIXELS: usize = 2;
const PALETTE_SIZE: usize = 5;

/// A validated picture: sRGB-encoded RGBA8, row after row.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    width: u32,
    height: u32,
    rgba: &'a [u8],
}

impl<'a> Frame<'a> {
    /// Checks the size before anything is allocated. Bytes beyond `width * height * 4` are ignored.
    pub fn new(width: u32, height: u32, rgba: &'a [u8]) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(AnalysisError::Empty);
        }
        let pixels = u64::from(width) * u64::from(height);
        if width > MAX_DIMENSION || height > MAX_DIMENSION || pixels > MAX_PIXELS {
            return Err(AnalysisError::TooLarge { width, height });
        }
        let need = usize::try_from(pixels).ok().and_then(|p| p.checked_mul(4)).ok_or(AnalysisError::TooLarge { width, height })?;
        match rgba.get(..need) {
            Some(rgba) => Ok(Frame { width, height, rgba }),
            None => Err(AnalysisError::ShortBuffer { need, have: rgba.len() }),
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}

/// One colour of the palette.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteColor {
    /// CIE L*a*b* (D65).
    pub lab: [f32; 3],
    /// sRGB 0..1, for drawing.
    pub rgb: [f32; 3],
    /// The share of the picture this colour stands for, 0..1; the palette adds up to 1.
    pub share: f32,
}

/// The black bars around the picture, as fractions of its height (top, bottom) and width (left, right).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Letterbox {
    pub top: f32,
    pub bottom: f32,
    pub left: f32,
    pub right: f32,
}

impl Letterbox {
    pub fn any(&self) -> bool {
        self.top > 0.0 || self.bottom > 0.0 || self.left > 0.0 || self.right > 0.0
    }
}

/// Everything measured in one picture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameStats {
    pub width: u32,
    pub height: u32,
    pub luma_mean: f32,
    pub luma_p5: f32,
    pub luma_p95: f32,
    pub contrast: f32,
    pub saturation: f32,
    /// Most-covering first; empty only for a picture whose colours cannot be told apart from nothing.
    pub palette: Vec<PaletteColor>,
    /// Kelvin; `None` for an all-black picture.
    pub colour_temperature: Option<f32>,
    pub sharpness: f32,
    pub grain: f32,
    pub letterbox: Letterbox,
    /// Width over height of the whole picture.
    pub frame_aspect: f32,
    /// Width over height of what is inside the bars.
    pub active_aspect: f32,
}

/// Measures a picture given as sRGB-encoded RGBA8; see the module docs for what each number is.
pub fn analyze_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<FrameStats> {
    Ok(analyze(&Frame::new(width, height, rgba)?))
}

fn ceil_div(a: usize, b: usize) -> usize {
    a.div_ceil(b.max(1))
}

/// The box-averaged copy of the picture on which the colour measures run.
struct Reduced {
    w: usize,
    h: usize,
    /// sRGB-encoded 0..1.
    rgb: Vec<[f32; 3]>,
}

struct Pass {
    reduced: Reduced,
    hist: [u64; 256],
    saturation: f32,
}

fn pass(f: &Frame) -> Pass {
    let (w, h) = (f.width as usize, f.height as usize);
    let step = ceil_div(w, REDUCE_W).max(ceil_div(h, REDUCE_H));
    let (pw, ph) = (ceil_div(w, step), ceil_div(h, step));
    let mut sums = vec![[0u32; 3]; pw * ph];
    let mut counts = vec![0u32; pw * ph];
    let mut hist = [0u64; 256];
    let (mut sat_sum, mut sat_n) = (0f64, 0u64);
    for (y, row) in f.rgba.chunks_exact(w * 4).enumerate() {
        let base = (y / step) * pw;
        for (x, &[r, g, b, _]) in row.as_chunks::<4>().0.iter().enumerate() {
            let i = base + x / step;
            if let (Some(s), Some(c)) = (sums.get_mut(i), counts.get_mut(i)) {
                s[0] += u32::from(r);
                s[1] += u32::from(g);
                s[2] += u32::from(b);
                *c += 1;
            }
            // Rec.709 weights in 1/256: 54 + 183 + 19 = 256
            let luma = (54 * u32::from(r) + 183 * u32::from(g) + 19 * u32::from(b)) >> 8;
            if let Some(slot) = hist.get_mut(luma.min(255) as usize) {
                *slot += 1;
            }
            let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
            if mx > 10 {
                sat_sum += f64::from(mx - mn) / f64::from(mx);
                sat_n += 1;
            }
        }
    }
    let rgb = sums
        .iter()
        .zip(&counts)
        .map(|(s, c)| {
            let d = (*c).max(1) as f32 * 255.0;
            [s[0] as f32 / d, s[1] as f32 / d, s[2] as f32 / d]
        })
        .collect();
    Pass { reduced: Reduced { w: pw, h: ph, rgb }, hist, saturation: if sat_n == 0 { 0.0 } else { (sat_sum / sat_n as f64) as f32 } }
}

/// The value below which `fraction` of the pixels lie, from a 256-bin histogram, as 0..1.
fn percentile(hist: &[u64; 256], total: u64, fraction: f64) -> f32 {
    let target = (total as f64 * fraction).ceil() as u64;
    let mut seen = 0u64;
    for (i, n) in hist.iter().enumerate() {
        seen += n;
        if seen >= target.max(1) {
            return i as f32 / 255.0;
        }
    }
    1.0
}

fn luma_of([r, g, b]: [f32; 3]) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// The variance of the 4-neighbour Laplacian over the interior of the reduced luma.
fn sharpness(r: &Reduced) -> f32 {
    if r.w < 3 || r.h < 3 {
        return 0.0;
    }
    let luma: Vec<f32> = r.rgb.iter().map(|p| luma_of(*p)).collect();
    let at = |x: usize, y: usize| luma.get(y * r.w + x).copied().unwrap_or(0.0);
    let (mut sum, mut sum_sq, mut n) = (0f64, 0f64, 0u64);
    for y in 1..r.h - 1 {
        for x in 1..r.w - 1 {
            let l = f64::from(at(x - 1, y) + at(x + 1, y) + at(x, y - 1) + at(x, y + 1) - 4.0 * at(x, y));
            sum += l;
            sum_sq += l * l;
            n += 1;
        }
    }
    if n == 0 {
        return 0.0;
    }
    let mean = sum / n as f64;
    ((sum_sq / n as f64) - mean * mean).max(0.0) as f32
}

/// The noise level from the median absolute response of the 3×3 mask `[1 -2 1; -2 4 -2; 1 -2 1]`
/// (Immerkaer 1996) on a centre crop at full resolution. White noise of deviation σ gives a response
/// of deviation 6σ, and the median of its absolute value is 0.6745 of that; the median keeps edges
/// from counting as noise.
fn grain(f: &Frame) -> f32 {
    let (w, h) = (f.width as usize, f.height as usize);
    let (cw, ch) = (w.min(GRAIN_CROP), h.min(GRAIN_CROP));
    if cw < 3 || ch < 3 {
        return 0.0;
    }
    let (x0, y0) = ((w - cw) / 2, (h - ch) / 2);
    let mut luma = Vec::with_capacity(cw * ch);
    for y in y0..y0 + ch {
        let start = (y * w + x0) * 4;
        let Some(row) = f.rgba.get(start..start + cw * 4) else { return 0.0 };
        luma.extend(row.as_chunks::<4>().0.iter().map(|&[r, g, b, _]| (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0));
    }
    let at = |x: usize, y: usize| luma.get(y * cw + x).copied().unwrap_or(0.0);
    let mut responses = Vec::with_capacity((cw - 2) * (ch - 2));
    for y in 1..ch - 1 {
        for x in 1..cw - 1 {
            let r = at(x - 1, y - 1) + at(x + 1, y - 1) + at(x - 1, y + 1) + at(x + 1, y + 1)
                - 2.0 * (at(x, y - 1) + at(x - 1, y) + at(x + 1, y) + at(x, y + 1))
                + 4.0 * at(x, y);
            responses.push(r.abs());
        }
    }
    if responses.is_empty() {
        return 0.0;
    }
    let mid = responses.len() / 2;
    let (_, median, _) = responses.select_nth_unstable_by(mid, f32::total_cmp);
    *median / (0.6745 * 6.0)
}

/// How many of the picture's rows (or columns) at the start are black, stopping at `limit`.
fn black_run(limit: usize, is_black: impl Fn(usize) -> bool) -> usize {
    let mut n = 0;
    while n < limit && is_black(n) {
        n += 1;
    }
    n
}

fn letterbox(f: &Frame) -> Letterbox {
    let (w, h) = (f.width as usize, f.height as usize);
    let px = |x: usize, y: usize| -> bool {
        match f.rgba.get((y * w + x) * 4..(y * w + x) * 4 + 3) {
            Some(&[r, g, b]) => r <= BLACK && g <= BLACK && b <= BLACK,
            _ => false,
        }
    };
    let row_black = |y: usize| (0..w).all(|x| px(x, y));
    // a bar never covers half the picture: a wholly dark frame has no bars, it is just dark
    let (vlimit, hlimit) = (h / 2, w / 2);
    let top = black_run(vlimit, row_black);
    let bottom = black_run(vlimit, |i| row_black(h - 1 - i));
    if top >= vlimit && bottom >= vlimit {
        return Letterbox::default();
    }
    let (y0, y1) = (top, h - bottom);
    let col_black = |x: usize| (y0..y1).all(|y| px(x, y));
    let left = black_run(hlimit, col_black);
    let right = black_run(hlimit, |i| col_black(w - 1 - i));
    let frac = |n: usize, whole: usize| if n >= MIN_BAR_PIXELS { n as f32 / whole as f32 } else { 0.0 };
    let lb = Letterbox { top: frac(top, h), bottom: frac(bottom, h), left: frac(left, w), right: frac(right, w) };
    if left >= hlimit && right >= hlimit { Letterbox { left: 0.0, right: 0.0, ..lb } } else { lb }
}

/// Up to [`PALETTE_SIZE`] colours: k-means in Lab, started from well-spread points (the pixel
/// nearest the mean, then each time the pixel farthest from all chosen), so the same picture always
/// gives the same palette.
fn palette(r: &Reduced) -> Vec<PaletteColor> {
    let labs: Vec<[f32; 3]> = r.rgb.iter().map(|p| srgb_to_lab(*p)).collect();
    let n = labs.len();
    if n == 0 {
        return Vec::new();
    }
    let mean = labs.iter().fold([0f32; 3], |a, l| [a[0] + l[0], a[1] + l[1], a[2] + l[2]]).map(|v| v / n as f32);
    let nearest_to =
        |target: [f32; 3]| labs.iter().enumerate().min_by(|a, b| lab_distance(*a.1, target).total_cmp(&lab_distance(*b.1, target))).map(|(i, _)| i);
    let mut centers: Vec<[f32; 3]> = nearest_to(mean).and_then(|i| labs.get(i).copied()).into_iter().collect();
    while centers.len() < PALETTE_SIZE {
        let far = labs
            .iter()
            .map(|l| (centers.iter().map(|c| lab_distance(*l, *c)).fold(f32::INFINITY, f32::min), *l))
            .fold((0.0f32, None), |best, (d, l)| if d > best.0 { (d, Some(l)) } else { best });
        match far {
            // within a few units of an existing colour is the same colour
            (d, Some(l)) if d > 4.0 => centers.push(l),
            _ => break,
        }
    }
    let mut assign = vec![0usize; n];
    for _ in 0..12 {
        let mut changed = false;
        for (a, l) in assign.iter_mut().zip(&labs) {
            let best = centers.iter().enumerate().min_by(|x, y| lab_distance(*l, *x.1).total_cmp(&lab_distance(*l, *y.1))).map_or(0, |(i, _)| i);
            if *a != best {
                *a = best;
                changed = true;
            }
        }
        let mut sums = vec![([0f32; 3], 0u32); centers.len()];
        for (a, l) in assign.iter().zip(&labs) {
            if let Some((s, c)) = sums.get_mut(*a) {
                *s = [s[0] + l[0], s[1] + l[1], s[2] + l[2]];
                *c += 1;
            }
        }
        for (center, (s, c)) in centers.iter_mut().zip(&sums) {
            if *c > 0 {
                *center = s.map(|v| v / *c as f32);
            }
        }
        if !changed {
            break;
        }
    }
    let mut shares = vec![(0u32, [0f32; 3]); centers.len()];
    for (a, p) in assign.iter().zip(&r.rgb) {
        if let Some((c, s)) = shares.get_mut(*a) {
            *c += 1;
            *s = [s[0] + p[0], s[1] + p[1], s[2] + p[2]];
        }
    }
    let mut out: Vec<PaletteColor> = centers
        .iter()
        .zip(&shares)
        .filter(|(_, (c, _))| *c > 0)
        .map(|(lab, (c, s))| PaletteColor { lab: *lab, rgb: s.map(|v| v / *c as f32), share: *c as f32 / n as f32 })
        .collect();
    out.sort_by(|a, b| b.share.total_cmp(&a.share));
    out
}

/// The average colour in linear light, for the colour temperature.
fn mean_linear(r: &Reduced) -> [f32; 3] {
    let n = r.rgb.len().max(1) as f64;
    let sum = r
        .rgb
        .iter()
        .fold([0f64; 3], |a, p| [a[0] + f64::from(srgb_to_linear(p[0])), a[1] + f64::from(srgb_to_linear(p[1])), a[2] + f64::from(srgb_to_linear(p[2]))]);
    [(sum[0] / n) as f32, (sum[1] / n) as f32, (sum[2] / n) as f32]
}

fn analyze(f: &Frame) -> FrameStats {
    let p = pass(f);
    let total = u64::from(f.width) * u64::from(f.height);
    let mean = p.hist.iter().enumerate().map(|(i, n)| i as f64 * *n as f64).sum::<f64>() / total.max(1) as f64 / 255.0;
    let (p5, p95) = (percentile(&p.hist, total, 0.05), percentile(&p.hist, total, 0.95));
    let lb = letterbox(f);
    let active_w = f64::from(f.width) * (1.0 - f64::from(lb.left) - f64::from(lb.right));
    let active_h = f64::from(f.height) * (1.0 - f64::from(lb.top) - f64::from(lb.bottom));
    FrameStats {
        width: f.width,
        height: f.height,
        luma_mean: mean as f32,
        luma_p5: p5,
        luma_p95: p95,
        contrast: (p95 - p5).max(0.0),
        saturation: p.saturation,
        palette: palette(&p.reduced),
        colour_temperature: colour_temperature(mean_linear(&p.reduced)),
        sharpness: sharpness(&p.reduced),
        grain: grain(f),
        letterbox: lb,
        frame_aspect: (f64::from(f.width) / f64::from(f.height)) as f32,
        active_aspect: if active_h > 0.0 { (active_w / active_h) as f32 } else { 0.0 },
    }
}
