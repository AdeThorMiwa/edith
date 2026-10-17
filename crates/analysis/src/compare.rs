//! Comparing two fingerprints ([`compare`] → [`Comparison`]): yours against a film you admire, row by
//! row, each with a verdict and a sentence in plain words.
//!
//! "Yours" is a fingerprint of your own edit made the same way (`fingerprint` over its cuts, and over
//! its pictures and mix when those have been measured), so both sides are in the same terms. A row
//! appears only when both sides have the number: an edit whose mix has not been measured simply has no
//! loudness rows, and the comparison says which parts it could not compare (`missing`).
//!
//! A verdict is [`Verdict::Close`] when the two are within a tolerance chosen for what is measured (a
//! fifth for lengths, 2 LU for loudness, a few points for shares), otherwise higher or lower.

use serde::{Deserialize, Serialize};

use crate::fingerprint::{Fingerprint, PaceArc};

/// Bumped whenever a row, a tolerance or a wording could change.
pub const COMPARE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Close,
    /// Yours is more than the reference.
    Higher,
    /// Yours is less than the reference.
    Lower,
    /// A category that differs (the pace arc).
    Different,
}

/// One thing compared.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    /// `shots`, `rhythm`, `picture`, `motion` or `sound`.
    pub group: String,
    /// A stable name for the row (`meanShot`, `loudness`, …).
    pub metric: String,
    pub label: String,
    pub unit: String,
    pub yours: Option<f64>,
    pub theirs: Option<f64>,
    /// The two numbers as they are written in the sentence (`5.4 s`, `-20.0`, `30%`), for drawing.
    pub yours_text: String,
    pub theirs_text: String,
    pub verdict: Verdict,
    /// The row in a sentence.
    pub words: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub reference: String,
    pub rows: Vec<Row>,
    pub close: u32,
    pub differ: u32,
    /// Which parts could not be compared because one side has no such numbers.
    pub missing: Vec<String>,
}

/// How a number is read, which decides how close is close and how the sentence goes.
#[derive(Clone, Copy)]
enum Kind {
    /// A length of time in seconds: close within a fifth, said as "x times longer".
    Length,
    /// A loudness difference in LU: close within `tol`.
    Loudness,
    /// A share 0..1, said in per cent: close within `tol`.
    Share,
    /// A plain level (brightness, contrast…): close within `tol`.
    Level,
    /// Kelvin: close within `tol`.
    Kelvin,
    /// A rate (cuts a minute, energy): close within a fifth (or `tol` as a fraction).
    Rate,
}

fn secs(v: f64) -> String {
    if v < 10.0 { format!("{v:.1} s") } else { format!("{v:.0} s") }
}

fn row(group: &str, metric: &str, label: &str, unit: &str, kind: Kind, tol: f64, yours: Option<f64>, theirs: Option<f64>) -> Option<Row> {
    let (y, t) = (yours.filter(|v| v.is_finite())?, theirs.filter(|v| v.is_finite())?);
    let (verdict, words) = match kind {
        Kind::Length | Kind::Rate => {
            let ratio = if t.abs() > f64::EPSILON {
                y / t
            } else if y.abs() <= f64::EPSILON {
                1.0
            } else {
                f64::INFINITY
            };
            let close = (ratio - 1.0).abs() <= tol;
            let verdict = if close {
                Verdict::Close
            } else if ratio > 1.0 {
                Verdict::Higher
            } else {
                Verdict::Lower
            };
            let fmt = |v: f64| if matches!(kind, Kind::Length) { secs(v) } else { format!("{v:.1}") };
            // "as long" for lengths, "as fast" for rates
            let (more, less) = if matches!(kind, Kind::Length) { ("as long", "shorter") } else { ("as fast", "slower") };
            let how = if close {
                "close".to_string()
            } else if (0.45..=0.55).contains(&ratio) {
                format!("about half {more}")
            } else if (1.8..=2.2).contains(&ratio) {
                format!("about twice {more}")
            } else if ratio.is_finite() && ratio > 1.0 {
                format!("{ratio:.1} times {more}")
            } else if ratio.is_finite() && ratio > 0.0 {
                format!("{:.1} times {less}", 1.0 / ratio)
            } else {
                "very different".to_string()
            };
            (verdict, format!("{label}: yours {}{unit}, theirs {}{unit}: {how}.", fmt(y), fmt(t)))
        }
        Kind::Loudness | Kind::Level | Kind::Kelvin | Kind::Share => {
            let d = y - t;
            let verdict = if d.abs() <= tol {
                Verdict::Close
            } else if d > 0.0 {
                Verdict::Higher
            } else {
                Verdict::Lower
            };
            let (fmt, delta): (Box<dyn Fn(f64) -> String>, String) = match kind {
                Kind::Share => (Box::new(|v| format!("{:.0}%", v * 100.0)), format!("{:.0} points", d.abs() * 100.0)),
                Kind::Loudness => (Box::new(|v| format!("{v:.1}")), format!("{:.1} LU", d.abs())),
                Kind::Kelvin => (Box::new(|v| format!("{v:.0} K")), format!("{:.0} K", d.abs())),
                _ => (Box::new(|v| format!("{v:.2}")), format!("{:.2}", d.abs())),
            };
            let dir = match (kind, d > 0.0) {
                (Kind::Loudness, true) => "louder",
                (Kind::Loudness, false) => "quieter",
                (Kind::Kelvin, true) => "cooler",
                (Kind::Kelvin, false) => "warmer",
                (_, true) => "higher",
                (_, false) => "lower",
            };
            let u = if matches!(kind, Kind::Share | Kind::Kelvin) || unit.is_empty() { String::new() } else { format!(" {unit}") };
            let said = if matches!(verdict, Verdict::Close) { "close".to_string() } else { format!("yours is {delta} {dir}") };
            (verdict, format!("{label}: yours {}{u}, theirs {}{u}: {said}.", fmt(y), fmt(t)))
        }
    };
    let text = |v: f64| match kind {
        Kind::Length => secs(v),
        Kind::Rate => format!("{v:.1}"),
        Kind::Loudness => format!("{v:.1}"),
        Kind::Share => format!("{:.0}%", v * 100.0),
        Kind::Kelvin => format!("{v:.0} K"),
        Kind::Level => format!("{v:.2}"),
    };
    Some(Row {
        group: group.into(),
        metric: metric.into(),
        label: label.into(),
        unit: unit.into(),
        yours: Some(y),
        theirs: Some(t),
        yours_text: text(y),
        theirs_text: text(t),
        verdict,
        words,
    })
}

fn arc_name(a: PaceArc) -> &'static str {
    match a {
        PaceArc::Quickening => "quickening",
        PaceArc::Steady => "steady",
        PaceArc::Slowing => "slowing",
    }
}

/// Yours against theirs.
pub fn compare(yours: &Fingerprint, theirs: &Fingerprint) -> Comparison {
    let mut rows: Vec<Row> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let (ys, ts) = (&yours.shots, &theirs.shots);
    rows.extend(row("shots", "meanShot", "Average shot", "", Kind::Length, 0.2, Some(ys.mean_seconds), Some(ts.mean_seconds)));
    rows.extend(row("shots", "medianShot", "Median shot", "", Kind::Length, 0.2, Some(ys.median_seconds), Some(ts.median_seconds)));
    rows.extend(row("shots", "shortShots", "Short end (10th percentile)", "", Kind::Length, 0.25, Some(ys.p10_seconds), Some(ts.p10_seconds)));
    rows.extend(row("shots", "longShots", "Long end (90th percentile)", "", Kind::Length, 0.25, Some(ys.p90_seconds), Some(ts.p90_seconds)));
    rows.extend(row("shots", "cutsPerMinute", "Cuts a minute", "", Kind::Rate, 0.2, Some(ys.cuts_per_minute), Some(ts.cuts_per_minute)));
    rows.extend(row(
        "rhythm",
        "openingShot",
        "Shots in the opening third",
        "",
        Kind::Length,
        0.25,
        yours.rhythm.opening_mean_seconds,
        theirs.rhythm.opening_mean_seconds,
    ));
    rows.extend(row(
        "rhythm",
        "closingShot",
        "Shots in the closing third",
        "",
        Kind::Length,
        0.25,
        yours.rhythm.closing_mean_seconds,
        theirs.rhythm.closing_mean_seconds,
    ));
    let (ya, ta) = (yours.rhythm.arc, theirs.rhythm.arc);
    rows.push(Row {
        group: "rhythm".into(),
        metric: "arc".into(),
        label: "How the pace moves".into(),
        unit: String::new(),
        yours: None,
        theirs: None,
        yours_text: arc_name(ya).to_string(),
        theirs_text: arc_name(ta).to_string(),
        verdict: if ya == ta { Verdict::Close } else { Verdict::Different },
        words: if ya == ta {
            format!("How the pace moves: both {}.", arc_name(ya))
        } else {
            format!("How the pace moves: yours is {}; theirs is {}.", arc_name(ya), arc_name(ta))
        },
    });
    match (&yours.picture, &theirs.picture) {
        (Some(y), Some(t)) => {
            rows.extend(row("picture", "brightness", "Brightness", "", Kind::Level, 0.08, Some(f64::from(y.luma_mean)), Some(f64::from(t.luma_mean))));
            rows.extend(row("picture", "contrast", "Contrast", "", Kind::Level, 0.08, Some(f64::from(y.contrast)), Some(f64::from(t.contrast))));
            rows.extend(row("picture", "saturation", "Colour saturation", "", Kind::Level, 0.08, Some(f64::from(y.saturation)), Some(f64::from(t.saturation))));
            rows.extend(row(
                "picture",
                "warmth",
                "Colour warmth",
                "",
                Kind::Kelvin,
                600.0,
                y.colour_temperature.map(f64::from),
                t.colour_temperature.map(f64::from),
            ));
            rows.extend(row("picture", "grain", "Grain", "", Kind::Level, 0.01, Some(f64::from(y.grain)), Some(f64::from(t.grain))));
            rows.extend(row(
                "picture",
                "aspect",
                "Picture shape (width over height)",
                "",
                Kind::Level,
                0.05,
                Some(f64::from(y.active_aspect)),
                Some(f64::from(t.active_aspect)),
            ));
        }
        _ => {
            missing.push(if yours.picture.is_none() { "the picture (measure your shots first)".into() } else { "the picture (the reference has none)".into() })
        }
    }
    match (&yours.motion, &theirs.motion) {
        (Some(y), Some(t)) => {
            rows.extend(row("motion", "motion", "Movement between frames", "", Kind::Rate, 0.4, Some(f64::from(y.energy)), Some(f64::from(t.energy))));
            rows.extend(row(
                "motion",
                "staticCamera",
                "Share of the film with a still camera",
                "",
                Kind::Share,
                0.1,
                y.camera.get("static").map(|v| f64::from(*v)),
                t.camera.get("static").map(|v| f64::from(*v)),
            ));
        }
        _ => missing.push("camera movement".into()),
    }
    match (&yours.sound, &theirs.sound) {
        (Some(y), Some(t)) => {
            rows.extend(row("sound", "loudness", "Loudness (LUFS)", "LUFS", Kind::Loudness, 2.0, y.integrated_lufs, t.integrated_lufs));
            rows.extend(row("sound", "loudnessRange", "Loudness range (LU)", "LU", Kind::Loudness, 3.0, y.loudness_range_lu, t.loudness_range_lu));
            rows.extend(row("sound", "truePeak", "Loudest peak (dBTP)", "dBTP", Kind::Loudness, 2.0, y.true_peak_dbtp, t.true_peak_dbtp));
            rows.extend(row(
                "sound",
                "silence",
                "Time that is silent",
                "",
                Kind::Share,
                0.08,
                Some(f64::from(y.silence_share)),
                Some(f64::from(t.silence_share)),
            ));
            if y.speech_share + y.music_share + y.ambience_share > 0.0 && t.speech_share + t.music_share + t.ambience_share > 0.0 {
                rows.extend(row(
                    "sound",
                    "speech",
                    "Time that sounds like speech",
                    "",
                    Kind::Share,
                    0.1,
                    Some(f64::from(y.speech_share)),
                    Some(f64::from(t.speech_share)),
                ));
                rows.extend(row(
                    "sound",
                    "music",
                    "Time that sounds like music",
                    "",
                    Kind::Share,
                    0.1,
                    Some(f64::from(y.music_share)),
                    Some(f64::from(t.music_share)),
                ));
            }
        }
        _ => missing.push(if yours.sound.is_none() { "the sound (measure your mix first)".into() } else { "the sound (the reference has none)".into() }),
    }
    let close = rows.iter().filter(|r| r.verdict == Verdict::Close).count() as u32;
    let differ = rows.len() as u32 - close;
    Comparison { reference: theirs.name.clone(), rows, close, differ, missing }
}
