//! Tests of the sequence numbers on small timelines worked out by hand, and on hostile ones.

use filmcraft_time::TICKS_PER_SECOND;

use crate::sequence::{ClipSpan, MAX_CLIPS, NarrationSpan, SequenceInput, TrackSpans, TransitionSpan, analyze_sequence};
use crate::{AnalysisError, SequenceStats};

const S: i64 = TICKS_PER_SECOND;

fn clip(id: u64, start: f64, len: f64, link: Option<u64>) -> ClipSpan {
    ClipSpan { id, start: (start * S as f64) as i64, duration: (len * S as f64) as i64, link, is_shot: true }
}

fn track(clips: Vec<ClipSpan>) -> TrackSpans {
    TrackSpans { clips, transitions: Vec::new() }
}

fn input(video: Vec<TrackSpans>, audio: Vec<TrackSpans>) -> SequenceInput {
    SequenceInput { video, audio, narration: Vec::new() }
}

fn run(i: &SequenceInput) -> SequenceStats {
    analyze_sequence(i).unwrap()
}

#[test]
fn three_shots_end_to_end_have_the_lengths_and_cuts_you_can_count() {
    let s = run(&input(vec![track(vec![clip(1, 0.0, 2.0, None), clip(2, 2.0, 4.0, None), clip(3, 6.0, 6.0, None)])], vec![]));
    assert_eq!(s.duration_seconds, 12.0);
    assert_eq!(s.shots.count, 3);
    assert_eq!((s.shots.mean_seconds, s.shots.median_seconds, s.shots.min_seconds, s.shots.max_seconds), (4.0, 4.0, 2.0, 6.0));
    assert_eq!(s.cuts.count, 2);
    assert!((s.cuts.per_minute - 10.0).abs() < 1e-9, "two cuts in 12 s is ten a minute: {}", s.cuts.per_minute);
    let counts: Vec<u32> = s.shots.histogram.iter().map(|b| b.count).collect();
    assert_eq!(counts, [0, 0, 1, 2, 0, 0], "2 s is not under 2 s so it is under 4; 4 s and 6 s are under 8");
    assert_eq!(s.shots.histogram.last().map(|b| b.below_seconds), Some(None));
    assert_eq!(s.tracks.len(), 1);
    assert_eq!((s.tracks[0].share, s.video_coverage, s.audio_coverage), (1.0, 1.0, 0.0));
}

#[test]
fn a_gap_is_not_a_cut_and_the_percentiles_follow_the_lengths() {
    let s = run(&input(vec![track(vec![clip(1, 0.0, 2.0, None), clip(2, 5.0, 2.0, None)])], vec![]));
    assert_eq!((s.shots.count, s.cuts.count), (2, 0));
    assert!((s.tracks[0].share - 4.0 / 7.0).abs() < 1e-9);
    // ten shots of 1..10 s: nearest-rank percentiles
    let clips = (0..10).map(|i| clip(i + 1, (0..i).map(|j| j as f64 + 1.0).sum::<f64>(), i as f64 + 1.0, None)).collect();
    let t = run(&input(vec![track(clips)], vec![]));
    assert_eq!((t.shots.p10_seconds, t.shots.median_seconds, t.shots.p90_seconds), (1.0, 5.0, 9.0));
}

#[test]
fn a_title_is_not_a_shot_but_a_cutaway_on_a_higher_track_is() {
    let base = track(vec![clip(1, 0.0, 10.0, None)]);
    let mut title = clip(2, 2.0, 2.0, None);
    title.is_shot = false;
    let over_title = run(&input(vec![base.clone(), track(vec![title])], vec![]));
    assert_eq!((over_title.shots.count, over_title.cuts.count), (1, 0));
    assert_eq!(over_title.tracks[1].share, 0.2, "it still occupies its track");
    let cutaway = run(&input(vec![base, track(vec![clip(3, 4.0, 2.0, None)])], vec![]));
    assert_eq!((cutaway.shots.count, cutaway.cuts.count), (3, 2), "V1, then the cutaway on V2, then V1 again");
    assert_eq!(cutaway.shots.min_seconds, 2.0);
}

#[test]
fn j_and_l_cuts_are_the_cuts_whose_sound_does_not_change_with_the_picture() {
    let video = vec![track(vec![clip(1, 0.0, 4.0, Some(10)), clip(2, 4.0, 4.0, Some(20)), clip(3, 8.0, 4.0, Some(30))])];
    // shot 2's sound starts a second early (J at the first cut); shot 2's sound runs a second past the second cut (L)
    let audio = vec![track(vec![clip(11, 0.0, 4.0, Some(10)), clip(12, 3.0, 6.0, Some(20)), clip(13, 8.0, 4.0, Some(30))])];
    let s = run(&input(video.clone(), audio));
    assert_eq!(s.cuts.count, 2);
    assert_eq!((s.split_cuts.cuts_with_audio, s.split_cuts.j_cuts, s.split_cuts.l_cuts), (2, 1, 1));
    assert_eq!(s.split_cuts.share, 1.0);
    // straight cuts: sound changes with the picture
    let straight = run(&input(video.clone(), vec![track(vec![clip(11, 0.0, 4.0, Some(10)), clip(12, 4.0, 4.0, Some(20)), clip(13, 8.0, 4.0, Some(30))])]));
    assert_eq!((straight.split_cuts.cuts_with_audio, straight.split_cuts.share), (2, 0.0));
    // half a frame off is still a straight cut
    let near = run(&input(video.clone(), vec![track(vec![clip(11, 0.0, 4.0, Some(10)), clip(12, 3.98, 4.0, Some(20)), clip(13, 8.0, 4.0, Some(30))])]));
    assert_eq!(near.split_cuts.j_cuts, 0);
    // no linked audio: nothing to say, and no divide by zero
    let none = run(&input(video, vec![]));
    assert_eq!((none.split_cuts.cuts_with_audio, none.split_cuts.share), (0, 0.0));
}

#[test]
fn transitions_are_counted_by_kind_and_against_the_joints() {
    let mut v = track(vec![clip(1, 0.0, 4.0, None), clip(2, 4.0, 4.0, None), clip(3, 8.0, 4.0, None)]);
    let t = |at: f64, kind: &str, between: bool| TransitionSpan { start: (at * S as f64) as i64, duration: S, kind: kind.into(), between_clips: between };
    v.transitions = vec![t(3.5, "Cross Dissolve", true), t(0.0, "Fade In", false)];
    let s = run(&input(vec![v], vec![]));
    assert_eq!(s.transitions.count, 2);
    assert_eq!(s.transitions.by_kind.get("Cross Dissolve"), Some(&1));
    assert_eq!(s.transitions.by_kind.get("Fade In"), Some(&1));
    // 2 cuts + 1 dissolve = 3 joints, 1 of them a dissolve
    assert!((s.transitions.share_of_joints - 1.0 / 3.0).abs() < 1e-9);
}

#[test]
fn narration_pace_is_words_over_the_minutes_spoken() {
    let mut i = input(vec![track(vec![clip(1, 0.0, 60.0, None)])], vec![]);
    let span = |start: f64, len: f64, words: u32| NarrationSpan { start: (start * S as f64) as i64, duration: (len * S as f64) as i64, words };
    i.narration = vec![span(0.0, 10.0, 25), span(20.0, 20.0, 60)];
    let n = run(&i).narration;
    assert_eq!(n.words, 85);
    assert!((n.spoken_seconds - 30.0).abs() < 1e-9);
    assert!((n.words_per_minute.unwrap() - 170.0).abs() < 1e-9);
    assert!((n.share - 0.5).abs() < 1e-9);
    assert_eq!(run(&input(vec![], vec![])).narration.words_per_minute, None);
}

#[test]
fn occupancy_counts_overlaps_once_and_the_cut_series_follows_the_timeline() {
    let a = track(vec![clip(1, 0.0, 6.0, None), clip(2, 4.0, 6.0, None)]);
    let s = run(&input(vec![a], vec![track(vec![clip(3, 0.0, 5.0, None)])]));
    assert!((s.tracks[0].share - 1.0).abs() < 1e-9, "0 to 10 s covered once");
    assert!((s.tracks[1].share - 0.5).abs() < 1e-9 && !s.tracks[1].video);
    assert!((s.audio_coverage - 0.5).abs() < 1e-9);
    // cuts at 10, 50 and 70 s fall in the 1st, 2nd and 3rd 30 s windows
    let clips = vec![clip(1, 0.0, 10.0, None), clip(2, 10.0, 40.0, None), clip(3, 50.0, 20.0, None), clip(4, 70.0, 20.0, None)];
    let c = run(&input(vec![track(clips)], vec![])).cuts;
    assert_eq!((c.count, c.window_seconds), (3, 30.0));
    assert_eq!(c.series, [1, 1, 1]);
}

#[test]
fn an_empty_timeline_has_nothing_and_nothing_divides_by_zero() {
    let s = run(&SequenceInput::default());
    assert_eq!((s.duration_seconds, s.shots.count, s.cuts.count, s.cuts.per_minute, s.video_coverage), (0.0, 0, 0, 0.0, 0.0));
    assert_eq!(s.shots.histogram.len(), 6);
    assert!(s.shots.mean_seconds == 0.0 && s.narration.share == 0.0);
}

#[test]
fn hostile_spans_are_cut_to_fit_and_never_panic() {
    let bad = |start: i64, duration: i64| ClipSpan { id: 1, start, duration, link: Some(1), is_shot: true };
    let hostile = vec![
        bad(-5 * S, 10 * S),
        bad(0, 0),
        bad(0, -S),
        bad(i64::MAX, i64::MAX),
        bad(i64::MIN, i64::MAX),
        bad(i64::MAX - 1, 5),
        bad(S, i64::MIN),
        bad(0, i64::MAX),
        bad(0, 5 * S),
        bad(2 * S, 5 * S),
    ];
    let mut i = input(vec![track(hostile.clone()), track(hostile.clone())], vec![track(hostile)]);
    i.video[0].transitions = vec![
        TransitionSpan { start: i64::MAX, duration: i64::MAX, kind: String::new(), between_clips: true },
        TransitionSpan { start: -1, duration: -1, kind: "x".into(), between_clips: false },
    ];
    i.narration = vec![
        NarrationSpan { start: i64::MAX, duration: i64::MAX, words: u32::MAX },
        NarrationSpan { start: -3, duration: 9, words: u32::MAX },
        NarrationSpan { start: 0, duration: 0, words: 5 },
    ];
    let s = analyze_sequence(&i).unwrap();
    assert!(s.duration_seconds.is_finite() && s.cuts.per_minute.is_finite() && s.shots.mean_seconds.is_finite());
    assert!(s.tracks.iter().all(|t| (0.0..=1.0 + 1e-9).contains(&t.share)), "{:?}", s.tracks);
    assert!(s.narration.words_per_minute.is_none_or(f64::is_finite));
    assert!(s.cuts.series.len() <= 2000);
}

#[test]
fn too_many_clips_are_refused() {
    let many = TrackSpans { clips: vec![clip(1, 0.0, 1.0, None); MAX_CLIPS + 1], transitions: Vec::new() };
    assert_eq!(analyze_sequence(&input(vec![many], vec![])), Err(AnalysisError::TooManyClips { clips: MAX_CLIPS + 1 }));
}

#[test]
fn a_long_timeline_of_many_short_shots_is_measured_quickly_and_the_same_every_time() {
    // 100,000 one-second shots on three tracks
    let v1 = track((0..100_000).map(|i| clip(i + 1, i as f64, 1.0, Some(i + 1))).collect());
    let v2 = track((0..20_000).map(|i| clip(200_000 + i, i as f64 * 5.0 + 2.0, 1.0, None)).collect());
    let a1 = track((0..100_000).map(|i| clip(300_000 + i, i as f64, 1.0, Some(i + 1))).collect());
    let i = input(vec![v1, v2], vec![a1]);
    let s = run(&i);
    assert_eq!(s, run(&i));
    // the cutaways on V2 each replace one second of V1, so the shots stay 100,000 and the cuts 99,999
    assert_eq!((s.shots.count, s.cuts.count), (100_000, 99_999));
    let back: SequenceStats = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    assert_eq!(s, back);
    assert!(serde_json::to_value(&s).unwrap().get("splitCuts").is_some());
}
