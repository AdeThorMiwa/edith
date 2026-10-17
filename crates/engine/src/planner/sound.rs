//! The sound plan of each beat (fork roadmap F13.1), the spotting sheet's data.
//!
//! | command | does |
//! |---|---|
//! | `planner.setSound {beat, ambience?, music?, musicInSeconds?, musicOutSeconds?, musicToEnd?}` | the bed under a beat and its music: `music` is the mood (empty or null takes the music off), `musicIn…` where it comes in and `musicOut…` where it goes out, from the start of the beat; `musicToEnd: true` lets it play to the end |
//! | `planner.addCue {beat, kind?, text?, atSeconds, lengthSeconds?}` | an effect (`kind: "effect"`, the default) or a deliberate silence (`kind: "silence"`, which needs a length) at a time from the start of the beat |
//! | `planner.updateCue {cue, kind?, text?, atSeconds?, lengthSeconds?}`, `planner.removeCue {cue}` | change or remove one |
//!
//! Times are from the start of the beat, so a beat that moves takes its sound plan with it. Every change
//! is one undo step; everything from a parameter is untrusted and is checked against the planner's
//! limits (refused, not cut).

use edith_planner::model::{MAX_CUES_PER_BEAT, MAX_NAME, MAX_NOTE, MAX_TARGET};
use edith_planner::{Cue, CueKind, MusicCue, Planner};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use super::{edit, length, required_id, text};
use crate::commands::{CommandSpec, always, bad};
use crate::{Result, Session};

fn set_sound(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.setSound";
    let bid = required_id(p, "beat", cmd)?;
    let ambience = text(p, "ambience", MAX_NOTE, true, cmd)?;
    // music: left out, nothing changes; null or empty, it comes off; text, that is the mood
    let music: Option<Option<String>> = match p.get("music") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(m)) => {
            if m.chars().count() > MAX_NAME || m.chars().any(char::is_control) {
                return Err(bad(cmd, format!("`music` must be a mood of at most {MAX_NAME} characters without control characters")));
            }
            Some(Some(m.trim().to_string()).filter(|m| !m.is_empty()))
        }
        Some(_) => return Err(bad(cmd, "`music` must be text (the mood), or null to take the music off")),
    };
    let enters = length(p, "musicIn", cmd)?;
    let leaves = length(p, "musicOut", cmd)?;
    let to_end = crate::commands::bool_p(p, "musicToEnd");
    for t in [enters, leaves].into_iter().flatten() {
        if t > MAX_TARGET {
            return Err(bad(cmd, "a music time can be up to 12 hours"));
        }
    }
    edit(s, cmd, "Set Beat Sound", |pl| {
        let b = pl.beat_mut(bid).ok_or_else(|| bad(cmd, format!("no beat {bid}")))?;
        if let Some(a) = ambience {
            b.sound.ambience = a;
        }
        match music {
            Some(None) => b.sound.music = None,
            Some(Some(mood)) => b.sound.music.get_or_insert_with(MusicCue::default).mood = mood,
            None => {}
        }
        if enters.is_some() || leaves.is_some() || to_end.is_some() {
            let m = b.sound.music.as_mut().ok_or_else(|| bad(cmd, "this beat has no music: give `music` (its mood) first"))?;
            if let Some(t) = enters {
                m.enters = t;
            }
            if let Some(t) = leaves {
                m.leaves = Some(t);
            }
            if to_end == Some(true) {
                m.leaves = None;
            }
            if m.leaves.is_some_and(|l| l <= m.enters) {
                return Err(bad(cmd, "the music has to go out after it comes in"));
            }
        }
        Ok(())
    })?;
    Ok(json!({"beat": bid}))
}

fn cue_fields(c: &mut Cue, p: &Value, cmd: &str) -> Result<()> {
    if let Some(k) = text(p, "kind", 20, false, cmd)? {
        c.kind = CueKind::from_id(&k).ok_or_else(|| bad(cmd, "`kind` must be effect or silence"))?;
    }
    if let Some(t) = text(p, "text", MAX_NAME, false, cmd)? {
        c.text = t;
    }
    if let Some(t) = length(p, "at", cmd)? {
        if t > MAX_TARGET {
            return Err(bad(cmd, "a cue time can be up to 12 hours"));
        }
        c.at = t;
    }
    if let Some(t) = length(p, "length", cmd)? {
        if t > MAX_TARGET {
            return Err(bad(cmd, "a silence can be up to 12 hours"));
        }
        c.length = t;
    }
    match c.kind {
        CueKind::Effect => c.length = Tick::ZERO,
        CueKind::Silence if c.length.0 <= 0 => return Err(bad(cmd, "a silence needs a length: give `lengthSeconds`")),
        CueKind::Silence => {}
    }
    Ok(())
}

/// Keeps a beat's cues in the order they happen (ties stay as they were added).
fn sort_cues(cues: &mut [Cue]) {
    cues.sort_by_key(|c| c.at.0);
}

fn add_cue(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.addCue";
    let bid = required_id(p, "beat", cmd)?;
    let id = edit(s, cmd, "Add Sound Cue", |pl| {
        let mut cue = Cue::default();
        cue_fields(&mut cue, p, cmd)?;
        cue.id = pl.new_id();
        let id = cue.id;
        let b = pl.beat_mut(bid).ok_or_else(|| bad(cmd, format!("no beat {bid}")))?;
        if b.sound.cues.len() >= MAX_CUES_PER_BEAT {
            return Err(bad(cmd, format!("a beat holds at most {MAX_CUES_PER_BEAT} sound cues")));
        }
        b.sound.cues.push(cue);
        sort_cues(&mut b.sound.cues);
        Ok(id)
    })?;
    Ok(json!({"cue": id}))
}

fn find_cue(pl: &mut Planner, id: u64) -> Option<&mut Cue> {
    pl.sections.iter_mut().flat_map(|s| s.beats.iter_mut()).flat_map(|b| b.sound.cues.iter_mut()).find(|c| c.id == id)
}

fn update_cue(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.updateCue";
    let cid = required_id(p, "cue", cmd)?;
    edit(s, cmd, "Edit Sound Cue", |pl| {
        let c = find_cue(pl, cid).ok_or_else(|| bad(cmd, format!("no cue {cid}")))?;
        cue_fields(c, p, cmd)?;
        for sec in &mut pl.sections {
            for b in &mut sec.beats {
                if b.sound.cues.iter().any(|c| c.id == cid) {
                    sort_cues(&mut b.sound.cues);
                }
            }
        }
        Ok(())
    })?;
    Ok(json!({"cue": cid}))
}

fn remove_cue(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planner.removeCue";
    let cid = required_id(p, "cue", cmd)?;
    edit(s, cmd, "Remove Sound Cue", |pl| {
        for sec in &mut pl.sections {
            for b in &mut sec.beats {
                if let Some(i) = b.sound.cues.iter().position(|c| c.id == cid) {
                    b.sound.cues.remove(i);
                    return Ok(());
                }
            }
        }
        Err(bad(cmd, format!("no cue {cid}")))
    })?;
    Ok(json!({"removed": cid}))
}

pub(super) fn commands() -> Vec<CommandSpec> {
    let spec = |id, label, params, run: fn(&mut Session, &Value) -> Result<Value>| CommandSpec {
        id,
        label,
        menu: &[],
        shortcut: None,
        params,
        enabled: always,
        run,
        journal: true,
    };
    vec![
        spec(
            "planner.setSound",
            "Set Beat Sound",
            r#"{"beat":id,"ambience":str?,"music":str|null?,"musicInSeconds"|"musicInTicks":n?,"musicOutSeconds"|"musicOutTicks":n?,"musicToEnd":bool?}"#,
            set_sound,
        ),
        spec(
            "planner.addCue",
            "Add Sound Cue",
            r#"{"beat":id,"kind":"effect|silence"?,"text":str?,"atSeconds"|"atTicks":n?,"lengthSeconds"|"lengthTicks":n?}"#,
            add_cue,
        ),
        spec(
            "planner.updateCue",
            "Edit Sound Cue",
            r#"{"cue":id,"kind":"effect|silence"?,"text":str?,"atSeconds"|"atTicks":n?,"lengthSeconds"|"lengthTicks":n?}"#,
            update_cue,
        ),
        spec("planner.removeCue", "Remove Sound Cue", r#"{"cue":id}"#, remove_cue),
    ]
}
