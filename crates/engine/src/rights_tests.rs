//! Tests of [`crate::rights`]: the policy, the audit of what a timeline uses against it, and the
//! credits block.

use super::*;
use crate::rights::POLICY_KEY;
use serde_json::json;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn item(s: &Session, name: &str) -> u64 {
    s.project.items.values().find(|i| i.name == name).map(|i| i.id.0).unwrap_or_else(|| panic!("no item {name}"))
}

fn set(s: &mut Session, name: &str, mut p: Value) {
    p["item"] = json!(item(s, name));
    s.execute("provenance.set", p).unwrap();
}

fn codes(audit: &Value, severity: &str) -> Vec<String> {
    audit["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] == severity)
        .map(|f| format!("{}:{}", f["code"].as_str().unwrap(), f["name"].as_str().unwrap()))
        .collect()
}

const USED: [&str; 7] =
    ["Ocean_Sunset.mp4", "Aurora_Timelapse.mp4", "City_Night_Drive.mp4", "Desert_Dunes.mp4", "Misty_Forest.mp4", "Neon_Loop.mov", "Ambient_Score.wav"];

#[test]
fn the_policy_starts_open_changes_with_undo_and_leaves_nothing_when_back_to_default() {
    let mut s = demo();
    let d = s.execute("provenance.policy", json!({})).unwrap();
    assert_eq!(d["policy"], json!({"commercial": true, "attributionOk": true, "syntheticOk": true}));
    s.execute("provenance.policy", json!({"attributionOk": false})).unwrap();
    assert_eq!(s.execute("provenance.policy", json!({})).unwrap()["policy"]["attributionOk"], false);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("provenance.policy", json!({})).unwrap()["policy"]["attributionOk"], true);
    s.execute("edit.redo", json!({})).unwrap();
    s.execute("provenance.policy", json!({"attributionOk": true})).unwrap();
    assert!(s.project.ext.is_empty(), "the default policy is not stored");
    let undo = s.history.undo.len();
    s.execute("provenance.policy", json!({"commercial": true})).unwrap();
    assert_eq!(s.history.undo.len(), undo, "setting what is already set is not an edit");
    for bad in [json!({"commercial": "yes"}), json!({"attributionOk": 1}), json!({"syntheticOk": []})] {
        assert!(s.execute("provenance.policy", bad.clone()).is_err(), "{bad}");
    }
    // a damaged value in the file is the default policy, not an error
    let mut p = (*s.project).clone();
    p.ext.insert(POLICY_KEY.to_string(), json!("garbage"));
    s.project = std::sync::Arc::new(p);
    assert_eq!(s.execute("provenance.policy", json!({})).unwrap()["policy"]["commercial"], true);
}

#[test]
fn an_edit_with_no_records_is_all_warnings_and_not_a_failure() {
    let mut s = demo();
    let a = s.execute("provenance.audit", json!({})).unwrap();
    assert_eq!((a["ok"].clone(), a["errors"].clone(), a["clipsChecked"].clone()), (json!(true), json!(0), json!(7)), "{a}");
    assert_eq!(a["warnings"], 7);
    assert!(codes(&a, "warning").iter().all(|c| c.starts_with("unknown_origin:")));
    // each warning says where the clip is
    let ocean = a["findings"].as_array().unwrap().iter().find(|f| f["name"] == "Ocean_Sunset.mp4").unwrap();
    assert_eq!(ocean["uses"][0]["timecode"], "00:00:00:00");
    let aurora = a["findings"].as_array().unwrap().iter().find(|f| f["name"] == "Aurora_Timelapse.mp4").unwrap();
    assert_eq!(aurora["uses"][0]["timecode"], "00:00:04:23");
    // audio uses the same clip: it is listed once for the item, with both places
    assert!(ocean["uses"].as_array().unwrap().len() >= 2, "{ocean}");
}

#[test]
fn the_audit_checks_every_clip_against_the_policy() {
    let mut s = demo();
    set(&mut s, "Ocean_Sunset.mp4", json!({"tag": "own"}));
    set(
        &mut s,
        "Aurora_Timelapse.mp4",
        json!({"source": "wikimedia", "author": "A. Person", "license": {"name": "CC BY 4.0", "commercial": true, "attributionRequired": true}}),
    );
    set(&mut s, "City_Night_Drive.mp4", json!({"source": "web", "license": {"name": "CC BY-NC", "commercial": false, "attributionRequired": true}}));
    set(&mut s, "Desert_Dunes.mp4", json!({"tag": "own", "synthetic": {"model": "some-model"}}));
    set(&mut s, "Misty_Forest.mp4", json!({"source": "web", "author": "someone"}));
    set(&mut s, "Neon_Loop.mov", json!({"tag": "licensed"}));
    set(&mut s, "Ambient_Score.wav", json!({"tag": "own"}));

    // open policy: the non-commercial clip is already an error because commercial defaults to on
    let a = s.execute("provenance.audit", json!({})).unwrap();
    assert_eq!(a["ok"], false);
    assert_eq!(codes(&a, "error"), ["not_commercial:City_Night_Drive.mp4"]);
    assert!(codes(&a, "warning").contains(&"terms_unknown:Misty_Forest.mp4".to_string()));
    assert!(codes(&a, "warning").contains(&"credit_unknown:Misty_Forest.mp4".to_string()));
    assert_eq!(codes(&a, "note"), ["ai_disclosure:Desert_Dunes.mp4"]);
    // errors sort before warnings before notes
    let sev: Vec<&str> = a["findings"].as_array().unwrap().iter().map(|f| f["severity"].as_str().unwrap()).collect();
    assert!(sev.windows(2).all(|w| crate::rights::rank(w[0]) <= crate::rights::rank(w[1])), "{sev:?}");

    // not a commercial project: the NC clip is fine
    s.execute("provenance.policy", json!({"commercial": false})).unwrap();
    let a = s.execute("provenance.audit", json!({})).unwrap();
    assert!(!codes(&a, "error").iter().any(|c| c.starts_with("not_commercial")));
    // credits not allowed, AI not allowed: each becomes an error naming the clip
    s.execute("provenance.policy", json!({"attributionOk": false, "syntheticOk": false})).unwrap();
    let a = s.execute("provenance.audit", json!({})).unwrap();
    let errs = codes(&a, "error");
    assert!(
        errs.contains(&"credit_not_allowed:Aurora_Timelapse.mp4".to_string()) && errs.contains(&"credit_not_allowed:City_Night_Drive.mp4".to_string()),
        "{errs:?}"
    );
    assert!(errs.contains(&"ai_not_allowed:Desert_Dunes.mp4".to_string()), "{errs:?}");
    assert_eq!(a["ok"], false);
    // fix everything: a clean audit
    for n in ["Aurora_Timelapse.mp4", "City_Night_Drive.mp4", "Desert_Dunes.mp4", "Misty_Forest.mp4"] {
        set(&mut s, n, json!({"tag": "licensed", "synthetic": null}));
    }
    let a = s.execute("provenance.audit", json!({})).unwrap();
    assert_eq!((a["ok"].clone(), a["errors"].clone(), a["warnings"].clone()), (json!(true), json!(0), json!(0)), "{a}");
}

#[test]
fn only_clips_that_are_really_in_the_edit_count() {
    let mut s = demo();
    let mut p = (*s.project).clone();
    let seq_id = s.state.active_sequence.unwrap();
    {
        let q = p.sequence_mut(seq_id).unwrap();
        // switch off the Neon_Loop video clip and mute the music track
        for t in q.video_tracks.iter_mut() {
            for i in t.items.iter_mut().filter(|i| i.name.starts_with("Neon_Loop")) {
                i.enabled = false;
            }
        }
        for t in q.audio_tracks.iter_mut() {
            if t.items.iter().any(|i| i.name.starts_with("Ambient")) {
                t.muted = true;
            }
        }
    }
    s.project = std::sync::Arc::new(p);
    let a = s.execute("provenance.audit", json!({})).unwrap();
    let names: Vec<&str> = a["findings"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert!(!names.contains(&"Ambient_Score.wav"), "a muted track is not heard: {names:?}");
    // Neon_Loop's audio is still on A1, so the item is still used, but only once
    let neon = a["findings"].as_array().unwrap().iter().find(|f| f["name"] == "Neon_Loop.mov").unwrap();
    assert_eq!(neon["uses"].as_array().unwrap().len(), 1, "{neon}");
}

#[test]
fn the_credits_block_lists_what_needs_a_credit_in_order_once_each() {
    let mut s = demo();
    set(&mut s, "Ocean_Sunset.mp4", json!({"tag": "own"}));
    set(
        &mut s,
        "Desert_Dunes.mp4",
        json!({"source": "pexels", "author": "Jane Doe", "license": {"name": "CC BY 4.0", "commercial": true, "attributionRequired": true}, "sourceUrl": "https://www.pexels.com/video/9/"}),
    );
    set(
        &mut s,
        "Aurora_Timelapse.mp4",
        json!({"source": "wikimedia", "license": {"commercial": true, "attributionRequired": true, "creditText": "Aurora by A. Person, CC BY 4.0"}}),
    );
    set(&mut s, "Neon_Loop.mov", json!({"source": "web", "license": {"name": "CC BY", "commercial": true, "attributionRequired": true}}));
    let c = s.execute("provenance.credits", json!({})).unwrap();
    assert_eq!(c["lines"].as_array().unwrap().len(), 3, "{c}");
    let lines: Vec<&str> = c["lines"].as_array().unwrap().iter().map(|l| l.as_str().unwrap()).collect();
    // in timeline order: Aurora (4s), Desert (13s), Neon (21s)
    assert_eq!(lines[0], "Aurora by A. Person, CC BY 4.0");
    assert_eq!(lines[1], "Desert_Dunes.mp4 by Jane Doe · CC BY 4.0 · https://www.pexels.com/video/9/");
    assert_eq!(lines[2], "Neon_Loop.mov · CC BY");
    assert_eq!(c["missingCreditText"], json!(["Neon_Loop.mov"]), "no author and no credit text: the line is thin");
    assert_eq!(c["unrecorded"], 3, "three used clips have no record");
    assert_eq!(c["text"].as_str().unwrap().lines().count(), 3);
    let md = s.execute("provenance.credits", json!({"format": "markdown"})).unwrap();
    assert!(md["text"].as_str().unwrap().lines().all(|l| l.starts_with("- ")));
    let yt = s.execute("provenance.credits", json!({"format": "youtube"})).unwrap();
    assert!(yt["text"].as_str().unwrap().starts_with("Credits:\n- Aurora by"));
    // nothing to credit: nothing printed, not an empty heading
    let mut clean = demo();
    set(&mut clean, "Ocean_Sunset.mp4", json!({"tag": "own"}));
    let none = clean.execute("provenance.credits", json!({"format": "youtube"})).unwrap();
    assert_eq!((none["text"].clone(), none["count"].clone()), (json!(""), json!(0)));
}

#[test]
fn the_other_sequence_and_hostile_parameters() {
    let mut s = demo();
    let rough = s.project.items.values().find(|i| i.name == "Rough Cut").map(|i| i.id.0).unwrap();
    let a = s.execute("provenance.audit", json!({"sequence": rough})).unwrap();
    assert_eq!(a["sequence"], rough);
    let snap = (*s.project).clone();
    let undo = s.history.undo.len();
    for (cmd, p) in [
        ("provenance.audit", json!({"sequence": "x"})),
        ("provenance.audit", json!({"sequence": 999_999})),
        ("provenance.audit", json!({"sequence": item(&s, "Ocean_Sunset.mp4")})),
        ("provenance.credits", json!({"format": "html"})),
        ("provenance.credits", json!({"format": 5})),
        ("provenance.credits", json!({"sequence": -1})),
    ] {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute(cmd, p.clone())));
        assert!(r.is_ok() && r.unwrap().is_err(), "{cmd} {p}");
    }
    assert_eq!(*s.project, snap);
    assert_eq!(s.history.undo.len(), undo);
    assert!(
        s.execute("provenance.audit", json!({})).is_ok() && Session::default().execute("provenance.audit", json!({})).is_err(),
        "no sequence is an error, not a panic"
    );
    for (id, journal) in [("provenance.policy", true), ("provenance.credits", false), ("provenance.audit", false)] {
        assert_eq!(crate::commands::command_specs().iter().find(|c| c.id == id).map(|c| c.journal), Some(journal), "{id}");
    }
    let _ = USED;
}
