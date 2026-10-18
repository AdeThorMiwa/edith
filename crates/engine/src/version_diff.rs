//! What a version changes against main, in plain words (fork roadmap F14.2): clips added, removed,
//! moved, trimmed, retimed, switched on or off, effects added or adjusted, markers, transitions,
//! tracks, items in the project, and the plan. Pure comparison of two snapshots of the project.
//!
//! A sequence both snapshots share (the same `Arc`) is not looked into: it did not change. Names
//! listed in a sentence are cut to three. Nothing here reads anything but the two projects.

use std::collections::{BTreeMap, BTreeSet};

use filmcraft_project::{ClipId, ItemKind, Project, Sequence, TrackItem, TrackKind};
use serde::Serialize;

/// Names shown in one sentence.
const NAMES_SHOWN: usize = 3;
/// Sentences kept.
const MAX_LINES: usize = 60;

/// One kind of change in one place: what, how many, which.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Change {
    pub kind: &'static str,
    pub scope: String,
    pub count: usize,
    pub names: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Diff {
    pub changes: Vec<Change>,
    /// The same, as sentences.
    pub lines: Vec<String>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    fn push(&mut self, kind: &'static str, scope: &str, mut names: Vec<String>, count: usize) {
        if count == 0 {
            return;
        }
        names.retain(|n| !n.trim().is_empty());
        names.dedup();
        let shown: Vec<String> = names.iter().take(NAMES_SHOWN).cloned().collect();
        let more = names.len().saturating_sub(shown.len());
        let list = if shown.is_empty() {
            String::new()
        } else {
            format!(": {}{}", shown.join(", "), if more > 0 { format!(" and {more} more") } else { String::new() })
        };
        let s = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
        let place = if scope.is_empty() { String::new() } else { format!(" in “{scope}”") };
        let sentence = match kind {
            "clipsAdded" => format!("Adds {}{place}{list}", s(count, "clip", "clips")),
            "clipsRemoved" => format!("Removes {}{place}{list}", s(count, "clip", "clips")),
            "clipsMoved" => format!("Moves {}{place}{list}", s(count, "clip", "clips")),
            "clipsTrimmed" => format!("Trims {}{place}{list}", s(count, "clip", "clips")),
            "clipsRetimed" => format!("Changes the speed of {}{place}{list}", s(count, "clip", "clips")),
            "clipsSwitched" => format!("Switches {} on or off{place}{list}", s(count, "clip", "clips")),
            "effectsAdded" => format!("Adds effects to {}{place}{list}", s(count, "clip", "clips")),
            "effectsRemoved" => format!("Removes effects from {}{place}{list}", s(count, "clip", "clips")),
            "effectsAdjusted" => format!("Adjusts effects on {}{place}{list}", s(count, "clip", "clips")),
            "soundChanged" => format!("Changes the sound of {}{place}{list}", s(count, "clip", "clips")),
            "clipsEdited" => format!("Edits {}{place}{list}", s(count, "clip", "clips")),
            "markersAdded" => format!("Adds {}{place}{list}", s(count, "marker", "markers")),
            "markersRemoved" => format!("Removes {}{place}{list}", s(count, "marker", "markers")),
            "markersChanged" => format!("Changes {}{place}{list}", s(count, "marker", "markers")),
            "transitionsAdded" => format!("Adds {}{place}", s(count, "transition", "transitions")),
            "transitionsRemoved" => format!("Removes {}{place}", s(count, "transition", "transitions")),
            "tracksChanged" => format!("Adds or removes {}{place}", s(count, "track", "tracks")),
            "sequenceSettings" => format!("Changes the settings{place}"),
            "itemsAdded" => format!("Adds {} to the project{list}", s(count, "item", "items")),
            "itemsRemoved" => format!("Removes {} from the project{list}", s(count, "item", "items")),
            "itemsRenamed" => format!("Renames {} in the project{list}", s(count, "item", "items")),
            "notes" => "Changes the project notes".to_string(),
            "planBeats" => format!("Changes the plan: {}{list}", s(count, "beat", "beats")),
            "planOther" => "Changes the plan".to_string(),
            "projectSettings" => "Changes the project settings".to_string(),
            "otherData" => "Changes other project data".to_string(),
            _ => format!("Changes {count}"),
        };
        self.lines.push(sentence);
        self.changes.push(Change { kind, scope: scope.to_string(), count, names });
    }
}

/// A clip with the name of the track it is on.
struct Placed<'a> {
    track: String,
    clip: &'a TrackItem,
}

fn clips_of(q: &Sequence) -> BTreeMap<ClipId, Placed<'_>> {
    let mut out = BTreeMap::new();
    for (kind, tracks) in [(TrackKind::Video, &q.video_tracks), (TrackKind::Audio, &q.audio_tracks)] {
        let prefix = if kind == TrackKind::Video { "V" } else { "A" };
        for (i, t) in tracks.iter().enumerate() {
            for c in &t.items {
                out.insert(c.id, Placed { track: format!("{prefix}{}", i + 1), clip: c });
            }
        }
    }
    out
}

fn effect_names(c: &TrackItem) -> BTreeSet<String> {
    c.effects.iter().map(|e| e.effect.clone()).collect()
}

fn clip_name(c: &TrackItem) -> String {
    c.name.clone()
}

/// What changed between two versions of one clip, as the kinds it falls under.
fn clip_kinds(a: &Placed<'_>, b: &Placed<'_>) -> Vec<&'static str> {
    let (x, y) = (a.clip, b.clip);
    let mut kinds = Vec::new();
    if a.track != b.track || x.start != y.start {
        kinds.push("clipsMoved");
    }
    if x.speed != y.speed || x.reverse != y.reverse || x.frame_hold != y.frame_hold {
        kinds.push("clipsRetimed");
    } else if x.duration != y.duration || x.source_in != y.source_in {
        kinds.push("clipsTrimmed");
    }
    if x.enabled != y.enabled {
        kinds.push("clipsSwitched");
    }
    if x.effects != y.effects {
        let (na, nb) = (effect_names(x), effect_names(y));
        if nb.difference(&na).next().is_some() {
            kinds.push("effectsAdded");
        }
        if na.difference(&nb).next().is_some() {
            kinds.push("effectsRemoved");
        }
        if na == nb {
            kinds.push("effectsAdjusted");
        }
    }
    if x.gain_db != y.gain_db || x.essential != y.essential {
        kinds.push("soundChanged");
    }
    if kinds.is_empty() && x != y {
        kinds.push("clipsEdited");
    }
    kinds
}

fn sequence_diff(out: &mut Diff, name: &str, a: &Sequence, b: &Sequence) {
    let (ca, cb) = (clips_of(a), clips_of(b));
    let added: Vec<String> = cb.iter().filter(|(id, _)| !ca.contains_key(*id)).map(|(_, p)| clip_name(p.clip)).collect();
    let removed: Vec<String> = ca.iter().filter(|(id, _)| !cb.contains_key(*id)).map(|(_, p)| clip_name(p.clip)).collect();
    out.push("clipsAdded", name, added.clone(), added.len());
    out.push("clipsRemoved", name, removed.clone(), removed.len());
    let mut by_kind: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for (id, pa) in &ca {
        if let Some(pb) = cb.get(id) {
            for k in clip_kinds(pa, pb) {
                by_kind.entry(k).or_default().push(clip_name(pb.clip));
            }
        }
    }
    for kind in
        ["clipsMoved", "clipsTrimmed", "clipsRetimed", "clipsSwitched", "effectsAdded", "effectsRemoved", "effectsAdjusted", "soundChanged", "clipsEdited"]
    {
        if let Some(names) = by_kind.remove(kind) {
            let n = names.len();
            out.push(kind, name, names, n);
        }
    }
    // markers
    let (ma, mb): (BTreeMap<_, _>, BTreeMap<_, _>) = (a.markers.iter().map(|m| (m.id, m)).collect(), b.markers.iter().map(|m| (m.id, m)).collect());
    let m_added: Vec<String> = mb.iter().filter(|(id, _)| !ma.contains_key(*id)).map(|(_, m)| m.name.clone()).collect();
    let m_removed: Vec<String> = ma.iter().filter(|(id, _)| !mb.contains_key(*id)).map(|(_, m)| m.name.clone()).collect();
    let m_changed: Vec<String> = ma.iter().filter_map(|(id, m)| mb.get(id).filter(|n| **n != *m).map(|n| n.name.clone())).collect();
    out.push("markersAdded", name, m_added.clone(), m_added.len());
    out.push("markersRemoved", name, m_removed.clone(), m_removed.len());
    out.push("markersChanged", name, m_changed.clone(), m_changed.len());
    // transitions
    let ids =
        |q: &Sequence| -> BTreeSet<u64> { q.video_tracks.iter().chain(q.audio_tracks.iter()).flat_map(|t| t.transitions.iter().map(|x| x.id.0)).collect() };
    let (ta, tb) = (ids(a), ids(b));
    out.push("transitionsAdded", name, Vec::new(), tb.difference(&ta).count());
    out.push("transitionsRemoved", name, Vec::new(), ta.difference(&tb).count());
    // tracks
    let tracks = a.video_tracks.len().abs_diff(b.video_tracks.len()) + a.audio_tracks.len().abs_diff(b.audio_tracks.len());
    out.push("tracksChanged", name, Vec::new(), tracks);
    if a.settings != b.settings {
        out.push("sequenceSettings", name, Vec::new(), 1);
    }
}

/// What `b` changes against `a`.
pub fn diff(a: &Project, b: &Project) -> Diff {
    let mut out = Diff::default();
    for (id, ib) in &b.items {
        let Some(ia) = a.items.get(id) else { continue };
        if let (ItemKind::Sequence(qa), ItemKind::Sequence(qb)) = (&ia.kind, &ib.kind)
            && !std::sync::Arc::ptr_eq(qa, qb)
            && **qa != **qb
        {
            sequence_diff(&mut out, &ib.name, qa, qb);
        }
    }
    let added: Vec<String> = b.items.iter().filter(|(id, _)| !a.items.contains_key(*id)).map(|(_, i)| i.name.clone()).collect();
    let removed: Vec<String> = a.items.iter().filter(|(id, _)| !b.items.contains_key(*id)).map(|(_, i)| i.name.clone()).collect();
    let renamed: Vec<String> = b.items.iter().filter(|(id, i)| a.items.get(*id).is_some_and(|x| x.name != i.name)).map(|(_, i)| i.name.clone()).collect();
    out.push("itemsAdded", "", added.clone(), added.len());
    out.push("itemsRemoved", "", removed.clone(), removed.len());
    out.push("itemsRenamed", "", renamed.clone(), renamed.len());
    if a.notes != b.notes {
        out.push("notes", "", Vec::new(), 1);
    }
    // the plan: beats added, removed or edited
    let (pa, pb) = (crate::planner::get(a), crate::planner::get(b));
    match (&pa, &pb) {
        (Some(x), Some(y)) if x != y => {
            let ba: BTreeMap<u64, &edith_planner::Beat> = x.beats().map(|b| (b.id, b)).collect();
            let bb: BTreeMap<u64, &edith_planner::Beat> = y.beats().map(|b| (b.id, b)).collect();
            let mut names: Vec<String> = bb.iter().filter(|(id, _)| !ba.contains_key(*id)).map(|(_, b)| b.name.clone()).collect();
            names.extend(ba.iter().filter(|(id, _)| !bb.contains_key(*id)).map(|(_, b)| b.name.clone()));
            names.extend(ba.iter().filter_map(|(id, b)| bb.get(id).filter(|n| ***n != **b).map(|n| n.name.clone())));
            if names.is_empty() {
                out.push("planOther", "", Vec::new(), 1);
            } else {
                let n = names.len();
                out.push("planBeats", "", names, n);
            }
        }
        (None, Some(_)) | (Some(_), None) => out.push("planOther", "", Vec::new(), 1),
        _ => {}
    }
    if a.settings != b.settings {
        out.push("projectSettings", "", Vec::new(), 1);
    }
    let other = |p: &Project| -> BTreeMap<String, serde_json::Value> {
        p.ext.iter().filter(|(k, _)| k.as_str() != crate::planner::KEY).map(|(k, v)| (k.clone(), v.clone())).collect()
    };
    if other(a) != other(b) {
        out.push("otherData", "", Vec::new(), 1);
    }
    out.lines.truncate(MAX_LINES);
    out
}
