//! Format Packs: the data file that gives a kind of video its starting structure (roadmap D4).
//!
//! A pack is TOML: a name, a few notes (a default look preset, a title style), a table of numbers
//! the coach will use later, and sections of beats, each beat a share of the runtime with a purpose
//! and an intensity. Packs are data, never code, so a new kind of video needs no change to the
//! program. A pack read from a file is untrusted: [`FormatPack::parse`] checks every field and
//! refuses what does not add up.
//!
//! Turning a pack into a plan ([`FormatPack::instantiate`]) divides the video's length between the
//! beats with exact integer arithmetic, largest remainder first, so the beats always add up to the
//! length to the tick.

use std::collections::BTreeMap;

use filmcraft_time::Tick;
use serde::{Deserialize, Serialize};

use crate::model::{MAX_BEATS, MAX_NAME, MAX_NOTE, MAX_PURPOSE, MAX_SECTIONS, MAX_TARGET, Planner, clean};

/// The file format this program reads.
pub const PACK_VERSION: u32 = 1;
/// Largest pack file read, in bytes.
pub const MAX_PACK_BYTES: usize = 256 * 1024;
/// Most beats a pack may make (after repeats).
pub const MAX_PACK_BEATS: usize = 40;
const MAX_REPEAT: u32 = 10;
/// A gap or overlap between two beats up to this many tenths of a percent is tolerated.
const SLACK_PERMILLE: i64 = 5;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Minutes {
    pub min: f64,
    pub max: f64,
}

/// A beat that comes in several copies (the clue beats of a mystery).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Repeat {
    pub min: u32,
    pub max: u32,
    pub default: u32,
    /// The name of each copy; `{n}` becomes its number.
    #[serde(default)]
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BeatTemplate {
    pub name: String,
    #[serde(default)]
    pub purpose: String,
    /// Where it starts and ends, as percentages of the runtime.
    pub from: f64,
    pub to: f64,
    /// 0 to 10.
    pub intensity: f64,
    /// Set for a beat that ramps: the intensity at its end (or across its copies).
    #[serde(default)]
    pub intensity_end: Option<f64>,
    #[serde(default)]
    pub sound: String,
    #[serde(default)]
    pub repeat: Option<Repeat>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SectionTemplate {
    pub name: String,
    #[serde(default, rename = "beat")]
    pub beats: Vec<BeatTemplate>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FormatPack {
    pub version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub typical_minutes: Option<Minutes>,
    /// The name of a look preset that suits this kind of video.
    #[serde(default)]
    pub look_preset: String,
    #[serde(default)]
    pub title_style: String,
    /// Numbers for the coach (words per minute, shot length, …).
    #[serde(default)]
    pub targets: BTreeMap<String, f64>,
    #[serde(default, rename = "section")]
    pub sections: Vec<SectionTemplate>,
}

fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn permille(pct: f64) -> Option<i64> {
    (pct.is_finite() && (0.0..=100.0).contains(&pct)).then(|| (pct * 10.0).round() as i64)
}

/// `total` divided between `weights` with no rounding loss: each gets its share rounded down, then
/// the leftover ticks go one each to the largest remainders (earlier first on a tie). The parts
/// always add up to `total`.
pub fn split(total: i64, weights: &[u64]) -> Vec<i64> {
    let sum: u128 = weights.iter().map(|w| u128::from(*w)).sum();
    if weights.is_empty() || sum == 0 || total <= 0 {
        return vec![0; weights.len()];
    }
    let mut parts: Vec<(i64, u128, usize)> = weights
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let num = total as u128 * u128::from(*w);
            ((num / sum) as i64, num % sum, i)
        })
        .collect();
    let given: i64 = parts.iter().map(|p| p.0).sum();
    let mut left = total - given;
    let mut order: Vec<usize> = (0..parts.len()).collect();
    order.sort_by(|a, b| parts[*b].1.cmp(&parts[*a].1).then(a.cmp(b)));
    for i in order {
        if left <= 0 {
            break;
        }
        parts[i].0 += 1;
        left -= 1;
    }
    parts.sort_by_key(|p| p.2);
    parts.into_iter().map(|p| p.0).collect()
}

impl FormatPack {
    /// Read a pack from TOML, checked. The error says what is wrong in plain words.
    pub fn parse(text: &str) -> Result<FormatPack, String> {
        if text.len() > MAX_PACK_BYTES {
            return Err(format!("the pack is larger than {} KB", MAX_PACK_BYTES / 1024));
        }
        let pack: FormatPack = toml::from_str(text).map_err(|e| format!("not a valid pack file: {}", clean(e.message(), 200, false)))?;
        pack.check()?;
        Ok(pack)
    }

    /// Whether the pack makes sense.
    pub fn check(&self) -> Result<(), String> {
        if self.version != PACK_VERSION {
            return Err(format!("this pack is format version {}, and this program reads version {PACK_VERSION}", self.version));
        }
        if !id_ok(&self.id) {
            return Err("the pack id must be 1 to 40 characters: lower-case letters, digits, - and _".into());
        }
        let name_ok = |s: &str, max: usize| !s.trim().is_empty() && s.chars().count() <= max && !s.chars().any(char::is_control);
        if !name_ok(&self.name, MAX_NAME) {
            return Err("the pack needs a name of up to 120 characters".into());
        }
        for (what, text, max) in
            [("summary", &self.summary, MAX_PURPOSE), ("look preset", &self.look_preset, MAX_NAME), ("title style", &self.title_style, MAX_NOTE)]
        {
            if text.chars().count() > max {
                return Err(format!("the {what} is longer than {max} characters"));
            }
        }
        if let Some(m) = &self.typical_minutes
            && !(m.min.is_finite() && m.max.is_finite() && m.min >= 1.0 && m.min <= m.max && m.max <= 600.0)
        {
            return Err("typical_minutes must satisfy 1 <= min <= max <= 600".into());
        }
        if self.targets.len() > 20 || self.targets.iter().any(|(k, v)| !v.is_finite() || k.is_empty() || k.chars().count() > 60) {
            return Err("targets: at most 20 named numbers".into());
        }
        if self.sections.is_empty() || self.sections.len() > MAX_SECTIONS.min(12) {
            return Err("a pack has 1 to 12 sections".into());
        }
        let mut count = 0usize;
        let mut edge = 0i64;
        let mut first = true;
        let mut names: Vec<String> = Vec::new();
        for s in &self.sections {
            if !name_ok(&s.name, MAX_NAME) {
                return Err("every section needs a name of up to 120 characters".into());
            }
            if s.beats.is_empty() {
                return Err(format!("section \"{}\" has no beats", s.name));
            }
            for b in &s.beats {
                if !name_ok(&b.name, MAX_NAME) {
                    return Err("every beat needs a name of up to 120 characters".into());
                }
                if names.iter().any(|n| n.eq_ignore_ascii_case(&b.name)) {
                    return Err(format!("two beats are called \"{}\"", b.name));
                }
                names.push(b.name.clone());
                if b.purpose.chars().count() > MAX_PURPOSE || b.sound.chars().count() > MAX_NOTE {
                    return Err(format!("beat \"{}\": purpose or sound note too long", b.name));
                }
                let (from, to) = (permille(b.from), permille(b.to));
                let (Some(from), Some(to)) = (from, to) else { return Err(format!("beat \"{}\": from and to are percentages between 0 and 100", b.name)) };
                if to <= from {
                    return Err(format!("beat \"{}\" must end after it starts", b.name));
                }
                if first {
                    if from > SLACK_PERMILLE {
                        return Err(format!("the first beat \"{}\" must start at 0", b.name));
                    }
                    first = false;
                } else if (from - edge).abs() > SLACK_PERMILLE {
                    return Err(format!("beat \"{}\" does not start where the one before it ends", b.name));
                }
                edge = to;
                if !(b.intensity.is_finite() && (0.0..=10.0).contains(&b.intensity))
                    || b.intensity_end.is_some_and(|e| !(e.is_finite() && (0.0..=10.0).contains(&e)))
                {
                    return Err(format!("beat \"{}\": intensity is 0 to 10", b.name));
                }
                match &b.repeat {
                    Some(r) => {
                        if r.min < 1 || r.max > MAX_REPEAT || r.min > r.max || r.default < r.min || r.default > r.max {
                            return Err(format!("beat \"{}\": repeat needs 1 <= min <= default <= max <= {MAX_REPEAT}", b.name));
                        }
                        if r.name.chars().count() > MAX_NAME {
                            return Err(format!("beat \"{}\": repeat name too long", b.name));
                        }
                        count += r.max as usize;
                    }
                    None => count += 1,
                }
            }
        }
        if edge < 1000 - SLACK_PERMILLE {
            return Err("the beats must reach 100% of the runtime".into());
        }
        if count > MAX_PACK_BEATS || count > MAX_BEATS {
            return Err(format!("a pack makes at most {MAX_PACK_BEATS} beats"));
        }
        Ok(())
    }

    /// The most clue-like repeat range in the pack: (min, max, default), when any beat repeats.
    pub fn repeat_range(&self) -> Option<(u32, u32, u32)> {
        self.sections.iter().flat_map(|s| s.beats.iter()).find_map(|b| b.repeat.as_ref().map(|r| (r.min, r.max, r.default)))
    }

    /// The plan this pack makes for a video of `target`. `repeats` is how many copies of a repeating
    /// beat (clamped into each beat's own range; None is its default). The beats' lengths add up to
    /// `target` exactly.
    pub fn instantiate(&self, target: Tick, repeats: Option<u32>) -> Result<Planner, String> {
        self.check()?;
        if target.0 < 1 || target > MAX_TARGET {
            return Err("the video length must be between a tick and 12 hours".into());
        }
        // 1. each beat's share, from the boundaries (so gaps and overlaps within the slack vanish)
        let mut bounds: Vec<(i64, &SectionTemplate, &BeatTemplate)> = Vec::new();
        for s in &self.sections {
            for b in &s.beats {
                bounds.push((permille(b.to).unwrap_or(1000), s, b));
            }
        }
        let n = bounds.len();
        let mut edge = 0i64;
        let mut weights: Vec<u64> = Vec::with_capacity(n);
        for (i, (to, _, _)) in bounds.iter().enumerate() {
            let to = if i + 1 == n { 1000 } else { (*to).clamp(edge + 1, 1000 - (n - i - 1) as i64) };
            weights.push((to - edge).max(1) as u64);
            edge = to;
        }
        let beat_ticks = split(target.0, &weights);
        // 2. build sections, beats and (for repeats) their copies
        let mut plan = Planner::new("", target);
        plan.pack = Some(self.id.clone());
        let mut at = 0usize;
        for s in &self.sections {
            let sid = plan.new_id();
            let mut beats = Vec::new();
            for b in &s.beats {
                let ticks = beat_ticks.get(at).copied().unwrap_or(0);
                at += 1;
                let tenth = |v: f64| (v * 10.0).round().clamp(0.0, 100.0) as u8;
                let start = b.intensity;
                let end = b.intensity_end.unwrap_or(b.intensity);
                let copies = match &b.repeat {
                    Some(r) => repeats.unwrap_or(r.default).clamp(r.min, r.max),
                    None => 1,
                };
                let each = split(ticks, &vec![1; copies as usize]);
                for (j, t) in each.iter().enumerate() {
                    let (name, intensity) = match &b.repeat {
                        Some(r) => {
                            let k = copies as f64;
                            let v = if copies <= 1 { start } else { start + (end - start) * j as f64 / (k - 1.0) };
                            let label = if r.name.is_empty() { format!("{} {}", b.name, j + 1) } else { r.name.replace("{n}", &(j + 1).to_string()) };
                            (label, tenth(v))
                        }
                        // a beat that ramps without repeating is one beat: its middle value
                        None => (b.name.clone(), tenth((start + end) / 2.0)),
                    };
                    beats.push(crate::model::Beat {
                        id: plan.new_id(),
                        name: clean(&name, MAX_NAME, false),
                        purpose: clean(&b.purpose, MAX_PURPOSE, true),
                        intensity,
                        target: Tick(*t),
                        sound_note: clean(&b.sound, MAX_NOTE, true),
                        ..Default::default()
                    });
                }
            }
            plan.sections.push(crate::model::Section { id: sid, name: clean(&s.name, MAX_NAME, false), beats });
        }
        plan.check()?;
        Ok(plan)
    }
}

/// The packs that ship with the program.
pub fn builtin_packs() -> Vec<FormatPack> {
    [include_str!("../packs/mystery.toml"), include_str!("../packs/explainer.toml")].iter().filter_map(|t| FormatPack::parse(t).ok()).collect()
}

/// The text of the shipped packs, for the tests that check they parse (and for showing as examples).
pub const BUILTIN_SOURCES: [(&str, &str); 2] = [("mystery", include_str!("../packs/mystery.toml")), ("explainer", include_str!("../packs/explainer.toml"))];
