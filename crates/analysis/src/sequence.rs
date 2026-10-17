//! The edit's own numbers ([`analyze_sequence`] → [`SequenceStats`]), from the layout of the
//! timeline alone: no decoding, so they are instant and always exact.
//!
//! The caller describes the timeline as plain spans ([`SequenceInput`]): clips with their start and
//! length in ticks on numbered tracks, the transitions, the narration. Track 0 is the lowest
//! (V1, A1); a higher video track covers a lower one.
//!
//! - **Shots** are what the viewer sees: the clip on the highest video track at each moment, with a
//!   clip that is not a shot (a title, a graphic, an adjustment layer: `is_shot: false`) left out.
//!   A change from one clip to another with nothing between them is a **cut**; a gap is not a cut.
//! - **J and L cuts** are cuts whose linked audio does not change at the same moment: the next
//!   shot's sound starting before the cut (J) or the last shot's sound running past it (L). Only
//!   cuts where both sides have linked audio count.
//! - **Transitions** are counted by kind and as a share of the cuts.
//! - **Narration pace** is words over the minutes of narration actually spoken.
//! - **Occupancy** is the share of the timeline each track has something on, overlaps counted once.

use std::collections::BTreeMap;

use filmcraft_time::TICKS_PER_SECOND;
use serde::{Deserialize, Serialize};

use crate::{AnalysisError, Result};

/// Bumped whenever a number in [`SequenceStats`] could change for the same timeline.
pub const SEQUENCE_VERSION: u32 = 1;
/// The most clips accepted over all tracks.
pub const MAX_CLIPS: usize = 500_000;
/// A change of sound this much before or after a cut makes it a J or L cut (two frames at 25 fps).
const JL_TOLERANCE_SECONDS: f64 = 0.08;
/// The width of a bar of the cuts-over-time series, at least.
const WINDOW_SECONDS: f64 = 30.0;
const MAX_WINDOWS: usize = 2000;

/// One clip on a track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipSpan {
    pub id: u64,
    /// Ticks from the start of the timeline.
    pub start: i64,
    pub duration: i64,
    /// Clips with the same link are the picture and sound of one source.
    pub link: Option<u64>,
    /// False for a title, graphic or adjustment layer: it is on screen but is not a shot.
    #[serde(default = "yes")]
    pub is_shot: bool,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionSpan {
    pub start: i64,
    pub duration: i64,
    /// What it is called (Cross Dissolve, Dip to Black…), for counting.
    pub kind: String,
    /// Whether it joins two clips (a transition from or to nothing is a fade, not a join).
    pub between_clips: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackSpans {
    pub clips: Vec<ClipSpan>,
    pub transitions: Vec<TransitionSpan>,
}

/// A stretch of narration: where it is and how many words it speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NarrationSpan {
    pub start: i64,
    pub duration: i64,
    pub words: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SequenceInput {
    pub video: Vec<TrackSpans>,
    pub audio: Vec<TrackSpans>,
    pub narration: Vec<NarrationSpan>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    /// Shots shorter than this many seconds (and at least the bucket before); `None` for the last, open one.
    pub below_seconds: Option<f64>,
    pub count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotStats {
    pub count: u32,
    pub mean_seconds: f64,
    pub median_seconds: f64,
    pub p10_seconds: f64,
    pub p90_seconds: f64,
    pub min_seconds: f64,
    pub max_seconds: f64,
    /// Under 1 s, 2 s, 4 s, 8 s, 16 s, and longer.
    pub histogram: Vec<Bucket>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CutStats {
    pub count: u32,
    /// Over the length of the timeline.
    pub per_minute: f64,
    /// The cuts in each window of the timeline.
    pub window_seconds: f64,
    pub series: Vec<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitStats {
    /// Cuts with linked audio on both sides: the ones that can be J or L cuts.
    pub cuts_with_audio: u32,
    pub j_cuts: u32,
    pub l_cuts: u32,
    /// Cuts that are J, L or both, over `cuts_with_audio`; 0 when there are none.
    pub share: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionStats {
    pub count: u32,
    pub by_kind: BTreeMap<String, u32>,
    /// Transitions that join two clips, over the cuts plus those transitions (every joint of two shots).
    pub share_of_joints: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NarrationStats {
    pub words: u64,
    pub spoken_seconds: f64,
    /// Words over spoken minutes; `None` with no narration.
    pub words_per_minute: Option<f64>,
    /// The share of the timeline with narration on it.
    pub share: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackOccupancy {
    pub video: bool,
    /// 0 is V1 or A1.
    pub index: u32,
    pub share: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SequenceStats {
    pub duration_seconds: f64,
    pub shots: ShotStats,
    pub cuts: CutStats,
    pub split_cuts: SplitStats,
    pub transitions: TransitionStats,
    pub narration: NarrationStats,
    pub tracks: Vec<TrackOccupancy>,
    /// The share of the timeline with some video / some audio on it.
    pub video_coverage: f64,
    pub audio_coverage: f64,
}

fn seconds(ticks: i64) -> f64 {
    ticks as f64 / TICKS_PER_SECOND as f64
}

/// A clip as it is measured: start and end in ticks within `0..`, positive length.
#[derive(Clone, Copy, Debug)]
struct Span {
    start: i64,
    end: i64,
    id: u64,
    link: Option<u64>,
    is_shot: bool,
}

fn normalise(c: &ClipSpan) -> Option<Span> {
    let end = c.start.saturating_add(c.duration);
    let (start, end) = (c.start.max(0), end);
    (c.duration > 0 && end > start).then_some(Span { start, end, id: c.id, link: c.link, is_shot: c.is_shot })
}

/// The merged length of intervals (overlaps counted once).
fn union_length(mut spans: Vec<(i64, i64)>) -> i64 {
    spans.sort_unstable();
    let (mut total, mut reach) = (0i64, i64::MIN);
    for (s, e) in spans {
        let s = s.max(reach);
        if e > s {
            total = total.saturating_add(e - s);
        }
        reach = reach.max(e);
    }
    total
}

/// One stretch of the timeline and the shot showing in it.
#[derive(Clone, Copy, Debug)]
struct Shot {
    start: i64,
    end: i64,
    clip: Span,
}

/// The shots: the clip on the highest track at each moment, consecutive stretches of one clip merged.
fn shots(video: &[Vec<Span>]) -> Vec<Shot> {
    let mut points: Vec<i64> = video.iter().flatten().filter(|c| c.is_shot).flat_map(|c| [c.start, c.end]).collect();
    points.sort_unstable();
    points.dedup();
    // per track the clips sorted by start, with a cursor that only moves forward
    let mut cursors = vec![0usize; video.len()];
    let mut out: Vec<Shot> = Vec::new();
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let mut top: Option<Span> = None;
        for (track, clips) in video.iter().enumerate().rev() {
            let Some(cursor) = cursors.get_mut(track) else { continue };
            while clips.get(*cursor).is_some_and(|c| c.end <= a) {
                *cursor += 1;
            }
            // clips can overlap within a track in a damaged project: any that covers `a` will do
            if let Some(c) = clips.iter().skip(*cursor).take_while(|c| c.start <= a).find(|c| c.is_shot && c.end > a) {
                top = Some(*c);
                break;
            }
        }
        let Some(clip) = top else { continue };
        match out.last_mut() {
            Some(last) if last.end == a && last.clip.id == clip.id => last.end = b,
            _ => out.push(Shot { start: a, end: b, clip }),
        }
    }
    out
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((sorted.len() as f64 * fraction).ceil() as usize).clamp(1, sorted.len());
    sorted.get(rank - 1).copied().unwrap_or(0.0)
}

/// Measures a timeline described as spans; see the module docs for what each number is.
pub fn analyze_sequence(input: &SequenceInput) -> Result<SequenceStats> {
    let total: usize = input.video.iter().chain(&input.audio).map(|t| t.clips.len()).sum();
    if total > MAX_CLIPS {
        return Err(AnalysisError::TooManyClips { clips: total });
    }
    let prepare = |tracks: &[TrackSpans]| -> Vec<Vec<Span>> {
        tracks
            .iter()
            .map(|t| {
                let mut v: Vec<Span> = t.clips.iter().filter_map(normalise).collect();
                v.sort_by_key(|c| (c.start, c.end));
                v
            })
            .collect()
    };
    let (video, audio) = (prepare(&input.video), prepare(&input.audio));
    let narration: Vec<(i64, i64, u32)> = input
        .narration
        .iter()
        .filter(|n| n.duration > 0)
        .map(|n| (n.start.max(0), n.start.saturating_add(n.duration), n.words))
        .filter(|(s, e, _)| e > s)
        .collect();
    let transitions: Vec<&TransitionSpan> = input.video.iter().flat_map(|t| &t.transitions).collect();
    let end = video
        .iter()
        .chain(&audio)
        .flatten()
        .map(|c| c.end)
        .chain(narration.iter().map(|n| n.1))
        .chain(transitions.iter().map(|t| t.start.saturating_add(t.duration.max(0))))
        .max()
        .unwrap_or(0);
    let duration = seconds(end);

    let shot_list = shots(&video);
    let mut lengths: Vec<f64> = shot_list.iter().map(|s| seconds(s.end - s.start)).collect();
    lengths.sort_by(f64::total_cmp);
    let bounds = [1.0, 2.0, 4.0, 8.0, 16.0];
    let mut histogram: Vec<Bucket> = bounds.iter().map(|b| Bucket { below_seconds: Some(*b), count: 0 }).collect();
    histogram.push(Bucket { below_seconds: None, count: 0 });
    for l in &lengths {
        let i = bounds.iter().position(|b| l < b).unwrap_or(bounds.len());
        if let Some(b) = histogram.get_mut(i) {
            b.count += 1;
        }
    }
    let shot_stats = ShotStats {
        count: lengths.len() as u32,
        mean_seconds: if lengths.is_empty() { 0.0 } else { lengths.iter().sum::<f64>() / lengths.len() as f64 },
        median_seconds: percentile(&lengths, 0.5),
        p10_seconds: percentile(&lengths, 0.1),
        p90_seconds: percentile(&lengths, 0.9),
        min_seconds: lengths.first().copied().unwrap_or(0.0),
        max_seconds: lengths.last().copied().unwrap_or(0.0),
        histogram,
    };

    // the cuts: one shot straight into another
    let cuts: Vec<(&Shot, &Shot)> = shot_list.windows(2).filter(|w| w[0].end == w[1].start).map(|w| (&w[0], &w[1])).collect();
    let window = WINDOW_SECONDS.max((duration / MAX_WINDOWS as f64).ceil());
    let windows = ((duration / window).ceil() as usize).clamp(1, MAX_WINDOWS);
    let mut series = vec![0u32; windows];
    for (_, next) in &cuts {
        let i = ((seconds(next.start) / window) as usize).min(windows - 1);
        if let Some(s) = series.get_mut(i) {
            *s += 1;
        }
    }
    let cut_stats = CutStats {
        count: cuts.len() as u32,
        per_minute: if duration > 0.0 { cuts.len() as f64 / (duration / 60.0) } else { 0.0 },
        window_seconds: window,
        series,
    };

    // J and L cuts: look at the audio linked to the two clips
    let mut audio_of: BTreeMap<u64, (i64, i64)> = BTreeMap::new();
    for c in audio.iter().flatten() {
        if let Some(link) = c.link {
            let e = audio_of.entry(link).or_insert((c.start, c.end));
            e.0 = e.0.min(c.start);
            e.1 = e.1.max(c.end);
        }
    }
    let tolerance = (JL_TOLERANCE_SECONDS * TICKS_PER_SECOND as f64) as i64;
    let mut split = SplitStats::default();
    for (prev, next) in &cuts {
        let (Some(a), Some(b)) = (prev.clip.link.and_then(|l| audio_of.get(&l)), next.clip.link.and_then(|l| audio_of.get(&l))) else { continue };
        let at = next.start;
        let (j, l) = (b.0 < at.saturating_sub(tolerance), a.1 > at.saturating_add(tolerance));
        split.cuts_with_audio += 1;
        split.j_cuts += u32::from(j);
        split.l_cuts += u32::from(l);
        split.share += f64::from(u8::from(j || l));
    }
    split.share = if split.cuts_with_audio > 0 { split.share / f64::from(split.cuts_with_audio) } else { 0.0 };

    let mut by_kind: BTreeMap<String, u32> = BTreeMap::new();
    for t in &transitions {
        *by_kind.entry(t.kind.clone()).or_insert(0) += 1;
    }
    let joins = transitions.iter().filter(|t| t.between_clips).count();
    let transition_stats = TransitionStats {
        count: transitions.len() as u32,
        by_kind,
        share_of_joints: if cuts.len() + joins > 0 { joins as f64 / (cuts.len() + joins) as f64 } else { 0.0 },
    };

    let words: u64 = narration.iter().map(|n| u64::from(n.2)).sum();
    let spoken: i64 = narration.iter().map(|n| n.1 - n.0).fold(0i64, i64::saturating_add);
    let spoken_union = union_length(narration.iter().map(|n| (n.0, n.1)).collect());
    let narration_stats = NarrationStats {
        words,
        spoken_seconds: seconds(spoken),
        words_per_minute: (spoken > 0).then(|| words as f64 / (seconds(spoken) / 60.0)),
        share: if end > 0 { spoken_union as f64 / end as f64 } else { 0.0 },
    };

    let share = |clips: &[Span]| -> f64 {
        if end <= 0 {
            return 0.0;
        }
        union_length(clips.iter().map(|c| (c.start, c.end)).collect()) as f64 / end as f64
    };
    let tracks: Vec<TrackOccupancy> = video
        .iter()
        .enumerate()
        .map(|(i, t)| TrackOccupancy { video: true, index: i as u32, share: share(t) })
        .chain(audio.iter().enumerate().map(|(i, t)| TrackOccupancy { video: false, index: i as u32, share: share(t) }))
        .collect();
    let all = |tracks: &[Vec<Span>]| share(&tracks.iter().flatten().copied().collect::<Vec<_>>());
    Ok(SequenceStats {
        duration_seconds: duration,
        shots: shot_stats,
        cuts: cut_stats,
        split_cuts: split,
        transitions: transition_stats,
        narration: narration_stats,
        tracks,
        video_coverage: all(&video),
        audio_coverage: all(&audio),
    })
}
