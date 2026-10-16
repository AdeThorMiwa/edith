//! The Film Look pack through the editor (fork roadmap F07): effects appear in the Effects panel's
//! Film Look folder, apply to a clip as one undo step, change the rendered frame, and leave the
//! project as it was when removed. Set `EDITH_SAMPLES_DIR` to keep before/after pictures to look at.

use filmcraft_media::{DemoScene, Generator, generators::GeneratorSource};
use filmcraft_project::Label;
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};
use std::sync::Arc;

use crate::Session;

const W: u32 = 960;
const H: u32 = 540;

fn session() -> (Session, u64) {
    session_with(DemoScene::Aurora)
}

fn session_with(scene: DemoScene) -> (Session, u64) {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"width": W, "height": H, "fps": 24})).unwrap();
    let item = crate::demo::add_generator(
        Arc::make_mut(&mut s.project),
        &s.media,
        GeneratorSource::new(Generator::Demo(scene), W, H, FrameRate::FPS_24, Tick(5 * TICKS_PER_SECOND)),
        "Source clip",
        Label::Iris,
        None,
    );
    let r = s.execute("timeline.place", json!({"item": item.0, "track": "V1"})).unwrap();
    let clip = r["clips"][0]
        .as_u64()
        .or_else(|| r["clip"].as_u64())
        .unwrap_or_else(|| s.active_sequence().unwrap().all_tracks().flat_map(|t| t.items.iter()).next().unwrap().id.0);
    s.set_playhead(FrameRate::FPS_24.tick_of(30));
    (s, clip)
}

fn frame(s: &mut Session, name: &str) -> Vec<u8> {
    let dir = std::env::var("EDITH_SAMPLES_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join(format!("edith-look-{}", std::process::id())));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.png"));
    s.execute("file.exportFrame", json!({"target": "program", "path": path.to_string_lossy()})).unwrap();
    image::load_from_memory(&std::fs::read(path).unwrap()).unwrap().to_rgba8().into_raw()
}

fn effect_count(s: &Session, clip: u64) -> usize {
    s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(clip)).unwrap().1.effects.iter().filter(|e| e.effect == "film_grain").count()
}

#[test]
fn the_pack_is_listed_in_the_effects_panel_and_film_grain_applies_renders_and_undoes() {
    let (mut s, clip) = session();
    let listed = s.execute("effects.list", json!({"folder": "Video Effects/Film Look"})).unwrap();
    let names: Vec<String> = listed.to_string().matches("Film Grain").map(|_| "Film Grain".to_string()).collect();
    assert!(!names.is_empty(), "{listed}");
    let before = frame(&mut s, "01-original");
    let undo = s.history.undo.len();
    s.execute("effects.apply", json!({"clips": [clip], "effect": "Film Grain"})).unwrap();
    assert_eq!((effect_count(&s, clip), s.history.undo.len()), (1, undo + 1));
    let with = frame(&mut s, "02-film-grain");
    assert_ne!(before, with, "the grain changes the picture");
    // stronger grain changes it more
    s.execute("effects.setParam", json!({"clip": clip, "effect": "film_grain", "param": "amount", "value": 70.0})).unwrap();
    let strong = frame(&mut s, "03-film-grain-strong");
    let dist = |a: &[u8], b: &[u8]| a.iter().zip(b).map(|(x, y)| (i32::from(*x) - i32::from(*y)).unsigned_abs() as u64).sum::<u64>();
    assert!(dist(&before, &strong) > dist(&before, &with));
    // the same frame renders the same again (preview and export agree)
    assert_eq!(strong, frame(&mut s, "03-film-grain-strong-again"));
    s.undo().unwrap();
    s.undo().unwrap();
    assert_eq!(effect_count(&s, clip), 0);
    assert_eq!(before, frame(&mut s, "04-after-undo"), "undo gives back exactly the original picture");
    // the project saves and loads with the effect on it
    s.redo().unwrap();
    let json: Value = serde_json::from_str(&s.project.to_json()).unwrap();
    assert!(json.to_string().contains("film_grain"));
}

#[test]
fn halation_and_diffusion_apply_by_name_change_the_frame_and_undo_exactly() {
    for (name, id) in [("Halation", "halation"), ("Diffusion", "diffusion")] {
        // a sunset: it has a bright sun for the glow to come from
        let (mut s, clip) = session_with(DemoScene::OceanSunset);
        let before = frame(&mut s, &format!("10-{id}-original"));
        let undo0 = s.history.undo.len();
        s.execute("effects.apply", json!({"clips": [clip], "effect": name})).unwrap();
        let with = frame(&mut s, &format!("11-{id}"));
        assert_ne!(before, with, "{name} changes the picture");
        s.execute("effects.setParam", json!({"clip": clip, "effect": id, "param": "strength", "value": 0.0})).unwrap();
        if id == "diffusion" {
            s.execute("effects.setParam", json!({"clip": clip, "effect": id, "param": "black_lift", "value": 0.0})).unwrap();
        }
        assert_eq!(before, frame(&mut s, &format!("12-{id}-off")), "{name} at zero leaves the picture as it was");
        while s.history.undo.len() > undo0 {
            s.undo().unwrap();
        }
        assert_eq!(before, frame(&mut s, &format!("13-{id}-undone")));
    }
}

#[test]
fn film_gate_streak_and_fringing_apply_by_name_change_the_frame_and_turn_off_exactly() {
    let cases: [(&str, &str, &[&str]); 3] = [
        ("Film Gate", "film_gate", &["weave", "flicker", "dust", "scratches"]),
        ("Anamorphic Streak", "anamorphic_streak", &["strength"]),
        ("Lens Fringing", "lens_fringing", &["amount"]),
    ];
    for (name, id, zero) in cases {
        let (mut s, clip) = session_with(DemoScene::OceanSunset);
        let before = frame(&mut s, &format!("20-{id}-original"));
        let undo0 = s.history.undo.len();
        s.execute("effects.apply", json!({"clips": [clip], "effect": name})).unwrap();
        let with = frame(&mut s, &format!("21-{id}"));
        assert_ne!(before, with, "{name} changes the picture");
        for p in zero {
            s.execute("effects.setParam", json!({"clip": clip, "effect": id, "param": p, "value": 0.0})).unwrap();
        }
        assert_eq!(before, frame(&mut s, &format!("22-{id}-off")), "{name} at zero leaves the picture as it was");
        while s.history.undo.len() > undo0 {
            s.undo().unwrap();
        }
        assert_eq!(before, frame(&mut s, &format!("23-{id}-undone")));
    }
}
