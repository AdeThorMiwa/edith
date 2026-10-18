//! `project_items` (fork roadmap F16 spike): the media in the project, with the ids the other tools take and
//! what is recorded about where each came from. The spike showed the assistant guessing item ids and
//! unable to see licences, because `project_overview` only counts the pool.
//!
//! Read-only, in every mode. Rights are the person's to judge: a clip with no record is reported as "no record
//! of where it came from", never as forbidden, and a clip the person has said is fine stays theirs to decide.

use edith_ai::ToolOutput;
use filmcraft_project::{ItemId, ItemKind};
use serde_json::{Value, json};

use crate::Session;
use crate::assistant_tools::{ToolDef, def};
use crate::provenance;

/// Items listed in one answer.
const MAX_LISTED: usize = 100;

fn kind_word(info: &filmcraft_media::MediaInfo) -> &'static str {
    use filmcraft_media::MediaKind;
    match info.kind {
        MediaKind::Movie => {
            if info.video.is_some() {
                "video"
            } else {
                "audio"
            }
        }
        MediaKind::AudioOnly => "audio",
        MediaKind::Still => "still",
        MediaKind::ImageSequence => "image sequence",
        MediaKind::Synthetic => "generated",
    }
}

fn seconds_text(t: filmcraft_time::Tick) -> String {
    let s = t.0 as f64 / filmcraft_time::TICKS_PER_SECOND as f64;
    if s < 10.0 { format!("{s:.1} s") } else { format!("{s:.0} s") }
}

fn rights_text(s: &Session, id: ItemId) -> String {
    match provenance::get(&s.project, id) {
        Some(r) => r.summary(),
        None => "no record of where it came from, so its rights are not known".to_string(),
    }
}

/// How many clips on the timelines use each media item.
fn usage_counts(s: &Session) -> std::collections::HashMap<u64, usize> {
    let mut uses: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for seq in s.project.sequences() {
        if let ItemKind::Sequence(q) = &seq.kind {
            for t in q.video_tracks.iter().chain(q.audio_tracks.iter()) {
                for c in &t.items {
                    *uses.entry(c.item.0).or_default() += 1;
                }
            }
        }
    }
    uses
}

/// One media item as a line: id, name, kind, size, length, sound, offline, rights and use.
fn item_line(s: &Session, it: &filmcraft_project::ProjectItem, m: &filmcraft_project::MediaClip, uses: &std::collections::HashMap<u64, usize>) -> String {
    let video = m.info.video.as_ref().map_or(String::new(), |v| format!(", {}x{}", v.width, v.height));
    let audio = if m.info.audio_streams.is_empty() { "" } else { ", with sound" };
    let used = uses.get(&it.id.0).copied().unwrap_or(0);
    format!(
        "#{} “{}” — {}{}, {}{}{}; rights: {}{}",
        it.id.0,
        it.name,
        kind_word(&m.info),
        video,
        seconds_text(m.info.duration),
        audio,
        if m.offline { ", OFFLINE (the file is missing)" } else { "" },
        rights_text(s, it.id),
        if used > 0 { format!("; used in {used} place{}", if used == 1 { "" } else { "s" }) } else { String::new() }
    )
}

/// Files attached to one message, at most.
pub(crate) const MAX_ATTACHED: usize = 6;

/// The context block for a project file the person attached to their message: what it is, where it came from and
/// what its licence allows, what has been measured about it, and its transcript if it has one. Only media in
/// the project; the file's name is data, not an instruction.
pub(crate) fn item_block(s: &Session, id: u64) -> std::result::Result<edith_ai::ContextBlock, String> {
    let Some(it) = s.project.items.get(&ItemId(id)) else { return Err(format!("there is no file {id} in the project")) };
    let ItemKind::Media(m) = &it.kind else { return Err(format!("“{}” is not a file in the project (a media item is needed)", it.name)) };
    let mut text = item_line(s, it, m, &usage_counts(s));
    let cache = crate::analysis::Cache::of(s);
    if let Some(report) = crate::analysis::current_report_for(s, &cache, "item", id) {
        let measured = crate::analysis_tools::describe(&report);
        text.push_str("\nMeasured:\n");
        text.push_str(&edith_ai::clip_text(measured.trim(), 1500));
    } else {
        text.push_str("\nNot measured yet (analysis_run with this item measures its pictures and sound).");
    }
    if let Some(t) = s.project.transcripts.get(&it.id).filter(|t| !t.words.is_empty()) {
        text.push_str(&format!("\nTranscript: {} words. Begins: {}", t.words.len(), edith_ai::clip_text(&t.text().chars().take(300).collect::<String>(), 300)));
    }
    let label: String = format!("file: {}", it.name.chars().take(30).collect::<String>());
    Ok(edith_ai::ContextBlock::text(label, text))
}

fn project_items(s: &Session, input: &Value) -> ToolOutput {
    let query = input.get("query").and_then(Value::as_str).unwrap_or("").trim().to_ascii_lowercase();
    let want = input.get("kind").and_then(Value::as_str).map(str::to_ascii_lowercase);
    if let Some(k) = &want
        && !matches!(k.as_str(), "video" | "audio" | "still" | "generated" | "image sequence")
    {
        return ToolOutput::error("`kind` must be video, audio, still, generated or image sequence");
    }
    let uses = usage_counts(s);
    let mut lines = Vec::new();
    let mut total = 0usize;
    for it in s.project.items.values() {
        let ItemKind::Media(m) = &it.kind else { continue };
        let kind = kind_word(&m.info);
        if want.as_deref().is_some_and(|k| k != kind) || (!query.is_empty() && !it.name.to_ascii_lowercase().contains(&query)) {
            continue;
        }
        total += 1;
        if lines.len() >= MAX_LISTED {
            continue;
        }
        lines.push(item_line(s, it, m, &uses));
    }
    if total == 0 {
        return ToolOutput::ok("No media in the project matches.");
    }
    let mut out = format!("{total} media item{} in the project (ids are the ones other tools take as `item`):\n", if total == 1 { "" } else { "s" });
    out.push_str(&lines.join("\n"));
    if total > MAX_LISTED {
        out.push_str(&format!("\n({} more not listed: narrow with `query` or `kind`)", total - MAX_LISTED));
    }
    ToolOutput::ok(out)
}

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![def(
        "project_items",
        "The media in the project: each item's id (what other tools take as `item`), name, kind, length, size, whether it has sound, and what is recorded about where it came from and what its licence allows. A clip with no record is reported as unknown, which is for the person to decide, not a ban: flag it and ask them. Use it instead of guessing ids. Filter with `query` (a word of the name) or `kind` (video, audio, still).",
        json!({
            "query": {"type": "string", "description": "A word from the item's name."},
            "kind": {"type": "string", "enum": ["video", "audio", "still", "generated", "image sequence"], "description": "Only this kind."},
        }),
        project_items,
    )]
}
