//! Tests of `project_items` and of the clock-time reading the assistant's tools share.

use serde_json::json;

use crate::Session;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn items(s: &Session, input: serde_json::Value) -> edith_ai::ToolOutput {
    s.run_assistant_tool("project_items", &input)
}

#[test]
fn the_media_in_the_project_is_listed_with_ids_kinds_lengths_and_rights() {
    let mut s = demo();
    let out = items(&s, json!({}));
    assert!(!out.is_error, "{}", out.content);
    for want in [
        "media items in the project",
        "Ocean_Sunset",
        "Aurora_Timelapse",
        "video",
        "with sound",
        "no record of where it came from, so its rights are not known",
    ] {
        assert!(out.content.contains(want), "{want:?} is not in\n{}", out.content);
    }
    // the ids are the ones the other tools take
    let id: u64 = out
        .content
        .lines()
        .find(|l| l.contains("Ocean_Sunset"))
        .and_then(|l| l.strip_prefix('#'))
        .and_then(|l| l.split(' ').next())
        .and_then(|n| n.parse().ok())
        .expect("an id");
    assert!(s.project.item(filmcraft_project::ItemId(id)).is_some_and(|i| i.name.contains("Ocean_Sunset")));
    assert!(out.content.contains("used in"), "clips already on a timeline say so: {}", out.content);
    // a recorded licence is shown instead of the unknown
    s.execute("provenance.set", json!({"item": id, "source": "own", "license": {"name": "Own work", "commercial": true, "attributionRequired": false}}))
        .unwrap();
    let again = items(&s, json!({"query": "ocean"}));
    let line = again.content.lines().find(|l| l.contains("Ocean_Sunset")).unwrap();
    assert!(line.contains("Your own work") && line.contains("commercial use allowed") && !line.contains("not known"), "{line}");
    // filters
    let sounds = items(&s, json!({"kind": "audio"}));
    assert!(sounds.content.contains("Ambient_Score") && !sounds.content.contains("Ocean_Sunset"), "{}", sounds.content);
    assert!(items(&s, json!({"query": "zzzz"})).content.contains("No media in the project matches"));
    assert!(items(&s, json!({"kind": "banana"})).is_error);
    for odd in [json!(null), json!([1]), json!("x"), json!({"query": 5}), json!({"query": "a".repeat(10_000)})] {
        let _ = items(&s, odd);
    }
    // an empty project has nothing to list
    assert!(items(&Session::default(), json!({})).content.contains("No media"));
}

#[test]
fn the_tool_is_read_only_and_in_every_mode_and_the_overview_still_counts() {
    let t = crate::assistant_tools::tools().into_iter().find(|t| t.spec.name == "project_items").expect("the tool");
    assert!(t.read_only);
    let mut s = demo();
    for mode in ["ask", "coach", "edit"] {
        s.execute("assistant.settings", json!({"mode": mode})).unwrap();
        assert!(!items(&s, json!({})).is_error, "{mode}");
    }
}

#[test]
fn two_and_three_part_times_are_clock_times_and_four_parts_are_timecode() {
    let s = demo();
    let q = s.active_sequence().unwrap();
    let t = |v: serde_json::Value| crate::assistant_tools::parse_time(&v, q, "t").map(|t| t.0 as f64 / filmcraft_time::TICKS_PER_SECOND as f64);
    let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
    assert!(close(t(json!("0:00:08")).unwrap(), 8.0), "the slip the spike found: eight seconds, not eight frames");
    assert!(close(t(json!("1:25")).unwrap(), 85.0));
    assert!(close(t(json!("0:01:25")).unwrap(), 85.0));
    assert!(close(t(json!("1:00:00")).unwrap(), 3600.0));
    assert!(close(t(json!("12")).unwrap(), 12.0));
    assert!(close(t(json!(12.5)).unwrap(), 12.5));
    assert!(close(t(json!("00:00:02:00")).unwrap(), 2.0));
    let one_frame = t(json!("00:00:00:01")).unwrap();
    assert!((one_frame - 1.0 / 23.976).abs() < 1e-3, "{one_frame}");
    for bad in [json!("1:2:3:4:5"), json!("a:b"), json!("::"), json!("-1:00"), json!(-3), json!(1e30), json!("1:")] {
        assert!(t(bad.clone()).is_err(), "{bad}");
    }
}

// ------------------------------------------------------------------------------ attaching files (F16.1c)

fn first_media(s: &Session, name: &str) -> u64 {
    s.project.items.values().find(|i| i.name.contains(name)).map(|i| i.id.0).expect(name)
}

/// Every text part the model was sent in its first request, joined.
fn sent(model: &edith_ai::scripted::ScriptedModel) -> String {
    model
        .requests()
        .first()
        .map(|r| {
            r.messages
                .iter()
                .flat_map(|m| m.parts.iter())
                .filter_map(|p| if let edith_ai::Part::Text { text, .. } = p { Some(text.clone()) } else { None })
                .collect::<Vec<_>>()
                .join("\n---\n")
        })
        .unwrap_or_default()
}

#[test]
fn an_attached_project_file_reaches_the_model_with_what_is_known_about_it() {
    let mut s = demo();
    let id = first_media(&s, "Ocean_Sunset");
    let model = crate::assistant_tests::scripted(vec![crate::assistant_tests::text_reply("Noted.")]);
    s.assistant.use_model(model.clone());
    let r = s.execute("assistant.send", json!({"text": "what do you make of this clip?", "wait": true, "context": [{"kind": "item", "item": id}]})).unwrap();
    assert_eq!(r["finish"]["kind"], "done", "{r}");
    let text = sent(&model);
    assert!(
        text.contains("file: Ocean_Sunset") && text.contains(&format!("#{id} ")) && text.contains("rights:") && text.contains("Not measured yet"),
        "{text}"
    );
    assert!(text.contains("what do you make of this clip?"));
    // the person sees it listed among what was sent along
    let v = s.execute("assistant.thread", json!({"thread": r["thread"]})).unwrap();
    let attached = v["events"].as_array().unwrap().iter().find(|e| e["type"] == "user_message").unwrap()["attached"].clone();
    assert!(attached.as_array().unwrap().iter().any(|a| a.as_str().is_some_and(|l| l.starts_with("file: Ocean_Sunset"))), "{attached}");
    // measured, and with a transcript, it says so
    s.execute("analysis.run", json!({"item": id, "samples": 2, "wait": true})).unwrap();
    s.execute(
        "transcript.set",
        json!({"item": id, "transcript": {"language": "en", "words": [{"text": "Hello", "start": 0, "end": 1000}, {"text": "there.", "start": 2000, "end": 3000}]}}),
    )
    .unwrap();
    let model = crate::assistant_tests::scripted(vec![crate::assistant_tests::text_reply("Ok.")]);
    s.assistant.use_model(model.clone());
    s.execute("assistant.send", json!({"text": "and now?", "wait": true, "context": [{"kind": "item", "item": id}]})).unwrap();
    let text = sent(&model);
    assert!(
        text.contains("Measured:") && !text.contains("Not measured yet") && text.contains("Transcript: 2 words") && text.contains("Hello there."),
        "{text}"
    );
}

#[test]
fn only_media_in_the_project_can_be_attached_and_never_too_many() {
    let mut s = demo();
    let id = first_media(&s, "Ocean_Sunset");
    let seq = s.project.items.values().find(|i| matches!(i.kind, filmcraft_project::ItemKind::Sequence(_))).map(|i| i.id.0).unwrap();
    s.assistant.use_model(crate::assistant_tests::scripted(vec![crate::assistant_tests::text_reply("x")]));
    for (ctx, want) in [
        (json!([{"kind": "item", "item": 999_999}]), "no file 999999"),
        (json!([{"kind": "item", "item": seq}]), "not a file in the project"),
        (json!([{"kind": "item"}]), "needs `item`"),
        (json!([{"kind": "item", "item": "one"}]), "needs `item`"),
        (json!([{"kind": "item", "item": -1}]), "needs `item`"),
        (json!((0..7).map(|_| json!({"kind": "item", "item": id})).collect::<Vec<_>>()), "at most 6 project files"),
    ] {
        let e = s.execute("assistant.send", json!({"text": "look", "context": ctx})).unwrap_err().to_string();
        assert!(e.contains(want), "{want}: {e}");
    }
    assert!(
        s.assistant.threads.is_empty() || s.execute("assistant.threads", json!({})).unwrap()["threads"].as_array().unwrap().iter().all(|t| t["events"] == 0),
        "a refused message leaves no conversation"
    );
}

#[test]
fn six_files_and_the_playhead_chips_travel_together() {
    let mut s = demo();
    let ids: Vec<u64> = s.project.items.values().filter(|i| matches!(i.kind, filmcraft_project::ItemKind::Media(_))).map(|i| i.id.0).take(6).collect();
    assert_eq!(ids.len(), 6);
    let model = crate::assistant_tests::scripted(vec![crate::assistant_tests::text_reply("ok")]);
    s.assistant.use_model(model.clone());
    let mut ctx: Vec<serde_json::Value> = ids.iter().map(|i| json!({"kind": "item", "item": i})).collect();
    ctx.push(json!({"label": "playhead", "text": "The playhead is at 0:04."}));
    s.execute("assistant.send", json!({"text": "compare these", "wait": true, "context": ctx})).unwrap();
    let text = sent(&model);
    for i in ids {
        assert!(text.contains(&format!("#{i} ")), "{i} missing");
    }
    assert!(text.contains("The playhead is at 0:04."));
}
