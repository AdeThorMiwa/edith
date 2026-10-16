//! Tests of [`crate::pronounce`]: the project and global lists, their precedence, undo, saving,
//! hostile input, and that a changed word changes what a narration is rendered from.

use super::*;
use crate::pronounce::{FILE, KEY, effective, for_call};
use filmcraft_tts as tts;
use serde_json::json;

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("edith-pronounce-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn set(s: &mut Session, word: &str, say: &str, scope: &str) -> Value {
    s.execute("tts.setPronunciation", json!({"word": word, "say": say, "scope": scope})).unwrap()
}

fn say_of(s: &Session, word: &str) -> Option<String> {
    effective(s).0.get(word).cloned()
}

#[test]
fn the_commands_exist_and_only_the_changing_ones_are_journaled() {
    let all = crate::commands::command_specs();
    for (id, journal) in [("tts.pronunciations", false), ("tts.setPronunciation", true), ("tts.removePronunciation", true), ("tts.checkScript", false)] {
        let c = all.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("{id}"));
        assert_eq!(c.journal, journal, "{id}");
    }
}

#[test]
fn a_project_word_is_saved_in_the_project_and_is_one_undo_step() {
    let mut s = demo();
    let r = set(&mut s, "  Kubrick ", " koo brick ", "project");
    assert_eq!(
        (r["word"].clone(), r["say"].clone(), r["scope"].clone(), r["changed"].clone()),
        (json!("kubrick"), json!("koo brick"), json!("project"), json!(true))
    );
    assert_eq!(say_of(&s, "kubrick").as_deref(), Some("koo brick"));
    assert_eq!(s.project.ext[KEY]["kubrick"], "koo brick");
    // the same words again change nothing and add no undo step
    let steps = s.history.undo.len();
    assert_eq!(set(&mut s, "kubrick", "koo brick", "project")["changed"], false);
    assert_eq!(s.history.undo.len(), steps);
    // a change is one step, and undo brings the old way back
    set(&mut s, "Kubrick", "kyoo brick", "project");
    assert_eq!(s.history.undo.len(), steps + 1);
    s.undo().unwrap();
    assert_eq!(say_of(&s, "kubrick").as_deref(), Some("koo brick"));
    s.undo().unwrap();
    assert!(say_of(&s, "kubrick").is_none() && !s.project.ext.contains_key(KEY), "an empty list leaves nothing in the project");
    s.redo().unwrap();
    assert_eq!(say_of(&s, "kubrick").as_deref(), Some("koo brick"));
    // removing
    s.execute("tts.removePronunciation", json!({"word": "KUBRICK"})).unwrap();
    assert!(say_of(&s, "kubrick").is_none());
    assert!(s.execute("tts.removePronunciation", json!({"word": "kubrick"})).is_err(), "nothing left to remove");
}

#[test]
fn the_global_list_is_remembered_and_the_project_wins_over_it() {
    let data = scratch("global");
    let mut s = Session { prefs_path: Some(data.join("preferences.json")), ..Default::default() };
    s.execute("file.openDemoProject", json!({})).unwrap();
    let steps = s.history.undo.len();
    set(&mut s, "reykjavik", "rake yah veek", "global");
    set(&mut s, "kubrick", "koo brick", "global");
    assert_eq!(s.history.undo.len(), steps, "the global list is not part of any project's history");
    assert!(!s.project.ext.contains_key(KEY));
    // a new session on the same data folder finds them
    let mut again = Session { prefs_path: Some(data.join("preferences.json")), ..Default::default() };
    assert_eq!(effective(&again).0.len(), 2);
    again.execute("file.openDemoProject", json!({})).unwrap();
    // the project's way wins for this project only
    set(&mut again, "kubrick", "kyoo brick", "project");
    assert_eq!(say_of(&again, "kubrick").as_deref(), Some("kyoo brick"));
    assert_eq!(say_of(&again, "reykjavik").as_deref(), Some("rake yah veek"));
    let r = again.execute("tts.pronunciations", json!({})).unwrap();
    assert_eq!((r["project"].as_array().unwrap().len(), r["global"].as_array().unwrap().len(), r["count"].clone()), (1, 2, json!(2)));
    again.execute("tts.removePronunciation", json!({"word": "kubrick", "scope": "global"})).unwrap();
    assert_eq!(say_of(&again, "kubrick").as_deref(), Some("kyoo brick"), "still the project's");
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn a_damaged_or_hostile_list_is_ignored_entry_by_entry() {
    let data = scratch("damaged");
    let file = data.join(FILE);
    for text in ["{ not json", "[1,2]", "", "{\"a\": 5}"] {
        std::fs::write(&file, text).unwrap();
        let s = Session { prefs_path: Some(data.join("preferences.json")), ..Default::default() };
        assert!(effective(&s).0.is_empty(), "{text}");
    }
    let long = "x".repeat(300);
    let doc = json!({"fine": "fyn", "two words": "x", "bad": "/not closed", "ctl": "a\u{7}b", "": "x", "long": long, "ok-one": "oh kay"});
    std::fs::write(&file, doc.to_string()).unwrap();
    let s = Session { prefs_path: Some(data.join("preferences.json")), ..Default::default() };
    assert_eq!(effective(&s).0.keys().cloned().collect::<Vec<_>>(), ["fine", "ok-one"]);
    // a list in a project file gets the same treatment
    let mut s = demo();
    s.project = std::sync::Arc::new({
        let mut p = (*s.project).clone();
        p.ext.insert(KEY.to_string(), json!({"good": "gud", "bad word": "x", "n": 5}));
        p
    });
    assert_eq!(effective(&s).0.keys().cloned().collect::<Vec<_>>(), ["good"]);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn hostile_parameters_are_refused_and_nothing_changes() {
    let mut s = demo();
    let before = (*s.project).clone();
    for (cmd, p) in [
        ("tts.setPronunciation", json!({})),
        ("tts.setPronunciation", json!({"word": "a"})),
        ("tts.setPronunciation", json!({"word": 5, "say": "x"})),
        ("tts.setPronunciation", json!({"word": "two words", "say": "x"})),
        ("tts.setPronunciation", json!({"word": "a", "say": ""})),
        ("tts.setPronunciation", json!({"word": "a", "say": "/bad"})),
        ("tts.setPronunciation", json!({"word": "a", "say": "x", "scope": "everyone"})),
        ("tts.setPronunciation", json!({"word": "a", "say": "x", "scope": 3})),
        ("tts.removePronunciation", json!({})),
        ("tts.removePronunciation", json!({"word": "never-added"})),
        ("tts.removePronunciation", json!({"word": "x", "scope": "all"})),
        ("tts.checkScript", json!({})),
        ("tts.checkScript", json!({"text": 5})),
        ("tts.checkScript", json!({"text": "hi", "pronounce": [1]})),
        ("tts.checkScript", json!({"text": "hi", "pronounce": {"bad word": "x"}})),
        ("tts.preview", json!({"text": "hi", "pronounce": "x"})),
        ("tts.preview", json!({"text": "hi", "pronounce": {"a": 5}})),
    ] {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute(cmd, p.clone())));
        assert!(r.is_ok() && r.unwrap().is_err(), "{cmd} accepted {p}");
    }
    assert_eq!(*s.project, before);
}

#[test]
fn a_changed_word_changes_what_a_narration_is_rendered_from_and_a_trial_word_is_not_saved() {
    let mut s = demo();
    let n = filmcraft_project::Narration {
        text: "Hello Kubrick.".into(),
        language: "en-US".into(),
        voice: tts::default_voice_id().into(),
        pitch: Default::default(),
        pace: 1.0,
        take: 0,
        speech_duration: filmcraft_time::Tick::ZERO,
    };
    let none = effective(&s);
    set(&mut s, "kubrick", "koo brick", "project");
    let with = effective(&s);
    assert_ne!(crate::narration::cache_key(&n, &none), crate::narration::cache_key(&n, &with));
    // a trial word applies to that one call only
    let trial = for_call(&s, "tts.preview", &json!({"pronounce": {"Kubrick": "kyoo brick"}})).unwrap();
    assert_eq!(trial.0["kubrick"], "kyoo brick");
    assert_eq!(effective(&s).0["kubrick"], "koo brick");
    // the built-in voice ignores the list, so the preview still works with it
    s.execute("tts.preview", json!({"text": "Hello Kubrick.", "pronounce": {"kubrick": "kyoo brick"}})).unwrap();
    assert!(s.tts_preview.is_some());
}

#[test]
fn checking_a_script_says_so_when_the_natural_voices_are_not_installed() {
    let data = scratch("nomodels");
    let mut s = Session { prefs_path: Some(data.join("preferences.json")), ..Default::default() };
    // the voice models directory comes from the data directory, which has none here
    let r = s.execute("tts.checkScript", json!({"text": "Kubrick filmed in Reykjavik."})).unwrap();
    if r["available"] == json!(false) {
        assert!(r["reason"].as_str().unwrap().contains("natural voices"));
        assert!(r["words"].as_array().unwrap().is_empty());
    }
    // a script with nothing to say is an error where the voices could be checked at all
    if cfg!(feature = "neural-voices") {
        assert!(s.execute("tts.checkScript", json!({"text": "   "})).is_err());
    }
    let _ = std::fs::remove_dir_all(&data);
}

/// With the natural voices installed on this machine: invented names are flagged (real ones like Kubrick are already in the dictionary), and fixing them changes
/// the audio. Run it with `cargo test -p filmcraft-engine --features neural-voices --lib -- --ignored real_voices`.
#[test]
#[ignore = "needs the downloaded natural voices"]
fn real_voices_flag_names_and_a_fix_changes_the_audio() {
    let Some(dir) = crate::transcript::models_dir().filter(|d| tts::catalog::installed(d)) else {
        eprintln!("natural voices are not installed: skipped");
        return;
    };
    let _ = dir;
    let mut s = demo();
    let text = "Zorblat filmed in Vlornak with Quenzel.";
    let r = s.execute("tts.checkScript", json!({"text": text})).unwrap();
    assert_eq!(r["available"], true, "{r}");
    let words: Vec<String> = r["words"].as_array().unwrap().iter().map(|w| w["word"].as_str().unwrap().to_lowercase()).collect();
    for w in ["zorblat", "vlornak", "quenzel"] {
        assert!(words.contains(&w.to_string()), "{w} should be flagged: {words:?}");
    }
    assert!(!words.contains(&"filmed".to_string()) && !words.contains(&"with".to_string()), "{words:?}");
    let voice = tts::catalog::VOICES.first().unwrap().info.id;
    let wav = |s: &mut Session, name: &str| {
        let path = std::env::temp_dir().join(format!("edith-pronounce-{}-{name}.wav", std::process::id()));
        s.execute("tts.preview", json!({"text": text, "voice": voice, "path": path.to_string_lossy()})).unwrap();
        std::fs::read(&path).unwrap()
    };
    let before = wav(&mut s, "before");
    set(&mut s, "zorblat", "zor blot", "project");
    set(&mut s, "vlornak", "vuh lor nack", "project");
    set(&mut s, "quenzel", "kwen zel", "project");
    let r = s.execute("tts.checkScript", json!({"text": text})).unwrap();
    assert!(r["words"].as_array().unwrap().is_empty(), "{r}");
    let after = wav(&mut s, "after");
    assert_ne!(before, after, "the fixes change the audio");
    assert_eq!(after, wav(&mut s, "after-again"), "and it is repeatable");
    // takes: other deliveries of the same words, each repeatable, take 0 unchanged by their existence
    let take = |s: &mut Session, take: u64| {
        let path = std::env::temp_dir().join(format!("edith-pronounce-{}-take{take}.wav", std::process::id()));
        s.execute("tts.preview", json!({"text": text, "voice": voice, "take": take, "path": path.to_string_lossy()})).unwrap();
        std::fs::read(&path).unwrap()
    };
    let (t0, t2) = (take(&mut s, 0), take(&mut s, 2));
    assert_eq!(t0, after, "take 0 is the plain reading");
    assert_ne!(t0, t2, "another take is another delivery");
    assert_eq!(t2, take(&mut s, 2), "and a take repeats");
}
