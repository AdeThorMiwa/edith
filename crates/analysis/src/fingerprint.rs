//! A style fingerprint ([`fingerprint`] → [`Fingerprint`]): the measurable character of a whole film
//! in one small record of numbers, so an edit can be compared with it and the coach can aim at it.
//!
//! The input ([`ReferenceInput`]) is what a measuring job found in a film: where its hard cuts are,
//! pictures measured at moments across it, and its sound in pieces plus the whole programme's loudness.
//! The fingerprint never holds footage, stills or sound: only numbers, a few hundred of them, small
//! enough to export and share.
//!
//! | part | what it says |
//! |---|---|
//! | `shots` | how many, how long (mean, median, 10th and 90th percentile, shortest, longest, a histogram), cuts a minute |
//! | `rhythm` | cuts a minute in each twentieth of the runtime, the mean shot length in the opening, middle and closing thirds, and whether the pace is quickening, steady or slowing |
//! | `picture` | brightness, contrast, saturation, sharpness, grain, colour warmth, black bars and the aspect inside them, a five-colour palette, and brightness, contrast, saturation and warmth through the film (twenty points) |
//! | `motion` | how much changes from frame to frame and the share of the film shot with each camera movement |
//! | `sound` | loudness, loudness range, true peak, silence, the shares that sound like speech, music and ambience, and loudness through the film (twenty points) |
//!
//! Cuts are the *hard* cuts a detector finds; a dissolve is not a cut and is not counted, which the
//! fingerprint says by saying nothing about transitions rather than guessing.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::audio::{AudioKind, Mix};
use crate::image::{FrameStats, Letterbox, PaletteColor};
use crate::motion::CameraMove;
use crate::sequence::{Bucket, percentile_of};
use crate::{AnalysisError, Result};

/// Bumped whenever a number in [`Fingerprint`] could change for the same measurements.
pub const FINGERPRINT_VERSION: u32 = 1;
/// How many points a curve through the film has.
pub const CURVE_POINTS: usize = 20;
const MAX_CUTS: usize = 200_000;
const MAX_PICTURES: usize = 5_000;
const MAX_SEGMENTS: usize = 20_000;
/// The longest film accepted: 48 hours.
const MAX_SECONDS: f64 = 48.0 * 3600.0;
/// Shot-length bands, as in the edit's own numbers: under 1, 2, 4, 8, 16 s, and longer.
const BANDS: [f64; 5] = [1.0, 2.0, 4.0, 8.0, 16.0];
/// What a quiet stretch counts as when drawing the loudness curve.
const FLOOR_LUFS: f64 = -70.0;

/// One picture measured at a moment of the film.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PictureSample {
    pub time_seconds: f64,
    pub stats: FrameStats,
    /// What the camera was doing there, when measured.
    pub camera: Option<CameraMove>,
    /// How much changed between frames there (0..1), when measured.
    pub motion_energy: Option<f32>,
}

/// A piece of the film's sound.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundSegment {
    pub start_seconds: f64,
    pub duration_seconds: f64,
    pub kind: AudioKind,
    pub loudness_lufs: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundInput {
    /// The whole programme: loudness, range, peak, silence.
    pub mix: Mix,
    pub segments: Vec<SoundSegment>,
}

/// What a measuring job found in a film.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceInput {
    pub name: String,
    pub duration_seconds: f64,
    pub width: u32,
    pub height: u32,
    /// Seconds from the start of each hard cut (the first moment of a new shot).
    pub cuts: Vec<f64>,
    pub pictures: Vec<PictureSample>,
    /// `None` for a film with no sound.
    pub sound: Option<SoundInput>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotShape {
    pub count: u32,
    pub mean_seconds: f64,
    pub median_seconds: f64,
    pub p10_seconds: f64,
    pub p90_seconds: f64,
    pub min_seconds: f64,
    pub max_seconds: f64,
    pub cuts_per_minute: f64,
    /// Under 1, 2, 4, 8, 16 s, and longer.
    pub histogram: Vec<Bucket>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PaceArc {
    /// The shots get shorter toward the end.
    Quickening,
    Steady,
    /// The shots get longer toward the end.
    Slowing,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rhythm {
    /// Cuts a minute in each twentieth of the runtime.
    pub curve: Vec<f32>,
    /// Mean shot length by where the shot starts: first, middle and last third of the runtime.
    pub opening_mean_seconds: Option<f64>,
    pub middle_mean_seconds: Option<f64>,
    pub closing_mean_seconds: Option<f64>,
    pub arc: PaceArc,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PicturePoint {
    /// Where in the film, 0 to 1.
    pub at: f32,
    pub luma: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub colour_temperature: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Picture {
    pub luma_mean: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub sharpness: f32,
    pub grain: f32,
    pub colour_temperature: Option<f32>,
    /// The bars that are there in most pictures (the median of each side).
    pub letterbox: Letterbox,
    /// Width over height of what is inside the bars (the median).
    pub active_aspect: f32,
    pub palette: Vec<PaletteColor>,
    pub timeline: Vec<PicturePoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MotionShape {
    /// The mean of how much changes between frames, 0..1.
    pub energy: f32,
    /// The share of the pictures with each camera movement (`static`, `pan`, `zoomIn`…), adding up to 1.
    pub camera: BTreeMap<String, f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundShape {
    pub integrated_lufs: Option<f64>,
    pub loudness_range_lu: Option<f64>,
    pub true_peak_dbtp: Option<f64>,
    pub silence_share: f32,
    /// Shares of the measured time that sound like speech, music and ambience.
    pub speech_share: f32,
    pub music_share: f32,
    pub ambience_share: f32,
    /// Loudness (LUFS) through the film, twenty points; −70 where it was silent.
    pub loudness_curve: Vec<f32>,
}

/// The style of a film, as numbers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fingerprint {
    pub version: u32,
    pub name: String,
    pub duration_seconds: f64,
    pub width: u32,
    pub height: u32,
    pub frame_aspect: f32,
    pub shots: ShotShape,
    pub rhythm: Rhythm,
    /// `None` when no picture could be measured.
    pub picture: Option<Picture>,
    pub motion: Option<MotionShape>,
    /// `None` for a film with no sound.
    pub sound: Option<SoundShape>,
}

fn finite(v: f64) -> Option<f64> {
    v.is_finite().then_some(v)
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let (s, n) = values.filter(|v| v.is_finite()).fold((0.0, 0u32), |(s, n), v| (s + v, n + 1));
    (n > 0).then(|| s / f64::from(n))
}

fn median(mut v: Vec<f32>) -> f32 {
    v.retain(|x| x.is_finite());
    v.sort_by(f32::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(0.0)
}

/// Which of the [`CURVE_POINTS`] twentieths of the film `t` falls in.
fn slot(t: f64, duration: f64) -> usize {
    ((t / duration * CURVE_POINTS as f64) as usize).min(CURVE_POINTS - 1)
}

/// The shots, as lengths and start times, from the cuts.
fn shots_of(cuts: &[f64], duration: f64) -> Vec<(f64, f64)> {
    let mut edges: Vec<f64> = cuts.iter().copied().filter(|c| c.is_finite() && *c > 0.0 && *c < duration).collect();
    edges.sort_by(f64::total_cmp);
    edges.dedup();
    let mut out = Vec::with_capacity(edges.len() + 1);
    let mut start = 0.0;
    for e in edges.into_iter().chain(std::iter::once(duration)) {
        out.push((start, e - start));
        start = e;
    }
    out
}

fn shape_of(shots: &[(f64, f64)], duration: f64) -> ShotShape {
    let mut lengths: Vec<f64> = shots.iter().map(|s| s.1).collect();
    lengths.sort_by(f64::total_cmp);
    let mut histogram: Vec<Bucket> = BANDS.iter().map(|b| Bucket { below_seconds: Some(*b), count: 0 }).collect();
    histogram.push(Bucket { below_seconds: None, count: 0 });
    for l in &lengths {
        let i = BANDS.iter().position(|b| l < b).unwrap_or(BANDS.len());
        if let Some(b) = histogram.get_mut(i) {
            b.count += 1;
        }
    }
    ShotShape {
        count: lengths.len() as u32,
        mean_seconds: lengths.iter().sum::<f64>() / lengths.len().max(1) as f64,
        median_seconds: percentile_of(&lengths, 0.5),
        p10_seconds: percentile_of(&lengths, 0.1),
        p90_seconds: percentile_of(&lengths, 0.9),
        min_seconds: lengths.first().copied().unwrap_or(0.0),
        max_seconds: lengths.last().copied().unwrap_or(0.0),
        cuts_per_minute: (lengths.len().saturating_sub(1)) as f64 / (duration / 60.0),
        histogram,
    }
}

fn rhythm_of(shots: &[(f64, f64)], duration: f64) -> Rhythm {
    let window_minutes = duration / CURVE_POINTS as f64 / 60.0;
    let mut counts = [0u32; CURVE_POINTS];
    // a cut is where a shot after the first begins
    for (start, _) in shots.iter().skip(1) {
        if let Some(c) = counts.get_mut(slot(*start, duration)) {
            *c += 1;
        }
    }
    let curve: Vec<f32> = counts.iter().map(|c| (f64::from(*c) / window_minutes) as f32).collect();
    let third = |k: usize| -> Option<f64> {
        let (a, b) = (duration * k as f64 / 3.0, duration * (k as f64 + 1.0) / 3.0);
        mean(shots.iter().filter(|(s, _)| *s >= a && (*s < b || (k == 2 && *s <= b))).map(|(_, l)| *l))
    };
    let (open, mid, close) = (third(0), third(1), third(2));
    let arc = match (open, close) {
        (Some(o), Some(c)) if o > 0.0 && c / o > 1.25 => PaceArc::Slowing,
        (Some(o), Some(c)) if o > 0.0 && c / o < 0.8 => PaceArc::Quickening,
        _ => PaceArc::Steady,
    };
    Rhythm { curve, opening_mean_seconds: open, middle_mean_seconds: mid, closing_mean_seconds: close, arc }
}

/// The colours that recur across the pictures: close colours merged, the biggest five.
pub fn merged_palette(samples: &[&FrameStats]) -> Vec<PaletteColor> {
    let mut merged: Vec<(PaletteColor, f32)> = Vec::new();
    for c in samples.iter().flat_map(|s| s.palette.iter()) {
        if !(c.share.is_finite() && c.lab.iter().all(|v| v.is_finite()) && c.rgb.iter().all(|v| v.is_finite())) {
            continue;
        }
        match merged.iter_mut().find(|(m, _)| crate::color::lab_distance(m.lab, c.lab) < 12.0) {
            Some((m, weight)) => {
                let total = *weight + c.share;
                if total > 0.0 {
                    for i in 0..3 {
                        m.lab[i] = (m.lab[i] * *weight + c.lab[i] * c.share) / total;
                        m.rgb[i] = (m.rgb[i] * *weight + c.rgb[i] * c.share) / total;
                    }
                }
                *weight = total;
            }
            None => merged.push((*c, c.share)),
        }
    }
    merged.sort_by(|a, b| b.1.total_cmp(&a.1));
    let total: f32 = merged.iter().take(5).map(|m| m.1).sum();
    merged
        .into_iter()
        .take(5)
        .map(|(mut c, w)| {
            c.share = if total > 0.0 { w / total } else { 0.0 };
            c
        })
        .collect()
}

fn picture_of(input: &ReferenceInput, duration: f64) -> Option<Picture> {
    let usable: Vec<&PictureSample> = input.pictures.iter().filter(|p| p.time_seconds.is_finite() && p.stats.luma_mean.is_finite()).collect();
    if usable.is_empty() {
        return None;
    }
    let avg = |f: &dyn Fn(&FrameStats) -> f32| mean(usable.iter().map(|p| f64::from(f(&p.stats)))).unwrap_or(0.0) as f32;
    let mut bins: Vec<Vec<&PictureSample>> = vec![Vec::new(); CURVE_POINTS];
    for p in &usable {
        if let Some(b) = bins.get_mut(slot(p.time_seconds.clamp(0.0, duration), duration)) {
            b.push(p);
        }
    }
    let timeline: Vec<PicturePoint> = bins
        .iter()
        .enumerate()
        .filter(|(_, b)| !b.is_empty())
        .map(|(i, b)| {
            let m = |f: &dyn Fn(&FrameStats) -> f32| mean(b.iter().map(|p| f64::from(f(&p.stats)))).unwrap_or(0.0) as f32;
            PicturePoint {
                at: (i as f32 + 0.5) / CURVE_POINTS as f32,
                luma: m(&|s| s.luma_mean),
                contrast: m(&|s| s.contrast),
                saturation: m(&|s| s.saturation),
                colour_temperature: mean(b.iter().filter_map(|p| p.stats.colour_temperature).map(f64::from)).map(|v| v as f32),
            }
        })
        .collect();
    let stats: Vec<&FrameStats> = usable.iter().map(|p| &p.stats).collect();
    Some(Picture {
        luma_mean: avg(&|s| s.luma_mean),
        contrast: avg(&|s| s.contrast),
        saturation: avg(&|s| s.saturation),
        sharpness: avg(&|s| s.sharpness),
        grain: avg(&|s| s.grain),
        colour_temperature: mean(usable.iter().filter_map(|p| p.stats.colour_temperature).map(f64::from)).map(|v| v as f32),
        letterbox: Letterbox {
            top: median(usable.iter().map(|p| p.stats.letterbox.top).collect()),
            bottom: median(usable.iter().map(|p| p.stats.letterbox.bottom).collect()),
            left: median(usable.iter().map(|p| p.stats.letterbox.left).collect()),
            right: median(usable.iter().map(|p| p.stats.letterbox.right).collect()),
        },
        active_aspect: median(usable.iter().map(|p| p.stats.active_aspect).collect()),
        palette: merged_palette(&stats),
        timeline,
    })
}

fn camera_name(c: CameraMove) -> &'static str {
    match c {
        CameraMove::Static => "static",
        CameraMove::Pan => "pan",
        CameraMove::Tilt => "tilt",
        CameraMove::ZoomIn => "zoomIn",
        CameraMove::ZoomOut => "zoomOut",
        CameraMove::Handheld => "handheld",
        CameraMove::Mixed => "mixed",
        CameraMove::Unknown => "unknown",
    }
}

fn motion_of(input: &ReferenceInput) -> Option<MotionShape> {
    let energy = mean(input.pictures.iter().filter_map(|p| p.motion_energy).map(f64::from))?;
    let cams: Vec<CameraMove> = input.pictures.iter().filter_map(|p| p.camera).filter(|c| *c != CameraMove::Unknown).collect();
    let mut camera = BTreeMap::new();
    for c in &cams {
        *camera.entry(camera_name(*c).to_string()).or_insert(0.0f32) += 1.0 / cams.len() as f32;
    }
    Some(MotionShape { energy: energy as f32, camera })
}

fn sound_of(s: &SoundInput, duration: f64) -> SoundShape {
    let segs: Vec<&SoundSegment> =
        s.segments.iter().filter(|g| g.start_seconds.is_finite() && g.duration_seconds.is_finite() && g.duration_seconds > 0.0).collect();
    let total: f64 = segs.iter().map(|g| g.duration_seconds).sum();
    let share = |k: AudioKind| if total > 0.0 { (segs.iter().filter(|g| g.kind == k).map(|g| g.duration_seconds).sum::<f64>() / total) as f32 } else { 0.0 };
    let mut bins: Vec<Vec<f64>> = vec![Vec::new(); CURVE_POINTS];
    let mut silent: Vec<bool> = vec![false; CURVE_POINTS];
    let mut seen: Vec<bool> = vec![false; CURVE_POINTS];
    for g in &segs {
        let i = slot((g.start_seconds + g.duration_seconds / 2.0).clamp(0.0, duration), duration);
        if let (Some(b), Some(sn), Some(sl)) = (bins.get_mut(i), seen.get_mut(i), silent.get_mut(i)) {
            *sn = true;
            match g.loudness_lufs.and_then(finite) {
                Some(l) => b.push(l),
                None => *sl = true,
            }
        }
    }
    // a twentieth with sound is the mean of its pieces; with none measured it is the quiet floor; with no piece at all, the one before
    let mut curve: Vec<f32> = Vec::with_capacity(CURVE_POINTS);
    for i in 0..CURVE_POINTS {
        let v = match (bins.get(i).and_then(|b| mean(b.iter().copied())), seen.get(i), silent.get(i)) {
            (Some(m), _, _) => m,
            (None, Some(true), Some(true)) => FLOOR_LUFS,
            _ => curve.last().map_or(FLOOR_LUFS, |p| f64::from(*p)),
        };
        curve.push(v.max(FLOOR_LUFS) as f32);
    }
    SoundShape {
        integrated_lufs: s.mix.integrated_lufs.and_then(finite),
        loudness_range_lu: s.mix.loudness_range_lu.and_then(finite),
        true_peak_dbtp: s.mix.true_peak_dbtp.and_then(finite),
        silence_share: if s.mix.silence_share.is_finite() { s.mix.silence_share.clamp(0.0, 1.0) } else { 0.0 },
        speech_share: share(AudioKind::Speech),
        music_share: share(AudioKind::Music),
        ambience_share: share(AudioKind::Ambience),
        loudness_curve: curve,
    }
}

/// The fingerprint of a film from what was measured in it. Refused: a length that is not a number from
/// above zero to 48 hours, or more cuts, pictures or sound pieces than a film can have.
pub fn fingerprint(input: &ReferenceInput) -> Result<Fingerprint> {
    let duration = input.duration_seconds;
    if !(duration.is_finite() && duration > 0.0 && duration <= MAX_SECONDS) {
        return Err(AnalysisError::BadInterval);
    }
    let too_many = |what: &'static str, n: usize, max: usize| (n > max).then_some(AnalysisError::TooMany { what, count: n });
    if let Some(e) = too_many("cuts", input.cuts.len(), MAX_CUTS)
        .or_else(|| too_many("pictures", input.pictures.len(), MAX_PICTURES))
        .or_else(|| too_many("pieces of sound", input.sound.as_ref().map_or(0, |s| s.segments.len()), MAX_SEGMENTS))
    {
        return Err(e);
    }
    let shots = shots_of(&input.cuts, duration);
    let (w, h) = (input.width.max(1), input.height.max(1));
    Ok(Fingerprint {
        version: FINGERPRINT_VERSION,
        name: input.name.chars().filter(|c| !c.is_control()).take(120).collect(),
        duration_seconds: duration,
        width: input.width,
        height: input.height,
        frame_aspect: (f64::from(w) / f64::from(h)) as f32,
        shots: shape_of(&shots, duration),
        rhythm: rhythm_of(&shots, duration),
        picture: picture_of(input, duration),
        motion: motion_of(input),
        sound: input.sound.as_ref().map(|s| sound_of(s, duration)),
    })
}
