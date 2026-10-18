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
