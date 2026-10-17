//! Tests of the coach's rules on edits worked out by hand: what each rule fires on, what it leaves
//! alone (an ordinary edit gets no findings), and hostile input.

use filmcraft_time::TICKS_PER_SECOND;

use crate::audio::Mix;
use crate::coach::{Finding, Kind, MAX_FINDINGS, SectionTarget, Severity, Targets, findings};
use crate::sequence::{ClipSpan, NarrationSpan, SequenceInput, TrackSpans, TransitionSpan};

const S: i64 = TICKS_PER_SECOND;

fn clip(id: u64, start: f64, len: f64, link: Option<u64>) -> ClipSpan {
    ClipSpan { id, start: (start * S as f64) as i64, duration: (len * S as f64) as i64, link, is_shot: true }
}

/// Shots end to end on V1 with the given lengths (seconds).
fn edit(lengths: &[f64]) -> SequenceInput {
    let mut at = 0.0;
    let clips = lengths
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let c = clip(i as u64 + 1, at, *l, None);
            at += l;
            c
        })
        .collect();
    SequenceInput { video: vec![TrackSpans { clips, transitions: vec![] }], audio: vec![], narration: vec![] }
}

fn run(i: &SequenceInput, t: &Targets) -> Vec<Finding> {
    findings(i, t, None).unwrap()
}

fn kinds(f: &[Finding]) -> Vec<Kind> {
    f.iter().map(|f| f.kind).collect()
}

fn target(avg: f64, wpm: f64) -> Targets {
    Targets { words_per_minute: Some(wpm), average_shot_seconds: Some(avg), ..Default::default() }
}

#[test]
fn a_jarring_short_shot_between_two_long_ones_is_found_at_its_timecode_and_explained() {
    // the roadmap's own example: 6 s, then 0.8 s, then 6 s
    let f = run(&edit(&[6.0, 0.8, 6.0]), &Targets::default());
    assert_eq!(f.len(), 1, "{f:?}");
    let x = &f[0];
    assert_eq!((x.kind, x.severity, x.start, x.end), (Kind::ShortBetweenLong, Severity::Look, 6 * S, Some(6 * S + 8 * S / 10)));
    assert_eq!(x.id, format!("shortBetweenLong@{}", 6 * S));
    assert!(x.title.contains("0.8 s") && x.title.contains("6 s") && x.title.contains("0:06"), "{}", x.title);
    assert!(x.detail.contains("0.8 s") && x.detail.contains("jolt"), "{}", x.detail);
    assert_eq!(x.idea.as_deref(), Some("rhythm"));
    assert_eq!((x.numbers["seconds"], x.numbers["before"], x.numbers["after"]), (0.8, 6.0, 6.0));
}

#[test]
fn an_ordinary_edit_gets_no_findings() {
    let e = edit(&[4.0, 5.0, 6.0, 3.0, 5.0, 4.5, 6.0, 5.0, 4.0, 5.5]);
    assert!(run(&e, &Targets::default()).is_empty());
    assert!(run(&e, &target(5.0, 150.0)).is_empty(), "on target: nothing to say");
    // a quick shot that is not next to two long ones is a style, not a jolt
    assert!(run(&edit(&[2.0, 1.0, 2.0, 1.2, 2.0, 1.0]), &Targets::default()).is_empty());
    // a short shot beside a long one only
    assert!(run(&edit(&[6.0, 0.8, 1.0, 6.0]), &Targets::default()).is_empty());
    assert!(run(&SequenceInput::default(), &Targets::default()).is_empty());
}

#[test]
fn a_flash_frame_is_a_problem_and_is_not_also_a_jolt() {
    let f = run(&edit(&[5.0, 0.04, 5.0]), &Targets::default());
    assert_eq!(kinds(&f), [Kind::FlashFrame]);
    assert_eq!((f[0].severity, f[0].start), (Severity::Problem, 5 * S));
    assert!(f[0].detail.contains("glitch"));
}

#[test]
fn a_gap_that_plays_as_black_is_a_problem_unless_something_covers_it() {
    let mut e = edit(&[5.0]);
    e.video[0].clips.push(clip(2, 7.0, 5.0, None));
    let f = run(&e, &Targets::default());
    assert_eq!(kinds(&f), [Kind::EmptyGap]);
    assert_eq!((f[0].severity, f[0].start, f[0].end), (Severity::Problem, 5 * S, Some(7 * S)));
    assert!(f[0].title.contains("2 s") && f[0].title.contains("0:05"));
    // a title card on a higher track covers it: not a gap
    let mut covered = e.clone();
    let mut title = clip(9, 5.0, 2.0, None);
    title.is_shot = false;
    covered.video.push(TrackSpans { clips: vec![title], transitions: vec![] });
    assert!(run(&covered, &Targets::default()).is_empty());
    // the picture starting late is only worth a look
    let mut late = edit(&[5.0, 5.0]);
    for c in &mut late.video[0].clips {
        c.start += 2 * S;
    }
    let f = run(&late, &Targets::default());
    assert_eq!((kinds(&f), f[0].severity, f[0].start), (vec![Kind::EmptyGap], Severity::Look, 0));
    // a frame's worth of gap is not worth a note
    let mut tiny = edit(&[5.0]);
    tiny.video[0].clips.push(clip(2, 5.03, 5.0, None));
    assert!(run(&tiny, &Targets::default()).is_empty());
}

#[test]
fn a_long_hold_is_measured_against_the_target() {
    let e = edit(&[5.0, 25.0, 5.0, 15.0, 5.0]);
    let with = run(&e, &target(6.0, 150.0));
    let holds: Vec<_> = with.iter().filter(|f| f.kind == Kind::LongHold).collect();
    assert_eq!(holds.len(), 1, "25 s is over 3x6; 15 s is not: {with:?}");
    assert_eq!(holds[0].start, 5 * S);
    assert!(holds[0].detail.contains("target is about 6 s"));
    // with no target the bar is 20 s
    let without = run(&e, &Targets::default());
    assert_eq!(without.iter().filter(|f| f.kind == Kind::LongHold).count(), 1);
    assert_eq!(run(&edit(&[5.0, 18.0, 5.0]), &Targets::default()).len(), 0);
}

#[test]
fn the_pace_is_compared_with_the_plan_overall_or_by_section() {
    let slow = edit(&[12.0; 6]);
    let f = run(&slow, &target(6.0, 150.0));
    let p = f.iter().find(|f| f.kind == Kind::PaceVsPlan).expect("slower than planned");
    assert!(p.title.contains("slower") && p.title.contains("12 s") && p.title.contains("6 s"), "{}", p.title);
    let fast = run(&edit(&[2.0; 8]), &target(6.0, 150.0));
    assert!(fast.iter().any(|f| f.kind == Kind::PaceVsPlan && f.title.contains("faster")));
    assert!(!run(&edit(&[7.0, 8.0, 7.0, 8.0, 7.0, 8.0]), &target(6.0, 150.0)).iter().any(|f| f.kind == Kind::PaceVsPlan), "25% off is within reach");
    assert!(!run(&edit(&[12.0, 12.0]), &target(6.0, 150.0)).iter().any(|f| f.kind == Kind::PaceVsPlan), "two shots say little");
    // sections: the opening is meant to be quick (3 s) and runs at 8 s; the rest is fine
    let e = edit(&[8.0, 8.0, 8.0, 6.0, 6.0, 6.0]);
    let mut t = target(6.0, 150.0);
    t.sections = vec![
        SectionTarget { name: "Opening".into(), start: 0, end: 24 * S, average_shot_seconds: Some(3.0) },
        SectionTarget { name: "Middle".into(), start: 24 * S, end: 42 * S, average_shot_seconds: None },
    ];
    let f = run(&e, &t);
    let sec: Vec<_> = f.iter().filter(|f| f.kind == Kind::SectionPace).collect();
    assert_eq!(sec.len(), 1, "{f:?}");
    assert!(sec[0].title.contains("Opening") && sec[0].title.contains("slower"), "{}", sec[0].title);
    assert_eq!((sec[0].start, sec[0].end), (0, Some(24 * S)));
    assert!(!f.iter().any(|f| f.kind == Kind::PaceVsPlan), "sections replace the overall check");
}

#[test]
fn a_burst_of_quick_cuts_is_noticed() {
    let mut lengths = vec![1.0; 20];
    lengths.extend([20.0, 20.0, 20.0, 20.0, 20.0]);
    let f = run(&edit(&lengths), &Targets::default());
    let bursts: Vec<_> = f.iter().filter(|f| f.kind == Kind::CutBurst).collect();
    assert_eq!(bursts.len(), 1, "{f:?}");
    assert_eq!(bursts[0].start, 0);
    assert!(bursts[0].numbers["perMinute"] > bursts[0].numbers["overallPerMinute"] * 2.5);
    // an even pace has no burst
    assert!(!run(&edit(&[3.0; 60]), &Targets::default()).iter().any(|f| f.kind == Kind::CutBurst));
}

#[test]
fn the_narration_pace_is_flagged_fast_or_slow_and_graded() {
    let with = |words: u32| {
        let mut e = edit(&[60.0]);
        e.narration = vec![NarrationSpan { start: 4 * S, duration: 60 * S, words }];
        run(&e, &target(6.0, 150.0)).into_iter().find(|f| f.kind == Kind::NarrationPace)
    };
    let fast = with(180).unwrap();
    assert_eq!((fast.severity, fast.start), (Severity::Look, 4 * S));
    assert!(fast.title.contains("fast") && fast.title.contains("180") && fast.title.contains("150"), "{}", fast.title);
    assert_eq!(with(250).unwrap().severity, Severity::Problem);
    assert_eq!(with(110).unwrap().severity, Severity::Look);
    assert!(with(110).unwrap().title.contains("slow"));
    assert_eq!(with(80).unwrap().severity, Severity::Problem);
    assert!(with(150).is_none() && with(165).is_none() && with(125).is_none(), "within the band");
    assert!(with(15).is_none(), "too few words to judge");
    let mut e = edit(&[60.0]);
    e.narration = vec![NarrationSpan { start: 0, duration: 60 * S, words: 300 }];
    assert!(!run(&e, &Targets::default()).iter().any(|f| f.kind == Kind::NarrationPace), "no target, no verdict");
}

#[test]
fn few_split_cuts_and_many_dissolves_are_pointed_out() {
    // eleven shots with linked sound that changes with the picture
    let mut e = edit(&[4.0; 11]);
    let audio: Vec<ClipSpan> = (0..11).map(|i| clip(100 + i, i as f64 * 4.0, 4.0, Some(i + 1))).collect();
    for (i, c) in e.video[0].clips.iter_mut().enumerate() {
        c.link = Some(i as u64 + 1);
    }
    e.audio = vec![TrackSpans { clips: audio.clone(), transitions: vec![] }];
    assert!(run(&e, &Targets::default()).iter().any(|f| f.kind == Kind::FewSplitCuts));
    // with J cuts (sound a second early) on most cuts, it is fine
    let mut j = e.clone();
    for c in j.audio[0].clips.iter_mut().skip(1) {
        c.start -= S;
        c.duration += S;
    }
    assert!(!run(&j, &Targets::default()).iter().any(|f| f.kind == Kind::FewSplitCuts));
    // dissolves on most joints
    let mut d = edit(&[4.0; 10]);
    d.video[0].transitions =
        (1..10).map(|i| TransitionSpan { start: i * 4 * S - S / 2, duration: S, kind: "Cross Dissolve".into(), between_clips: true }).collect();
    let f = run(&d, &Targets::default());
    assert!(f.iter().any(|f| f.kind == Kind::ManyDissolves && f.title.contains("9 transitions")), "{f:?}");
    d.video[0].transitions.truncate(4);
    assert!(!run(&d, &Targets::default()).iter().any(|f| f.kind == Kind::ManyDissolves));
}

fn mix(silence: f32, lra: Option<f64>, lufs: Option<f64>, peak: Option<f64>) -> Mix {
    Mix { duration_seconds: 120.0, integrated_lufs: lufs, loudness_range_lu: lra, true_peak_dbtp: peak, silence_share: silence }
}

#[test]
fn a_measured_mix_adds_the_loudness_rules() {
    let e = edit(&[4.0, 5.0, 6.0, 5.0]);
    let t = Targets::default();
    let f = findings(&e, &t, Some(&mix(0.4, Some(15.0), Some(-23.0), Some(-0.2)))).unwrap();
    let by = |k: Kind| f.iter().find(|x| x.kind == k).unwrap_or_else(|| panic!("no {k:?}: {f:?}"));
    assert_eq!(by(Kind::MostlySilent).title, "40% of the video is silent");
    assert!(by(Kind::WideLoudness).title.contains("15 LU"));
    assert_eq!(by(Kind::LoudnessOffTarget).severity, Severity::Problem, "9 LU under the target");
    assert!(by(Kind::LoudnessOffTarget).title.contains("-23.0") && by(Kind::LoudnessOffTarget).title.contains("below"));
    assert_eq!(by(Kind::PeakTooHigh).severity, Severity::Problem);
    // problems first
    assert!(f.windows(2).all(|w| w[0].severity >= w[1].severity));
    // a healthy mix says nothing; a mild miss is only worth a look
    assert!(findings(&e, &t, Some(&mix(0.05, Some(7.0), Some(-14.5), Some(-2.0)))).unwrap().is_empty());
    let mild = findings(&e, &t, Some(&mix(0.0, None, Some(-18.5), None))).unwrap();
    assert_eq!((mild.len(), mild[0].severity), (1, Severity::Look));
    assert!(findings(&e, &t, Some(&mix(0.9, None, None, None))).unwrap().iter().any(|f| f.kind == Kind::MostlySilent));
    let short = Mix { duration_seconds: 5.0, ..mix(0.9, None, None, None) };
    assert!(findings(&e, &t, Some(&short)).unwrap().is_empty(), "a five second clip being quiet is not news");
    let loud = Targets { loudness_lufs: -23.0, ..Default::default() };
    assert!(findings(&e, &loud, Some(&mix(0.0, None, Some(-23.5), None))).unwrap().is_empty(), "the target can be changed");
}

#[test]
fn findings_come_worst_first_then_in_time_and_a_flood_is_summed_up() {
    let mut lengths = vec![5.0];
    for _ in 0..60 {
        lengths.extend([0.05, 5.0]);
    }
    let f = run(&edit(&lengths), &Targets::default());
    let flashes: Vec<_> = f.iter().filter(|f| f.kind == Kind::FlashFrame).collect();
    assert_eq!(flashes.len(), 26, "25 listed and one line for the rest: {}", f.len());
    let more = flashes.iter().find(|f| f.id == "flashFrame@more").unwrap();
    assert_eq!(more.numbers["more"], 35.0);
    assert!(more.title.contains("35 more"));
    assert!(f.len() <= MAX_FINDINGS);
    let ids: std::collections::BTreeSet<&str> = f.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids.len(), f.len(), "ids are unique");
    // order: problems before looks, then by time
    let mut mixed = edit(&[6.0, 0.8, 6.0, 5.0, 0.04, 5.0]);
    mixed.video[0].clips.push(clip(99, 40.0, 5.0, None));
    let g = run(&mixed, &Targets::default());
    assert!(g.windows(2).all(|w| (w[0].severity, std::cmp::Reverse(w[0].start)) >= (w[1].severity, std::cmp::Reverse(w[1].start))), "{g:?}");
    assert_eq!(g[0].severity, Severity::Problem);
}

#[test]
fn hostile_targets_and_edits_never_panic_or_make_nonsense() {
    let e = edit(&[6.0, 0.8, 6.0, 12.0, 12.0, 12.0, 12.0]);
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -3.0, f64::MIN_POSITIVE, 1e300, f64::MAX] {
        let t = Targets {
            words_per_minute: Some(bad),
            average_shot_seconds: Some(bad),
            loudness_lufs: bad,
            sections: vec![
                SectionTarget { name: "x".repeat(10_000), start: i64::MIN, end: i64::MAX, average_shot_seconds: Some(bad) },
                SectionTarget { name: "back".into(), start: 10 * S, end: S, average_shot_seconds: Some(bad) },
                SectionTarget { name: "neg".into(), start: -5, end: -1, average_shot_seconds: None },
            ],
        };
        let f = findings(&e, &t, Some(&mix(f32::NAN, Some(f64::NAN), Some(f64::INFINITY), Some(f64::NAN)))).unwrap();
        for x in &f {
            assert!(x.numbers.values().all(|v| v.is_finite()), "{bad}: {x:?}");
            assert!(!x.title.contains("NaN") && !x.title.contains("inf") && !x.detail.contains("NaN") && !x.detail.contains("inf"), "{bad}: {x:?}");
        }
    }
    // hostile spans
    let mut h = SequenceInput::default();
    h.video.push(TrackSpans {
        clips: vec![
            ClipSpan { id: 1, start: i64::MAX, duration: i64::MAX, link: None, is_shot: true },
            ClipSpan { id: 2, start: i64::MIN, duration: i64::MAX, link: None, is_shot: true },
            ClipSpan { id: 3, start: 0, duration: -1, link: None, is_shot: true },
        ],
        transitions: vec![TransitionSpan { start: i64::MAX, duration: i64::MAX, kind: String::new(), between_clips: true }],
    });
    h.narration = vec![NarrationSpan { start: i64::MAX, duration: i64::MAX, words: u32::MAX }];
    let f = findings(&h, &target(5.0, 150.0), None).unwrap();
    assert!(f.iter().all(|x| x.numbers.values().all(|v| v.is_finite())));
}

#[test]
fn the_same_edit_always_gives_the_same_findings_and_they_round_trip_as_json() {
    let e = edit(&[6.0, 0.8, 6.0, 5.0, 0.04, 5.0, 30.0]);
    let t = target(5.0, 150.0);
    let a = run(&e, &t);
    assert_eq!(a, run(&e, &t));
    let json = serde_json::to_value(&a).unwrap();
    let back: Vec<Finding> = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(a, back);
    let first = json.as_array().unwrap().iter().find(|f| f["kind"] == "shortBetweenLong").unwrap();
    assert_eq!((first["severity"].as_str(), first["idea"].as_str()), (Some("look"), Some("rhythm")));
    assert!(first["numbers"]["seconds"].is_f64());
    let t2: Targets = serde_json::from_value(serde_json::json!({"wordsPerMinute": 140})).unwrap();
    assert_eq!((t2.words_per_minute, t2.average_shot_seconds, t2.loudness_lufs), (Some(140.0), None, -14.0));
}
