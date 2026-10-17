//! The sound map (fork roadmap F13.2): where each kind of sound is on the timeline, how loud it is, and
//! what is missing or off.
//!
//! The timeline's audio is sorted into **lanes**: narration, music, ambience (room tone and beds),
//! effects, and anything not yet sorted. The engine decides which lane a clip belongs to (its Essential
//! Sound tag, else narration, else the analysis's guess); this module takes the clips already sorted
//! and answers from them alone, so the map and its main findings exist before anything is measured.
//! A measurement adds loudness over time per lane ([`LevelMeter`], K-weighted momentary loudness in
//! LUFS, one number every half second).
//!
//! | rule | fires when |
//! |---|---|
//! | no ambience | the video is at least 20 s and has sound, every clip is sorted into a lane, but nothing is in the ambience lane |
//! | ambience gap | the ambience lane has sound but stops for 2 s or more (or starts late) |
//! | dead air | nothing at all plays for 1.5 s or more (Problem from 5 s); with a measurement, clips that are silent count too |
//! | room tone jump | (measured) the ambience changes by 6 LU or more where one ambience clip meets the next (Problem from 10) |
//!
//! A **deliberate silence** from the sound plan is subtracted before the rules look: planned quiet is not
//! dead air and not a gap. Everything is plain integer ticks and every input is untrusted: invalid
//! clips are ignored, counts are capped, and non-finite levels count as silence.

use filmcraft_audio_dsp::loudness::LoudnessMeter;
use serde::{Deserialize, Serialize};

use crate::coach::{Finding, Kind, Severity, clock, finding, finish, len_text, secs};
use crate::{AnalysisError, Result};
use filmcraft_time::TICKS_PER_SECOND;

/// Bumped whenever a rule, a threshold or a number here could change.
pub const SOUNDMAP_VERSION: u32 = 1;
/// Seconds between two loudness readings.
pub const HOP_SECONDS: f64 = 0.5;
/// The most clips and loudness readings accepted.
pub const MAX_CLIPS: usize = 50_000;
pub const MAX_READINGS: usize = 200_000;
/// The longest programme: twelve hours.
pub const MAX_TICKS: i64 = 12 * 3600 * TICKS_PER_SECOND;

const NO_AMBIENCE_MIN_SECONDS: f64 = 20.0;
const AMBIENCE_GAP_SECONDS: f64 = 2.0;
const DEAD_AIR_SECONDS: f64 = 1.5;
const DEAD_AIR_PROBLEM_SECONDS: f64 = 5.0;
/// A reading at or below this is silence.
const SILENT_LUFS: f32 = -60.0;
const JUMP_LU: f64 = 6.0;
const JUMP_PROBLEM_LU: f64 = 10.0;
const JUMP_SIDE_SECONDS: f64 = 2.0;
/// Two ambience clips closer than this meet.
const MEET_SECONDS: f64 = 0.25;

/// A kind of sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Lane {
    Narration,
    Music,
    Ambience,
    Effects,
    /// Sound nobody has said what it is.
    Other,
}

impl Lane {
    pub const ALL: [Lane; 5] = [Lane::Narration, Lane::Music, Lane::Ambience, Lane::Effects, Lane::Other];

    pub fn name(self) -> &'static str {
        match self {
            Lane::Narration => "narration",
            Lane::Music => "music",
            Lane::Ambience => "ambience",
            Lane::Effects => "effects",
            Lane::Other => "other",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Lane::Narration => "Narration",
            Lane::Music => "Music",
            Lane::Ambience => "Ambience",
            Lane::Effects => "Effects",
            Lane::Other => "Unsorted",
        }
    }
}

/// A clip on the timeline, in a lane. Ticks from the start of the timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneClip {
    pub lane: Lane,
    pub id: u64,
    pub start: i64,
    pub end: i64,
}

/// One lane's loudness over time: LUFS every [`HOP_SECONDS`], `None` for silence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneLevels {
    pub lane: Lane,
    pub values: Vec<Option<f32>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundMapInput {
    /// The end of the programme, ticks: the end of the picture or of the sound, whichever is later.
    pub duration: i64,
    pub clips: Vec<LaneClip>,
    /// Deliberate silences from the sound plan, `(start, end)` in ticks.
    pub silences: Vec<(i64, i64)>,
    /// Empty when the sound has not been measured.
    pub levels: Vec<LaneLevels>,
}

/// What one lane holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneSummary {
    pub lane: Lane,
    /// Where it sounds, merged: `(start, end)` in ticks.
    pub spans: Vec<(i64, i64)>,
    pub covered_seconds: f64,
    /// The share of the programme it covers, 0 to 1.
    pub share: f32,
    /// The middle loudness over the time it sounds, once measured.
    pub median_lufs: Option<f64>,
    /// The loudest reading, once measured.
    pub loudest_lufs: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundMap {
    pub duration: i64,
    pub measured: bool,
    /// Narration, music, ambience and effects always; unsorted only when something is in it.
    pub lanes: Vec<LaneSummary>,
}

// ---------------------------------------------------------------------------- intervals

/// Sorted, with overlapping and touching intervals joined.
fn merge(mut v: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    v.retain(|(s, e)| e > s);
    v.sort_unstable();
    let mut out: Vec<(i64, i64)> = Vec::with_capacity(v.len());
    for (s, e) in v {
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// `a` without `b` (both merged).
fn subtract(a: &[(i64, i64)], b: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    for &(s, e) in a {
        let mut at = s;
        for &(bs, be) in b {
            if be <= at {
                continue;
            }
            if bs >= e {
                break;
            }
            if bs > at {
                out.push((at, bs));
            }
            at = at.max(be);
            if at >= e {
                break;
            }
        }
        if at < e {
            out.push((at, e));
        }
    }
    out
}

/// The parts of `from..to` that `covered` (merged) does not reach.
fn uncovered(covered: &[(i64, i64)], from: i64, to: i64) -> Vec<(i64, i64)> {
    if to <= from {
        return Vec::new();
    }
    subtract(&[(from, to)], covered)
}

fn ticks(seconds: f64) -> i64 {
    (seconds * TICKS_PER_SECOND as f64) as i64
}

fn median(mut v: Vec<f32>) -> Option<f64> {
    v.retain(|x| x.is_finite());
    v.sort_by(f32::total_cmp);
    v.get(v.len() / 2).map(|x| f64::from(*x))
}

// ---------------------------------------------------------------------------- the input

impl SoundMapInput {
    /// Checks the sizes. Invalid clips are not an error: [`SoundMapInput::spans`] ignores them.
    pub fn check(&self) -> Result<()> {
        if self.duration <= 0 || self.duration > MAX_TICKS {
            return Err(AnalysisError::BadInterval);
        }
        if self.clips.len() > MAX_CLIPS {
            return Err(AnalysisError::TooManyClips { clips: self.clips.len() });
        }
        let readings: usize = self.levels.iter().map(|l| l.values.len()).sum();
        if readings > MAX_READINGS * Lane::ALL.len() || self.silences.len() > MAX_CLIPS {
            return Err(AnalysisError::TooMany { what: "readings", count: readings });
        }
        Ok(())
    }

    /// The merged spans of one lane, kept inside the programme.
    pub fn spans(&self, lane: Lane) -> Vec<(i64, i64)> {
        merge(self.clips.iter().filter(|c| c.lane == lane).map(|c| (c.start.max(0), c.end.min(self.duration))).collect())
    }

    /// The merged spans of every lane together.
    fn all_spans(&self) -> Vec<(i64, i64)> {
        merge(self.clips.iter().map(|c| (c.start.max(0), c.end.min(self.duration))).collect())
    }

    fn silences(&self) -> Vec<(i64, i64)> {
        merge(self.silences.iter().map(|(s, e)| (*s, *e)).collect())
    }

    fn levels_of(&self, lane: Lane) -> Option<&[Option<f32>]> {
        self.levels.iter().find(|l| l.lane == lane).map(|l| l.values.as_slice())
    }

    pub fn measured(&self) -> bool {
        !self.levels.is_empty()
    }
}

/// The reading for the half second that starts at `second` (None when there is none or it is silent).
fn reading(values: &[Option<f32>], index: usize) -> Option<f32> {
    values.get(index).copied().flatten().filter(|v| v.is_finite() && *v > SILENT_LUFS)
}

// ---------------------------------------------------------------------------- the map

/// Where each lane sounds and how loud it is.
pub fn lane_map(input: &SoundMapInput) -> Result<SoundMap> {
    input.check()?;
    let total = secs(input.duration).max(f64::MIN_POSITIVE);
    let mut lanes = Vec::new();
    for lane in Lane::ALL {
        let spans = input.spans(lane);
        if lane == Lane::Other && spans.is_empty() {
            continue;
        }
        let covered: i64 = spans.iter().map(|(s, e)| e - s).sum();
        let (mut median_lufs, mut loudest_lufs) = (None, None);
        if let Some(values) = input.levels_of(lane) {
            let mut inside: Vec<f32> = Vec::new();
            for (i, _) in values.iter().enumerate() {
                let t = ticks(i as f64 * HOP_SECONDS);
                if spans.iter().any(|(s, e)| t >= *s && t < *e)
                    && let Some(v) = reading(values, i)
                {
                    inside.push(v);
                }
            }
            loudest_lufs = inside.iter().copied().map(f64::from).fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))));
            median_lufs = median(inside);
        }
        lanes.push(LaneSummary {
            lane,
            spans,
            covered_seconds: secs(covered),
            share: (secs(covered) / total).clamp(0.0, 1.0) as f32,
            median_lufs,
            loudest_lufs,
        });
    }
    Ok(SoundMap { duration: input.duration, measured: input.measured(), lanes })
}

// ---------------------------------------------------------------------------- the rules

/// What is missing or off in the sound, the worst first and then in time order.
pub fn findings(input: &SoundMapInput) -> Result<Vec<Finding>> {
    input.check()?;
    let mut out = Vec::new();
    ambience_rules(input, &mut out);
    dead_air(input, &mut out);
    if input.measured() {
        room_tone_jumps(input, &mut out);
    }
    Ok(finish(out))
}

fn ambience_rules(input: &SoundMapInput, out: &mut Vec<Finding>) {
    let ambience = input.spans(Lane::Ambience);
    // "there is no ambience" is only said when every clip has a lane: a clip nobody has sorted may be it
    let sorted = input.spans(Lane::Other).is_empty();
    let any_sound = !input.all_spans().is_empty();
    if ambience.is_empty() {
        if any_sound && sorted && secs(input.duration) >= NO_AMBIENCE_MIN_SECONDS {
            out.push(finding(
                Kind::NoAmbience,
                Severity::Look,
                0,
                None,
                "There is no ambience or room tone".to_string(),
                "Nothing is in the ambience lane, so every pause in the narration and every gap between sounds falls to complete silence, and the viewer hears each cut. Real places are never silent: a quiet bed of room tone, wind or distant traffic under the whole video makes the pauses feel like a place. If you tagged the clips in Essential Sound, check one is marked Ambience.".to_string(),
                "continuous room tone",
                &[("seconds", secs(input.duration))],
            ));
        }
        return;
    }
    let planned = input.silences();
    let min_gap = ticks(AMBIENCE_GAP_SECONDS);
    let gaps = subtract(&uncovered(&ambience, 0, input.duration), &planned);
    for (s, e) in gaps.into_iter().filter(|(s, e)| e - s >= min_gap) {
        let length = secs(e - s);
        let (title, detail) = if s == 0 {
            (
                format!("The ambience only starts at {}", clock(e)),
                format!(
                    "The first {} has no ambience under it, so the opening sits in silence and then the bed arrives. Start the bed at the beginning so the place is there from the first frame.",
                    len_text(length)
                ),
            )
        } else if e >= input.duration {
            (
                format!("The ambience stops at {}", clock(s)),
                format!(
                    "From {} to the end ({}) there is no ambience, so the last stretch drops into silence. Let the bed run under the whole video, and out with the end.",
                    clock(s),
                    len_text(length)
                ),
            )
        } else {
            (
                format!("The ambience stops for {} at {}", len_text(length), clock(s)),
                format!(
                    "There is a hole in the ambience from {} to {}. When room tone drops out the viewer hears the edit. Fill it with the same bed, or lay a quick crossfade between two beds. If the silence is on purpose, mark it in the sound plan.",
                    clock(s),
                    clock(e)
                ),
            )
        };
        out.push(finding(Kind::AmbienceGap, Severity::Look, s, Some(e), title, detail, "continuous room tone", &[("seconds", length)]));
    }
}

/// The stretches where nothing plays: no clip at all, or (when measured) clips that are silent.
fn silent_stretches(input: &SoundMapInput) -> Vec<(i64, i64)> {
    if !input.measured() {
        return uncovered(&input.all_spans(), 0, input.duration);
    }
    let windows = (secs(input.duration) / HOP_SECONDS).ceil() as usize;
    let windows = windows.min(MAX_READINGS * 2);
    let mut quiet: Vec<(i64, i64)> = Vec::new();
    for i in 0..windows {
        let heard = input.levels.iter().any(|l| reading(&l.values, i).is_some());
        if !heard {
            let (s, e) = (ticks(i as f64 * HOP_SECONDS), ticks((i + 1) as f64 * HOP_SECONDS).min(input.duration));
            quiet.push((s, e));
        }
    }
    merge(quiet)
}

fn dead_air(input: &SoundMapInput, out: &mut Vec<Finding>) {
    if input.all_spans().is_empty() {
        return;
    }
    let planned = input.silences();
    let min = ticks(DEAD_AIR_SECONDS);
    for (s, e) in subtract(&silent_stretches(input), &planned).into_iter().filter(|(s, e)| e - s >= min) {
        let length = secs(e - s);
        let severity = if length >= DEAD_AIR_PROBLEM_SECONDS { Severity::Problem } else { Severity::Look };
        out.push(finding(
            Kind::DeadAir,
            severity,
            s,
            Some(e),
            format!("{} of dead air at {}", len_text(length), clock(s)),
            format!(
                "Nothing is playing from {} to {}. A beat of quiet can be strong when it is chosen, but unplanned silence reads as a mistake or as the sound cutting out. Lay room tone or music under it, or, if it is on purpose, mark it as a deliberate silence in the sound plan and it will stop being flagged.",
                clock(s),
                clock(e)
            ),
            "silence as a tool, not an accident",
            &[("seconds", length)],
        ));
    }
}

fn room_tone_jumps(input: &SoundMapInput, out: &mut Vec<Finding>) {
    let Some(values) = input.levels_of(Lane::Ambience) else { return };
    let mut clips: Vec<&LaneClip> = input.clips.iter().filter(|c| c.lane == Lane::Ambience && c.end > c.start).collect();
    clips.sort_by_key(|c| (c.start, c.end));
    let meet = ticks(MEET_SECONDS);
    let side = (JUMP_SIDE_SECONDS / HOP_SECONDS).round().max(1.0) as usize;
    let index = |t: i64| usize::try_from(t.max(0)).unwrap_or(0) / usize::try_from(ticks(HOP_SECONDS).max(1)).unwrap_or(1);
    for pair in clips.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if b.start - a.end > meet {
            continue;
        }
        let at = b.start.max(0);
        let i = index(at);
        let before: Vec<f32> = (i.saturating_sub(side)..i).filter_map(|k| reading(values, k)).collect();
        let after: Vec<f32> = (i..i.saturating_add(side)).filter_map(|k| reading(values, k)).collect();
        if before.len() < 2 || after.len() < 2 {
            continue;
        }
        let (Some(was), Some(now)) = (median(before), median(after)) else { continue };
        let step = (now - was).abs();
        if step >= JUMP_LU {
            let severity = if step >= JUMP_PROBLEM_LU { Severity::Problem } else { Severity::Look };
            out.push(finding(
                Kind::RoomToneJump,
                severity,
                at,
                None,
                format!("The room tone jumps {step:.0} LU at {}", clock(at)),
                format!(
                    "The ambience is about {step:.0} LU {} after {} than before it, where one ambience clip meets the next. The ear takes a sudden change in the background as a cut in the world, not just in the picture. Match the two levels, or crossfade the beds over a second or two.",
                    if now > was { "louder" } else { "quieter" },
                    clock(at)
                ),
                "matching room tone across a cut",
                &[("lu", step), ("before_lufs", was), ("after_lufs", now)],
            ));
        }
    }
}

// ---------------------------------------------------------------------------- measuring

/// Measures loudness over time as the sound is produced: feed interleaved stereo samples with
/// [`LevelMeter::push`] and read one K-weighted momentary reading (LUFS) for every half second with
/// [`LevelMeter::finish`]. `None` is silence.
pub struct LevelMeter {
    meter: LoudnessMeter,
    hop: usize,
    pending: Vec<f32>,
    readings: Vec<Option<f32>>,
}

impl LevelMeter {
    pub fn new(sample_rate: u32) -> Result<LevelMeter> {
        if !(crate::audio::MIN_SAMPLE_RATE..=crate::audio::MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(AnalysisError::BadAudioFormat { sample_rate, channels: 2 });
        }
        let hop = ((f64::from(sample_rate) * HOP_SECONDS) as usize).max(1);
        Ok(LevelMeter { meter: LoudnessMeter::new(f64::from(sample_rate), 2), hop, pending: Vec::new(), readings: Vec::new() })
    }

    fn read(&mut self) {
        let v = self.meter.momentary();
        self.readings.push((v.is_finite() && v > f64::from(SILENT_LUFS)).then_some(v as f32));
    }

    /// Takes interleaved stereo samples (any length). Samples that are not numbers are silence.
    pub fn push(&mut self, interleaved: &[f32]) {
        self.pending.extend(interleaved.iter().map(|v| if v.is_finite() { *v } else { 0.0 }));
        let step = self.hop * 2;
        let mut used = 0;
        while self.pending.len() - used >= step && self.readings.len() < MAX_READINGS * 2 {
            if let Some(chunk) = self.pending.get(used..used + step) {
                self.meter.process_interleaved(chunk);
            }
            used += step;
            self.read();
        }
        self.pending.drain(..used);
    }

    /// The readings so far, with whatever is left over counted as a last reading.
    pub fn finish(mut self) -> Vec<Option<f32>> {
        if self.pending.len() >= self.hop / 2 && self.readings.len() < MAX_READINGS * 2 {
            let rest = std::mem::take(&mut self.pending);
            // whole stereo frames only
            if let Some(whole) = rest.get(..rest.len() / 2 * 2) {
                self.meter.process_interleaved(whole);
            }
            self.read();
        }
        self.readings
    }
}
