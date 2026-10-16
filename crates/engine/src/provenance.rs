//! Provenance and licence records (fork roadmap F04): where each media item came from and what
//! you may do with it.
//!
//! A record says the source (Pexels, your own camera, a generator), the author, the page it came
//! from, the licence and what it allows (commercial use, credit), whether it is AI-generated (with
//! the model, prompt and seed), a content hash, and a free note. The records live **inside the
//! project** (`Project::ext["edith.provenance"]`), so they are undone, autosaved and recovered with
//! it. They record what a source says about itself; they are not legal advice and cannot tell
//! whether a source mislabelled something.
//!
//! | command | does |
//! |---|---|
//! | `provenance.get {item}` | the record of a media item and what it means for rights |
//! | `provenance.set {item, …}` | adds or changes a record (undoable): `tag` (`own`, `licensed`), `source`, `author`, `sourceUrl`, `license {name,url,commercial,attributionRequired,creditText}`, `synthetic {model,prompt,seed}` (or `null` to remove), `hash`, `note`; `clear: true` removes it |
//! | `provenance.list {}` | every media item with its rights status, and how many have no record |
//!
//! Everything read from a project file is untrusted: a record that does not parse is ignored, text
//! is cut to a safe length, and nothing here can panic on a hostile file.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use filmcraft_project::{ItemId, ItemKind, Project};

use crate::commands::{CommandSpec, always, bad};
use crate::{Result, Session};

/// The key in `Project::ext` that holds the records.
pub const KEY: &str = "edith.provenance";

const MAX_SHORT: usize = 300;
const MAX_URL: usize = 1000;
const MAX_PROMPT: usize = 4000;
const MAX_NOTE: usize = 2000;

/// What a source lets you do with a clip, as the source states it. `None` means "not stated".
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct License {
    pub name: String,
    pub url: String,
    pub commercial: Option<bool>,
    pub attribution_required: Option<bool>,
    /// The credit line to print, when one is required.
    pub credit_text: String,
}

/// How a generated clip was made.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Synthetic {
    pub model: String,
    pub prompt: String,
    pub seed: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Provenance {
    /// `own`, `licensed`, a provider id such as `pexels`, or any short word.
    pub source: String,
    pub author: String,
    pub source_url: String,
    pub license: License,
    pub synthetic: Option<Synthetic>,
    /// SHA-256 of the file when it was acquired, as hex (empty when not recorded).
    pub hash: String,
    /// When it was recorded (seconds since the Unix epoch).
    pub recorded_unix: u64,
    pub note: String,
}

/// What a record means for using the clip, without any policy: a badge colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Yours, or licensed with nothing owed: use freely.
    Clear,
    /// Free to use commercially, but the licence asks for (or does not rule out) a credit.
    Credit,
    /// Not allowed commercially, or the terms are not known: check before publishing.
    Risk,
    /// AI-generated: may need a disclosure on some platforms.
    Synthetic,
}

fn cut(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect::<String>().trim().to_string()
}

/// A web address kept only if it is plain `http(s)`: it is shown and opened, so nothing else
/// (`javascript:`, `file:`) may be stored.
fn clean_url(s: &str) -> String {
    let t = cut(s, MAX_URL);
    let lower = t.to_ascii_lowercase();
    if (lower.starts_with("https://") || lower.starts_with("http://")) && !t.chars().any(char::is_whitespace) { t } else { String::new() }
}

impl Provenance {
    /// This record with every text cut to its limit and every address checked.
    pub fn sanitized(mut self) -> Self {
        self.source = cut(&self.source, 60).to_ascii_lowercase();
        self.author = cut(&self.author, MAX_SHORT);
        self.source_url = clean_url(&self.source_url);
        self.license.name = cut(&self.license.name, MAX_SHORT);
        self.license.url = clean_url(&self.license.url);
        self.license.credit_text = cut(&self.license.credit_text, MAX_SHORT);
        if let Some(s) = &mut self.synthetic {
            s.model = cut(&s.model, MAX_SHORT);
            s.prompt = cut(&s.prompt, MAX_PROMPT);
        }
        self.hash = self.hash.trim().to_ascii_lowercase().chars().filter(char::is_ascii_hexdigit).take(128).collect();
        self.note = cut(&self.note, MAX_NOTE);
        self
    }

    /// A record for what you made or were licensed for yourself.
    pub fn preset(tag: &str) -> Option<Provenance> {
        let (source, name) = match tag {
            "own" => ("own", "Own work"),
            "licensed" => ("licensed", "Licensed to you"),
            _ => return None,
        };
        Some(Provenance {
            source: source.into(),
            license: License { name: name.into(), commercial: Some(true), attribution_required: Some(false), ..License::default() },
            ..Provenance::default()
        })
    }

    pub fn status(&self) -> Status {
        if self.synthetic.is_some() {
            return Status::Synthetic;
        }
        if self.license.commercial != Some(true) {
            return Status::Risk;
        }
        if self.license.attribution_required != Some(false) { Status::Credit } else { Status::Clear }
    }

    /// One plain line for a tooltip: where it is from and what it allows.
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.source.is_empty() {
            parts.push(match self.source.as_str() {
                "own" => "Your own work".to_string(),
                "licensed" => "Licensed to you".to_string(),
                other => {
                    let mut c = other.chars();
                    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                }
            });
        }
        if !self.license.name.is_empty() {
            parts.push(self.license.name.clone());
        }
        if let Some(s) = &self.synthetic {
            parts.push(if s.model.is_empty() { "AI-generated".to_string() } else { format!("AI-generated by {}", s.model) });
        }
        parts.push(
            match self.license.commercial {
                Some(true) => "commercial use allowed",
                Some(false) => "NOT for commercial use",
                None => "commercial use not stated",
            }
            .to_string(),
        );
        parts.push(
            match self.license.attribution_required {
                Some(true) => "credit required",
                Some(false) => "no credit needed",
                None => "credit not stated",
            }
            .to_string(),
        );
        if !self.author.is_empty() {
            parts.push(format!("by {}", self.author));
        }
        parts.join(" · ")
    }
}

/// The item a record belongs to: a subclip uses its source clip's record. `None` unless it is media.
pub fn owner(p: &Project, id: ItemId) -> Option<ItemId> {
    let it = p.item(id)?;
    match &it.kind {
        ItemKind::Media(_) => Some(id),
        ItemKind::Subclip { parent, .. } => p.item(*parent).filter(|x| x.as_media().is_some()).map(|x| x.id),
        _ => None,
    }
}

fn records(p: &Project) -> BTreeMap<ItemId, Provenance> {
    let Some(map) = p.ext.get(KEY).and_then(Value::as_object) else { return BTreeMap::new() };
    map.iter()
        .filter_map(|(k, v)| {
            let id = ItemId(k.parse::<u64>().ok()?);
            // a record for something no longer in the project is not shown
            p.item(id)?;
            Some((id, serde_json::from_value::<Provenance>(v.clone()).ok()?.sanitized()))
        })
        .collect()
}

/// The record of a media item, if it has one.
pub fn get(p: &Project, id: ItemId) -> Option<Provenance> {
    let id = owner(p, id)?;
    let v = p.ext.get(KEY)?.get(id.0.to_string())?;
    serde_json::from_value::<Provenance>(v.clone()).ok().map(Provenance::sanitized)
}

/// The rights status of an item, for badges.
pub fn status_of(p: &Project, id: ItemId) -> Option<Status> {
    get(p, id).map(|r| r.status())
}

/// Stores (or, with `None`, removes) the record of `id`. The caller has checked `id` is media.
pub fn put(p: &mut Project, id: ItemId, record: Option<Provenance>) {
    let entry = p.ext.entry(KEY.to_string()).or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    if let Some(map) = entry.as_object_mut() {
        match record {
            Some(r) => {
                map.insert(id.0.to_string(), serde_json::to_value(r.sanitized()).unwrap_or(Value::Null));
            }
            None => {
                map.remove(&id.0.to_string());
            }
        }
        if map.is_empty() {
            p.ext.remove(KEY);
        }
    }
}

/// Drops the records of items not in `keep` (a project saved as a subset must not carry the rest).
pub fn retain_items(p: &mut Project, keep: &std::collections::BTreeSet<ItemId>) {
    if let Some(map) = p.ext.get_mut(KEY).and_then(Value::as_object_mut) {
        map.retain(|k, _| k.parse::<u64>().is_ok_and(|n| keep.contains(&ItemId(n))));
    }
}

fn item_arg(s: &Session, p: &Value, cmd: &str) -> Result<ItemId> {
    let n = p.get("item").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "`item` must be a media item id (from project_overview or the Project panel)"))?;
    owner(&s.project, ItemId(n)).ok_or_else(|| bad(cmd, format!("item {n} is not a media item; only clips, images and audio have a record")))
}

fn record_json(r: &Provenance) -> Value {
    json!({"record": r, "status": r.status(), "summary": r.summary()})
}

fn get_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let id = item_arg(s, p, "provenance.get")?;
    let name = s.project.item(id).map(|i| i.name.clone()).unwrap_or_default();
    Ok(match get(&s.project, id) {
        Some(r) => {
            let mut v = record_json(&r);
            v["item"] = json!(id.0);
            v["name"] = json!(name);
            v
        }
        None => json!({"item": id.0, "name": name, "record": null, "status": null, "summary": "No record: where this came from is not known."}),
    })
}

fn opt_bool(v: &Value, what: &str, cmd: &str) -> Result<Option<bool>> {
    match v {
        Value::Null => Ok(None),
        Value::Bool(b) => Ok(Some(*b)),
        _ => Err(bad(cmd, format!("`{what}` must be true, false or null"))),
    }
}

fn text(v: &Value, what: &str, max: usize, cmd: &str) -> Result<String> {
    match v {
        Value::Null => Ok(String::new()),
        Value::String(s) if s.chars().count() <= max => Ok(s.clone()),
        _ => Err(bad(cmd, format!("`{what}` must be text of at most {max} characters"))),
    }
}

fn set_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "provenance.set";
    let id = item_arg(s, p, cmd)?;
    let clear = match p.get("clear") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(cmd, "`clear` must be true or false")),
    };
    if clear {
        let had = get(&s.project, id).is_some();
        if had {
            s.edit("Clear Provenance", |pr, _| {
                put(pr, id, None);
                Ok(())
            })?;
        }
        return Ok(json!({"item": id.0, "cleared": had}));
    }
    let mut r = get(&s.project, id).unwrap_or_default();
    if let Some(tag) = p.get("tag").filter(|v| !v.is_null()) {
        let t = tag.as_str().and_then(Provenance::preset).ok_or_else(|| bad(cmd, "`tag` must be `own` or `licensed`"))?;
        r = Provenance { note: r.note, hash: r.hash, ..t };
    }
    for (key, max) in [("source", 60usize), ("author", MAX_SHORT), ("sourceUrl", MAX_URL), ("hash", 128), ("note", MAX_NOTE)] {
        if let Some(v) = p.get(key) {
            let t = text(v, key, max, cmd)?;
            match key {
                "source" => r.source = t,
                "author" => r.author = t,
                "sourceUrl" => {
                    if !t.is_empty() && clean_url(&t).is_empty() {
                        return Err(bad(cmd, "`sourceUrl` must be an http or https address"));
                    }
                    r.source_url = t;
                }
                "hash" => r.hash = t,
                _ => r.note = t,
            }
        }
    }
    if let Some(l) = p.get("license").filter(|v| !v.is_null()) {
        let o = l.as_object().ok_or_else(|| bad(cmd, "`license` must be an object"))?;
        for (k, v) in o {
            match k.as_str() {
                "name" => r.license.name = text(v, "license.name", MAX_SHORT, cmd)?,
                "url" => {
                    let t = text(v, "license.url", MAX_URL, cmd)?;
                    if !t.is_empty() && clean_url(&t).is_empty() {
                        return Err(bad(cmd, "`license.url` must be an http or https address"));
                    }
                    r.license.url = t;
                }
                "commercial" => r.license.commercial = opt_bool(v, "license.commercial", cmd)?,
                "attributionRequired" => r.license.attribution_required = opt_bool(v, "license.attributionRequired", cmd)?,
                "creditText" => r.license.credit_text = text(v, "license.creditText", MAX_SHORT, cmd)?,
                other => {
                    return Err(bad(
                        cmd,
                        format!(
                            "`license.{}` is not a field; use name, url, commercial, attributionRequired, creditText",
                            other.chars().take(20).collect::<String>()
                        ),
                    ));
                }
            }
        }
    }
    if let Some(v) = p.get("synthetic") {
        r.synthetic = match v {
            Value::Null => None,
            Value::Object(o) => {
                let mut sy = r.synthetic.take().unwrap_or_default();
                for (k, v) in o {
                    match k.as_str() {
                        "model" => sy.model = text(v, "synthetic.model", MAX_SHORT, cmd)?,
                        "prompt" => sy.prompt = text(v, "synthetic.prompt", MAX_PROMPT, cmd)?,
                        "seed" => {
                            sy.seed = if v.is_null() { None } else { Some(v.as_u64().ok_or_else(|| bad(cmd, "`synthetic.seed` must be a whole number"))?) }
                        }
                        other => {
                            return Err(bad(
                                cmd,
                                format!("`synthetic.{}` is not a field; use model, prompt, seed", other.chars().take(20).collect::<String>()),
                            ));
                        }
                    }
                }
                Some(sy)
            }
            _ => return Err(bad(cmd, "`synthetic` must be an object, or null to remove it")),
        };
    }
    r.recorded_unix = u64::try_from(crate::autosave::unix_now()).unwrap_or(0);
    let r = r.sanitized();
    if r.source.is_empty()
        && r.license == License::default()
        && r.synthetic.is_none()
        && r.author.is_empty()
        && r.source_url.is_empty()
        && r.note.is_empty()
        && r.hash.is_empty()
    {
        return Err(bad(cmd, "nothing to record: give a `tag`, `source`, `license` or another field (or `clear: true` to remove the record)"));
    }
    let shown = r.clone();
    s.edit("Set Provenance", |pr, _| {
        put(pr, id, Some(r));
        Ok(())
    })?;
    let mut v = record_json(&shown);
    v["item"] = json!(id.0);
    Ok(v)
}

fn list_cmd(s: &mut Session, _p: &Value) -> Result<Value> {
    let all = records(&s.project);
    let mut rows = Vec::new();
    let mut unrecorded = 0u64;
    for (id, it) in &s.project.items {
        if it.as_media().is_none() {
            continue;
        }
        match all.get(id) {
            Some(r) => rows.push(json!({"item": id.0, "name": it.name, "status": r.status(), "source": r.source, "summary": r.summary()})),
            None => {
                unrecorded += 1;
                rows.push(json!({"item": id.0, "name": it.name, "status": null, "summary": "No record"}));
            }
        }
    }
    let count = |st: Status| all.values().filter(|r| r.status() == st).count();
    Ok(json!({
        "items": rows,
        "unrecorded": unrecorded,
        "counts": {"clear": count(Status::Clear), "credit": count(Status::Credit), "risk": count(Status::Risk), "synthetic": count(Status::Synthetic)},
    }))
}

pub fn commands() -> Vec<CommandSpec> {
    let spec = |id, label, params, journal, run: fn(&mut Session, &Value) -> Result<Value>| CommandSpec {
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
        spec("provenance.get", "Get Provenance", r#"{"item":id}"#, false, get_cmd),
        spec(
            "provenance.set",
            "Set Provenance",
            r#"{"item":id,"tag":"own|licensed"?,"source":str?,"author":str?,"sourceUrl":str?,"license":{"name":str?,"url":str?,"commercial":bool|null?,"attributionRequired":bool|null?,"creditText":str?}?,"synthetic":{"model":str?,"prompt":str?,"seed":u64?}|null?,"hash":str?,"note":str?,"clear":bool?}"#,
            true,
            set_cmd,
        ),
        spec("provenance.list", "List Provenance", "{}", false, list_cmd),
    ]
}
