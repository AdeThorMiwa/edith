//! How the narrator says your words (fork roadmap F06.1): names, places and terms the voice
//! would only guess at. A word is given a respelling in plain English (`koo brick`) or phonemes
//! between slashes; the natural voices then say it that way, and its possessive (`Kubrick's`)
//! follows.
//!
//! Two lists, the project's winning over the global one:
//! - the **project** list is in the project (`Project::ext["edith.pronunciations"]`), so it is
//!   undone, saved and recovered with the project;
//! - the **global** list is yours for every project (all projects, now and later), kept in `edith-pronunciations.json` beside the
//!   preferences (in memory only when there is no data directory, as in tests).
//!
//! Nothing here re-renders a narration that already exists: change a word, then run Edit Narration
//! (`tts.edit`); its cache key includes the list, so it renders again exactly when a word changed.
//!
//! | command | does |
//! |---|---|
//! | `tts.pronunciations` | the project list, the global list and how many words apply |
//! | `tts.setPronunciation {word, say, scope?}` | adds or changes a word (`scope` `project` (default) or `global`); a project change is one undo step |
//! | `tts.removePronunciation {word, scope?}` | takes a word out of that list |
//! | `tts.checkScript {text?, clip?}` | the words of a script (or a narration clip) the voice would guess at, each once with how often it appears; `available: false` with a reason when the natural voices are not installed |
//!
//! `tts.preview`, `tts.render`, `tts.create` and `tts.edit` also accept `pronounce: {word: say}`
//! to try a way of saying a word without saving it.
//!
//! Everything read from a file or a project is untrusted: an entry that does not pass
//! [`Pronunciations::checked_entry`] is dropped, and at most [`Pronunciations::MAX_ENTRIES`] are kept.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use serde_json::{Value, json};

use filmcraft_project::Project;
use filmcraft_tts::{Pronunciations, TtsError};

use crate::commands::{CommandSpec, always, bad, str_p};
use crate::{Result, Session};

/// The key in `Project::ext` that holds the project's words.
pub(crate) const KEY: &str = "edith.pronunciations";
pub(crate) const FILE: &str = "edith-pronunciations.json";
/// Largest file read.
const MAX_FILE: u64 = 1024 * 1024;

/// The global list, read from disk the first time it is needed.
#[derive(Default)]
pub struct Global {
    words: Mutex<Option<BTreeMap<String, String>>>,
}

fn file_path(s: &Session) -> Option<PathBuf> {
    s.prefs_path.as_ref().and_then(|p| p.parent()).map(|d| d.join(FILE))
}

/// Keeps the entries that pass the checks, at most [`Pronunciations::MAX_ENTRIES`].
fn tidy<'a>(entries: impl Iterator<Item = (&'a str, &'a str)>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (w, say) in entries {
        if out.len() >= Pronunciations::MAX_ENTRIES {
            break;
        }
        if let Ok((w, say)) = Pronunciations::checked_entry(w, say) {
            out.insert(w, say);
        }
    }
    out
}

fn read_file(s: &Session) -> BTreeMap<String, String> {
    let Some(p) = file_path(s) else { return BTreeMap::new() };
    let Ok(meta) = std::fs::metadata(&p) else { return BTreeMap::new() };
    if meta.len() > MAX_FILE {
        return BTreeMap::new();
    }
    let Ok(text) = std::fs::read_to_string(&p) else { return BTreeMap::new() };
    let Ok(map) = serde_json::from_str::<BTreeMap<String, String>>(&text) else { return BTreeMap::new() };
    tidy(map.iter().map(|(w, say)| (w.as_str(), say.as_str())))
}

/// The global list.
fn global(s: &Session) -> BTreeMap<String, String> {
    let mut g = s.pronounce.words.lock().unwrap_or_else(PoisonError::into_inner);
    g.get_or_insert_with(|| read_file(s)).clone()
}

fn set_global(s: &Session, words: BTreeMap<String, String>) {
    if let Some(p) = file_path(s)
        && let Ok(json) = serde_json::to_string_pretty(&words)
    {
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
    *s.pronounce.words.lock().unwrap_or_else(PoisonError::into_inner) = Some(words);
}

/// The project's list.
pub fn project_words(p: &Project) -> BTreeMap<String, String> {
    let Some(map) = p.ext.get(KEY).and_then(Value::as_object) else { return BTreeMap::new() };
    tidy(map.iter().filter_map(|(w, v)| Some((w.as_str(), v.as_str()?))))
}

fn put_project(p: &mut Project, words: &BTreeMap<String, String>) {
    if words.is_empty() {
        p.ext.remove(KEY);
    } else {
        p.ext.insert(KEY.to_string(), serde_json::to_value(words).unwrap_or(Value::Null));
    }
}

/// Every word that applies now: the global list, then the project's over them.
pub fn effective(s: &Session) -> Pronunciations {
    let mut all = global(s);
    all.extend(project_words(&s.project));
    while all.len() > Pronunciations::MAX_ENTRIES {
        let Some(k) = all.keys().next_back().cloned() else { break };
        all.remove(&k);
    }
    Pronunciations(all)
}

/// [`effective`] plus the words in a command's `pronounce` object, which win but are not saved.
pub fn for_call(s: &Session, cmd: &str, p: &Value) -> Result<Pronunciations> {
    let mut all = effective(s);
    match p.get("pronounce") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => {
            if m.len() > 50 {
                return Err(bad(cmd, "`pronounce` holds at most 50 words"));
            }
            for (w, v) in m {
                let say = v.as_str().ok_or_else(|| bad(cmd, format!("`pronounce.{w}` must be text")))?;
                let (w, say) = Pronunciations::checked_entry(w, say).map_err(|e| bad(cmd, e))?;
                all.0.insert(w, say);
            }
        }
        Some(_) => return Err(bad(cmd, "`pronounce` must be an object of word: how to say it")),
    }
    Ok(all)
}

fn scope(cmd: &str, p: &Value) -> Result<bool> {
    match p.get("scope") {
        None | Some(Value::Null) => Ok(true),
        Some(Value::String(v)) if v == "project" => Ok(true),
        Some(Value::String(v)) if v == "global" => Ok(false),
        Some(_) => Err(bad(cmd, "`scope` must be `project` or `global`")),
    }
}

fn rows(m: &BTreeMap<String, String>) -> Vec<Value> {
    m.iter().map(|(w, say)| json!({"word": w, "say": say})).collect()
}

fn list_json(s: &Session) -> Value {
    json!({"project": rows(&project_words(&s.project)), "global": rows(&global(s)), "count": effective(s).0.len()})
}

fn list(s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(list_json(s))
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "tts.setPronunciation";
    let (Some(word), Some(say)) = (str_p(p, "word"), str_p(p, "say")) else {
        return Err(bad(cmd, "`word` and `say` (how to say it) are required text"));
    };
    let (word, say) = Pronunciations::checked_entry(word, say).map_err(|e| bad(cmd, e))?;
    let in_project = scope(cmd, p)?;
    let mut words = if in_project { project_words(&s.project) } else { global(s) };
    let before = words.insert(word.clone(), say.clone());
    if words.len() > Pronunciations::MAX_ENTRIES {
        return Err(bad(cmd, format!("at most {} words per list", Pronunciations::MAX_ENTRIES)));
    }
    if before.as_deref() != Some(say.as_str()) {
        if in_project {
            s.edit("Set Pronunciation", |pr, _| {
                put_project(pr, &words);
                Ok(())
            })?;
        } else {
            set_global(s, words);
        }
    }
    let mut r = list_json(s);
    r["word"] = json!(word);
    r["say"] = json!(say);
    r["scope"] = json!(if in_project { "project" } else { "global" });
    r["changed"] = json!(before.as_deref() != Some(say.as_str()));
    Ok(r)
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "tts.removePronunciation";
    let word = str_p(p, "word").ok_or_else(|| bad(cmd, "`word` is required text"))?.trim().to_lowercase();
    let in_project = scope(cmd, p)?;
    let mut words = if in_project { project_words(&s.project) } else { global(s) };
    if words.remove(&word).is_none() {
        return Err(bad(cmd, format!("there is no pronunciation for \"{word}\" in the {} list", if in_project { "project" } else { "global" })));
    }
    if in_project {
        s.edit("Remove Pronunciation", |pr, _| {
            put_project(pr, &words);
            Ok(())
        })?;
    } else {
        set_global(s, words);
    }
    Ok(list_json(s))
}

fn check_script(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "tts.checkScript";
    if p.get("text").is_some_and(|v| !v.is_string()) {
        return Err(bad(cmd, "`text` must be a string"));
    }
    let text = match str_p(p, "text") {
        Some(t) => t.to_string(),
        None => crate::narration::target_clip(s, p)
            .and_then(|c| s.active_sequence()?.find_item(c).map(|(_, it)| it.item))
            .and_then(|i| s.project.narrations.get(&i).map(|n| n.text.clone()))
            .ok_or_else(|| bad(cmd, "give `text`, or select a narration clip (or pass `clip`)"))?,
    };
    let list = for_call(s, cmd, p)?;
    match filmcraft_tts::check_words(&text, crate::transcript::models_dir().as_deref(), &list) {
        Ok(words) => Ok(json!({
            "available": true,
            "words": words.iter().map(|w| json!({"word": w.word, "count": w.count})).collect::<Vec<_>>(),
        })),
        Err(TtsError::NotInstalled | TtsError::Unavailable) => Ok(json!({
            "available": false,
            "words": [],
            "reason": "Words can only be checked once the natural voices are downloaded (Text to Speech ▸ Download natural voices).",
        })),
        Err(e) => Err(bad(cmd, e.to_string())),
    }
}

pub fn commands() -> Vec<CommandSpec> {
    let spec = |id, label, params, run: fn(&mut Session, &Value) -> Result<Value>, journal| CommandSpec {
        id,
        label,
        menu: &[],
        shortcut: None,
        params,
        enabled: always,
        run,
        journal,
    };
    vec![
        spec("tts.pronunciations", "List Pronunciations", "{}", list, false),
        spec("tts.setPronunciation", "Set Pronunciation", r#"{"word":str,"say":str,"scope":"project|global"?}"#, set, true),
        spec("tts.removePronunciation", "Remove Pronunciation", r#"{"word":str,"scope":"project|global"?}"#, remove, true),
        spec("tts.checkScript", "Check Narration Words", r#"{"text":str?,"clip":id?,"pronounce":{word:str}?}"#, check_script, false),
    ]
}
