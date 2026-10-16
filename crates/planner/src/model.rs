//! The planner's data: a video as sections, beats and shots.
//!
//! Every length is a [`Tick`] and every sum is exact integer arithmetic. Everything here is also
//! read back from project files and imports, so every limit is checked by [`Planner::check`] and
//! enforced by [`Planner::sanitized`], which cuts what is too long and drops what does not make
//! sense instead of failing.

use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde::{Deserialize, Serialize};

pub const MAX_SECTIONS: usize = 40;
pub const MAX_BEATS: usize = 400;
pub const MAX_SHOTS_PER_BEAT: usize = 40;
pub const MAX_TITLE: usize = 200;
pub const MAX_NAME: usize = 120;
pub const MAX_PURPOSE: usize = 500;
pub const MAX_NOTE: usize = 500;
/// Longest narration of one beat, in characters.
pub const MAX_NARRATION: usize = 20_000;
/// Longest video planned: twelve hours.
pub const MAX_TARGET: Tick = Tick(12 * 3600 * TICKS_PER_SECOND);
/// The highest intensity, in tenths (10.0).
pub const MAX_INTENSITY: u8 = 100;

/// How far a beat has got.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// Just a purpose.
    #[default]
    Idea,
    /// Narration is written.
    Drafted,
    /// Pictures and sound are chosen.
    Sourced,
    /// It is on the timeline.
    Cut,
    Done,
}

impl Status {
    pub const ALL: [Status; 5] = [Status::Idea, Status::Drafted, Status::Sourced, Status::Cut, Status::Done];
    pub fn id(self) -> &'static str {
        match self {
            Status::Idea => "idea",
            Status::Drafted => "drafted",
            Status::Sourced => "sourced",
            Status::Cut => "cut",
            Status::Done => "done",
        }
    }
    pub fn from_id(s: &str) -> Option<Status> {
        Status::ALL.into_iter().find(|v| v.id().eq_ignore_ascii_case(s.trim()))
    }
}

/// Where a beat sits on the timeline, and the marker `buildSkeleton` made for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub start: Tick,
    pub duration: Tick,
    #[serde(default)]
    pub marker: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Shot {
    pub id: u64,
    /// What the picture should show.
    pub intent: String,
    pub shot_type: String,
    pub camera_move: String,
    pub target: Tick,
    /// A project item chosen for it.
    pub source: Option<u64>,
    /// The placeholder clip `buildSkeleton` put on the timeline.
    pub clip: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Beat {
    pub id: u64,
    pub name: String,
    /// What changes here?
    pub purpose: String,
    /// 0 to 10 in tenths (55 is 5.5).
    pub intensity: u8,
    pub target: Tick,
    pub narration: String,
    pub visual_intent: String,
    pub sound_note: String,
    pub status: Status,
    pub link: Option<Link>,
    pub shots: Vec<Shot>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Section {
    pub id: u64,
    pub name: String,
    pub beats: Vec<Beat>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Planner {
    pub title: String,
    /// The Format Pack it started from.
    pub pack: Option<String>,
    /// How long the whole video should be.
    pub target: Tick,
    /// The next id to hand out; ids are never reused.
    pub next_id: u64,
    pub sections: Vec<Section>,
}

/// `s` without control characters (newlines and tabs kept when `multiline`), cut to `max`
/// characters.
pub fn clean(s: &str, max: usize, multiline: bool) -> String {
    s.chars().filter(|c| !c.is_control() || (multiline && matches!(c, '\n' | '\t'))).take(max).collect()
}

/// A [`Tick`] length kept within 0 and `max`.
fn span(t: Tick, max: Tick) -> Tick {
    Tick(t.0.clamp(0, max.0))
}

impl Planner {
    pub fn new(title: &str, target: Tick) -> Planner {
        Planner { title: clean(title, MAX_TITLE, false), pack: None, target: span(target, MAX_TARGET), next_id: 1, sections: Vec::new() }
    }

    /// A fresh id.
    pub fn new_id(&mut self) -> u64 {
        let id = self.next_id.max(1);
        self.next_id = id.saturating_add(1);
        id
    }

    pub fn beat_count(&self) -> usize {
        self.sections.iter().map(|s| s.beats.len()).sum()
    }

    pub fn beats(&self) -> impl Iterator<Item = &Beat> {
        self.sections.iter().flat_map(|s| s.beats.iter())
    }

    /// The sum of the beats' lengths (saturating).
    pub fn planned(&self) -> Tick {
        Tick(self.beats().fold(0i64, |a, b| a.saturating_add(b.target.0)))
    }

    /// Section index and beat index of beat `id`.
    pub fn find_beat(&self, id: u64) -> Option<(usize, usize)> {
        self.sections.iter().enumerate().find_map(|(si, s)| s.beats.iter().position(|b| b.id == id).map(|bi| (si, bi)))
    }

    pub fn section_index(&self, id: u64) -> Option<usize> {
        self.sections.iter().position(|s| s.id == id)
    }

    pub fn beat_mut(&mut self, id: u64) -> Option<&mut Beat> {
        let (s, b) = self.find_beat(id)?;
        self.sections.get_mut(s)?.beats.get_mut(b)
    }

    pub fn beat_ref(&self, id: u64) -> Option<&Beat> {
        let (s, b) = self.find_beat(id)?;
        self.sections.get(s)?.beats.get(b)
    }

    /// Beat index, section index and shot index of shot `id`.
    pub fn find_shot(&self, id: u64) -> Option<(usize, usize, usize)> {
        self.sections
            .iter()
            .enumerate()
            .find_map(|(si, s)| s.beats.iter().enumerate().find_map(|(bi, b)| b.shots.iter().position(|x| x.id == id).map(|xi| (si, bi, xi))))
    }

    fn all_ids(&self) -> Vec<u64> {
        let mut v = Vec::new();
        for s in &self.sections {
            v.push(s.id);
            for b in &s.beats {
                v.push(b.id);
                v.extend(b.shots.iter().map(|x| x.id));
            }
        }
        v
    }

    /// Whether this planner is within every limit; the reason when not.
    pub fn check(&self) -> Result<(), String> {
        if self.title.chars().count() > MAX_TITLE {
            return Err(format!("the title is longer than {MAX_TITLE} characters"));
        }
        if !(0..=MAX_TARGET.0).contains(&self.target.0) {
            return Err("the video length is out of range (0 to 12 hours)".into());
        }
        if self.sections.len() > MAX_SECTIONS {
            return Err(format!("at most {MAX_SECTIONS} sections"));
        }
        if self.beat_count() > MAX_BEATS {
            return Err(format!("at most {MAX_BEATS} beats"));
        }
        let ids = self.all_ids();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        if sorted.windows(2).any(|w| w[0] == w[1]) || ids.contains(&0) {
            return Err("ids must be unique and not zero".into());
        }
        if ids.iter().any(|i| *i >= self.next_id) {
            return Err("an id is not below the next id".into());
        }
        for s in &self.sections {
            if s.name.chars().count() > MAX_NAME {
                return Err(format!("a section name is longer than {MAX_NAME} characters"));
            }
            for b in &s.beats {
                let text = |t: &str, max: usize, what: &str| {
                    if t.chars().count() > max { Err(format!("a beat's {what} is longer than {max} characters")) } else { Ok(()) }
                };
                text(&b.name, MAX_NAME, "name")?;
                text(&b.purpose, MAX_PURPOSE, "purpose")?;
                text(&b.narration, MAX_NARRATION, "narration")?;
                text(&b.visual_intent, MAX_PURPOSE, "visual intent")?;
                text(&b.sound_note, MAX_NOTE, "sound note")?;
                if b.intensity > MAX_INTENSITY {
                    return Err("an intensity is above 10".into());
                }
                if !(0..=MAX_TARGET.0).contains(&b.target.0) {
                    return Err("a beat length is out of range".into());
                }
                if let Some(l) = b.link
                    && (l.start.0 < 0 || l.duration.0 < 0 || l.start.0.checked_add(l.duration.0).is_none())
                {
                    return Err("a beat's timeline link is out of range".into());
                }
                if b.shots.len() > MAX_SHOTS_PER_BEAT {
                    return Err(format!("at most {MAX_SHOTS_PER_BEAT} shots per beat"));
                }
                for x in &b.shots {
                    text(&x.intent, MAX_PURPOSE, "shot intent")?;
                    text(&x.shot_type, MAX_NAME, "shot type")?;
                    text(&x.camera_move, MAX_NAME, "camera move")?;
                    if !(0..=MAX_TARGET.0).contains(&x.target.0) {
                        return Err("a shot length is out of range".into());
                    }
                }
            }
        }
        Ok(())
    }

    /// This planner made to fit every limit: text cut and cleaned, numbers clamped, extra sections,
    /// beats and shots dropped, ids made unique and the id counter put right. For data read from a
    /// project or an import.
    pub fn sanitized(mut self) -> Planner {
        self.title = clean(&self.title, MAX_TITLE, false);
        self.pack = self.pack.take().map(|p| clean(&p, MAX_NAME, false).trim().to_string()).filter(|p| !p.is_empty());
        self.target = span(self.target, MAX_TARGET);
        self.sections.truncate(MAX_SECTIONS);
        let mut left = MAX_BEATS;
        let mut seen: Vec<u64> = Vec::new();
        let mut max_id = 0u64;
        let keep = |id: u64, seen: &mut Vec<u64>| -> bool {
            if id == 0 || seen.contains(&id) {
                return false;
            }
            seen.push(id);
            true
        };
        let mut next = self.next_id.max(1);
        let fresh = |seen: &mut Vec<u64>, next: &mut u64| -> u64 {
            while seen.contains(next) || *next == 0 {
                *next = next.saturating_add(1);
            }
            let id = *next;
            seen.push(id);
            *next = next.saturating_add(1);
            id
        };
        for s in &mut self.sections {
            if !keep(s.id, &mut seen) {
                s.id = fresh(&mut seen, &mut next);
            }
            max_id = max_id.max(s.id);
            s.name = clean(&s.name, MAX_NAME, false);
            s.beats.truncate(left);
            left -= s.beats.len();
            for b in &mut s.beats {
                if !keep(b.id, &mut seen) {
                    b.id = fresh(&mut seen, &mut next);
                }
                max_id = max_id.max(b.id);
                b.name = clean(&b.name, MAX_NAME, false);
                b.purpose = clean(&b.purpose, MAX_PURPOSE, true);
                b.narration = clean(&b.narration, MAX_NARRATION, true);
                b.visual_intent = clean(&b.visual_intent, MAX_PURPOSE, true);
                b.sound_note = clean(&b.sound_note, MAX_NOTE, true);
                b.intensity = b.intensity.min(MAX_INTENSITY);
                b.target = span(b.target, MAX_TARGET);
                b.link = b.link.and_then(|l| {
                    let ok = l.start.0 >= 0 && l.duration.0 >= 0 && l.start.0.checked_add(l.duration.0).is_some();
                    ok.then_some(l)
                });
                b.shots.truncate(MAX_SHOTS_PER_BEAT);
                for x in &mut b.shots {
                    if !keep(x.id, &mut seen) {
                        x.id = fresh(&mut seen, &mut next);
                    }
                    max_id = max_id.max(x.id);
                    x.intent = clean(&x.intent, MAX_PURPOSE, true);
                    x.shot_type = clean(&x.shot_type, MAX_NAME, false);
                    x.camera_move = clean(&x.camera_move, MAX_NAME, false);
                    x.target = span(x.target, MAX_TARGET);
                }
            }
        }
        self.next_id = self.next_id.max(max_id.saturating_add(1)).max(next).max(1);
        self
    }
}
