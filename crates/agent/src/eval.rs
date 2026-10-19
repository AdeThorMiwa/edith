//! The evaluation suite (roadmap F03.6): tasks, code graders, and reports that compare runs.
//!
//! A [`Task`] is a question about a fixture project plus the checks its answer must pass. Running
//! a task produces a [`Trace`] (the same record a normal conversation keeps), and grading reads only
//! that trace: [`grade`] needs no model and no network, so graders can be tested against recorded or
//! scripted runs, a grader change can be re-applied to old traces, and a result is about the agent
//! or the model and never both at once (run the same suite on another model, or another prompt
//! pack, and [`compare`] the reports).
//!
//! Graders are code first: did it call the tools the question needs, are the times it cites inside
//! the edit, did it name the right clips, did it stay read-only, did it stay inside its budget. What
//! they cannot judge is craft; the research notes say that needs human-rated examples
//! (`docs/research/agent-and-editing.md`), which come with F10 and later.

use crate::agent::{AgentEvent, Finish};
use crate::trace::Trace;
use edith_llm::{Price, Usage};
use serde::{Deserialize, Serialize};

const BUILTIN: &str = include_str!("../suites/core.json");
const ROUGH_CUT: &str = include_str!("../suites/rough_cut.json");
const WHOLE_VIDEO: &str = include_str!("../suites/whole_video.json");
/// Most characters of an answer kept in a report.
const MAX_ANSWER: usize = 4000;

/// One thing an answer must satisfy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "check", rename_all = "snake_case")]
pub enum Grader {
    /// Called every tool in `all` at least once, and none in `none`.
    CallsTools {
        #[serde(default)]
        all: Vec<String>,
        #[serde(default)]
        none: Vec<String>,
    },
    /// Called `first` and, only after it, `then`: looked before it offered, listened before it chose. Fails when
    /// either was never called, or `then` came first.
    CallsInOrder { first: String, then: String },
    /// What was built passes these checks of the edit (`edit.checks`, including the should-checks): each id named must
    /// read `pass`. Fails when nothing was built to check, so an assistant that does nothing never passes.
    ChecksPass { ids: Vec<String> },
    /// The answer names something from each group: every inner list is alternatives (any one will
    /// do), and every group must be satisfied. Matching ignores case, `_` and `-`; a phrase that is
    /// only digits must match a whole number, so `6` is not found inside `26`.
    Mentions { groups: Vec<Vec<String>> },
    /// The answer contains none of these phrases.
    Avoids { phrases: Vec<String> },
    /// Every time the answer cites (timecodes, or numbers of seconds) lies inside the edit, and at
    /// least `min_count` are cited. A time is judged by its most generous reading, so a correct
    /// answer is never failed by an ambiguous notation.
    TimesWithin {
        max_seconds: f64,
        #[serde(default = "default_fps")]
        fps: f64,
        #[serde(default)]
        min_count: u32,
    },
    /// Stayed inside a budget.
    Budget {
        #[serde(default)]
        max_steps: Option<u32>,
        #[serde(default)]
        max_tokens: Option<u64>,
        #[serde(default)]
        max_cost_usd: Option<f64>,
    },
    /// The project is exactly as it was (the runner measures this). For a task in Edit mode it is the person's
    /// own edit that must be as it was: the assistant's work is in a version, which only the person can commit.
    ProjectUnchanged,
    /// What the assistant built passes every must-pass check of the edit (`edit.checks`): the runner runs the checks
    /// on the finished project (the version, in Edit mode). Fails when nothing was built to check.
    MustPassChecks,
    /// Every number the answer states was in a tool result (within rounding): the model reports the
    /// measurements and does not invent them. Whole numbers up to 10 are free (counting, list
    /// numbers, "two seconds" as advice); `min_numbers` says how many numbers an answer must carry at
    /// all, so an answer that avoids numbers does not pass by saying none.
    NumbersFromTools {
        #[serde(default)]
        min_numbers: u32,
    },
}

fn default_fps() -> f64 {
    24.0
}

/// A question about a fixture, and how to tell a good answer from a bad one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    /// What the person asks.
    pub question: String,
    /// The fixture project the runner builds before asking (`demo`, `demo-hostile-marker`).
    pub fixture: String,
    /// Context chips sent along (`frame`, `transcript`, `markers`).
    #[serde(default)]
    pub context: Vec<String>,
    pub graders: Vec<Grader>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// How the assistant works for this task: `ask` (the default) or `coach`.
    #[serde(default)]
    pub mode: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Suite {
    pub name: String,
    pub version: u32,
    pub tasks: Vec<Task>,
}

impl Suite {
    /// The suite shipped with the app.
    pub fn builtin() -> Suite {
        // The shipped file is checked by a test; an unreadable one would be an empty suite.
        Self::parse(BUILTIN).unwrap_or(Suite { name: "builtin".into(), version: 0, tasks: Vec::new() })
    }

    /// A suite that ships with the app, by name: `core` (questions about a project) or `rough-cut` (the rough-cut
    /// workflow: finding media, asking, cutting on word edges, checking, recording progress) or `whole-video` (looking
    /// before offering, sound, titles, polish).
    pub fn named(name: &str) -> Option<Suite> {
        match name {
            "core" => Some(Self::builtin()),
            "rough-cut" => Self::parse(ROUGH_CUT).ok(),
            "whole-video" => Self::parse(WHOLE_VIDEO).ok(),
            _ => None,
        }
    }

    /// Reads a suite. Ids must be unique and short, and a task needs a question and a grader.
    pub fn parse(text: &str) -> Result<Suite, String> {
        let suite: Suite =
            serde_json::from_str(text).map_err(|e| format!("the suite is not valid: {}", e.to_string().chars().take(120).collect::<String>()))?;
        if suite.tasks.len() > 200 {
            return Err("a suite may hold at most 200 tasks".into());
        }
        for (i, t) in suite.tasks.iter().enumerate() {
            let ok_id = !t.id.is_empty() && t.id.len() <= 60 && t.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
            if !ok_id || suite.tasks.iter().skip(i + 1).any(|o| o.id == t.id) {
                return Err(format!("task {} has a missing, repeated or unusable id", i + 1));
            }
            if t.question.trim().is_empty() || t.question.len() > 4000 || t.graders.is_empty() {
                return Err(format!("task `{}` needs a question (up to 4000 characters) and at least one grader", t.id));
            }
        }
        Ok(suite)
    }

    pub fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Grade {
    /// Which check (`finished`, `read_only`, `calls_tools`, …).
    pub check: String,
    pub passed: bool,
    /// Plain words saying what was found.
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskResult {
    pub task: String,
    pub title: String,
    pub passed: bool,
    pub grades: Vec<Grade>,
    pub finish: String,
    pub steps: u32,
    pub usage: Usage,
    pub cost_usd: Option<f64>,
    pub seconds: f64,
    pub answer: String,
    pub tools: Vec<String>,
}

/// What grading needs besides the trace.
pub struct GradeContext<'a> {
    /// Whether a tool leaves the project unchanged (`None`: unknown tool).
    pub read_only: &'a dyn Fn(&str) -> Option<bool>,
    pub price: Option<&'a Price>,
    /// `Some(true)` when the runner saw the project unchanged; `None` when it did not measure.
    pub project_unchanged: Option<bool>,
    /// How many must-pass checks of the finished edit failed; `None` when there was nothing to check or the runner
    /// did not measure.
    pub must_fail: Option<u32>,
    /// What each check of the finished edit read (`pass`, `fail`, `note`, `needsYou`, `na`), by id; `None` when
    /// nothing was built to check or the runner did not measure.
    pub checks: Option<&'a [(String, String)]>,
    pub seconds: f64,
}

/// Everything the assistant said this turn. Text from a step that was asked again is dropped.
pub fn answer_text(events: &[AgentEvent]) -> String {
    let mut out = String::new();
    let mut step_start = 0usize;
    for e in events {
        match e {
            AgentEvent::StepStarted { .. } => step_start = out.len(),
            AgentEvent::StepRestarted => out.truncate(step_start),
            AgentEvent::Text { text } => out.push_str(text),
            _ => {}
        }
    }
    out
}

fn tools_called(events: &[AgentEvent]) -> Vec<String> {
    events.iter().filter_map(|e| if let AgentEvent::ToolCall { name, .. } = e { Some(name.clone()) } else { None }).collect()
}

fn finish_of(events: &[AgentEvent]) -> Option<&Finish> {
    events.iter().rev().find_map(|e| if let AgentEvent::Finished { finish } = e { Some(finish) } else { None })
}

fn normal(s: &str) -> String {
    s.to_lowercase().replace(['_', '-'], " ")
}

fn tokens(s: &str) -> Vec<&str> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).collect()
}

fn phrase_found(answer: &str, phrase: &str) -> bool {
    let p = phrase.trim();
    if p.is_empty() {
        return false;
    }
    if p.bytes().all(|b| b.is_ascii_digit()) {
        return tokens(answer).contains(&p);
    }
    normal(answer).contains(&normal(p))
}

/// The times an answer cites, in seconds, each by its most generous reading.
pub fn times_in(text: &str, fps: f64) -> Vec<f64> {
    let fps = if fps.is_finite() && fps >= 1.0 { fps } else { 24.0 };
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let starts =
            chars.get(i).is_some_and(char::is_ascii_digit) && (i == 0 || !chars.get(i - 1).is_some_and(|c| c.is_alphanumeric() || *c == '.' || *c == ':'));
        if !starts {
            i += 1;
            continue;
        }
        let mut j = i;
        while chars.get(j).is_some_and(|c| c.is_ascii_digit() || *c == ':' || *c == '.') {
            j += 1;
        }
        let token: String = chars[i..j].iter().collect::<String>().trim_end_matches(['.', ':']).to_string();
        if token.contains(':') {
            let parts: Vec<Option<f64>> = token.split(':').map(|p| if p.is_empty() || p.contains('.') { None } else { p.parse().ok() }).collect();
            if let Some(v) = parts.iter().copied().collect::<Option<Vec<f64>>>() {
                let t = match v.as_slice() {
                    [a, b] => Some((a * 60.0 + b).min(a + b / fps)),
                    [a, b, c] => Some((a * 3600.0 + b * 60.0 + c).min(a * 60.0 + b + c / fps)),
                    [a, b, c, d] => Some(a * 3600.0 + b * 60.0 + c + d / fps),
                    _ => None,
                };
                out.extend(t);
            }
        } else if let Ok(n) = token.parse::<f64>() {
            // a plain number counts only with a seconds unit after it: "9.5 s", "12 seconds"
            let mut k = j;
            while chars.get(k) == Some(&' ') {
                k += 1;
            }
            let mut w = k;
            while chars.get(w).is_some_and(|c| c.is_ascii_alphabetic()) {
                w += 1;
            }
            let word: String = chars[k..w].iter().collect::<String>().to_lowercase();
            if matches!(word.as_str(), "s" | "sec" | "secs" | "second" | "seconds") {
                out.push(n);
            }
        }
        i = j.max(i + 1);
    }
    out
}

/// The numbers written in `text`, in order: `12`, `0.8`, `1,840`, `89%`. A time like `2:41` gives `2`
/// and `41`. A number glued to letters (`#29`, `V1`, `h264`) is a name, not a measurement.
pub fn numbers_in(text: &str) -> Vec<f64> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let prev = i.checked_sub(1).and_then(|p| chars.get(p)).copied();
        if !chars.get(i).is_some_and(char::is_ascii_digit) || prev.is_some_and(|c| c.is_alphanumeric() || c == '#' || c == '.') {
            i += 1;
            continue;
        }
        let mut j = i;
        let mut token = String::new();
        while let Some(c) = chars.get(j).copied() {
            if c.is_ascii_digit() || (c == '.' && chars.get(j + 1).is_some_and(char::is_ascii_digit) && !token.contains('.')) {
                token.push(c);
            } else if c == ',' && chars.get(j + 1).is_some_and(char::is_ascii_digit) && !token.contains('.') {
                // a thousands separator
            } else {
                break;
            }
            j += 1;
        }
        let glued = chars.get(j).is_some_and(|c| c.is_alphabetic());
        if !glued && let Ok(n) = token.parse::<f64>() {
            out.push(n);
        }
        i = j.max(i + 1);
    }
    out
}

/// Whether `n` is one of `found`, within rounding: a tenth of a unit, or six per cent.
fn supported(n: f64, found: &[f64]) -> bool {
    found.iter().any(|m| (n - m).abs() <= 0.06_f64.max(0.06 * m.abs()))
}

fn grade_one(g: &Grader, events: &[AgentEvent], answer: &str, usage: &Usage, steps: u32, cost: Option<f64>, cx: &GradeContext<'_>) -> Grade {
    let (check, passed, detail) = match g {
        Grader::CallsTools { all, none } => {
            let called = tools_called(events);
            let missing: Vec<&str> = all.iter().filter(|t| !called.contains(t)).map(String::as_str).collect();
            let forbidden: Vec<&str> = none.iter().filter(|t| called.contains(t)).map(String::as_str).collect();
            let detail = if missing.is_empty() && forbidden.is_empty() {
                format!("called {}", if called.is_empty() { "no tools".to_string() } else { called.join(", ") })
            } else {
                format!("missing: {}; should not have called: {}", missing.join(", "), forbidden.join(", "))
            };
            ("calls_tools", missing.is_empty() && forbidden.is_empty(), detail)
        }
        Grader::CallsInOrder { first, then } => {
            let called = tools_called(events);
            let a = called.iter().position(|t| t == first);
            let b = called.iter().position(|t| t == then);
            let detail = match (a, b) {
                (Some(a), Some(b)) if a < b => format!("{first} came before {then}"),
                (Some(_), Some(_)) => format!("{then} was called before {first}"),
                (None, Some(_)) => format!("{then} was called without {first} first"),
                (Some(_), None) => format!("{then} was never called"),
                (None, None) => format!("neither {first} nor {then} was called"),
            };
            ("calls_in_order", matches!((a, b), (Some(a), Some(b)) if a < b), detail)
        }
        Grader::ChecksPass { ids } => match cx.checks {
            None => ("checks_pass", false, "nothing was built to check, or the runner did not measure it".to_string()),
            Some(got) => {
                let not: Vec<String> = ids
                    .iter()
                    .filter(|id| got.iter().find(|(i, _)| i == *id).is_none_or(|(_, st)| st != "pass"))
                    .map(|id| format!("{id} ({})", got.iter().find(|(i, _)| i == id).map_or("not run", |(_, st)| st.as_str())))
                    .collect();
                ("checks_pass", not.is_empty(), if not.is_empty() { format!("{} pass", ids.join(", ")) } else { format!("not passing: {}", not.join(", ")) })
            }
        },
        Grader::Mentions { groups } => {
            let lacking: Vec<String> = groups.iter().filter(|g| !g.iter().any(|p| phrase_found(answer, p))).map(|g| g.join(" / ")).collect();
            (
                "mentions",
                lacking.is_empty(),
                if lacking.is_empty() { "named everything expected".to_string() } else { format!("never mentioned: {}", lacking.join("; ")) },
            )
        }
        Grader::Avoids { phrases } => {
            let found: Vec<&str> = phrases.iter().filter(|p| phrase_found(answer, p)).map(String::as_str).collect();
            ("avoids", found.is_empty(), if found.is_empty() { "none of the forbidden phrases".to_string() } else { format!("said: {}", found.join("; ")) })
        }
        Grader::TimesWithin { max_seconds, fps, min_count } => {
            let times = times_in(answer, *fps);
            let outside: Vec<String> = times.iter().filter(|t| **t > max_seconds + 1.0).map(|t| format!("{t:.1}s")).collect();
            let enough = times.len() as u32 >= *min_count;
            let detail = match (outside.is_empty(), enough) {
                (true, true) => format!("{} time(s) cited, all inside the {:.1}s edit", times.len(), max_seconds),
                (false, _) => format!("cited times past the end of the {:.1}s edit: {}", max_seconds, outside.join(", ")),
                (true, false) => format!("cited {} time(s), wanted at least {}", times.len(), min_count),
            };
            ("times_within", outside.is_empty() && enough, detail)
        }
        Grader::Budget { max_steps, max_tokens, max_cost_usd } => {
            let tokens =
                usage.input_tokens.saturating_add(usage.output_tokens).saturating_add(usage.cache_read_tokens).saturating_add(usage.cache_write_tokens);
            let mut over = Vec::new();
            if max_steps.is_some_and(|m| steps > m) {
                over.push(format!("{steps} steps"));
            }
            if max_tokens.is_some_and(|m| tokens > m) {
                over.push(format!("{tokens} tokens"));
            }
            if let (Some(m), Some(c)) = (max_cost_usd, cost)
                && c > *m
            {
                over.push(format!("${c:.4}"));
            }
            ("budget", over.is_empty(), if over.is_empty() { format!("{steps} steps, {tokens} tokens") } else { format!("over budget: {}", over.join(", ")) })
        }
        Grader::NumbersFromTools { min_numbers } => {
            let from_tools: Vec<f64> =
                events.iter().filter_map(|e| if let AgentEvent::ToolResult { content, .. } = e { Some(numbers_in(content)) } else { None }).flatten().collect();
            let stated: Vec<f64> = numbers_in(answer).into_iter().filter(|n| !(n.fract() == 0.0 && (0.0..=10.0).contains(n))).collect();
            let invented: Vec<String> = stated.iter().filter(|n| !supported(**n, &from_tools)).map(|n| format!("{n}")).collect();
            let enough = stated.len() as u32 >= *min_numbers;
            let detail = match (invented.is_empty(), enough) {
                (true, true) => format!("{} number(s) stated, every one in a tool result", stated.len()),
                (false, _) => format!("numbers that no tool returned: {}", invented.join(", ")),
                (true, false) => format!("stated {} number(s), wanted at least {min_numbers}", stated.len()),
            };
            ("numbers_from_tools", invented.is_empty() && enough, detail)
        }
        Grader::MustPassChecks => match cx.must_fail {
            Some(0) => ("must_pass_checks", true, "every must-pass check passes on what was built".to_string()),
            Some(n) => ("must_pass_checks", false, format!("{n} must-pass check{} fail", if n == 1 { "" } else { "s" })),
            None => ("must_pass_checks", false, "nothing was built to check, or the runner did not measure it".to_string()),
        },
        Grader::ProjectUnchanged => match cx.project_unchanged {
            Some(true) => ("project_unchanged", true, "the project is exactly as it was".to_string()),
            Some(false) => ("project_unchanged", false, "the project was changed".to_string()),
            None => ("project_unchanged", false, "the runner did not measure the project".to_string()),
        },
    };
    Grade { check: check.to_string(), passed, detail }
}

/// Grades a run. Always checked, besides the task's own graders: the turn finished normally, and no
/// tool that changes the project was called.
pub fn grade(task: &Task, trace: &Trace, cx: &GradeContext<'_>) -> TaskResult {
    let events = &trace.events;
    let answer = answer_text(events);
    let tools = tools_called(events);
    let usage = events.iter().rev().find_map(|e| if let AgentEvent::Usage { total, .. } = e { Some(*total) } else { None }).unwrap_or_else(|| {
        trace.replies.iter().fold(Usage::default(), |mut u, r| {
            u.add(&r.usage);
            u
        })
    });
    let steps = u32::try_from(trace.replies.len()).unwrap_or(u32::MAX);
    let cost = cx.price.map(|p| p.estimate(&usage));
    let finish = finish_of(events);

    let mut grades = vec![Grade {
        check: "finished".into(),
        passed: matches!(finish, Some(Finish::Done)),
        detail: match finish {
            Some(Finish::Done) => "the turn finished normally".to_string(),
            Some(other) => format!("the turn ended early: {}", serde_json::to_string(other).unwrap_or_default().chars().take(100).collect::<String>()),
            None => "the run never finished".to_string(),
        },
    }];
    // in Edit mode the tools that work inside a version are expected; anything else that changes the project is not
    const EDIT_MODE_TOOLS: [&str; 5] = ["begin_version", "edit", "undo_steps", "redo_step", "workflow_progress"];
    let editing: Vec<&str> = tools
        .iter()
        .filter(|t| (cx.read_only)(t) != Some(true) && !(task.mode == "edit" && EDIT_MODE_TOOLS.contains(&t.as_str())))
        .map(String::as_str)
        .collect();
    grades.push(Grade {
        check: "read_only".into(),
        passed: editing.is_empty(),
        detail: if editing.is_empty() { "no tool that changes the project was called".to_string() } else { format!("called: {}", editing.join(", ")) },
    });
    grades.extend(task.graders.iter().map(|g| grade_one(g, events, &answer, &usage, steps, cost, cx)));
    TaskResult {
        task: task.id.clone(),
        title: task.title.clone(),
        passed: grades.iter().all(|g| g.passed),
        grades,
        finish: finish.map_or("none".to_string(), |f| match f {
            Finish::Done => "done".into(),
            Finish::StepLimit => "step_limit".into(),
            Finish::BudgetReached => "budget".into(),
            Finish::Stopped => "stopped".into(),
            Finish::Refused => "refused".into(),
            Finish::CutOff => "cut_off".into(),
            Finish::Failed { .. } => "failed".into(),
        }),
        steps,
        usage,
        cost_usd: cost,
        seconds: cx.seconds,
        answer: answer.chars().take(MAX_ANSWER).collect(),
        tools,
    }
}

/// Which model and prompt a report is about.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunInfo {
    pub suite: String,
    pub suite_version: u32,
    pub service: String,
    pub model: String,
    pub prompt_version: String,
    pub started_unix: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub info: RunInfo,
    pub results: Vec<TaskResult>,
}

impl Report {
    pub fn passed(&self) -> usize {
        self.results.iter().filter(|r| r.passed).count()
    }

    pub fn tokens(&self) -> u64 {
        self.results.iter().map(|r| r.usage.input_tokens + r.usage.output_tokens + r.usage.cache_read_tokens + r.usage.cache_write_tokens).sum()
    }

    /// Estimated dollars, or `None` when the model's price is not known.
    pub fn cost(&self) -> Option<f64> {
        self.results.iter().map(|r| r.cost_usd).sum()
    }

    pub fn seconds(&self) -> f64 {
        self.results.iter().map(|r| r.seconds).sum()
    }

    pub fn label(&self) -> String {
        format!("{} · {} · prompt v{}", self.info.service, self.info.model, self.info.prompt_version)
    }

    pub fn to_markdown(&self) -> String {
        let mut s = format!(
            "# {} v{}: {}\n\n{} of {} tasks passed · {} tokens · {} · {:.0}s\n\n| task | result | steps | tokens | cost | what failed |\n|---|---|---|---|---|---|\n",
            self.info.suite,
            self.info.suite_version,
            self.label(),
            self.passed(),
            self.results.len(),
            self.tokens(),
            cost_text(self.cost()),
            self.seconds()
        );
        for r in &self.results {
            let failed: Vec<String> = r.grades.iter().filter(|g| !g.passed).map(|g| format!("{}: {}", g.check, g.detail)).collect();
            s.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                r.task,
                if r.passed { "pass" } else { "FAIL" },
                r.steps,
                r.usage.input_tokens + r.usage.output_tokens + r.usage.cache_read_tokens + r.usage.cache_write_tokens,
                cost_text(r.cost_usd),
                failed.join("; ").replace('|', "/")
            ));
        }
        s
    }
}

fn cost_text(c: Option<f64>) -> String {
    c.map_or("cost unknown".to_string(), |c| if c >= 0.01 { format!("about ${c:.2}") } else { format!("about ${c:.4}") })
}

/// A table of several runs of the same suite side by side, with the change from the first (the
/// baseline) to each of the others. Tasks are matched by id; a task a run lacks shows `-`.
pub fn compare(reports: &[Report]) -> String {
    let Some(base) = reports.first() else { return "nothing to compare\n".to_string() };
    let mut s = String::from("| run | passed | tokens | cost | seconds |\n|---|---|---|---|---|\n");
    for (i, r) in reports.iter().enumerate() {
        s.push_str(&format!(
            "| {}{} | {} of {} | {} | {} | {:.0} |\n",
            if i == 0 { "(baseline) " } else { "" },
            r.label(),
            r.passed(),
            r.results.len(),
            r.tokens(),
            cost_text(r.cost()),
            r.seconds()
        ));
    }
    let mut ids: Vec<&str> = Vec::new();
    for r in reports {
        for t in &r.results {
            if !ids.contains(&t.task.as_str()) {
                ids.push(&t.task);
            }
        }
    }
    s.push_str("\n| task |");
    for i in 0..reports.len() {
        s.push_str(&format!(" run {} |", i + 1));
    }
    s.push_str("\n|---|");
    for _ in reports {
        s.push_str("---|");
    }
    s.push('\n');
    for id in ids {
        s.push_str(&format!("| {id} |"));
        let was = base.results.iter().find(|t| t.task == id).map(|t| t.passed);
        for (i, r) in reports.iter().enumerate() {
            let now = r.results.iter().find(|t| t.task == id).map(|t| t.passed);
            let cell = match (now, was) {
                (None, _) => "-".to_string(),
                (Some(true), Some(false)) if i > 0 => "pass (fixed)".to_string(),
                (Some(false), Some(true)) if i > 0 => "FAIL (regressed)".to_string(),
                (Some(true), _) => "pass".to_string(),
                (Some(false), _) => "FAIL".to_string(),
            };
            s.push_str(&format!(" {cell} |"));
        }
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::TraceMeta;
    use crate::{Profile, TurnReport};
    use edith_llm::{Message, Response, Role, StopReason};
    use serde_json::json;

    fn meta() -> TraceMeta {
        TraceMeta { prompt_version: "2".into(), model: "m".into(), profile: Profile::ReadOnly, started_unix: 0 }
    }

    /// A finished run: the tools called (name, input), the text said, and how it ended.
    fn run(tools: &[&str], text: &str, finish: Finish) -> Trace {
        let mut t = Trace::new(meta());
        t.record(&AgentEvent::UserMessage { text: "q".into(), attached: vec![] });
        t.record(&AgentEvent::StepStarted { step: 1 });
        for (i, name) in tools.iter().enumerate() {
            t.record(&AgentEvent::ToolCall { id: format!("t{i}"), name: (*name).into(), input: json!({}) });
        }
        t.record(&AgentEvent::Text { text: text.into() });
        t.record(&AgentEvent::Usage {
            reply: Usage { input_tokens: 1000, output_tokens: 100, ..Default::default() },
            total: Usage { input_tokens: 1000, output_tokens: 100, ..Default::default() },
        });
        t.record(&AgentEvent::Finished { finish });
        t.finish(&TurnReport {
            finish: Finish::Done,
            steps: 1,
            replies: vec![Response {
                message: Message { role: Role::Assistant, parts: vec![] },
                stop_reason: StopReason::EndTurn,
                usage: Usage { input_tokens: 1000, output_tokens: 100, ..Default::default() },
                model: "m".into(),
            }],
        });
        t
    }

    fn cx(unchanged: Option<bool>) -> GradeContext<'static> {
        GradeContext { read_only: &|n| Some(n != "delete_clip"), price: None, project_unchanged: unchanged, must_fail: None, checks: None, seconds: 1.5 }
    }

    fn task(graders: Vec<Grader>) -> Task {
        Task { id: "t".into(), title: "T".into(), question: "q".into(), fixture: "demo".into(), context: vec![], graders, tags: vec![], mode: String::new() }
    }

    #[test]
    fn the_builtin_suite_reads_and_every_task_is_well_formed() {
        let s = Suite::builtin();
        assert!(s.tasks.len() >= 8 && s.version >= 1, "{} tasks", s.tasks.len());
        assert_eq!(Suite::parse(BUILTIN).map(|s| s.tasks.len()), Ok(s.tasks.len()), "the shipped file parses");
        for t in &s.tasks {
            assert!(!t.graders.is_empty() && t.question.ends_with(['?', '.']), "{}", t.id);
        }
    }

    #[test]
    fn a_bad_suite_is_refused_with_a_reason() {
        let one = |extra: &str| {
            format!(
                r#"{{"name":"s","version":1,"tasks":[{{"id":"a","title":"A","question":"q","fixture":"demo","graders":[{{"check":"project_unchanged"}}]{extra}}}]}}"#
            )
        };
        assert!(Suite::parse(&one("")).is_ok());
        assert!(Suite::parse("nope").is_err());
        assert!(Suite::parse(&one("").replace("\"id\":\"a\"", "\"id\":\"has space\"")).is_err());
        assert!(Suite::parse(&one("").replace("\"question\":\"q\"", "\"question\":\"  \"")).is_err());
        assert!(Suite::parse(&one("").replace("[{\"check\":\"project_unchanged\"}]", "[]")).is_err());
        let twice = r#"{"name":"s","version":1,"tasks":[
            {"id":"a","title":"A","question":"q","fixture":"demo","graders":[{"check":"project_unchanged"}]},
            {"id":"a","title":"B","question":"q","fixture":"demo","graders":[{"check":"project_unchanged"}]}]}"#;
        assert!(Suite::parse(twice).is_err());
    }

    #[test]
    fn times_are_read_from_timecodes_and_seconds_by_their_most_generous_reading() {
        let near = |a: Vec<f64>, b: Vec<f64>| a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 0.05);
        assert!(near(times_in("the cut at 00:00:09:11 and 00:00:26:18", 24.0), vec![9.0 + 11.0 / 24.0, 26.75]));
        assert!(near(times_in("around 9.5 seconds, again at 12 s, and 3 sec", 24.0), vec![9.5, 12.0, 3.0]));
        assert!(near(times_in("0:42 is abrupt", 24.0), vec![42.0_f64.min(0.0 + 42.0 / 24.0)]), "MM:SS or SS:FF, whichever is smaller");
        assert!(times_in("clip #29 is 1920x1080 at 23.976fps with 6 tracks, 14 clips", 24.0).is_empty(), "plain numbers are not times");
        assert!(times_in("", 24.0).is_empty() && times_in("::: ... 1:", 24.0).is_empty());
        assert!(times_in("99999999999999999999:99", 24.0).len() <= 1, "huge numbers do not crash");
    }

    #[test]
    fn a_good_answer_passes_every_check_and_the_report_says_what_it_cost() {
        let t = task(vec![
            Grader::CallsTools { all: vec!["sequence_view".into()], none: vec![] },
            Grader::Mentions { groups: vec![vec!["Neon_Loop".into()], vec!["5".into(), "five".into()]] },
            Grader::TimesWithin { max_seconds: 26.75, fps: 24.0, min_count: 1 },
            Grader::Budget { max_steps: Some(3), max_tokens: Some(5000), max_cost_usd: Some(0.01) },
            Grader::Avoids { phrases: vec!["I cannot".into()] },
        ]);
        let trace = run(&["sequence_view"], "The longest is neon loop (00:00:21:07 to 00:00:26:18), about 5 seconds.", Finish::Done);
        let price = Price { input_per_mtok: 0.1, output_per_mtok: 0.5, cache_read_per_mtok: None, source: "t".into() };
        let r = grade(&t, &trace, &GradeContext { price: Some(&price), ..cx(None) });
        assert!(r.passed, "{:#?}", r.grades);
        assert_eq!((r.steps, r.finish.as_str(), r.tools.clone()), (1, "done", vec!["sequence_view".to_string()]));
        assert!((r.cost_usd.unwrap() - 0.00015).abs() < 1e-9);
        assert_eq!(r.grades.len(), 7, "two always-on checks and five of the task's own");
    }

    /// A run that called these tools in this order.
    fn run_in_order(tools: &[&str]) -> Trace {
        run(tools, "done", Finish::Done)
    }

    #[test]
    fn looking_has_to_come_before_offering_and_both_have_to_happen() {
        let g = Grader::CallsInOrder { first: "media_look".into(), then: "request_download".into() };
        let t = task(vec![g]);
        let detail = |tools: &[&str]| {
            let r = grade(&t, &run_in_order(tools), &cx(None));
            let c = r.grades.iter().find(|g| g.check == "calls_in_order").unwrap().clone();
            (c.passed, c.detail)
        };
        assert_eq!(detail(&["media_search", "media_look", "request_download"]), (true, "media_look came before request_download".to_string()));
        assert_eq!(detail(&["media_search", "request_download", "media_look"]), (false, "request_download was called before media_look".to_string()));
        assert_eq!(detail(&["media_search", "request_download"]), (false, "request_download was called without media_look first".to_string()));
        assert_eq!(detail(&["media_look"]), (false, "request_download was never called".to_string()));
        assert_eq!(detail(&[]), (false, "neither media_look nor request_download was called".to_string()));
    }

    #[test]
    fn checks_that_must_pass_are_read_from_what_was_built_and_nothing_built_never_passes() {
        let t = task(vec![Grader::ChecksPass { ids: vec!["musicDucked".into(), "soundUnderIdeas".into()] }]);
        let got = |c: Option<&[(String, String)]>| {
            let r = grade(&t, &run(&[], "x", Finish::Done), &GradeContext { checks: c, ..cx(None) });
            let g = r.grades.iter().find(|g| g.check == "checks_pass").unwrap().clone();
            (g.passed, g.detail)
        };
        let pass = vec![
            ("musicDucked".to_string(), "pass".to_string()),
            ("soundUnderIdeas".to_string(), "pass".to_string()),
            ("pace".to_string(), "note".to_string()),
        ];
        assert_eq!(got(Some(&pass)), (true, "musicDucked, soundUnderIdeas pass".to_string()));
        let note = vec![("musicDucked".to_string(), "note".to_string()), ("soundUnderIdeas".to_string(), "pass".to_string())];
        assert_eq!(got(Some(&note)), (false, "not passing: musicDucked (note)".to_string()));
        // `na` is not a pass: nothing there to be ducked is not the same as ducked
        let na = vec![("musicDucked".to_string(), "na".to_string())];
        assert_eq!(got(Some(&na)), (false, "not passing: musicDucked (na), soundUnderIdeas (not run)".to_string()));
        assert!(!got(None).0);
    }

    #[test]
    fn the_whole_video_suite_ships_and_every_task_names_a_fixture_and_graders() {
        let suite = Suite::named("whole-video").expect("it ships");
        assert_eq!(suite.name, "whole-video");
        assert_eq!(suite.tasks.len(), 5);
        assert!(suite.tasks.iter().all(|t| !t.graders.is_empty() && !t.question.is_empty()));
        assert!(Suite::named("nope").is_none());
    }

    #[test]
    fn a_bad_answer_fails_the_checks_it_breaks_and_says_why() {
        let t = task(vec![
            Grader::CallsTools { all: vec!["sequence_view".into()], none: vec![] },
            Grader::Mentions { groups: vec![vec!["Neon_Loop".into()]] },
            Grader::TimesWithin { max_seconds: 26.75, fps: 24.0, min_count: 0 },
        ]);
        // answers from memory: no tool, a wrong clip, a time past the end of the edit
        let trace = run(&[], "I think it is Desert_Dunes, around 00:01:30:00.", Finish::Done);
        let r = grade(&t, &trace, &cx(None));
        assert!(!r.passed);
        let failed: Vec<&str> = r.grades.iter().filter(|g| !g.passed).map(|g| g.check.as_str()).collect();
        assert_eq!(failed, ["calls_tools", "mentions", "times_within"]);
        assert!(r.grades.iter().any(|g| g.detail.contains("missing: sequence_view")) && r.grades.iter().any(|g| g.detail.contains("past the end")));
    }

    #[test]
    fn editing_and_unfinished_runs_always_fail() {
        let t = task(vec![Grader::ProjectUnchanged]);
        let r = grade(&t, &run(&["delete_clip"], "done", Finish::Done), &cx(Some(false)));
        let failed: Vec<&str> = r.grades.iter().filter(|g| !g.passed).map(|g| g.check.as_str()).collect();
        assert_eq!(failed, ["read_only", "project_unchanged"]);
        let r = grade(&t, &run(&[], "x", Finish::StepLimit), &cx(Some(true)));
        assert_eq!(r.grades.iter().filter(|g| !g.passed).map(|g| g.check.as_str()).collect::<Vec<_>>(), ["finished"]);
        assert_eq!(r.finish, "step_limit");
        // an unmeasured project cannot pass the check; an empty trace never finished
        assert!(!grade(&t, &run(&[], "x", Finish::Done), &cx(None)).passed);
        let empty = grade(&t, &Trace::new(meta()), &cx(Some(true)));
        assert!(!empty.passed && empty.finish == "none" && empty.steps == 0);
    }

    #[test]
    fn phrases_ignore_case_and_underscores_but_numbers_match_whole() {
        assert!(phrase_found("Uses a Dip To Black", "dip_to_black") && phrase_found("the DIP-TO-BLACK", "dip to black"));
        assert!(phrase_found("There are 6 clips", "6") && !phrase_found("length 00:00:26:18", "6") && !phrase_found("16 clips", "6"));
        assert!(!phrase_found("anything", "") && !phrase_found("anything", "   "));
    }

    #[test]
    fn text_from_a_step_that_was_asked_again_is_not_the_answer() {
        let ev = vec![
            AgentEvent::StepStarted { step: 1 },
            AgentEvent::Text { text: "draft that was thrown away".into() },
            AgentEvent::StepRestarted,
            AgentEvent::Text { text: "the real answer".into() },
        ];
        assert_eq!(answer_text(&ev), "the real answer");
    }

    fn report(model: &str, passes: &[(&str, bool)]) -> Report {
        Report {
            info: RunInfo { suite: "core".into(), suite_version: 1, service: "s".into(), model: model.into(), prompt_version: "2".into(), started_unix: 0 },
            results: passes
                .iter()
                .map(|(id, ok)| {
                    let mut r = grade(
                        &task(vec![Grader::Avoids { phrases: vec![if *ok { "zzz".into() } else { "hello".into() }] }]),
                        &run(&[], "hello", Finish::Done),
                        &cx(None),
                    );
                    r.task = (*id).into();
                    r
                })
                .collect(),
        }
    }

    #[test]
    fn reports_render_and_compare_runs_showing_what_got_better_or_worse() {
        let a = report("haiku", &[("one", true), ("two", false), ("three", true)]);
        let b = report("sonnet", &[("one", true), ("two", true), ("three", false)]);
        let md = a.to_markdown();
        assert!(md.contains("2 of 3 tasks passed") && md.contains("| two | FAIL |") && md.contains("avoids: said: hello"), "{md}");
        let c = compare(&[a.clone(), b]);
        assert!(c.contains("(baseline)") && c.contains("pass (fixed)") && c.contains("FAIL (regressed)"), "{c}");
        assert!(compare(&[]).contains("nothing"));
        // a report survives a trip through JSON
        let back: Report = serde_json::from_str(&serde_json::to_string(&a).unwrap_or_default()).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn numbers_are_read_out_of_prose_and_tool_text_alike() {
        assert_eq!(numbers_in("A 0.8 s shot at 2:41, 1,840 words, 89% of it, clip #29 on V1, 140 words a minute."), [0.8, 2.0, 41.0, 1840.0, 89.0, 140.0]);
        assert_eq!(numbers_in("h264 and 4K and A1 and 3rd"), Vec::<f64>::new(), "numbers glued to letters are names");
        assert_eq!(numbers_in("v1.2 is 7.5. Done."), [7.5], "a full stop after a number is not a decimal point");
        assert!(numbers_in("").is_empty() && numbers_in("no numbers here").is_empty());
        assert_eq!(numbers_in("9999999999999999999999"), [1e22]);
    }

    #[test]
    fn the_numbers_check_passes_what_the_tools_said_and_fails_what_they_did_not() {
        let mk = |tool: &str, answer: &str, min: u32| {
            let events = vec![
                AgentEvent::ToolResult { id: "t".into(), name: "coach_findings".into(), content: tool.into(), is_error: false, pictures: 0, view: None },
                AgentEvent::Text { text: answer.into() },
            ];
            let cx = GradeContext { read_only: &|_| Some(true), price: None, project_unchanged: Some(true), must_fail: None, checks: None, seconds: 1.0 };
            let usage = Usage::default();
            grade_one(&Grader::NumbersFromTools { min_numbers: min }, &events, answer, &usage, 1, None, &cx)
        };
        let tool = "A 0.8 s shot between two longer ones (6 s and 6 s) at 0:06; narration 168 words a minute, target 140; 142 shots.";
        assert!(mk(tool, "The 0.8 s shot at 0:06 sits between 6 s shots; narration runs at 168 words a minute against 140.", 3).passed);
        // rounding is allowed: 5.2 said as 5, 168.4 as 168
        assert!(mk("mean 5.2 s, 168.4 words", "The mean is about 5 s and 168 words a minute.", 1).passed);
        // an invented number fails and is named
        let bad = mk(tool, "You have 18 cuts a minute and a 0.8 s shot.", 0);
        assert!(!bad.passed && bad.detail.contains("18"), "{}", bad.detail);
        // small whole numbers are free
        assert!(mk(tool, "Try this in 2 steps: 1. lengthen it 2. check.", 0).passed);
        // saying no numbers cannot pass a task that wants some
        let none = mk(tool, "It is a bit fast.", 2);
        assert!(!none.passed && none.detail.contains("wanted at least 2"), "{}", none.detail);
        // a number only in the answer's own units of a different measure does not slip through
        assert!(!mk("mean 5.2 s", "It is 52 s long.", 0).passed);
    }
}
