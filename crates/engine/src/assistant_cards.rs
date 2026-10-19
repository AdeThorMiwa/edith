//! Talking to the person: question cards, download requests and the media search that feeds them
//! (fork roadmap F16.1).
//!
//! **A question ends the turn; the person's click is the reply.** `ask_user` shows a card and returns at
//! once, and the assistant ends its turn. It may put several cards in one turn (at most six); they form one
//! group, and one **Send answers** sends every answer as a single message in plain words, which starts the
//! next turn. Nothing blocks while the person thinks, and a question survives closing the app because the
//! card is saved in the conversation (its view), and the open group is worked out from the saved events.
//!
//! **The download is the person's click, never the assistant's.** `request_download` only shows a card
//! (picture, source, licence, size) with Yes, No and Show me others. **Yes** runs `hub.acquire` as the
//! person's own action; the assistant has no tool that downloads, and the editing allowlist
//! (`assistant_edit`) names neither `hub.` nor `assistant.`. The answers wait until the group's downloads
//! have finished, so the next turn already has the clip in the project.
//!
//! | command | does |
//! |---|---|
//! | `assistant.cards {thread?}` | the open cards of the latest turn, the choices made so far, and whether they can be sent |
//! | `assistant.card {card, option? \| other? \| decision?, thread?}` | choose an answer on a card (`decision`: `yes`, `no` or `others` on a download card; `yes` starts the download) |
//! | `assistant.answerCards {thread?, wait?}` | send the answers as one message |
//!
//! Tools: `ask_user`, `request_download`, `media_search` (read-only: a search sends the words to the stock
//! sites, nothing downloads).

use std::collections::HashMap;
use std::sync::mpsc::SyncSender;

use edith_ai::{AgentEvent, CardAnswer, ToolOutput};
use serde_json::{Value, json};

use crate::assistant::{ToolRequest, lock, push_event, thread_arg};
use crate::assistant_tools::{ToolDef, def};
use crate::commands::{CommandSpec, always, bad, bool_p, str_p};
use crate::hub::Acquired;
use crate::{Result, Session};

/// Cards (questions and download requests together) in one turn.
pub(crate) const MAX_CARDS: usize = 6;
const MIN_OPTIONS: usize = 2;
const MAX_OPTIONS: usize = 6;
const MAX_QUESTION: usize = 300;
const MAX_LABEL: usize = 80;
const MAX_DETAIL: usize = 200;
const MAX_OTHER: usize = 300;
/// Results listed to the model from one search.
const MAX_LISTED: usize = 12;
/// How long a media search is waited for, seconds.
const SEARCH_WAIT: u64 = 45;

/// What the person has chosen on a card, not yet sent.
#[derive(Clone, Debug, PartialEq)]
enum Draft {
    Option(usize),
    Other(String),
    Yes,
    No,
    Others,
}

struct PendingSearch {
    reply: SyncSender<ToolOutput>,
    since: web_time::Instant,
    text: String,
    count: usize,
}

/// The session's card state. Not part of the project; the cards themselves live in the conversation.
#[derive(Default)]
pub(crate) struct Cards {
    drafts: HashMap<(u64, String), Draft>,
    searches: Vec<PendingSearch>,
    /// What `media_look` has fetched this session, and the sounds `media_listen` is fetching (fork roadmap F29).
    pub(crate) looks: crate::assistant_look::Looks,
    /// Narrations being made for cards the person said Yes to (fork roadmap F16.7).
    pub(crate) voicing: crate::assistant_narration::VoicingMap,
}

// ---------------------------------------------------------------------------- reading the turn

/// The views of the question and download cards shown since the person last spoke in `thread`, oldest first.
fn open_cards(s: &Session, thread: u64) -> Vec<Value> {
    let Some(slot) = s.assistant.slot(thread) else { return Vec::new() };
    let st = lock(&slot.shared.state);
    let mut out = Vec::new();
    for ev in st.events.iter().rev() {
        match ev {
            AgentEvent::UserMessage { .. } => break,
            AgentEvent::ToolResult { view: Some(v), is_error: false, .. } if matches!(v["kind"].as_str(), Some("ask" | "download" | "narration")) => {
                out.push(v.clone())
            }
            _ => {}
        }
    }
    out.reverse();
    out
}

/// The words and kind of the latest `media_search` the assistant made in `thread`: what "Show me others" searches again.
fn last_search(s: &Session, thread: u64) -> Option<(String, String)> {
    let slot = s.assistant.slot(thread)?;
    let st = lock(&slot.shared.state);
    st.events.iter().rev().find_map(|ev| match ev {
        AgentEvent::ToolCall { name, input, .. } if name == "media_search" => {
            let text = input.get("text").and_then(|t| clean(t, 300))?;
            let kind = input.get("kind").and_then(Value::as_str).filter(|k| matches!(*k, "video" | "image" | "audio")).unwrap_or("video");
            Some((text, kind.to_string()))
        }
        _ => None,
    })
}

/// The next card id in `thread` (`c1`, `c2`, …), one past the highest in its saved events, so a restored
/// conversation never reuses one.
pub(crate) fn next_card_id(s: &Session, thread: u64) -> String {
    let highest = s.assistant.slot(thread).map_or(0, |slot| {
        lock(&slot.shared.state)
            .events
            .iter()
            .filter_map(|ev| match ev {
                AgentEvent::ToolResult { view: Some(v), .. } => v["id"].as_str()?.strip_prefix('c')?.parse::<u64>().ok(),
                _ => None,
            })
            .max()
            .unwrap_or(0)
    });
    format!("c{}", highest.saturating_add(1))
}

/// Text from a model, as one short plain line (no control characters), at most `max` characters.
fn clean(v: &Value, max: usize) -> Option<String> {
    let t: String = v.as_str()?.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    (!t.is_empty() && t.chars().count() <= max).then_some(t)
}

// ---------------------------------------------------------------------------- the tools

/// Answers `ask_user`, `request_download` and `media_search` on the session thread; hands back any other call.
pub(crate) fn serve(s: &mut Session, req: ToolRequest) -> Option<ToolRequest> {
    match req.name.as_str() {
        "ask_user" => {
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ask_user(s, req.thread, &req.input)))
                .unwrap_or_else(|_| ToolOutput::error("the question could not be shown"));
            let _ = req.reply.send(out);
            None
        }
        "request_narration" => {
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::assistant_narration::request(s, req.thread, &req.input)))
                .unwrap_or_else(|_| ToolOutput::error("the request could not be shown"));
            let _ = req.reply.send(out);
            None
        }
        "request_download" => {
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| request_download(s, req.thread, &req.input)))
                .unwrap_or_else(|_| ToolOutput::error("the request could not be shown"));
            let _ = req.reply.send(out);
            None
        }
        "media_search" => {
            if let Some(out) = start_search(s, &req) {
                let _ = req.reply.send(out);
            }
            None
        }
        "media_look" | "media_listen" => {
            let started = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::assistant_look::serve(s, &req)))
                .unwrap_or_else(|_| Some(ToolOutput::error("the look could not be taken")));
            if let Some(out) = started {
                let _ = req.reply.send(out);
            }
            None
        }
        _ => Some(req),
    }
}

pub(crate) fn room_check(s: &Session, thread: u64) -> std::result::Result<usize, ToolOutput> {
    let so_far = open_cards(s, thread).len();
    if so_far >= MAX_CARDS {
        return Err(ToolOutput::error(format!(
            "{MAX_CARDS} cards are already shown in this turn, which is the most there can be. Stop here: the person answers them together, and you can ask the rest after that."
        )));
    }
    Ok(so_far)
}

fn ask_user(s: &Session, thread: u64, input: &Value) -> ToolOutput {
    let so_far = match room_check(s, thread) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let Some(question) = input.get("question").and_then(|q| clean(q, MAX_QUESTION)) else {
        return ToolOutput::error(format!("`question` is required: one short question, up to {MAX_QUESTION} characters"));
    };
    let Some(list) = input.get("options").and_then(Value::as_array).filter(|o| (MIN_OPTIONS..=MAX_OPTIONS).contains(&o.len())) else {
        return ToolOutput::error(format!(
            "`options` must list {MIN_OPTIONS} to {MAX_OPTIONS} choices (each a short phrase, or {{label, description}}); the person can always answer in their own words unless `allowOther` is false"
        ));
    };
    let mut options: Vec<Value> = Vec::new();
    for o in list {
        let (label, detail) = match o {
            Value::String(_) => (clean(o, MAX_LABEL), None),
            Value::Object(m) => (m.get("label").and_then(|l| clean(l, MAX_LABEL)), m.get("description").and_then(|d| clean(d, MAX_DETAIL))),
            _ => (None, None),
        };
        let Some(label) = label else {
            return ToolOutput::error(format!("each option needs a label of 1 to {MAX_LABEL} characters (and a description of at most {MAX_DETAIL})"));
        };
        if options.iter().any(|x| x["label"].as_str().is_some_and(|l| l.eq_ignore_ascii_case(&label))) {
            return ToolOutput::error(format!("two options are both \"{label}\""));
        }
        options.push(json!({"label": label, "detail": detail}));
    }
    let allow_other = input.get("allowOther").and_then(Value::as_bool).unwrap_or(true);
    let id = next_card_id(s, thread);
    let view = json!({"kind": "ask", "id": id, "question": question, "options": options, "allowOther": allow_other});
    ToolOutput::ok(format!(
        "Question card {id} is now shown to the person: \"{question}\". That makes {} card{} this turn (at most {MAX_CARDS}). Stop here: their answers to every card arrive together as their next message. Do not ask it again in words, and do not carry on as if you knew the answer.",
        so_far + 1,
        if so_far == 0 { "" } else { "s" }
    ))
    .with_view(view)
}

fn source_label(source: &str) -> String {
    edith_sources::info(source).map_or_else(|| source.to_string(), |i| i.label.to_string())
}

/// What a candidate's licence says, in one line, as the site states it.
pub(crate) fn rights_line(c: &edith_sources::Candidate) -> String {
    let r = &c.rights;
    let name = if r.name.trim().is_empty() { "licence not stated" } else { r.name.trim() };
    let commercial = match r.commercial {
        Some(true) => "commercial use allowed",
        Some(false) => "NOT for commercial use",
        None => "commercial use not stated",
    };
    let credit = match r.attribution_required {
        Some(true) => "credit required",
        Some(false) => "no credit needed",
        None => "credit not stated",
    };
    format!("{name}; {commercial}; {credit}")
}

/// Why a clip may not be offered, when its licence is unknown or does not allow commercial use.
pub(crate) fn licence_problem(c: &edith_sources::Candidate) -> Option<String> {
    if c.rights.name.trim().is_empty() && c.rights.commercial.is_none() {
        return Some(format!("“{}” states no licence, and nothing is requested without a known licence; pick another result", c.title));
    }
    if c.rights.commercial == Some(false) {
        return Some(format!("“{}” is not licensed for commercial use ({}); pick another result", c.title, rights_line(c)));
    }
    None
}

fn candidate_card(c: &edith_sources::Candidate) -> Value {
    let d = c.download.as_ref();
    json!({
        "id": c.id, "title": c.title, "author": c.author, "source": c.source, "sourceLabel": source_label(&c.source), "kind": c.kind.name(),
        "width": d.and_then(|d| d.width).or(c.width), "height": d.and_then(|d| d.height).or(c.height), "seconds": c.seconds,
        "bytes": d.and_then(|d| d.bytes), "note": d.map(|d| d.note.clone()).unwrap_or_default(), "sourceUrl": c.source_url,
        "rights": {"name": c.rights.name, "url": c.rights.url, "commercial": c.rights.commercial, "attributionRequired": c.rights.attribution_required},
        "rightsLine": rights_line(c),
    })
}

fn request_download(s: &Session, thread: u64, input: &Value) -> ToolOutput {
    let so_far = match room_check(s, thread) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let Some(id) = input.get("candidate").and_then(|c| clean(c, 100)) else {
        return ToolOutput::error("`candidate` must be a result id from media_search (such as pexels:123)");
    };
    let why = input.get("why").and_then(|w| clean(w, MAX_QUESTION)).unwrap_or_default();
    let Some(c) = s.hub.candidate(&id) else {
        return ToolOutput::error(format!("no result {id} in this session: search with media_search first, and use an id from its results"));
    };
    if c.download.is_none() {
        return ToolOutput::error(format!("“{}” has no file to download; pick another result", c.title));
    }
    if c.source != "local" && edith_net::shared().offline() {
        return ToolOutput::error("Offline mode is on, so nothing can be downloaded; tell the person, or use what is already in the project (project_items)");
    }
    if let Some(problem) = licence_problem(&c) {
        return ToolOutput::error(problem);
    }
    if let Some(Acquired::Done { item }) = s.hub.acquired(&id) {
        return ToolOutput::error(format!("“{}” is already in the project as item {item}: use project_items", c.title));
    }
    if open_cards(s, thread).iter().any(|v| v["kind"] == "download" && v["candidate"]["id"] == id.as_str()) {
        return ToolOutput::error("a card for that clip is already shown in this turn");
    }
    let card = next_card_id(s, thread);
    let (query, kind) = last_search(s, thread).unwrap_or_else(|| (c.title.clone(), c.kind.name().to_string()));
    let view = json!({"kind": "download", "id": card, "candidate": candidate_card(&c), "why": why, "query": query, "searchKind": kind});
    ToolOutput::ok(format!(
        "Download card {card} is now shown to the person for “{}” ({}). That makes {} card{} this turn (at most {MAX_CARDS}). Nothing is downloaded unless they press Yes. Stop here: their answers to every card arrive together as their next message.",
        c.title,
        rights_line(&c),
        so_far + 1,
        if so_far == 0 { "" } else { "s" }
    ))
    .with_view(view)
}

// ---------------------------------------------------------------------------- media_search

fn start_search(s: &mut Session, req: &ToolRequest) -> Option<ToolOutput> {
    let input = if req.input.is_object() { &req.input } else { return Some(ToolOutput::error("media_search needs {text}")) };
    let Some(text) = input.get("text").and_then(|t| clean(t, 300)) else {
        return Some(ToolOutput::error("`text` is required: what to look for, in plain words, up to 300 characters"));
    };
    let kind = input.get("kind").and_then(Value::as_str).unwrap_or("video");
    if !matches!(kind, "video" | "image" | "audio") {
        return Some(ToolOutput::error("`kind` must be video, image or audio"));
    }
    let count = input.get("count").and_then(Value::as_u64).map_or(8, |n| n.clamp(1, MAX_LISTED as u64)) as usize;
    let mut params = json!({"text": text, "kinds": [kind], "count": count.max(6), "wait": false});
    for key in ["orientation", "minSeconds", "maxSeconds"] {
        if let Some(v) = input.get(key).filter(|v| !v.is_null()) {
            params[key] = v.clone();
        }
    }
    match s.execute("hub.search", params) {
        Err(e) => Some(ToolOutput::error(e.to_string())),
        Ok(_) => {
            s.assistant.cards.searches.push(PendingSearch { reply: req.reply.clone(), since: web_time::Instant::now(), text, count });
            None
        }
    }
}

/// Answers the `media_search` calls whose search has finished (or has waited too long).
pub(crate) fn finish_searches(s: &mut Session) {
    if s.assistant.cards.searches.is_empty() {
        return;
    }
    let mut keep = Vec::new();
    for p in std::mem::take(&mut s.assistant.cards.searches) {
        let results = s.execute("hub.results", json!({})).unwrap_or(Value::Null);
        let running = results["running"].as_bool().unwrap_or(false);
        let late = p.since.elapsed().as_secs() >= SEARCH_WAIT;
        if running && !late {
            keep.push(p);
            continue;
        }
        let _ = p.reply.send(search_text(s, &p, &results, running));
    }
    s.assistant.cards.searches.extend(keep);
}

/// One result in a line: id, title, author, kind, size, length, file size and licence.
pub(crate) fn candidate_line(c: &edith_sources::Candidate) -> String {
    let d = c.download.as_ref();
    let size = d.and_then(|d| d.bytes).map_or(String::new(), |b| format!(", {:.1} MB", b as f64 / 1_048_576.0));
    let dims = match (d.and_then(|d| d.width).or(c.width), d.and_then(|d| d.height).or(c.height)) {
        (Some(w), Some(h)) => format!(", {w}x{h}"),
        _ => String::new(),
    };
    let len = c.seconds.map_or(String::new(), |x| format!(", {x:.0} s"));
    format!(
        "{} | “{}” by {} | {}{dims}{len}{size} | licence: {}",
        c.id,
        c.title,
        if c.author.is_empty() { "unknown" } else { &c.author },
        c.kind.name(),
        rights_line(c)
    )
}

fn search_text(s: &Session, p: &PendingSearch, results: &Value, still_running: bool) -> ToolOutput {
    let sources: Vec<String> = results["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|x| {
            let id = x["id"].as_str().unwrap_or("?");
            match x["state"].as_str() {
                Some("failed") => format!("{id} failed ({})", x["message"].as_str().unwrap_or("unavailable")),
                Some("searching") => format!("{id} still searching"),
                _ => format!("{id} {} found", x["found"].as_u64().unwrap_or(0)),
            }
        })
        .collect();
    let list: Vec<edith_sources::Candidate> = results["candidates"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["id"].as_str().and_then(|id| s.hub.candidate(id)))
        .filter(|c| c.download.is_some())
        .take(p.count)
        .collect();
    let mut out = format!("Search for “{}”: {}.{}\n", p.text, sources.join(", "), if still_running { " (Some sources had not answered yet.)" } else { "" });
    if list.is_empty() {
        out.push_str("Nothing with a file to download was found. Try different words, another kind, or ask the person.");
        return ToolOutput::ok(out);
    }
    for c in &list {
        out.push_str(&format!("- {}\n", candidate_line(c)));
    }
    out.push_str("Nothing has been downloaded. To offer one, call request_download with its id: the person decides with Yes, No or Show me others. Offer only results whose licence allows commercial use.");
    ToolOutput::ok(out)
}

// ---------------------------------------------------------------------------- the person's side

fn thread_of(s: &Session, p: &Value, cmd: &str) -> Result<u64> {
    if p.get("thread").is_some_and(|v| !v.is_null()) {
        return thread_arg(s, p, cmd);
    }
    s.scoped_threads().last().map(|t| t.id).ok_or_else(|| bad(cmd, "there is no conversation yet"))
}

fn download_state(s: &Session, card: &Value) -> Option<Value> {
    if card["kind"] != "download" {
        return None;
    }
    let id = card["candidate"]["id"].as_str()?;
    Some(match s.hub.acquired(id) {
        None => json!({"state": "none"}),
        Some(Acquired::Downloading) => json!({"state": "downloading"}),
        Some(Acquired::Done { item }) => json!({"state": "done", "item": item}),
        Some(Acquired::Failed(m)) => json!({"state": "failed", "message": m}),
    })
}

fn draft_json(card: &Value, d: &Draft) -> Value {
    match d {
        Draft::Option(i) => json!({"kind": "option", "index": i, "label": card["options"][*i]["label"]}),
        Draft::Other(t) => json!({"kind": "other", "text": t}),
        Draft::Yes => json!({"kind": "yes"}),
        Draft::No => json!({"kind": "no"}),
        Draft::Others => json!({"kind": "others"}),
    }
}

/// What the answer to `card` says in the message to the assistant, and in the card afterwards.
fn answer_words(s: &Session, thread: u64, card: &Value, d: Option<&Draft>) -> String {
    match (card["kind"].as_str(), d) {
        (_, None) => "no answer".to_string(),
        (Some("narration"), Some(Draft::No)) => "no".to_string(),
        (Some("narration"), Some(Draft::Others)) => "they will make the narration themselves in Text to Speech, with their own choice of voice".to_string(),
        (Some("narration"), Some(Draft::Yes)) => crate::assistant_narration::answer_yes(s, thread, card),
        (Some("ask"), Some(Draft::Option(i))) => card["options"][*i]["label"].as_str().unwrap_or("(an option)").to_string(),
        (Some("ask"), Some(Draft::Other(t))) => format!("(in their own words) {t}"),
        (Some("download"), Some(Draft::No)) => "no".to_string(),
        (Some("download"), Some(Draft::Others)) => "they will look at other options themselves in Find Media".to_string(),
        (Some("download"), Some(Draft::Yes)) => match download_state(s, card).as_ref().map(|v| v["state"].as_str().unwrap_or("")) {
            Some("done") => {
                let item = download_state(s, card).map(|v| v["item"].as_u64().unwrap_or(0)).unwrap_or(0);
                format!("yes, it is now in the project as item {item}")
            }
            Some("failed") => format!(
                "yes, but the download failed: {}",
                download_state(s, card).map(|v| v["message"].as_str().unwrap_or("").to_string()).unwrap_or_default()
            ),
            _ => "yes (still downloading)".to_string(),
        },
        _ => "no answer".to_string(),
    }
}

fn card_title(card: &Value) -> String {
    match card["kind"].as_str() {
        Some("narration") => format!(
            "Make the narration ({} words, about {} seconds, voice {})",
            card["words"].as_u64().unwrap_or(0),
            card["estimateSeconds"].as_f64().unwrap_or(0.0).round(),
            card["voice"]["name"].as_str().unwrap_or("")
        ),
        Some("download") => format!(
            "Download “{}” from {} ({})",
            card["candidate"]["title"].as_str().unwrap_or(""),
            card["candidate"]["sourceLabel"].as_str().unwrap_or(""),
            card["candidate"]["rightsLine"].as_str().unwrap_or("")
        ),
        _ => card["question"].as_str().unwrap_or("").to_string(),
    }
}

fn cards_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "assistant.cards";
    s.pump_assistant();
    s.pump_hub();
    crate::assistant_narration::pump(s);
    let thread = thread_of(s, p, cmd)?;
    let running = s.assistant.slot(thread).is_some_and(|t| lock(&t.shared.state).running);
    let group = open_cards(s, thread);
    // choices for cards that are no longer open are forgotten
    let ids: Vec<String> = group.iter().filter_map(|c| c["id"].as_str().map(str::to_string)).collect();
    s.assistant.cards.drafts.retain(|(t, id), _| *t != thread || ids.contains(id));
    let mut waiting = 0;
    let cards: Vec<Value> = group
        .iter()
        .map(|c| {
            let id = c["id"].as_str().unwrap_or("").to_string();
            let draft = s.assistant.cards.drafts.get(&(thread, id.clone()));
            let dl = download_state(s, c);
            if dl.as_ref().is_some_and(|d| d["state"] == "downloading") {
                waiting += 1;
            }
            let narration = crate::assistant_narration::state_json(s, thread, c);
            if narration.as_ref().is_some_and(|d| d["state"] == "rendering") {
                waiting += 1;
            }
            let mut v = c.clone();
            v["narration"] = narration.unwrap_or(Value::Null);
            v["draft"] = draft.map_or(Value::Null, |d| draft_json(c, d));
            v["download"] = dl.unwrap_or(Value::Null);
            v["answer"] = json!(answer_words(s, thread, c, draft));
            v
        })
        .collect();
    let answered = cards.iter().filter(|c| !c["draft"].is_null()).count();
    Ok(json!({
        "thread": thread, "running": running, "cards": cards, "answered": answered, "waiting": waiting,
        "canSend": !cards.is_empty() && !running && waiting == 0,
    }))
}

fn card_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "assistant.card";
    s.pump_assistant();
    s.pump_hub();
    crate::assistant_narration::pump(s);
    let thread = thread_of(s, p, cmd)?;
    let id = str_p(p, "card").ok_or_else(|| bad(cmd, "`card` must be a card id (c1, c2, …; see assistant.cards)"))?.to_string();
    let card = open_cards(s, thread)
        .into_iter()
        .find(|c| c["id"].as_str() == Some(id.as_str()))
        .ok_or_else(|| bad(cmd, format!("there is no open card {id}: only the cards of the latest turn can be answered")))?;
    if s.assistant.slot(thread).is_some_and(|t| lock(&t.shared.state).running) {
        return Err(bad(cmd, "the assistant is still working; answer when it has stopped"));
    }
    let key = (thread, id.clone());
    let kind = card["kind"].as_str().unwrap_or("");
    let draft = if kind == "ask" {
        let given = [p.get("option").is_some_and(|v| !v.is_null()), p.get("other").is_some_and(|v| !v.is_null())].iter().filter(|b| **b).count();
        if given != 1 {
            return Err(bad(cmd, "give `option` (the number of a choice, from 0) or `other` (their own words), not both and not neither"));
        }
        if let Some(v) = p.get("option").filter(|v| !v.is_null()) {
            let count = card["options"].as_array().map_or(0, Vec::len);
            let i = v.as_u64().filter(|i| (*i as usize) < count).ok_or_else(|| bad(cmd, format!("`option` must be 0 to {}", count.saturating_sub(1))))?;
            Draft::Option(i as usize)
        } else {
            if card["allowOther"] != true {
                return Err(bad(cmd, "this question takes one of its choices only"));
            }
            let t = p
                .get("other")
                .and_then(|t| clean(t, MAX_OTHER))
                .ok_or_else(|| bad(cmd, format!("`other` must be 1 to {MAX_OTHER} characters of plain text")))?;
            Draft::Other(t)
        }
    } else if kind == "narration" {
        if matches!(s.assistant.cards.drafts.get(&key), Some(Draft::Yes)) {
            return Err(bad(cmd, "the narration has been started; it cannot be changed"));
        }
        match str_p(p, "decision") {
            Some("yes") => {
                // the person's own action: the voice is made now, and placed on the timeline when it is done
                crate::assistant_narration::start(s, thread, &card)?;
                Draft::Yes
            }
            Some("no") => Draft::No,
            Some("others") => Draft::Others,
            _ => return Err(bad(cmd, "`decision` must be yes, no or others")),
        }
    } else {
        // a download card
        let locked = matches!(s.assistant.cards.drafts.get(&key), Some(Draft::Yes));
        if locked {
            return Err(bad(cmd, "this download has been started; it cannot be changed"));
        }
        match str_p(p, "decision") {
            Some("yes") => {
                let cand = card["candidate"]["id"].as_str().unwrap_or_default().to_string();
                // the person's own action: the download starts now (a clip already coming or in the project is not fetched twice)
                if !matches!(s.hub.acquired(&cand), Some(Acquired::Downloading | Acquired::Done { .. })) {
                    s.execute("hub.acquire", json!({"id": cand, "wait": false}))?;
                }
                Draft::Yes
            }
            Some("no") => Draft::No,
            Some("others") => Draft::Others,
            _ => return Err(bad(cmd, "`decision` must be yes, no or others")),
        }
    };
    s.assistant.cards.drafts.insert(key, draft.clone());
    let mut out = json!({"thread": thread, "card": id, "draft": draft_json(&card, &draft)});
    if draft == Draft::Others && kind == "narration" {
        // the panel opens Text to Speech with the words typed in
        out["textToSpeech"] = json!({"text": card["text"], "voice": card["voice"]["id"]});
    } else if draft == Draft::Others {
        // the panel opens Find Media with this typed in
        out["findMedia"] = json!({"text": card["query"], "kind": card["searchKind"]});
    }
    Ok(out)
}

fn answer_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "assistant.answerCards";
    s.pump_assistant();
    s.pump_hub();
    crate::assistant_narration::pump(s);
    let thread = thread_of(s, p, cmd)?;
    let wait = bool_p(p, "wait").unwrap_or(false);
    let group = open_cards(s, thread);
    if group.is_empty() {
        return Err(bad(cmd, "there are no open cards to answer"));
    }
    if s.assistant.slot(thread).is_some_and(|t| lock(&t.shared.state).running) {
        return Err(bad(cmd, "the assistant is still working; send the answers when it has stopped"));
    }
    let downloading = group.iter().filter(|c| download_state(s, c).is_some_and(|d| d["state"] == "downloading")).count();
    if downloading > 0 {
        return Err(bad(
            cmd,
            format!(
                "{downloading} download{} still running: send the answers when {} finished, so the clip is in the project for the next turn",
                if downloading == 1 { " is" } else { "s are" },
                if downloading == 1 { "it has" } else { "they have" }
            ),
        ));
    }
    let voicing = group.iter().filter(|c| crate::assistant_narration::state_json(s, thread, c).is_some_and(|d| d["state"] == "rendering")).count();
    if voicing > 0 {
        return Err(bad(cmd, "the narration is still being made: send the answers when it has finished, so it is on the timeline for the next turn"));
    }
    let mut lines = Vec::new();
    let mut answers = Vec::new();
    for (n, c) in group.iter().enumerate() {
        let id = c["id"].as_str().unwrap_or("").to_string();
        let draft = s.assistant.cards.drafts.get(&(thread, id.clone())).cloned();
        let words = answer_words(s, thread, c, draft.as_ref());
        lines.push(format!("{}. {}: {}", n + 1, card_title(c), words));
        answers.push(CardAnswer { card: id, answer: words });
    }
    let text = format!("Here are my answers to your questions:\n{}", lines.join("\n"));
    let sent = s.execute("assistant.send", json!({"text": text, "thread": thread, "wait": wait}))?;
    s.assistant.cards.drafts.retain(|(t, _), _| *t != thread);
    if let Some(slot) = s.assistant.slot(thread) {
        push_event(&slot.shared, AgentEvent::CardsAnswered { answers });
    }
    Ok(sent)
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    let spec = |id: &'static str, label: &'static str, params: &'static str, run: fn(&mut Session, &Value) -> Result<Value>| CommandSpec {
        id,
        label,
        menu: &[],
        shortcut: None,
        params,
        enabled: always,
        run,
        journal: false,
    };
    vec![
        spec("assistant.cards", "Assistant Question Cards", r#"{"thread":id?}"#, cards_cmd),
        spec("assistant.card", "Answer a Question Card", r#"{"card":str,"option":u64?,"other":str?,"decision":"yes|no|others"?,"thread":id?}"#, card_cmd),
        spec("assistant.answerCards", "Send the Answers to the Assistant", r#"{"thread":id?,"wait":bool?}"#, answer_cmd),
    ]
}

// ---------------------------------------------------------------------------- the tool list

/// The model's side of the cards. They are answered by [`serve`] on the session thread; the function here is
/// only reached if something calls them where the thread is not known.
fn not_here(_: &Session, _: &Value) -> ToolOutput {
    ToolOutput::error("this tool works inside a conversation only")
}

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![
        def(
            "ask_user",
            "Ask the person a question as a card with choices, and END YOUR TURN. Use it for what is theirs to decide (taste, tone, how much to spend, rights, anything big or unclear), not for what you can find out with a tool. Ask everything you need in this turn (up to 6 cards in all, each its own call), then stop: their answers to all the cards arrive together as their next message. Give 2 to 6 short, distinct choices; they can also answer in their own words unless allowOther is false.",
            json!({
                "question": {"type": "string", "description": "One short question, up to 300 characters."},
                "options": {"type": "array", "description": "2 to 6 choices: a short phrase each, or {label, description}.", "items": {}},
                "allowOther": {"type": "boolean", "description": "Let them answer in their own words (default true)."},
            }),
            not_here,
        ),
        def(
            "request_download",
            "Offer the person a clip from a media_search result as a card (picture, where it is from, its licence, size) with Yes, No and Show me others, and END YOUR TURN as with ask_user. Nothing downloads unless they press Yes; you cannot download anything yourself. Say why this clip in `why`. Only offer a result whose licence is stated and allows commercial use. Counts toward the 6 cards of a turn.",
            json!({
                "candidate": {"type": "string", "description": "A result id from media_search, such as pexels:123."},
                "why": {"type": "string", "description": "Why this clip, in a sentence."},
            }),
            not_here,
        ),
        def(
            "media_search",
            "Search the free stock sites (and the person's own folders) for video, pictures or sound by plain words. Returns results with ids, titles, sizes and licences. Nothing is downloaded: to get one, offer it with request_download. Use it only when nothing in the project (project_items) fits, and search for what the idea shows, in plain words.",
            json!({
                "text": {"type": "string", "description": "What to look for, in plain words, up to 300 characters."},
                "kind": {"type": "string", "enum": ["video", "image", "audio"], "description": "Default video."},
                "orientation": {"type": "string", "enum": ["landscape", "portrait", "square"]},
                "minSeconds": {"type": "number", "description": "Shortest length, seconds (video and audio)."},
                "maxSeconds": {"type": "number", "description": "Longest length, seconds."},
                "count": {"type": "integer", "description": "How many results to list, 1 to 12. Default 8."},
            }),
            not_here,
        ),
    ]
}
