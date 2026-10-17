//! Tests of [`crate::soundmap`]: clips sorted into lanes by tag, narration and analysis, the map and its
//! findings on real sequences, the measurement of each lane over time, planned silences, and what goes stale.

use std::sync::Arc;

use filmcraft_project::ClipId;
use serde_json::{Value, json};

use crate::Session;

const SR: u32 = 48_000;

/// One signal on the timeline: a tone `db` loud for `seconds`, placed at `start` on audio track `track` (1-based),
/// tagged `tag` in Essential Sound when it is given.
struct Sig {
    track: usize,
    start: f64,
    seconds: f64,
    db: f64,
    tag: Option<&'static str>,
}

fn sig(track: usize, start: f64, seconds: f64, db: f64, tag: Option<&'static str>) -> Sig {
    Sig { track, start, seconds, db, tag }
}

fn tone(seconds: f64, db: f64) -> Vec<f32> {
    let amp = 10f64.powf(db / 20.0);
    (0..(seconds * f64::from(SR)) as usize)
        .flat_map(|i| {
            let v = (amp * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / f64::from(SR)).sin()) as f32;
            [v, v]
        })
        .collect()
}

/// A sequence with a picture under all of it, and the signals on audio tracks.
fn edit(sigs: &[Sig]) -> (Session, Vec<ClipId>) {
    let mut s = Session::default();
    let tracks = sigs.iter().map(|g| g.track).max().unwrap_or(1).max(2);
    s.execute("file.newSequence", json!({"name": "sm", "audio": tracks, "video": 1})).unwrap();
    let mut clips = Vec::new();
    for (k, g) in sigs.iter().enumerate() {
        let bytes: Arc<[u8]> = crate::previews::write_wav_f32(&tone(g.seconds, g.db), SR).into();
        let item = crate::commands::import_bytes(&mut s, &format!("/sig{k}.wav"), bytes, None).unwrap();
        let r = s.execute("timeline.place", json!({"item": item.0, "audioTrack": format!("A{}", g.track), "seconds": g.start})).unwrap();
        let clip = ClipId(r["clips"][0].as_u64().unwrap());
        if let Some(t) = g.tag {
            s.execute("essentialSound.setType", json!({"clips": [clip.0], "type": t})).unwrap();
        }
        clips.push(clip);
    }
    s.execute("edit.deselectAll", json!({})).unwrap();
    (s, clips)
}

fn map(s: &mut Session) -> Value {
    s.execute("sound.map", json!({})).unwrap()
}

fn lane<'a>(m: &'a Value, name: &str) -> &'a Value {
    m["lanes"].as_array().unwrap().iter().find(|l| l["lane"] == name).unwrap_or_else(|| panic!("no lane {name} in {m}"))
}

fn found(m: &Value) -> Vec<String> {
    m["findings"].as_array().unwrap().iter().map(|f| f["kind"].as_str().unwrap().to_string()).collect()
}

#[test]
fn the_command_exists_and_changes_nothing() {
    let all = crate::commands::command_specs();
    assert!(all.iter().any(|c| c.id == "sound.map"));
    let (mut s, _) = edit(&[sig(1, 0.0, 30.0, -20.0, Some("dialogue"))]);
    let before = s.project.clone();
    map(&mut s);
    assert!(Arc::ptr_eq(&before, &s.project), "reading the map is not an edit");
}

#[test]
fn narration_and_music_with_no_ambience_shows_a_map_with_the_gap() {
    let (mut s, _) = edit(&[sig(1, 0.0, 30.0, -20.0, Some("dialogue")), sig(2, 0.0, 30.0, -30.0, Some("music"))]);
    let m = map(&mut s);
    assert_eq!(m["measured"], false);
    assert!((m["durationSeconds"].as_f64().unwrap() - 30.0).abs() < 0.1);
    assert_eq!(lane(&m, "narration")["clips"].as_array().unwrap().len(), 1);
    assert_eq!(lane(&m, "music")["clips"].as_array().unwrap().len(), 1);
    assert!(lane(&m, "ambience")["spans"].as_array().unwrap().is_empty(), "the gap: nothing in the ambience lane");
    assert_eq!(lane(&m, "ambience")["share"], 0.0);
    assert!(lane(&m, "narration")["clips"][0]["name"].as_str().unwrap().contains("sig0"));
    assert_eq!(found(&m), ["noAmbience"], "{m}");
    // the coach says it too, among the rest of what it finds
    let c = s.execute("coach.findings", json!({})).unwrap();
    assert!(c["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "noAmbience"));
    // an ambience bed under all of it closes the gap
    let (mut with_bed, _) =
        edit(&[sig(1, 0.0, 30.0, -20.0, Some("dialogue")), sig(2, 0.0, 30.0, -30.0, Some("music")), sig(3, 0.0, 30.0, -45.0, Some("ambience"))]);
    let m = map(&mut with_bed);
    assert!(found(&m).is_empty(), "{m}");
    assert!((lane(&m, "ambience")["share"].as_f64().unwrap() - 1.0).abs() < 0.01);
}

#[test]
fn clips_nobody_has_sorted_are_not_called_missing_ambience() {
    let (mut s, _) = edit(&[sig(1, 0.0, 30.0, -20.0, Some("dialogue")), sig(2, 0.0, 30.0, -30.0, None)]);
    let m = map(&mut s);
    assert_eq!(lane(&m, "other")["clips"].as_array().unwrap().len(), 1, "the unsorted lane appears");
    assert!(found(&m).is_empty(), "the unsorted clip may be the bed: {m}");
}

#[test]
fn a_narration_made_here_is_narration_and_the_tags_pick_the_lane() {
    let (mut s, clips) = edit(&[sig(1, 0.0, 10.0, -20.0, None), sig(2, 0.0, 10.0, -20.0, Some("sfx"))]);
    // not narration, not tagged: unsorted
    assert_eq!(lane(&map(&mut s), "other")["clips"].as_array().unwrap().len(), 1);
    // tagged, it moves
    s.execute("essentialSound.setType", json!({"clips": [clips[0].0], "type": "ambience"})).unwrap();
    let m = map(&mut s);
    assert_eq!(lane(&m, "ambience")["clips"].as_array().unwrap().len(), 1);
    assert_eq!(lane(&m, "effects")["clips"].as_array().unwrap().len(), 1);
    assert!(m["lanes"].as_array().unwrap().iter().all(|l| l["lane"] != "other"));
    // a clip that is a narration made here goes in the narration lane with no tag at all
    s.execute("essentialSound.clearType", json!({"clips": [clips[0].0]})).unwrap();
    let item = s.active_sequence().unwrap().audio_tracks[0].items[0].item;
    let mut p = (*s.project).clone();
    p.narrations.insert(
        item,
        filmcraft_project::Narration {
            text: "Words.".into(),
            language: "en-US".into(),
            voice: "v".into(),
            pitch: Default::default(),
            pace: 1.0,
            take: 0,
            speech_duration: filmcraft_time::Tick(filmcraft_time::TICKS_PER_SECOND),
        },
    );
    s.project = Arc::new(p);
    assert_eq!(lane(&map(&mut s), "narration")["clips"].as_array().unwrap().len(), 1);
}

#[test]
fn a_muted_track_and_a_disabled_clip_sound_like_nothing() {
    let (mut s, clips) = edit(&[sig(1, 0.0, 30.0, -20.0, Some("dialogue")), sig(2, 0.0, 30.0, -30.0, Some("music"))]);
    let clips_in = |m: &Value, name: &str| lane(m, name)["clips"].as_array().unwrap().len();
    let m = map(&mut s);
    assert_eq!((clips_in(&m, "narration"), clips_in(&m, "music")), (1, 1));
    // switching a clip off (it is a toggle) takes it out of its lane
    s.execute("clip.enable", json!({"clips": [clips[1].0]})).unwrap();
    let m = map(&mut s);
    assert_eq!((clips_in(&m, "narration"), clips_in(&m, "music")), (1, 0));
    assert!(lane(&m, "music")["spans"].as_array().unwrap().is_empty());
    s.execute("clip.enable", json!({"clips": [clips[1].0]})).unwrap();
    assert_eq!(clips_in(&map(&mut s), "music"), 1);
    // a muted track is silent too
    s.execute("timeline.setTrack", json!({"track": "A1", "muted": true})).unwrap();
    let m = map(&mut s);
    assert_eq!((clips_in(&m, "narration"), clips_in(&m, "music")), (0, 1));
}

#[test]
fn dead_air_is_found_where_nothing_plays_and_a_planned_silence_is_not_dead_air() {
    let (mut s, _) =
        edit(&[sig(1, 0.0, 10.0, -20.0, Some("dialogue")), sig(1, 14.0, 16.0, -20.0, Some("dialogue")), sig(2, 0.0, 30.0, -50.0, Some("ambience"))]);
    // the bed runs all through, so nothing is dead
    assert!(!found(&map(&mut s)).contains(&"deadAir".to_string()));
    let (mut s, _) = edit(&[
        sig(1, 0.0, 10.0, -20.0, Some("dialogue")),
        sig(1, 14.0, 16.0, -20.0, Some("dialogue")),
        sig(2, 0.0, 10.0, -50.0, Some("ambience")),
        sig(2, 14.0, 16.0, -50.0, Some("ambience")),
    ]);
    let m = map(&mut s);
    let dead = m["findings"].as_array().unwrap().iter().find(|f| f["kind"] == "deadAir").cloned().unwrap_or_else(|| panic!("{m}"));
    assert!((dead["start"].as_i64().unwrap() - 10 * filmcraft_time::TICKS_PER_SECOND).abs() < filmcraft_time::TICKS_PER_SECOND / 10, "{dead}");
    assert!(dead["title"].as_str().unwrap().contains("4 s") && dead["title"].as_str().unwrap().contains("0:10"), "{dead}");
    // the plan asks for that silence: it is not dead air any more (a silence 10 s into the first beat)
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 1})).unwrap();
    let beat = crate::planner::get(&s.project).unwrap().beats().next().unwrap().id;
    s.execute("planner.addCue", json!({"beat": beat, "kind": "silence", "text": "after the name", "atSeconds": 10, "lengthSeconds": 4})).unwrap();
    let m = map(&mut s);
    assert!(!found(&m).contains(&"deadAir".to_string()), "{m}");
    assert_eq!(m["plannedSilences"], 1);
    // taking the cue away brings it back
    let cue = crate::planner::get(&s.project).unwrap().beats().next().unwrap().sound.cues[0].id;
    s.execute("planner.removeCue", json!({"cue": cue})).unwrap();
    assert!(found(&map(&mut s)).contains(&"deadAir".to_string()));
}

#[test]
fn measuring_gives_loudness_per_lane_and_goes_stale_when_a_clip_changes_lane() {
    let (mut s, clips) =
        edit(&[sig(1, 0.0, 12.0, -20.0, Some("dialogue")), sig(2, 0.0, 12.0, -32.0, Some("music")), sig(3, 0.0, 12.0, -44.0, Some("ambience"))]);
    let r = s.execute("analysis.run", json!({"sound": true, "wait": true})).unwrap();
    assert_eq!(r["cached"], false);
    let levels = &r["report"]["levels"];
    assert_eq!(levels["narration"].as_array().unwrap().len(), 24, "a reading every half second");
    assert!(levels["effects"].is_null(), "an empty lane is not measured");
    let m = map(&mut s);
    assert_eq!(m["measured"], true);
    let (n, mu, a) = (
        lane(&m, "narration")["medianLufs"].as_f64().unwrap(),
        lane(&m, "music")["medianLufs"].as_f64().unwrap(),
        lane(&m, "ambience")["medianLufs"].as_f64().unwrap(),
    );
    assert!(((n - mu) - 12.0).abs() < 1.0 && ((mu - a) - 12.0).abs() < 1.0, "narration {n}, music {mu}, ambience {a}");
    assert!(lane(&m, "narration")["levels"].as_array().is_some(), "the readings are in the map");
    assert!(s.execute("sound.map", json!({"levels": false})).unwrap()["lanes"][0].get("levels").is_none());
    // a second run is cached
    assert_eq!(s.execute("analysis.run", json!({"sound": true, "wait": true})).unwrap()["cached"], true);
    // the report is listed and current
    let list = s.execute("analysis.list", json!({})).unwrap();
    assert!(list["reports"].as_array().unwrap().iter().any(|e| e["kind"] == "sound" && e["current"] == true), "{list}");
    // tagging the music clip as effects changes what the lanes are: the measurement no longer applies
    s.execute("essentialSound.setType", json!({"clips": [clips[1].0], "type": "sfx"})).unwrap();
    let m = map(&mut s);
    assert_eq!(m["measured"], false, "it was measured with another sorting");
    let list = s.execute("analysis.list", json!({})).unwrap();
    assert!(list["reports"].as_array().unwrap().iter().any(|e| e["kind"] == "sound" && e["current"] == false));
}

#[test]
fn a_room_tone_that_steps_between_two_beds_is_flagged_once_measured() {
    let (mut s, _) =
        edit(&[sig(1, 0.0, 30.0, -20.0, Some("dialogue")), sig(2, 0.0, 15.0, -45.0, Some("ambience")), sig(2, 15.0, 15.0, -37.0, Some("ambience"))]);
    // before measuring there is nothing to say about levels
    assert!(!found(&map(&mut s)).contains(&"roomToneJump".to_string()));
    s.execute("analysis.run", json!({"sound": true, "wait": true})).unwrap();
    let m = map(&mut s);
    let jump = m["findings"].as_array().unwrap().iter().find(|f| f["kind"] == "roomToneJump").cloned().unwrap_or_else(|| panic!("{m}"));
    assert!((jump["start"].as_i64().unwrap() - 15 * filmcraft_time::TICKS_PER_SECOND).abs() < filmcraft_time::TICKS_PER_SECOND / 10, "{jump}");
    assert!((jump["numbers"]["lu"].as_f64().unwrap() - 8.0).abs() < 1.0, "{jump}");
    // the coach carries it too
    let c = s.execute("coach.findings", json!({})).unwrap();
    assert!(c["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "roomToneJump"));
}

#[test]
fn hostile_parameters_and_empty_timelines_are_refused_without_harm() {
    let mut none = Session::default();
    assert!(none.execute("sound.map", json!({})).is_err(), "no sequence open");
    assert!(none.execute("analysis.run", json!({"sound": true})).is_err());
    let mut empty = Session::default();
    empty.execute("file.newSequence", json!({"name": "e", "audio": 2, "video": 1})).unwrap();
    assert!(empty.execute("sound.map", json!({})).is_err(), "nothing on its tracks");
    assert!(empty.execute("analysis.run", json!({"sound": true})).is_err());
    let (mut s, _) = edit(&[sig(1, 0.0, 5.0, -20.0, Some("dialogue"))]);
    for bad in [json!({"sequence": 999_999}), json!({"sequence": "x"}), json!({"sequence": -1}), json!({"levels": "yes"})] {
        let r = s.execute("sound.map", bad.clone());
        assert!(r.is_err() || bad.get("levels").is_some(), "{bad}");
    }
    assert!(s.execute("analysis.run", json!({"sound": true, "sequence": 999_999})).is_err());
}

// ---------------------------------------------------------------------------- the assistant (F13.5)

fn tool(s: &Session, name: &str, input: Value) -> edith_ai::ToolOutput {
    s.run_assistant_tool(name, &input)
}

#[test]
fn the_sound_tools_exist_and_only_read() {
    let tools = crate::assistant_tools::tools();
    for name in ["sound_map", "sound_plan"] {
        assert!(tools.iter().find(|t| t.spec.name == name).unwrap_or_else(|| panic!("{name}")).read_only, "{name}");
    }
    let run = tools.iter().find(|t| t.spec.name == "analysis_run").unwrap();
    assert!(run.spec.input_schema["properties"]["sound"].is_object(), "the assistant can ask for the loudness to be measured");
    let show = tools.iter().find(|t| t.spec.name == "show").unwrap();
    assert!(show.spec.input_schema["properties"]["view"]["enum"].as_array().unwrap().iter().any(|v| v == "soundmap"));
}

#[test]
fn sound_map_tells_the_model_what_is_there_and_what_is_missing_in_words() {
    let (s, _) = edit(&[sig(1, 0.0, 30.0, -20.0, Some("dialogue")), sig(2, 0.0, 30.0, -30.0, Some("music"))]);
    let out = tool(&s, "sound_map", json!({}));
    assert!(!out.is_error, "{}", out.content);
    for want in [
        "loudness NOT measured yet",
        "analysis_run with sound true",
        "- Narration: 1 clip covering 30 s (100% of the video)",
        "- Ambience: nothing",
        "- Effects: nothing",
        "There is no ambience or room tone",
        "[look]",
    ] {
        assert!(out.content.contains(want), "{want:?} is not in\n{}", out.content);
    }
    assert!(!out.content.contains("noAmbience@") && !out.content.contains('@'), "no internal ids: {}", out.content);
    // unsorted clips are named as the reason the map cannot say more
    let (s, _) = edit(&[sig(1, 0.0, 30.0, -20.0, Some("dialogue")), sig(2, 0.0, 30.0, -30.0, None)]);
    let out = tool(&s, "sound_map", json!({}));
    assert!(out.content.contains("1 clip is not sorted into a lane") && out.content.contains("Essential Sound"), "{}", out.content);
    assert!(out.content.contains("Findings: none."), "{}", out.content);
    // measured: the levels are quoted
    let (mut s, _) = edit(&[sig(1, 0.0, 12.0, -20.0, Some("dialogue")), sig(2, 0.0, 12.0, -44.0, Some("ambience"))]);
    s.execute("analysis.run", json!({"sound": true, "wait": true})).unwrap();
    let out = tool(&s, "sound_map", json!({}));
    assert!(out.content.contains("loudness measured") && out.content.contains("typically -2"), "{}", out.content);
    // refusals
    assert!(tool(&Session::default(), "sound_map", json!({})).is_error);
    assert!(tool(&s, "sound_map", json!({"sequence": 999_999})).is_error);
    assert!(tool(&s, "sound_map", json!({"sequence": "x"})).is_error);
    for odd in [json!(null), json!([1]), json!("x")] {
        let _ = tool(&s, "sound_map", odd);
    }
}

#[test]
fn sound_plan_gives_the_spotting_sheet_and_refuses_without_a_plan() {
    let (mut s, _) = edit(&[sig(1, 0.0, 30.0, -20.0, Some("dialogue"))]);
    assert!(tool(&s, "sound_plan", json!({})).is_error, "no plan");
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 5})).unwrap();
    let beat = crate::planner::get(&s.project).unwrap().beats().next().unwrap().id;
    s.execute("planner.setSound", json!({"beat": beat, "ambience": "wind over a shore"})).unwrap();
    let out = tool(&s, "sound_plan", json!({}));
    assert!(!out.is_error && out.content.contains("SPOTTING SHEET") && out.content.contains("Ambience: wind over a shore"), "{}", out.content);
    let one = tool(&s, "sound_plan", json!({"beat": beat}));
    assert!(one.content.contains("Cold open") && !one.content.contains("The question"), "{}", one.content);
    assert!(tool(&s, "sound_plan", json!({"beat": 999_999})).is_error);
    assert!(tool(&s, "sound_plan", json!({"beat": "x"})).is_error);
}

#[test]
fn the_soundmap_view_carries_the_lanes_the_silences_and_the_findings_for_the_person() {
    let (mut s, _) = edit(&[sig(1, 0.0, 30.0, -20.0, Some("dialogue")), sig(2, 0.0, 30.0, -30.0, Some("music"))]);
    let out = tool(&s, "show", json!({"view": "soundmap", "caption": "What the sound is missing"}));
    assert!(!out.is_error, "{}", out.content);
    let v = out.view.clone().expect("a view");
    assert_eq!((v["view"].as_str(), v["caption"].as_str(), v["measured"].as_bool()), (Some("soundmap"), Some("What the sound is missing"), Some(false)));
    let lanes: Vec<&str> = v["lanes"].as_array().unwrap().iter().map(|l| l["lane"].as_str().unwrap()).collect();
    assert_eq!(lanes, ["narration", "music", "ambience", "effects"]);
    assert!(v["lanes"][0]["spans"].as_array().is_some_and(|a| !a.is_empty()));
    assert!(v["lanes"][0]["levels"].is_null(), "not measured");
    assert_eq!(v["findings"][0]["kind"], "noAmbience");
    assert!(out.content.contains("Shown to the person") && out.content.contains("loudness has not been measured"), "{}", out.content);
    // measured, the readings come thinned; planned silences are carried
    s.execute("analysis.run", json!({"sound": true, "wait": true})).unwrap();
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 1})).unwrap();
    let beat = crate::planner::get(&s.project).unwrap().beats().next().unwrap().id;
    s.execute("planner.addCue", json!({"beat": beat, "kind": "silence", "atSeconds": 3, "lengthSeconds": 2})).unwrap();
    let v = tool(&s, "show", json!({"view": "soundmap"})).view.unwrap();
    assert!(v["measured"].as_bool().unwrap());
    assert!(v["lanes"][0]["levels"].as_array().is_some_and(|a| !a.is_empty() && a.len() <= 240));
    assert_eq!(v["silences"].as_array().unwrap().len(), 1);
    assert!(!v.to_string().contains("jpeg"));
    // hostile
    assert!(tool(&Session::default(), "show", json!({"view": "soundmap"})).is_error);
    assert!(tool(&s, "show", json!({"view": "soundmap", "sequence": 999_999})).is_error);
}
