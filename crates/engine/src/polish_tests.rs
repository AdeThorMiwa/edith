//! Tests of `edit.polish` (fork roadmap F31): transitions with restraint. Hard cuts inside an idea, a dip where a
//! section ends, a dissolve between quiet ideas, fades at the start and end, a slow move on a held still, never more
//! than the pack's share of the cuts, nothing replaced, nothing done when there is no picture to spare.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::Session;

fn edit() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name": "Polish", "audio": 1, "video": 1})).unwrap();
    s
}

fn media(s: &Session, name: &str) -> u64 {
    s.project.items.values().find(|i| i.name.contains(name)).map(|i| i.id.0).expect(name)
}

fn polish(s: &mut Session, p: Value) -> Value {
    s.execute("edit.polish", p).unwrap()
}

fn video(s: &Session) -> &filmcraft_project::Track {
    &s.active_sequence().unwrap().video_tracks[0]
}

fn png() -> Arc<[u8]> {
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(64, 48, image::Rgba([90, 120, 160, 255])))
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out.into()
}

/// Seven ideas of the mystery plan laid six seconds each from the start, three two-second shots in each, with the
/// clue ideas quiet (intensity 4) so that they qualify for dissolves.
fn seven_ideas() -> Session {
    let mut s = edit();
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 1, "clues": 4})).unwrap();
    let beats: Vec<(u64, String)> = crate::planner::get(&s.project).unwrap().beats().map(|b| (b.id, b.name.clone())).take(7).collect();
    let (ocean, city) = (media(&s, "Ocean_Sunset"), media(&s, "City_Night"));
    let mut entries = Vec::new();
    for (k, (id, name)) in beats.iter().enumerate() {
        s.execute("planner.link", json!({"beat": id, "startSeconds": k as f64 * 6.0, "durationSeconds": 6})).unwrap();
        if name.starts_with("Clue") {
            s.execute("planner.updateBeat", json!({"beat": id, "intensity": 4})).unwrap();
        }
        for third in 0..3 {
            let n = k * 3 + third;
            let start = n as f64 * 2.0;
            entries.push(json!({"start": start, "end": start + 2.0, "item": if n % 2 == 0 { ocean } else { city }, "sourceIn": 1.0 + (n % 4) as f64 * 2.5}));
        }
    }
    s.execute("timeline.placeCutList", json!({"entries": entries})).unwrap();
    s
}

fn effect_of(t: &filmcraft_project::Transition) -> String {
    t.effect.effect.clone()
}

#[test]
fn a_dip_where_a_section_ends_a_dissolve_between_quiet_ideas_and_the_rest_left_hard_within_the_share() {
    let mut s = seven_ideas();
    let before = s.active_sequence().unwrap().clone();
    let r = polish(&mut s, json!({"fades": false, "moves": false}));
    // 20 cuts and the pack allows 15% of them a transition: three. The two section ends come first, then one dissolve
    assert_eq!((r["dips"].clone(), r["dissolves"].clone(), r["hardCuts"].clone()), (json!(2), json!(1), json!(17)), "{r}");
    assert_eq!(r["keptHard"].as_array().unwrap().len(), 2, "{r}");
    let t = video(&s);
    let dips: Vec<f64> = t
        .transitions
        .iter()
        .filter(|x| effect_of(x) == "dip_to_black")
        .map(|x| x.start.0 as f64 / 254016000000.0 + x.duration.0 as f64 / 2.0 / 254016000000.0)
        .collect();
    assert_eq!(dips.len(), 2);
    assert!(dips.iter().any(|d| (d - 12.0).abs() < 0.1) && dips.iter().any(|d| (d - 18.0).abs() < 0.1), "at the ends of Opening and Setup: {dips:?}");
    assert!(t.transitions.iter().filter(|x| effect_of(x) == "cross_dissolve").count() == 1);
    assert!(t.transitions.iter().all(|x| x.from.is_some() && x.to.is_some()), "between shots, not at the ends");
    let said = r["summary"].as_str().unwrap();
    assert!(said.contains("2 dips to black") && said.contains("1 dissolve") && said.contains("17 cuts stay hard cuts"), "{said}");
    // asked again it changes nothing
    let again = polish(&mut s, json!({"fades": false, "moves": false}));
    assert_eq!((again["dips"].clone(), again["dissolves"].clone()), (json!(0), json!(0)), "{again}");
    assert_eq!(video(&s).transitions.len(), 3);
    // one undo step takes it all back
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(*s.active_sequence().unwrap(), before);
}

#[test]
fn the_start_and_the_end_fade_and_a_held_still_gets_a_slow_move_and_a_dry_run_changes_nothing() {
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    let still = crate::commands::import_bytes(&mut s, "/still.png", png(), None).unwrap();
    s.execute(
        "timeline.placeCutList",
        json!({"entries": [{"start": 0, "end": 4, "item": still.0}, {"start": 4, "end": 10, "item": ocean, "sourceIn": 2}, {"start": 10, "end": 12, "item": still.0}]}),
    )
    .unwrap();
    let before = s.active_sequence().unwrap().clone();
    let dry = polish(&mut s, json!({"dryRun": true}));
    assert_eq!(
        (dry["dryRun"].clone(), dry["fadeIn"].clone(), dry["fadeOut"].clone(), dry["stillsMoved"].clone()),
        (json!(true), json!(true), json!(true), json!(2)),
        "{dry}"
    );
    assert!(dry["summary"].as_str().unwrap().starts_with("Would add"), "{dry}");
    assert_eq!(*s.active_sequence().unwrap(), before, "a dry run changes nothing");
    let r = polish(&mut s, json!({}));
    assert_eq!((r["fadeIn"].clone(), r["fadeOut"].clone(), r["stillsMoved"].clone()), (json!(true), json!(true), json!(2)), "{r}");
    let t = video(&s);
    assert_eq!(t.transitions.len(), 2, "a fade in and a fade out, no dissolve between: there is no plan to say where ideas change");
    assert!(t.transitions.iter().any(|x| x.from.is_none() && x.to.is_some()) && t.transitions.iter().any(|x| x.from.is_some() && x.to.is_none()));
    assert!(r["summary"].as_str().unwrap().contains("only the fades and the stills were done"), "{r}");
    // the stills now move, to different moves
    let moves: Vec<bool> =
        t.items.iter().filter(|i| i.duration.0 > 0).map(|i| i.effects.iter().any(|e| e.params.iter().any(|(_, p)| !p.keyframes.is_empty()))).collect();
    assert_eq!(moves, [true, false, true], "{moves:?}");
    // asked again, nothing more; and the switches turn each part off
    let again = polish(&mut s, json!({}));
    assert_eq!((again["fadeIn"].clone(), again["fadeOut"].clone(), again["stillsMoved"].clone()), (json!(false), json!(false), json!(0)), "{again}");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(*s.active_sequence().unwrap(), before);
    let none = polish(&mut s, json!({"fades": false, "moves": false}));
    assert_eq!(none["summary"].as_str().unwrap().split('.').next().unwrap(), "Nothing needed adding");
}

#[test]
fn a_cut_with_no_spare_picture_is_skipped_and_said_and_a_bad_scope_is_refused() {
    let mut s = edit();
    s.execute("planner.create", json!({"pack": "mystery", "minutes": 1})).unwrap();
    let beats: Vec<u64> = crate::planner::get(&s.project).unwrap().beats().map(|b| b.id).collect();
    let (ocean, city) = (media(&s, "Ocean_Sunset"), media(&s, "City_Night"));
    // two sections' ideas at 0 to 4 and 4 to 8, then 8 to 12 (the third is the first of Setup, so the cut at 8 is a section end);
    // the shot that begins at 8 starts at the very start of its clip, so a dissolve has nothing before it to use
    for (k, b) in beats.iter().take(3).enumerate() {
        s.execute("planner.link", json!({"beat": b, "startSeconds": k as f64 * 4.0, "durationSeconds": 4})).unwrap();
    }
    s.execute(
        "timeline.placeCutList",
        json!({"entries": [{"start": 0, "end": 4, "item": ocean, "sourceIn": 3}, {"start": 4, "end": 8, "item": ocean, "sourceIn": 8}, {"start": 8, "end": 12, "item": city, "sourceIn": 0}]}),
    )
    .unwrap();
    let r = polish(&mut s, json!({"fades": false, "moves": false}));
    assert_eq!((r["dips"].clone(), r["skipped"].as_array().unwrap().len()), (json!(0), 1), "{r}");
    assert!(r["skipped"][0]["why"].as_str().unwrap().contains("no spare picture"), "{r}");
    assert!(s.execute("edit.polish", json!({"from": 10, "to": 5})).unwrap_err().to_string().contains("`to` must be after `from`"));
    assert!(s.execute("edit.polish", json!({"from": "nonsense"})).is_err());
}

/// The mean brightness of the picture at `t` seconds.
fn brightness(s: &Session, t: f64) -> f32 {
    let img = s.try_render_program_at(0.25, filmcraft_time::Tick::from_seconds_f64(t)).unwrap();
    let rgb: Vec<f32> = img.px.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
    rgb.iter().sum::<f32>() / rgb.len().max(1) as f32
}

#[test]
fn a_dip_to_black_is_dark_in_the_middle_and_a_fade_starts_from_black() {
    let mut s = seven_ideas();
    polish(&mut s, json!({"moves": false}));
    // the dip at 12 s (the end of the Opening section) goes through black: the middle is darker than either side
    let (mid, side) = (brightness(&s, 12.0), (brightness(&s, 11.0) + brightness(&s, 13.0)) / 2.0);
    assert!(mid < side * 0.5, "the dip's middle {mid} against the shots around it {side}");
    // the film fades in from black
    assert!(brightness(&s, 0.0) < brightness(&s, 1.5) * 0.4, "{} then {}", brightness(&s, 0.0), brightness(&s, 1.5));
}
