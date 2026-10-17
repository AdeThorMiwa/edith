//! Tests of the planner crate: the shipped packs, exact division of a video's length, hostile pack
//! files and hostile planners.

use filmcraft_time::{TICKS_PER_SECOND, Tick};

use crate::model::*;
use crate::pack::*;

fn secs(s: i64) -> Tick {
    Tick(s * TICKS_PER_SECOND)
}

fn pack(id: &str) -> FormatPack {
    builtin_packs().into_iter().find(|p| p.id == id).unwrap_or_else(|| panic!("no pack {id}"))
}

#[test]
fn both_shipped_packs_parse_and_describe_the_roadmap_tables() {
    assert_eq!(builtin_packs().len(), 2);
    for (id, text) in BUILTIN_SOURCES {
        let p = FormatPack::parse(text).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(p.id, id);
        assert!(!p.sections.is_empty() && !p.summary.is_empty());
    }
    let m = pack("mystery");
    let names: Vec<&str> = m.sections.iter().flat_map(|s| s.beats.iter()).map(|b| b.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Cold open",
            "The question",
            "The scene",
            "Evidence trail",
            "False resolution",
            "The turn",
            "Best explanation",
            "What we still don't know",
            "Last image"
        ]
    );
    assert_eq!(m.repeat_range(), Some((3, 5, 4)));
    assert_eq!(pack("explainer").repeat_range(), Some((2, 4, 3)));
    assert_eq!(m.look_preset, "Cinema Look");
    assert!(m.targets.contains_key("words_per_minute"));
}

#[test]
fn a_plan_from_a_pack_adds_up_to_the_length_exactly_for_any_length() {
    for id in ["mystery", "explainer"] {
        let p = pack(id);
        for target in [Tick(1), Tick(7), Tick(999), secs(1), secs(59), secs(60), secs(12 * 60), secs(12 * 60) + Tick(1), secs(3 * 3600), MAX_TARGET] {
            for clues in [None, Some(0), Some(3), Some(5), Some(99)] {
                let plan = p.instantiate(target, clues).unwrap_or_else(|e| panic!("{id} {target:?}: {e}"));
                assert_eq!(plan.planned(), target, "{id} {target:?} {clues:?}");
                assert_eq!(plan.target, target);
                assert!(plan.check().is_ok(), "{:?}", plan.check());
                assert!(plan.beats().all(|b| b.target.0 >= 0));
                assert_eq!(plan.pack.as_deref(), Some(id));
            }
        }
    }
}

#[test]
fn twelve_minutes_of_mystery_gives_the_table_shares() {
    let plan = pack("mystery").instantiate(secs(12 * 60), None).unwrap();
    let by_name = |n: &str| plan.beats().find(|b| b.name == n).map(|b| b.target.0 as f64 / TICKS_PER_SECOND as f64).unwrap_or(f64::NAN);
    // 4% of 720 s = 28.8 s, 12% = 86.4 s, 10% = 72 s
    assert!((by_name("Cold open") - 28.8).abs() < 0.001);
    assert!((by_name("The scene") - 86.4).abs() < 0.001);
    assert!((by_name("False resolution") - 72.0).abs() < 0.001);
    // four clue beats share 35% = 252 s equally
    let clues: Vec<f64> = plan.beats().filter(|b| b.name.starts_with("Clue")).map(|b| b.target.0 as f64 / TICKS_PER_SECOND as f64).collect();
    assert_eq!(clues.len(), 4);
    assert!(clues.iter().all(|c| (c - 63.0).abs() < 0.001), "{clues:?}");
    // the clues' intensities ramp from 5 to 7
    let ramp: Vec<u8> = plan.beats().filter(|b| b.name.starts_with("Clue")).map(|b| b.intensity).collect();
    assert_eq!((ramp[0], ramp[3]), (50, 70));
    assert!(ramp.windows(2).all(|w| w[0] <= w[1]));
    // a beat that ramps without repeating takes its middle value
    assert_eq!(plan.beats().find(|b| b.name == "Best explanation").map(|b| b.intensity), Some(70));
    // names, ids and sections
    assert_eq!(plan.sections.len(), 4);
    assert_eq!(plan.beats().map(|b| b.name.as_str()).filter(|n| n.starts_with("Clue ")).collect::<Vec<_>>(), ["Clue 1", "Clue 2", "Clue 3", "Clue 4"]);
    let three = pack("mystery").instantiate(secs(600), Some(3)).unwrap();
    let five = pack("mystery").instantiate(secs(600), Some(5)).unwrap();
    assert_eq!((three.beat_count(), five.beat_count()), (11, 13));
    let clamped =
        (pack("mystery").instantiate(secs(600), Some(0)).unwrap().beat_count(), pack("mystery").instantiate(secs(600), Some(50)).unwrap().beat_count());
    assert_eq!(clamped, (11, 13), "a number outside the range is moved into it");
}

#[test]
fn the_division_never_loses_or_invents_a_tick() {
    for total in [0i64, 1, 2, 3, 10, 99, 1_000_000_007, i64::MAX / 4] {
        for weights in [vec![1u64], vec![1, 1, 1], vec![3, 7, 90], vec![0, 5, 0, 5], vec![u64::MAX / 4, 1, 2]] {
            let parts = split(total, &weights);
            assert_eq!(parts.len(), weights.len());
            assert!(parts.iter().all(|p| *p >= 0));
            if weights.iter().any(|w| *w > 0) {
                assert_eq!(parts.iter().sum::<i64>(), total.max(0), "{total} {weights:?}");
            }
        }
    }
    assert_eq!(split(10, &[1, 1, 1]), [4, 3, 3], "the leftover goes to the earliest of equal remainders");
    assert!(split(10, &[]).is_empty());
    assert_eq!(split(10, &[0, 0]), [0, 0]);
    assert_eq!(split(-5, &[1, 1]), [0, 0]);
}

#[test]
fn lengths_outside_the_range_are_refused() {
    let p = pack("mystery");
    for t in [Tick(0), Tick(-1), Tick(i64::MIN), Tick(i64::MAX), Tick(MAX_TARGET.0 + 1)] {
        assert!(p.instantiate(t, None).is_err(), "{t:?}");
    }
}

fn base() -> String {
    r#"
version = 1
id = "t"
name = "Test"
[[section]]
name = "A"
[[section.beat]]
name = "One"
from = 0
to = 40
intensity = 5
[[section.beat]]
name = "Two"
from = 40
to = 100
intensity = 6
"#
    .to_string()
}

#[test]
fn a_small_valid_pack_works_and_each_kind_of_mistake_is_named() {
    let ok = FormatPack::parse(&base()).unwrap();
    assert_eq!(ok.instantiate(secs(100), None).unwrap().beat_count(), 2);
    let bad = |from: &str, to: &str| {
        let r = FormatPack::parse(&base().replace(from, to));
        assert!(r.is_err(), "{from} -> {to} was accepted");
        r.err().unwrap()
    };
    bad("version = 1", "version = 2");
    bad("version = 1", "version = 0");
    bad("id = \"t\"", "id = \"Bad Id\"");
    bad("id = \"t\"", "id = \"\"");
    bad("name = \"Test\"", "name = \"  \"");
    bad("from = 40\nto = 100", "from = 50\nto = 100");
    bad("from = 40\nto = 100", "from = 30\nto = 100");
    bad("from = 40\nto = 100", "from = 40\nto = 90");
    bad("from = 0\nto = 40", "from = 10\nto = 40");
    bad("to = 40\nintensity = 5", "to = 30\nintensity = 5");
    bad("intensity = 5", "intensity = 11");
    bad("intensity = 5", "intensity = -1");
    bad("name = \"Two\"", "name = \"one\"");
    bad("from = 0\nto = 40", "from = 40\nto = 0");
    bad("from = 0\nto = 40", "from = 0\nto = 140");
    // a gap or overlap of half a percent is tolerated
    assert!(FormatPack::parse(&base().replace("from = 40\nto = 100", "from = 40.4\nto = 100")).is_ok());
    assert!(FormatPack::parse(&base().replace("[[section.beat]]\nname = \"Two\"", "[[section.beat]]\nname = \"Two\"\npurpose = \"x\"")).is_ok());
    // a section with no beats, a pack with no sections
    bad("[[section.beat]]\nname = \"One\"\nfrom = 0\nto = 40\nintensity = 5\n[[section.beat]]\nname = \"Two\"\nfrom = 40\nto = 100\nintensity = 6", "");
    assert!(FormatPack::parse("version = 1\nid = \"t\"\nname = \"x\"").is_err());
}

#[test]
fn repeat_ranges_are_checked() {
    let with = |r: &str| base().replace("intensity = 5", &format!("intensity = 5\nrepeat = {r}"));
    assert!(FormatPack::parse(&with("{ min = 2, max = 4, default = 3, name = \"Step {n}\" }")).is_ok());
    for r in [
        "{ min = 0, max = 4, default = 3 }",
        "{ min = 4, max = 2, default = 3 }",
        "{ min = 2, max = 4, default = 9 }",
        "{ min = 2, max = 400, default = 3 }",
        "{ min = 2, max = 4 }",
    ] {
        assert!(FormatPack::parse(&with(r)).is_err(), "{r}");
    }
    // a repeat without a name gets numbered copies of the beat's own name
    let p = FormatPack::parse(&with("{ min = 2, max = 3, default = 2 }")).unwrap().instantiate(secs(60), None).unwrap();
    assert_eq!(p.beats().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["One 1", "One 2", "Two"]);
}

#[test]
fn hostile_pack_files_are_refused_not_crashed_on() {
    for text in [
        "".to_string(),
        "not toml at all {{{".to_string(),
        "version = \"one\"".to_string(),
        "x".repeat(MAX_PACK_BYTES + 1),
        format!("version = 1\nid = \"t\"\nname = \"{}\"", "n".repeat(100_000)),
        "[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[".to_string(),
        base().replace("from = 0", "from = nan"),
        base().replace("to = 100", "to = inf"),
        base().replace("intensity = 5", "intensity = 1e999"),
        base().replace("name = \"One\"", "name = \"bad\\u0007name\""),
        format!("{}\n[targets]\n{}", base(), (0..50).map(|i| format!("k{i} = 1\n")).collect::<String>()),
        format!("{}\n[targets]\nk = nan", base()),
        format!("{}\n[[section]]\nname = \"B\"", base()),
    ] {
        let r = std::panic::catch_unwind(|| FormatPack::parse(&text));
        assert!(r.is_ok(), "panicked on {:?}", &text[..text.len().min(40)]);
        assert!(r.is_ok_and(|v| v.is_err()), "accepted {:?}", &text[..text.len().min(40)]);
    }
    // too many beats
    let many: String = (0..60)
        .map(|i| format!("[[section.beat]]\nname = \"B{i}\"\nfrom = {}\nto = {}\nintensity = 5\n", i as f64 * 100.0 / 60.0, (i + 1) as f64 * 100.0 / 60.0))
        .collect();
    assert!(FormatPack::parse(&format!("version = 1\nid = \"t\"\nname = \"x\"\n[[section]]\nname = \"A\"\n{many}")).is_err());
}

#[test]
fn a_plan_survives_json_unchanged_and_the_checks_catch_every_limit() {
    let plan = pack("mystery").instantiate(secs(900), None).unwrap();
    let json = serde_json::to_string(&plan).unwrap();
    assert_eq!(serde_json::from_str::<Planner>(&json).unwrap(), plan);
    assert!(plan.check().is_ok());
    let mut p = plan.clone();
    p.title = "t".repeat(MAX_TITLE + 1);
    assert!(p.check().is_err());
    let mut p = plan.clone();
    p.sections[0].beats[0].narration = "n".repeat(MAX_NARRATION + 1);
    assert!(p.check().is_err());
    let mut p = plan.clone();
    p.sections[0].beats[0].intensity = 101;
    assert!(p.check().is_err());
    let mut p = plan.clone();
    p.sections[0].beats[0].target = Tick(-1);
    assert!(p.check().is_err());
    let mut p = plan.clone();
    p.sections[0].beats[1].id = p.sections[0].beats[0].id;
    assert!(p.check().is_err(), "duplicate ids");
    let mut p = plan.clone();
    p.next_id = 1;
    assert!(p.check().is_err(), "an id at or above the counter");
    let mut p = plan.clone();
    p.sections[0].beats[0].link = Some(Link { start: Tick(i64::MAX), duration: Tick(5), marker: None });
    assert!(p.check().is_err());
    let mut p = plan;
    for _ in 0..MAX_SECTIONS + 1 {
        let id = p.new_id();
        p.sections.push(Section { id, name: "s".into(), beats: vec![] });
    }
    assert!(p.check().is_err());
}

#[test]
fn a_hostile_planner_is_cut_down_to_a_valid_one() {
    let mut p = Planner { title: "t\u{7}itle".repeat(500), pack: Some(" ".repeat(10)), target: Tick(i64::MAX), next_id: 0, sections: vec![] };
    for s in 0..60 {
        let mut beats = Vec::new();
        for b in 0..20 {
            beats.push(Beat {
                id: if b % 3 == 0 { 0 } else { 5 },
                name: "b\u{0}".repeat(300),
                narration: "x".repeat(MAX_NARRATION * 2),
                intensity: 250,
                target: Tick(i64::MIN),
                link: Some(Link { start: Tick(-5), duration: Tick(1), marker: None }),
                shots: (0..100).map(|_| Shot { id: 5, target: Tick(i64::MAX), ..Default::default() }).collect(),
                ..Default::default()
            });
        }
        p.sections.push(Section { id: s, name: "n".repeat(1000), beats });
    }
    let c = p.sanitized();
    assert!(c.check().is_ok(), "{:?}", c.check());
    assert!(c.sections.len() <= MAX_SECTIONS && c.beat_count() <= MAX_BEATS);
    assert!(c.pack.is_none(), "a blank pack name is dropped");
    assert!(c.beats().all(|b| b.shots.len() <= MAX_SHOTS_PER_BEAT && b.link.is_none() && b.intensity <= MAX_INTENSITY));
    // ids that were duplicated or zero are now unique
    let mut ids: Vec<u64> = c
        .sections
        .iter()
        .flat_map(|s| std::iter::once(s.id).chain(s.beats.iter().flat_map(|b| std::iter::once(b.id).chain(b.shots.iter().map(|x| x.id)))))
        .collect();
    let n = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), n);
    assert!(ids.iter().all(|i| *i >= 1 && *i < c.next_id));
    // an already valid plan is unchanged
    let good = pack("explainer").instantiate(secs(480), None).unwrap();
    assert_eq!(good.clone().sanitized(), good);
}

#[test]
fn lookups_find_beats_and_shots_and_new_ids_are_never_reused() {
    let mut plan = pack("explainer").instantiate(secs(480), None).unwrap();
    let first = plan.beats().next().map(|b| b.id).unwrap();
    assert_eq!(plan.find_beat(first), Some((0, 0)));
    assert!(plan.find_beat(0).is_none() && plan.find_beat(u64::MAX).is_none());
    let sid = plan.new_id();
    let shot = Shot { id: sid, intent: "x".into(), ..Default::default() };
    plan.beat_mut(first).unwrap().shots.push(shot);
    assert_eq!(plan.find_shot(sid), Some((0, 0, 0)));
    let a = plan.new_id();
    let b = plan.new_id();
    assert!(b > a && a > sid);
    plan.next_id = u64::MAX;
    assert!(plan.new_id() > 0, "an exhausted counter does not wrap to zero");
    assert_eq!(Status::from_id(" DRAFTED "), Some(Status::Drafted));
    assert_eq!(Status::from_id("nope"), None);
}

#[test]
fn the_schema_file_names_every_field_a_pack_has() {
    let schema: serde_json::Value = serde_json::from_str(include_str!("../../../docs/format-pack.schema.json")).unwrap();
    let text = schema.to_string();
    for field in [
        "version",
        "id",
        "name",
        "summary",
        "typical_minutes",
        "look_preset",
        "title_style",
        "targets",
        "section",
        "beat",
        "from",
        "to",
        "intensity",
        "intensity_end",
        "sound",
        "repeat",
        "purpose",
        "default",
    ] {
        assert!(text.contains(&format!("\"{field}\"")), "the schema does not mention {field}");
    }
}

// ---------------------------------------------------------------------------- sentences and time

use crate::script;

fn texts(s: &str) -> Vec<String> {
    script::split(s).parts.into_iter().map(|p| p.text).collect()
}

#[test]
fn text_is_cut_at_the_end_of_each_sentence() {
    assert_eq!(texts("It rang once. Nobody came! Why? Because."), ["It rang once.", "Nobody came!", "Why?", "Because."]);
    assert_eq!(texts("No full stop at the end"), ["No full stop at the end"]);
    assert_eq!(texts(""), Vec::<String>::new());
    assert_eq!(texts("   \n  "), Vec::<String>::new());
    assert_eq!(texts("Line one\nLine two\n\nLine three"), ["Line one", "Line two", "Line three"], "a line break ends a sentence");
    // several marks and closing quotes stay with their sentence
    assert_eq!(texts("Really?! \"Yes.\" He left."), ["Really?!", "\"Yes.\"", "He left."]);
    assert_eq!(texts("Wait... What? (Yes.) Fine."), ["Wait...", "What?", "(Yes.)", "Fine."]);
    // abbreviations, initials and numbers do not end a sentence
    assert_eq!(texts("Dr. Smith met Mr. Jones at 3.5 percent."), ["Dr. Smith met Mr. Jones at 3.5 percent."]);
    assert_eq!(texts("J. R. Smith left. Then Prof. Lee came, e.g. late."), ["J. R. Smith left.", "Then Prof. Lee came, e.g. late."]);
    assert_eq!(texts("Version 2. Next."), ["Version 2.", "Next."]);
    // markers never split, and one before a sentence stays with it
    assert_eq!(
        texts("It rang. [pause 1.5s] Nobody came. [pace 0.8] Then silence. [pitch -2]"),
        ["It rang.", "[pause 1.5s] Nobody came.", "[pace 0.8] Then silence.", "[pitch -2]"]
    );
    assert_eq!(texts("A [pause 1s] B."), ["A [pause 1s] B."]);
    assert_eq!(texts("Unclosed [marker. Still going. Yes."), ["Unclosed [marker. Still going. Yes."]);
}

#[test]
fn splitting_and_joining_gives_back_the_exact_text_whatever_it_is() {
    for text in [
        "",
        " ",
        "\n\n",
        "One.",
        "  Leading and trailing.  ",
        "A. B. C.",
        "A.\n\nB.\r\nC.",
        "Dr. X, e.g. Y. Z",
        "[pause 1s]",
        "[[nested]] marker. Next.",
        "emoji 🎬 here. And 日本語。そして。 More.",
        "tabs\tand  double  spaces.  Next one?  ",
        "Unclosed [bracket. Never closes",
        ".....",
        "?!?!",
        &"word. ".repeat(20_000),
        &"x".repeat(200_000),
    ] {
        let s = script::split(text);
        assert_eq!(s.join(), text, "{:?}", &text[..text.len().min(30)]);
        assert!(s.parts.iter().all(|p| !p.text.is_empty() && p.text == p.text.trim()));
    }
}

#[test]
fn speech_time_counts_words_not_markers_and_adds_pauses() {
    let tps = TICKS_PER_SECOND;
    // 150 words per minute is 2.5 words a second
    assert_eq!(script::speech_ticks("one two three four five", 150), Tick(2 * tps));
    assert_eq!(script::speech_ticks("", 150), Tick(0));
    assert_eq!(script::speech_ticks("  \n ", 150), Tick(0));
    assert_eq!(script::speech_ticks("word", 60), Tick(tps));
    // markers are not words; a pause adds its length; a direction adds nothing
    assert_eq!(script::speech_ticks("one two [pace 0.8] three four five", 150), Tick(2 * tps));
    assert_eq!(script::speech_ticks("one two three four five [pause 1.5s]", 150), Tick(2 * tps + tps * 3 / 2));
    assert_eq!(script::speech_ticks("[pause 500ms] hello", 60), Tick(tps + tps / 2));
    assert_eq!(script::speech_ticks("[pause 99999s]", 150), Tick(10 * tps), "a pause is capped at ten seconds");
    assert_eq!(script::speech_ticks("[pause x] [pause -1s] hello", 60), Tick(tps));
    // punctuation alone is not a word
    assert_eq!(script::speech_ticks("... --- !!!", 150), Tick(0));
    // a pace outside 40 to 400 is replaced by 150
    for bad in [0, 1, 39, 401, u32::MAX] {
        assert_eq!(script::speech_ticks("one two three four five", bad), Tick(2 * tps), "{bad}");
    }
    // very long text neither overflows nor panics
    let long = "word ".repeat(2_000_000);
    assert!(script::speech_ticks(&long, 40).0 > 0);
}

#[test]
fn beat_ranges_lie_end_to_end_and_add_up() {
    let plan = pack("mystery").instantiate(secs(600), None).unwrap();
    let r = script::ranges(&plan, secs(5));
    assert_eq!(r.len(), plan.beat_count());
    assert_eq!(r[0].start, secs(5));
    assert!(r.windows(2).all(|w| w[0].end == w[1].start));
    assert_eq!(r.last().unwrap().end, secs(605));
    assert_eq!(r.iter().map(|x| x.end.0 - x.start.0).sum::<i64>(), plan.planned().0);
    assert!(script::ranges(&Planner::default(), Tick(0)).is_empty());
    // a start close to the end of time saturates instead of overflowing
    let far = script::ranges(&plan, Tick(i64::MAX - 5));
    assert!(far.iter().all(|x| x.end.0 >= x.start.0));
}

#[test]
fn a_shot_remembers_its_sentence_and_older_plans_without_one_still_load() {
    let old = r#"{"id":3,"intent":"x","shotType":"","cameraMove":"","target":0,"source":null,"clip":null}"#;
    let shot: Shot = serde_json::from_str(old).unwrap();
    assert_eq!(shot.line, None);
    let with = Shot { line: Some(2), ..shot };
    assert_eq!(serde_json::from_str::<Shot>(&serde_json::to_string(&with).unwrap()).unwrap().line, Some(2));
}

// ---------------------------------------------------------------------------- the sound plan (F13.1)

fn plan_with_sound() -> Planner {
    let mut p = pack("mystery").instantiate(secs(600), None).unwrap();
    let id1 = p.new_id();
    let id2 = p.new_id();
    if let Some(b) = p.sections.get_mut(0).and_then(|s| s.beats.get_mut(0)) {
        b.sound = SoundPlan {
            ambience: "wind over a shore".into(),
            music: Some(MusicCue { mood: "uneasy strings".into(), enters: secs(2), leaves: Some(secs(20)) }),
            cues: vec![
                Cue { id: id1, kind: CueKind::Effect, at: secs(5), length: Tick::ZERO, text: "a bell, far off".into() },
                Cue { id: id2, kind: CueKind::Silence, at: secs(12), length: secs(3), text: "after the name".into() },
            ],
        };
    }
    p
}

#[test]
fn a_sound_plan_round_trips_and_a_plan_saved_before_it_existed_still_loads() {
    let p = plan_with_sound();
    assert!(p.check().is_ok(), "{:?}", p.check());
    let json = serde_json::to_string(&p).unwrap();
    let back: Planner = serde_json::from_str(&json).unwrap();
    assert_eq!(back, p);
    // an older plan has no `sound` on its beats: the field is empty, nothing fails
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    for s in v["sections"].as_array_mut().unwrap() {
        for b in s["beats"].as_array_mut().unwrap() {
            b.as_object_mut().unwrap().remove("sound");
        }
    }
    let old: Planner = serde_json::from_value(v).unwrap();
    assert!(old.beats().all(|b| b.sound == SoundPlan::default()));
    assert!(old.check().is_ok());
}

#[test]
fn the_sound_limits_are_checked() {
    let mut p = plan_with_sound();
    p.sections[0].beats[0].sound.ambience = "x".repeat(MAX_NOTE + 1);
    assert!(p.check().is_err());
    let mut p = plan_with_sound();
    p.sections[0].beats[0].sound.cues[0].at = Tick(-1);
    assert!(p.check().is_err());
    let mut p = plan_with_sound();
    p.sections[0].beats[0].sound.music.as_mut().unwrap().leaves = Some(Tick(MAX_TARGET.0 + 1));
    assert!(p.check().is_err());
    let mut p = plan_with_sound();
    let extra = p.new_id();
    p.sections[0].beats[0].sound.cues[0].id = extra - 2;
    p.sections[0].beats[0].sound.cues[1].id = extra - 2;
    assert!(p.check().is_err(), "two cues with one id");
    let mut p = plan_with_sound();
    for _ in 0..MAX_CUES_PER_BEAT {
        let id = p.new_id();
        p.sections[0].beats[0].sound.cues.push(Cue { id, ..Default::default() });
    }
    assert!(p.check().is_err(), "too many cues");
}

#[test]
fn hostile_sound_plans_are_cut_down_to_valid_ones() {
    let mut p = plan_with_sound();
    {
        let b = &mut p.sections[0].beats[0];
        b.sound.ambience = "a\u{7}".repeat(MAX_NOTE * 3);
        b.sound.music = Some(MusicCue { mood: "m\u{0}".repeat(500), enters: Tick(i64::MIN), leaves: Some(Tick(i64::MAX)) });
        b.sound.cues = (0..200)
            .map(|i| Cue {
                id: if i % 2 == 0 { 0 } else { 3 },
                kind: if i % 3 == 0 { CueKind::Silence } else { CueKind::Effect },
                at: Tick(i64::MIN),
                length: Tick(i64::MAX),
                text: "t\u{1}".repeat(400),
            })
            .collect();
    }
    let c = p.sanitized();
    assert!(c.check().is_ok(), "{:?}", c.check());
    let b = &c.sections[0].beats[0];
    assert!(b.sound.cues.len() <= MAX_CUES_PER_BEAT);
    assert!(b.sound.cues.iter().all(|c| c.kind == CueKind::Silence || c.length == Tick::ZERO), "an effect has no length");
    assert_eq!(b.sound.music.as_ref().unwrap().enters, Tick::ZERO);
    let mut ids: Vec<u64> = b.sound.cues.iter().map(|c| c.id).collect();
    let n = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), n, "cue ids are unique");
    // a valid sound plan is left exactly as it was
    let good = plan_with_sound();
    assert_eq!(good.clone().sanitized(), good);
}
