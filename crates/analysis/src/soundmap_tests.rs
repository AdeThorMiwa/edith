//! Tests of [`crate::soundmap`]: lanes and their spans, each rule on hand-made timelines, planned
//! silences, the level meter against known tones, and hostile input.

use filmcraft_time::TICKS_PER_SECOND;

use crate::coach::{Kind, Severity};
use crate::soundmap::*;

fn t(seconds: f64) -> i64 {
    (seconds * TICKS_PER_SECOND as f64) as i64
}

fn clip(lane: Lane, id: u64, from: f64, to: f64) -> LaneClip {
    LaneClip { lane, id, start: t(from), end: t(to) }
}

/// A 60 s video: narration and music all through, no ambience.
fn bare() -> SoundMapInput {
    SoundMapInput { duration: t(60.0), clips: vec![clip(Lane::Narration, 1, 0.0, 60.0), clip(Lane::Music, 2, 0.0, 60.0)], ..Default::default() }
}

fn kinds(input: &SoundMapInput) -> Vec<Kind> {
    findings(input).unwrap().iter().map(|f| f.kind).collect()
}

#[test]
fn the_map_shows_each_lane_and_where_it_has_nothing() {
    let m = lane_map(&bare()).unwrap();
    let names: Vec<&str> = m.lanes.iter().map(|l| l.lane.name()).collect();
    assert_eq!(names, ["narration", "music", "ambience", "effects"], "four lanes, and unsorted only when something is in it");
    let by = |n: &str| m.lanes.iter().find(|l| l.lane.name() == n).unwrap().clone();
    assert!((by("narration").share - 1.0).abs() < 1e-6 && (by("narration").covered_seconds - 60.0).abs() < 1e-9);
    assert_eq!(by("ambience").spans, vec![], "the gap: nothing in the ambience lane");
    assert_eq!(by("ambience").share, 0.0);
    assert!(!m.measured);
    // clips that overlap or touch are one span; the unsorted lane appears when used
    let mut i = bare();
    i.clips.push(clip(Lane::Effects, 3, 5.0, 6.0));
    i.clips.push(clip(Lane::Effects, 4, 5.5, 8.0));
    i.clips.push(clip(Lane::Effects, 5, 8.0, 9.0));
    i.clips.push(clip(Lane::Other, 6, 10.0, 11.0));
    let m = lane_map(&i).unwrap();
    assert_eq!(m.lanes.iter().find(|l| l.lane == Lane::Effects).unwrap().spans, vec![(t(5.0), t(9.0))]);
    assert!(m.lanes.iter().any(|l| l.lane == Lane::Other));
}

#[test]
fn a_video_with_narration_and_music_but_no_ambience_gets_the_missing_bed_flagged() {
    let f = findings(&bare()).unwrap();
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!((f[0].kind, f[0].severity, f[0].start), (Kind::NoAmbience, Severity::Look, 0));
    assert!(f[0].detail.contains("room tone") && f[0].idea.is_some());
    // a short video, an empty timeline, or a bed all the way through: nothing to say
    let mut short = bare();
    short.duration = t(15.0);
    short.clips = vec![clip(Lane::Narration, 1, 0.0, 15.0)];
    assert!(kinds(&short).is_empty(), "under 20 s");
    let mut bed = bare();
    bed.clips.push(clip(Lane::Ambience, 3, 0.0, 60.0));
    assert!(kinds(&bed).is_empty());
    let empty = SoundMapInput { duration: t(60.0), ..Default::default() };
    assert!(kinds(&empty).is_empty(), "no sound at all is a different problem");
    // a clip nobody has sorted may be the ambience, so nothing is claimed
    let mut unsorted = bare();
    unsorted.clips.push(clip(Lane::Other, 9, 0.0, 60.0));
    assert!(kinds(&unsorted).is_empty());
}

#[test]
fn an_ambience_that_stops_or_starts_late_is_flagged_where_it_happens() {
    let mut i = bare();
    i.clips.push(clip(Lane::Ambience, 3, 0.0, 20.0));
    i.clips.push(clip(Lane::Ambience, 4, 26.0, 60.0));
    let f = findings(&i).unwrap();
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!((f[0].kind, f[0].start, f[0].end), (Kind::AmbienceGap, t(20.0), Some(t(26.0))));
    assert!(f[0].title.contains("0:20") && f[0].title.contains("6 s"), "{}", f[0].title);
    // late start and early end
    let mut late = bare();
    late.clips.push(clip(Lane::Ambience, 3, 8.0, 50.0));
    let f = findings(&late).unwrap();
    assert_eq!(f.iter().map(|x| (x.kind, x.start)).collect::<Vec<_>>(), vec![(Kind::AmbienceGap, 0), (Kind::AmbienceGap, t(50.0))]);
    assert!(f[0].title.contains("only starts at 0:08") && f[1].title.contains("stops at 0:50"));
    // a hole under 2 s is let go
    let mut small = bare();
    small.clips.push(clip(Lane::Ambience, 3, 0.0, 30.0));
    small.clips.push(clip(Lane::Ambience, 4, 31.5, 60.0));
    assert!(kinds(&small).is_empty());
}

#[test]
fn dead_air_is_found_from_the_clips_and_a_planned_silence_is_not_dead_air() {
    let mut i = SoundMapInput {
        duration: t(40.0),
        clips: vec![clip(Lane::Narration, 1, 0.0, 10.0), clip(Lane::Narration, 2, 14.0, 40.0), clip(Lane::Ambience, 3, 0.0, 40.0)],
        ..Default::default()
    };
    let f = findings(&i).unwrap();
    let d: Vec<_> = f.iter().filter(|x| x.kind == Kind::DeadAir).collect();
    // the ambience bed runs all through, so there is no dead air: the gap is only in the narration
    assert!(d.is_empty(), "{f:?}");
    // without the bed, 10 s to 14 s is dead air
    i.clips.retain(|c| c.lane != Lane::Ambience);
    let f = findings(&i).unwrap();
    let d = f.iter().find(|x| x.kind == Kind::DeadAir).unwrap();
    assert_eq!((d.start, d.end, d.severity), (t(10.0), Some(t(14.0)), Severity::Look));
    assert!(d.title.contains("4 s") && d.title.contains("0:10"));
    // five seconds or more is a problem
    i.clips[1].start = t(16.0);
    assert_eq!(findings(&i).unwrap().iter().find(|x| x.kind == Kind::DeadAir).unwrap().severity, Severity::Problem);
    // a deliberate silence over it takes it off the list; one over part of it leaves the rest
    i.silences = vec![(t(9.0), t(17.0))];
    assert!(!kinds(&i).contains(&Kind::DeadAir));
    i.silences = vec![(t(10.0), t(13.0))];
    let rest: Vec<_> = findings(&i).unwrap().into_iter().filter(|x| x.kind == Kind::DeadAir).collect();
    assert_eq!(rest.len(), 1, "{rest:?}");
    assert_eq!((rest[0].start, rest[0].end), (t(13.0), Some(t(16.0))), "only the part that was not planned");
    // under a second and a half is not flagged
    let mut tiny = i.clone();
    tiny.silences.clear();
    tiny.clips[1].start = t(11.0);
    assert!(!kinds(&tiny).contains(&Kind::DeadAir));
}

fn levels(lane: Lane, f: impl Fn(f64) -> Option<f32>, seconds: f64) -> LaneLevels {
    LaneLevels { lane, values: (0..(seconds / HOP_SECONDS) as usize).map(|i| f(i as f64 * HOP_SECONDS)).collect() }
}

#[test]
fn a_measured_map_finds_clips_that_are_silent_and_room_tone_that_jumps() {
    let mut i = SoundMapInput {
        duration: t(30.0),
        clips: vec![clip(Lane::Narration, 1, 0.0, 30.0), clip(Lane::Ambience, 2, 0.0, 15.0), clip(Lane::Ambience, 3, 15.0, 30.0)],
        ..Default::default()
    };
    // the narration clip is there but silent from 5 s to 9 s; the ambience steps up 8 LU at 15 s
    i.levels = vec![
        levels(Lane::Narration, |s| if (5.0..9.0).contains(&s) { None } else { Some(-20.0) }, 30.0),
        levels(Lane::Ambience, |s| Some(if s < 15.0 { -45.0 } else { -37.0 }), 30.0),
    ];
    let f = findings(&i).unwrap();
    let jump = f.iter().find(|x| x.kind == Kind::RoomToneJump).expect("a jump");
    assert_eq!((jump.start, jump.severity), (t(15.0), Severity::Look));
    assert!((jump.numbers["lu"] - 8.0).abs() < 0.01 && jump.title.contains("8 LU"), "{jump:?}");
    assert!(jump.detail.contains("louder"));
    // the ambience bed keeps sound playing the whole time, so the silent narration is not dead air
    assert!(!f.iter().any(|x| x.kind == Kind::DeadAir), "{f:?}");
    // with no bed measured as sound (it is silent too), the same stretch is dead air
    i.levels[1] = levels(Lane::Ambience, |_| None, 30.0);
    let f = findings(&i).unwrap();
    let d = f.iter().find(|x| x.kind == Kind::DeadAir).unwrap();
    assert_eq!((d.start, d.end), (t(5.0), Some(t(9.0))));
    assert!(!f.iter().any(|x| x.kind == Kind::RoomToneJump), "silence is not a jump");
    // 10 LU or more is a problem; 5 LU is let go
    i.levels[1] = levels(Lane::Ambience, |s| Some(if s < 15.0 { -50.0 } else { -38.0 }), 30.0);
    assert_eq!(findings(&i).unwrap().iter().find(|x| x.kind == Kind::RoomToneJump).unwrap().severity, Severity::Problem);
    i.levels[1] = levels(Lane::Ambience, |s| Some(if s < 15.0 { -45.0 } else { -40.0 }), 30.0);
    assert!(!kinds(&i).contains(&Kind::RoomToneJump));
    // beds with a gap between them are not "meeting": that is a gap, not a jump
    i.clips[2].start = t(18.0);
    i.levels[1] = levels(Lane::Ambience, |s| Some(if s < 15.0 { -50.0 } else { -38.0 }), 30.0);
    assert!(!kinds(&i).contains(&Kind::RoomToneJump));
    // the lane summary reports the middle and the loudest level
    let m = lane_map(&i).unwrap();
    let amb = m.lanes.iter().find(|l| l.lane == Lane::Ambience).unwrap();
    assert!(m.measured && amb.median_lufs.is_some() && amb.loudest_lufs.unwrap() > -39.0, "{amb:?}");
}

#[test]
fn the_level_meter_reads_tones_at_the_levels_they_have() {
    let rate = 48_000u32;
    let tone = |db: f64, seconds: f64| -> Vec<f32> {
        let amp = 10f64.powf(db / 20.0);
        (0..(f64::from(rate) * seconds) as usize)
            .flat_map(|i| {
                let v = (amp * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / f64::from(rate)).sin()) as f32;
                [v, v]
            })
            .collect()
    };
    let mut loud = LevelMeter::new(rate).unwrap();
    loud.push(&tone(-20.0, 4.0));
    let loud = loud.finish();
    let mut quiet = LevelMeter::new(rate).unwrap();
    quiet.push(&tone(-32.0, 4.0));
    let quiet = quiet.finish();
    assert_eq!(loud.len(), 8, "one reading every half second");
    let (l, q) = (loud[6].unwrap(), quiet[6].unwrap());
    assert!(((l - q) - 12.0).abs() < 0.5, "twelve decibels apart: {l} and {q}");
    assert!(l < -15.0 && l > -30.0, "a -20 dB tone reads a little over -20 LUFS: {l}");
    // silence is none, and pushing in odd pieces gives the same readings
    let mut s = LevelMeter::new(rate).unwrap();
    s.push(&vec![0.0; 96_000]);
    assert!(s.finish().iter().all(Option::is_none));
    let all = tone(-20.0, 4.0);
    let mut odd = LevelMeter::new(rate).unwrap();
    for piece in all.chunks(7_001) {
        odd.push(piece);
    }
    let odd = odd.finish();
    assert_eq!(odd.len(), loud.len());
    assert!(odd.iter().zip(&loud).all(|(a, b)| match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() < 0.2,
        (None, None) => true,
        _ => false,
    }));
}

#[test]
fn hostile_input_is_refused_or_ignored_and_never_panics() {
    assert!(lane_map(&SoundMapInput::default()).is_err(), "no length");
    assert!(findings(&SoundMapInput { duration: i64::MAX, ..Default::default() }).is_err());
    assert!(findings(&SoundMapInput { duration: -5, ..Default::default() }).is_err());
    assert!(LevelMeter::new(0).is_err() && LevelMeter::new(u32::MAX).is_err());
    let i = SoundMapInput {
        duration: t(30.0),
        clips: vec![
            LaneClip { lane: Lane::Music, id: 1, start: i64::MIN, end: i64::MAX },
            LaneClip { lane: Lane::Ambience, id: 2, start: 5, end: 5 },
            LaneClip { lane: Lane::Ambience, id: 3, start: 9, end: 2 },
            LaneClip { lane: Lane::Effects, id: 4, start: -100, end: -50 },
        ],
        silences: vec![(i64::MIN, i64::MAX), (10, 5), (-3, -1)],
        levels: vec![LaneLevels { lane: Lane::Music, values: vec![Some(f32::NAN), Some(f32::INFINITY), None, Some(f32::NEG_INFINITY), Some(-20.0)] }],
    };
    let f = findings(&i).unwrap();
    assert!(f.iter().all(|x| x.start >= 0 && x.numbers.values().all(|v| v.is_finite())), "{f:?}");
    let m = lane_map(&i).unwrap();
    assert!(m.lanes.iter().all(|l| l.share.is_finite() && (0.0..=1.0).contains(&l.share)));
    // too many clips
    let many = SoundMapInput { duration: t(10.0), clips: vec![clip(Lane::Music, 1, 0.0, 1.0); MAX_CLIPS + 1], ..Default::default() };
    assert!(findings(&many).is_err());
    let mut m = LevelMeter::new(8_000).unwrap();
    m.push(&[f32::NAN; 20_000]);
    assert!(m.finish().iter().all(Option::is_none));
}
