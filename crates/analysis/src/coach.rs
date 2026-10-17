//! The coach's rules ([`findings`]): what is worth telling an editor about an edit, worked out from
//! the numbers and never by a model. A [`Finding`] has a severity, the place it is at, a short
//! title, a built-in explanation in plain words, the editing idea behind it and the numbers it
//! rests on. The assistant fetches findings with a tool and may rephrase and teach around them, but
//! it cannot change them or the numbers in them.
//!
//! The rules are deliberately few and each can be explained in a sentence. They flag things that
//! are *usually* worth a look, not mistakes: a fast burst of cuts or a long hold can be exactly what
//! the editor wants, so the wording says what the number means and leaves the decision to them.
//! Severity is [`Severity::Look`] ("worth a look") for style and pace, [`Severity::Problem`]
//! ("probably a problem") for things that are almost never intended: a flash frame, a gap that plays
//! as black, a peak that will clip.
//!
//! | rule | fires when |
//! |---|---|
//! | flash frame | a shot is under 0.12 s |
//! | empty gap | no video clip covers a stretch of more than 0.05 s inside the edit |
//! | short between long | a shot under 1.5 s and at most a quarter of both neighbours, which are each 3 s or more |
//! | long hold | a shot is over three times the target shot length (at least 12 s); 20 s with no target |
//! | pace against the plan | the average shot is more than 35 % off the target (50 % for a section of at least three shots) |
//! | cuts come faster | a 30 s stretch has at least six cuts and 2.5 times the cut rate of the whole |
//! | narration pace | words per minute more than 15 % over or 20 % under the target (Problem past 30 % / 35 %) |
//! | few J and L cuts | at least eight cuts have sound on both sides and under a tenth are J or L cuts |
//! | many dissolves | at least six transitions and more than half of the joints between shots |
//! | mostly silent | (with a measured mix) over a quarter of the video is quieter than −60 dBFS |
//! | wide loudness range | (with a measured mix) over 12 LU |
//! | loudness off target | (with a measured mix) more than 3 LU from the target (Problem past 6) |
//! | peak too high | (with a measured mix) true peak above −1 dBTP |

use std::collections::BTreeMap;

use filmcraft_time::TICKS_PER_SECOND;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::audio::AudioStats;
use crate::sequence::{SequenceInput, SequenceStats, ShotSpan, analyze_sequence, shot_spans};

/// Bumped whenever a rule, a threshold or a wording could change a finding.
pub const COACH_VERSION: u32 = 1;
/// The most findings returned.
pub const MAX_FINDINGS: usize = 200;
/// The most of one kind before the rest are summed up in one line.
const MAX_PER_KIND: usize = 25;
const FLASH_SECONDS: f64 = 0.12;
const GAP_SECONDS: f64 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    /// Worth a look: it may be exactly what you want.
    Look,
    /// Probably a problem: almost never intended.
    Problem,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    FlashFrame,
    EmptyGap,
    ShortBetweenLong,
    LongHold,
    PaceVsPlan,
    SectionPace,
    CutBurst,
    NarrationPace,
    FewSplitCuts,
    ManyDissolves,
    MostlySilent,
    WideLoudness,
    LoudnessOffTarget,
    PeakTooHigh,
}

impl Kind {
    /// The name used in ids and JSON.
    pub fn name(self) -> &'static str {
        match self {
            Kind::FlashFrame => "flashFrame",
            Kind::EmptyGap => "emptyGap",
            Kind::ShortBetweenLong => "shortBetweenLong",
            Kind::LongHold => "longHold",
            Kind::PaceVsPlan => "paceVsPlan",
            Kind::SectionPace => "sectionPace",
            Kind::CutBurst => "cutBurst",
            Kind::NarrationPace => "narrationPace",
            Kind::FewSplitCuts => "fewSplitCuts",
            Kind::ManyDissolves => "manyDissolves",
            Kind::MostlySilent => "mostlySilent",
            Kind::WideLoudness => "wideLoudness",
            Kind::LoudnessOffTarget => "loudnessOffTarget",
            Kind::PeakTooHigh => "peakTooHigh",
        }
    }
}

/// One thing worth telling the editor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// Stable for the same edit: the kind and where it is, such as `shortBetweenLong@1524096000000`.
    pub id: String,
    pub kind: Kind,
    pub severity: Severity,
    /// Ticks.
    pub start: i64,
    pub end: Option<i64>,
    pub title: String,
    /// Plain words: what the numbers mean here and what to consider.
    pub detail: String,
    /// The editing idea behind it (pace, rhythm, J and L cuts…), for the assistant to teach from.
    pub idea: Option<String>,
    /// The measurements it rests on, in seconds, counts or shares.
    pub numbers: BTreeMap<String, f64>,
}

/// A stretch of the timeline with its own target (a plan section laid on the timeline).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionTarget {
    pub name: String,
    /// Ticks.
    pub start: i64,
    pub end: i64,
    /// Seconds; the overall target when absent.
    pub average_shot_seconds: Option<f64>,
}

/// What the edit is measured against. Every field is optional: with none, only the rules that need no
/// target fire.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Targets {
    pub words_per_minute: Option<f64>,
    pub average_shot_seconds: Option<f64>,
    /// Integrated loudness to aim for; many platforms normalise toward about −14 LUFS.
    pub loudness_lufs: f64,
    pub sections: Vec<SectionTarget>,
}

impl Default for Targets {
    fn default() -> Self {
        Targets { words_per_minute: None, average_shot_seconds: None, loudness_lufs: -14.0, sections: Vec::new() }
    }
}

/// A target that is a real number inside `lo..=hi`; anything else (NaN, zero, absurd) is no target.
fn within(v: Option<f64>, lo: f64, hi: f64) -> Option<f64> {
    v.filter(|v| v.is_finite() && (lo..=hi).contains(v))
}

/// Shot lengths worth planning for: a tenth of a second to ten minutes.
fn shot_target(v: Option<f64>) -> Option<f64> {
    within(v, 0.1, 600.0)
}

/// Spoken pace worth planning for, in words a minute.
fn pace_target(v: Option<f64>) -> Option<f64> {
    within(v, 20.0, 500.0)
}

fn secs(ticks: i64) -> f64 {
    ticks as f64 / TICKS_PER_SECOND as f64
}

/// `0.8 s`, `6 s`, `45 s`: a length for a sentence.
fn len_text(seconds: f64) -> String {
    if seconds < 10.0 {
        let tenths = (seconds * 10.0).round() / 10.0;
        if tenths.fract() == 0.0 { format!("{tenths:.0} s") } else { format!("{tenths:.1} s") }
    } else {
        format!("{} s", seconds.round() as i64)
    }
}

/// `2:41`, `1:05:00`: a place in the timeline for a sentence.
fn clock(ticks: i64) -> String {
    let n = secs(ticks.max(0)).floor() as u64;
    if n >= 3600 { format!("{}:{:02}:{:02}", n / 3600, (n / 60) % 60, n % 60) } else { format!("{}:{:02}", n / 60, n % 60) }
}

fn finding(kind: Kind, severity: Severity, start: i64, end: Option<i64>, title: String, detail: String, idea: &str, numbers: &[(&str, f64)]) -> Finding {
    Finding {
        id: format!("{}@{start}", kind.name()),
        kind,
        severity,
        start,
        end,
        title,
        detail,
        idea: (!idea.is_empty()).then(|| idea.to_string()),
        numbers: numbers.iter().filter(|(_, v)| v.is_finite()).map(|(k, v)| ((*k).to_string(), *v)).collect(),
    }
}

/// The stretches of the edit, from its first video clip to its last, that no video clip covers.
fn gaps(input: &SequenceInput, out: &mut Vec<Finding>) {
    let mut spans: Vec<(i64, i64)> = input
        .video
        .iter()
        .flat_map(|t| &t.clips)
        .filter(|c| c.duration > 0)
        .map(|c| (c.start.max(0), c.start.saturating_add(c.duration)))
        .filter(|(s, e)| e > s)
        .collect();
    spans.sort_unstable();
    let Some(&(first, _)) = spans.first() else { return };
    let min_gap = (GAP_SECONDS * TICKS_PER_SECOND as f64) as i64;
    if first > min_gap {
        out.push(finding(
            Kind::EmptyGap,
            Severity::Look,
            0,
            Some(first),
            format!("The picture starts after {}", len_text(secs(first))),
            format!("Nothing is on the video tracks for the first {}, so it plays as black. If you did not mean a pause before the picture, move the first clip to the start.", len_text(secs(first))),
            "gaps",
            &[("seconds", secs(first))],
        ));
    }
    let mut reach = spans.first().map_or(0, |s| s.1);
    for &(s, e) in spans.iter().skip(1) {
        if s - reach > min_gap {
            out.push(finding(
                Kind::EmptyGap,
                Severity::Problem,
                reach,
                Some(s),
                format!("A {} gap at {}", len_text(secs(s - reach)), clock(reach)),
                format!(
                    "No video clip covers {} to {}, so the viewer sees black there. If it is not on purpose, close it (ripple delete the gap) or fill it with a clip.",
                    clock(reach),
                    clock(s)
                ),
                "gaps",
                &[("seconds", secs(s - reach))],
            ));
        }
        reach = reach.max(e);
    }
}

/// Shots that touch: the ones whose neighbours matter for rhythm.
fn touching(shots: &[ShotSpan], i: usize) -> (bool, bool) {
    let before = i.checked_sub(1).and_then(|p| shots.get(p)).is_some_and(|p| p.end == shots.get(i).map_or(i64::MIN, |s| s.start));
    let after = shots.get(i).zip(shots.get(i + 1)).is_some_and(|(a, b)| a.end == b.start);
    (before, after)
}

fn shot_rules(shots: &[ShotSpan], targets: &Targets, out: &mut Vec<Finding>) {
    let len = |i: usize| shots.get(i).map_or(0.0, |s| secs(s.end - s.start));
    let target = shot_target(targets.average_shot_seconds);
    let hold = target.map_or(20.0, |t| (3.0 * t).max(12.0));
    for (i, s) in shots.iter().enumerate() {
        let l = len(i);
        if l < FLASH_SECONDS {
            out.push(finding(
                Kind::FlashFrame,
                Severity::Problem,
                s.start,
                Some(s.end),
                format!("A {} flash at {}", len_text(l), clock(s.start)),
                format!(
                    "This shot is only {} long: too brief to see, but enough to read as a glitch. It is usually left behind by a cut that did not quite line up. Delete it, or lengthen it if it is meant to be there.",
                    len_text(l)
                ),
                "flash frames",
                &[("seconds", l)],
            ));
            continue;
        }
        let (before, after) = touching(shots, i);
        if before && after && l < 1.5 {
            let (p, n) = (len(i - 1), len(i + 1));
            if p >= 3.0 && n >= 3.0 && l <= 0.25 * p.min(n) {
                out.push(finding(
                    Kind::ShortBetweenLong,
                    Severity::Look,
                    s.start,
                    Some(s.end),
                    format!("A {} shot between two longer ones ({} and {}) at {}", len_text(l), len_text(p), len_text(n), clock(s.start)),
                    format!(
                        "After {} of calm, a {} shot, then {} again. A sudden short shot reads as a jolt: it pulls the eye and breaks the rhythm. It can be right as a deliberate beat or a surprise; if it is not, give it more time or fold it into a neighbour.",
                        len_text(p),
                        len_text(l),
                        len_text(n)
                    ),
                    "rhythm",
                    &[("seconds", l), ("before", p), ("after", n)],
                ));
            }
        }
        if l > hold {
            out.push(finding(
                Kind::LongHold,
                Severity::Look,
                s.start,
                Some(s.end),
                format!("A {} shot with no cut at {}", len_text(l), clock(s.start)),
                format!(
                    "This shot holds for {}{}. Long holds suit a slow reveal or a landscape you want to sit in. If nothing in the picture changes, the narration or the movement has to carry it; otherwise consider cutting away or adding a second angle.",
                    len_text(l),
                    target.map_or(String::new(), |t| format!(", where your target is about {} a shot", len_text(t)))
                ),
                "pacing",
                &[("seconds", l)],
            ));
        }
    }
}

fn pace_rules(stats: &SequenceStats, shots: &[ShotSpan], targets: &Targets, out: &mut Vec<Finding>) {
    let overall = shot_target(targets.average_shot_seconds);
    let sections: Vec<&SectionTarget> = targets.sections.iter().filter(|s| s.end > s.start && s.start >= 0).collect();
    if sections.is_empty() {
        if let Some(t) = overall
            && stats.shots.count >= 5
        {
            let mean = stats.shots.mean_seconds;
            let off = (mean - t) / t;
            if off.abs() > 0.35 {
                let (word, advice) = if off > 0.0 {
                    ("slower", "That makes the pace feel slower than planned. If that is not the feel you want, look for shots that can lose a few seconds.")
                } else {
                    ("faster", "That makes the pace feel faster than planned. If that is not the feel you want, look for places to let a shot breathe.")
                };
                out.push(finding(
                    Kind::PaceVsPlan,
                    Severity::Look,
                    0,
                    None,
                    format!("Shots average {}, {word} than the plan's {}", len_text(mean), len_text(t)),
                    format!(
                        "Across {} shots the average length is {} against a target of {}, {:.0}% off. {advice}",
                        stats.shots.count,
                        len_text(mean),
                        len_text(t),
                        off.abs() * 100.0
                    ),
                    "pace",
                    &[("meanSeconds", mean), ("targetSeconds", t), ("shots", f64::from(stats.shots.count))],
                ));
            }
        }
        return;
    }
    for sec in sections {
        let Some(t) = shot_target(sec.average_shot_seconds).or(overall) else { continue };
        let inside: Vec<f64> = shots.iter().filter(|s| s.start >= sec.start && s.start < sec.end).map(|s| secs(s.end - s.start)).collect();
        if inside.len() < 3 {
            continue;
        }
        let mean = inside.iter().sum::<f64>() / inside.len() as f64;
        let off = (mean - t) / t;
        if off.abs() > 0.5 {
            let (word, advice) = if off > 0.0 {
                ("slower", "Slower than planned: look for shots that can lose a few seconds, or a cutaway that adds change.")
            } else {
                ("faster", "Faster than planned: look for a place to let a shot breathe, if the section is meant to feel calmer.")
            };
            out.push(finding(
                Kind::SectionPace,
                Severity::Look,
                sec.start,
                Some(sec.end),
                format!("“{}”: shots average {}, {word} than planned ({})", sec.name, len_text(mean), len_text(t)),
                format!(
                    "In this section ({} shots) the average shot is {} against a target of {}, {:.0}% off. {advice}",
                    inside.len(),
                    len_text(mean),
                    len_text(t),
                    off.abs() * 100.0
                ),
                "pace",
                &[("meanSeconds", mean), ("targetSeconds", t), ("shots", inside.len() as f64)],
            ));
        }
    }
}

fn cut_rules(stats: &SequenceStats, out: &mut Vec<Finding>) {
    let overall = stats.cuts.per_minute;
    let w = stats.cuts.window_seconds;
    if !(w.is_finite() && w > 0.0) || overall <= 0.0 {
        return;
    }
    for (i, &count) in stats.cuts.series.iter().enumerate() {
        let rate = f64::from(count) * 60.0 / w;
        if count >= 6 && rate >= 2.5 * overall {
            let start = (i as f64 * w * TICKS_PER_SECOND as f64) as i64;
            let end = ((i as f64 + 1.0) * w * TICKS_PER_SECOND as f64) as i64;
            out.push(finding(
                Kind::CutBurst,
                Severity::Look,
                start,
                Some(end),
                format!("Cuts come much faster at {}: {count} in {}", clock(start), len_text(w)),
                format!(
                    "That is about {rate:.0} cuts a minute here against {overall:.0} across the whole edit. A burst of quick cuts can build energy for a montage or a climax; if this is not that moment, it will feel rushed next to the rest."
                ),
                "cut density",
                &[("cuts", f64::from(count)), ("perMinute", rate), ("overallPerMinute", overall)],
            ));
        }
    }
}

fn narration_rules(stats: &SequenceStats, input: &SequenceInput, targets: &Targets, out: &mut Vec<Finding>) {
    let (Some(wpm), Some(target)) = (stats.narration.words_per_minute, pace_target(targets.words_per_minute)) else { return };
    if stats.narration.words < 20 {
        return;
    }
    let ratio = wpm / target;
    let (word, severity) = if ratio > 1.15 {
        ("fast", if ratio > 1.30 { Severity::Problem } else { Severity::Look })
    } else if ratio < 0.80 {
        ("slow", if ratio < 0.65 { Severity::Problem } else { Severity::Look })
    } else {
        return;
    };
    let start = input.narration.iter().filter(|n| n.duration > 0).map(|n| n.start.max(0)).min().unwrap_or(0);
    let advice = if word == "fast" {
        "Listeners need a beat to take in each idea, and a fast read leaves no room for the picture. Try trimming words or adding a pause between ideas."
    } else {
        "A slow read can drag and lose attention. Try tightening the script, or let the voice run a little quicker."
    };
    out.push(finding(
        Kind::NarrationPace,
        severity,
        start,
        None,
        format!("The narration is {word}: {wpm:.0} words a minute, against {target:.0}"),
        format!(
            "Across {} words the narration runs at {wpm:.0} words a minute, {:.0}% {} the target of {target:.0}. {advice}",
            stats.narration.words,
            (ratio - 1.0).abs() * 100.0,
            if ratio > 1.0 { "above" } else { "below" }
        ),
        "narration pace",
        &[("wordsPerMinute", wpm), ("targetWordsPerMinute", target), ("words", stats.narration.words as f64)],
    ));
}

fn split_rules(stats: &SequenceStats, out: &mut Vec<Finding>) {
    let s = &stats.split_cuts;
    if s.cuts_with_audio >= 8 && s.share < 0.10 {
        out.push(finding(
            Kind::FewSplitCuts,
            Severity::Look,
            0,
            None,
            format!("Only {} of {} cuts are J or L cuts", s.j_cuts.max(s.l_cuts), s.cuts_with_audio),
            "Nearly every cut changes the picture and the sound at the same moment. A J cut lets the next shot's sound start just before its picture; an L cut lets the last shot's sound run over the next picture. Overlapping them smooths the joins and makes the edit feel less mechanical.".to_string(),
            "J and L cuts",
            &[("cutsWithAudio", f64::from(s.cuts_with_audio)), ("share", s.share)],
        ));
    }
    let t = &stats.transitions;
    if t.count >= 6 && t.share_of_joints > 0.5 {
        out.push(finding(
            Kind::ManyDissolves,
            Severity::Look,
            0,
            None,
            format!("{} transitions, over half of the joins between shots", t.count),
            "More than half of the joins between shots use a transition. A plain cut says \"and then\"; a dissolve says time has passed or one thing becomes another. Used on every join, the dissolves stop meaning anything.".to_string(),
            "cut or dissolve",
            &[("transitions", f64::from(t.count)), ("shareOfJoints", t.share_of_joints)],
        ));
    }
}

fn audio_rules(a: &AudioStats, targets: &Targets, out: &mut Vec<Finding>) {
    if a.silence_share > 0.25 && a.duration_seconds > 20.0 {
        out.push(finding(
            Kind::MostlySilent,
            Severity::Look,
            0,
            None,
            format!("{:.0}% of the video is silent", f64::from(a.silence_share) * 100.0),
            "More than a quarter of the video has nothing playing. Silence can be powerful when it is chosen; if it is not, a low music bed or room tone under the quiet parts keeps the video from feeling empty.".to_string(),
            "room tone",
            &[("silenceShare", f64::from(a.silence_share))],
        ));
    }
    if let Some(lra) = a.loudness_range_lu.filter(|v| v.is_finite() && *v > 12.0) {
        out.push(finding(
            Kind::WideLoudness,
            Severity::Look,
            0,
            None,
            format!("Loudness swings by {lra:.0} LU"),
            "The quietest and loudest passages differ a lot (LU is the unit of loudness difference). Viewers will reach for the volume. Compressing the loud parts or lifting the quiet ones narrows the range.".to_string(),
            "loudness range",
            &[("loudnessRangeLu", lra)],
        ));
    }
    if let Some(lufs) = a.integrated_lufs.filter(|v| v.is_finite()) {
        let aim = within(Some(targets.loudness_lufs), -60.0, 0.0).unwrap_or(-14.0);
        let off = lufs - aim;
        if off.abs() > 3.0 {
            out.push(finding(
                Kind::LoudnessOffTarget,
                if off.abs() > 6.0 { Severity::Problem } else { Severity::Look },
                0,
                None,
                format!("Overall loudness is {lufs:.1} LUFS, {:.0} {} the target", off.abs(), if off > 0.0 { "above" } else { "below" }),
                format!("The target is {aim:.0} LUFS; many platforms aim for around −14. Too loud and it gets turned down, too quiet and it sounds weak next to other videos. Adjust the master gain or the loudest tracks."),
                "loudness",
                &[("integratedLufs", lufs), ("targetLufs", aim)],
            ));
        }
    }
    if let Some(peak) = a.true_peak_dbtp.filter(|v| v.is_finite() && *v > -1.0) {
        out.push(finding(
            Kind::PeakTooHigh,
            Severity::Problem,
            0,
            None,
            format!("The loudest peak is {peak:.1} dBTP"),
            "The signal gets within a hair of the ceiling (or past it), and small speakers or compression can turn that into audible crackle (clipping). Lower the master or add a limiter so peaks stay under about −1 dBTP.".to_string(),
            "headroom",
            &[("truePeakDbtp", peak)],
        ));
    }
}

/// Keeps at most [`MAX_PER_KIND`] of a kind (the rest summed up in one line), the worst first.
fn finish(mut all: Vec<Finding>) -> Vec<Finding> {
    all.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.start.cmp(&b.start)).then(a.kind.cmp(&b.kind)));
    let mut seen: BTreeMap<Kind, usize> = BTreeMap::new();
    let mut omitted: BTreeMap<Kind, (usize, Severity, i64)> = BTreeMap::new();
    let mut out = Vec::new();
    for f in all {
        let n = seen.entry(f.kind).or_insert(0);
        *n += 1;
        if *n <= MAX_PER_KIND {
            out.push(f);
        } else {
            let e = omitted.entry(f.kind).or_insert((0, f.severity, f.start));
            e.0 += 1;
        }
    }
    for (kind, (n, severity, start)) in omitted {
        out.push(finding(
            kind,
            severity,
            start,
            None,
            format!("…and {n} more of the same kind"),
            format!("{n} further findings of this kind are not listed; fixing the ones above usually shows the pattern."),
            "",
            &[("more", n as f64)],
        ));
        if let Some(last) = out.last_mut() {
            last.id = format!("{}@more", kind.name());
        }
    }
    out.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.start.cmp(&b.start)).then(a.kind.cmp(&b.kind)));
    out.truncate(MAX_FINDINGS);
    out
}

/// What is worth telling the editor about this edit, the worst first and then in time order. `audio`
/// is a measurement of the mix when there is one (the audio rules need it).
pub fn findings(input: &SequenceInput, targets: &Targets, audio: Option<&AudioStats>) -> Result<Vec<Finding>> {
    let stats = analyze_sequence(input)?;
    let shots = shot_spans(input)?;
    let mut out = Vec::new();
    gaps(input, &mut out);
    shot_rules(&shots, targets, &mut out);
    pace_rules(&stats, &shots, targets, &mut out);
    cut_rules(&stats, &mut out);
    narration_rules(&stats, input, targets, &mut out);
    split_rules(&stats, &mut out);
    if let Some(a) = audio {
        audio_rules(a, targets, &mut out);
    }
    Ok(finish(out))
}
