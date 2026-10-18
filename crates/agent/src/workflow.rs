//! Workflows (fork roadmap F16.4): guidance files that tell the assistant which stages a job goes through, when to
//! ask the person, and what "done" means.
//!
//! A workflow is a Markdown file in the shape of a skill:
//!
//! ```text
//! ---
//! name: rough-cut-from-narration
//! description: One line the assistant reads to decide whether this workflow fits.
//! ---
//! Free guidance first (who decides what, the rules that hold throughout).
//!
//! ## Stage 1: understand
//! What to do, when to ask the person, what "done" is for this stage.
//!
//! ## Stage 2: …
//! ```
//!
//! Only the name and the description are offered up front (`workflow_list`), so the conversation stays small;
//! the whole text enters it only when the assistant loads one. Stages are the `## Stage <n>: <name>` headings,
//! numbered from 1 with no gaps; any other `##` heading is guidance that belongs to no stage.
//!
//! The shipped workflows are built in. A person's own files live in a `workflows` folder beside the preferences;
//! the loader in the engine skips a file that does not parse and says which and why. Nothing here ever fails
//! loudly on input: a file is text from outside, and is cut to size, not trusted.

/// Largest workflow file read, in bytes.
pub const MAX_BYTES: usize = 32 * 1024;
/// Most stages in one workflow.
pub const MAX_STAGES: usize = 12;
/// Longest name and description.
pub const MAX_NAME: usize = 40;
pub const MAX_DESCRIPTION: usize = 400;

/// One stage: its number from 1, its name, and its guidance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stage {
    pub number: usize,
    pub name: String,
    pub text: String,
}

/// A parsed workflow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workflow {
    pub name: String,
    pub description: String,
    pub stages: Vec<Stage>,
    /// Everything after the front matter, as written.
    pub body: String,
}

impl Workflow {
    /// The stage `key` names: its number (`"3"`) or its name (`"find the ideas"`), in any case.
    pub fn stage(&self, key: &str) -> Option<&Stage> {
        let k = key.trim().to_lowercase();
        if let Ok(n) = k.parse::<usize>() {
            return self.stages.iter().find(|s| s.number == n);
        }
        self.stages.iter().find(|s| s.name.to_lowercase() == k)
    }
}

fn clean_line(s: &str) -> String {
    s.chars().filter(|c| !c.is_control() || *c == '\t').collect::<String>().trim().to_string()
}

fn valid_name(n: &str) -> bool {
    let mut chars = n.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && n.len() <= MAX_NAME
        && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `## Stage 3: find the footage` as (3, "find the footage").
fn stage_heading(line: &str) -> Option<(usize, String)> {
    let rest = line.strip_prefix("## ")?.trim();
    let rest = rest.get(..5).filter(|p| p.eq_ignore_ascii_case("stage")).map(|_| rest[5..].trim_start())?;
    let (num, name) = rest.split_once(':')?;
    let number: usize = num.trim().parse().ok()?;
    let name = clean_line(name);
    (!name.is_empty()).then_some((number, name))
}

/// Reads a workflow file. An error is a sentence about what is wrong, for the person who wrote the file.
pub fn parse(text: &str) -> Result<Workflow, String> {
    if text.len() > MAX_BYTES {
        return Err(format!("the file is longer than {} KB", MAX_BYTES / 1024));
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Err("it must begin with a line of three dashes, then `name:` and `description:`, then three dashes".into());
    }
    let (mut name, mut description) = (None, None);
    let mut closed = false;
    for line in lines.by_ref() {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            match k.trim() {
                "name" => name = Some(clean_line(v)),
                "description" => description = Some(clean_line(v)),
                _ => {}
            }
        }
    }
    if !closed {
        return Err("the front matter is never closed with a second line of three dashes".into());
    }
    let name = name.filter(|n| !n.is_empty()).ok_or("it has no `name:`")?;
    if !valid_name(&name) {
        return Err(format!("the name “{name}” must be lowercase letters, digits and dashes, up to {MAX_NAME} characters"));
    }
    let description = description.filter(|d| !d.is_empty()).ok_or("it has no `description:` (one line saying when to use it)")?;
    if description.chars().count() > MAX_DESCRIPTION {
        return Err(format!("the description is longer than {MAX_DESCRIPTION} characters; keep it to a line or two"));
    }
    let body: String = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    let mut stages: Vec<Stage> = Vec::new();
    for line in body.lines() {
        if let Some((number, stage)) = stage_heading(line) {
            if number != stages.len() + 1 {
                return Err(format!(
                    "stages must be numbered 1, 2, 3 in order, but “{line}” comes after {} stage{}",
                    stages.len(),
                    if stages.len() == 1 { "" } else { "s" }
                ));
            }
            if stages.len() >= MAX_STAGES {
                return Err(format!("more than {MAX_STAGES} stages"));
            }
            stages.push(Stage { number, name: stage, text: String::new() });
        } else if line.starts_with("## ") {
            // another section: guidance that belongs to no stage ends the stage before it
            if let Some(last) = stages.last_mut() {
                last.text.push('\u{0}');
            }
        } else if let Some(last) = stages.last_mut()
            && !last.text.ends_with('\u{0}')
        {
            last.text.push_str(line);
            last.text.push('\n');
        }
    }
    for st in &mut stages {
        st.text = st.text.trim_end_matches('\u{0}').trim().to_string();
    }
    if stages.is_empty() {
        return Err("it has no stages: headings like `## Stage 1: understand`".into());
    }
    Ok(Workflow { name, description, stages, body })
}

/// The workflows that ship with Edith.
pub fn builtin() -> Vec<Workflow> {
    [include_str!("../workflows/rough-cut-from-narration.md")].into_iter().filter_map(|t| parse(t).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "---\nname: demo-flow\ndescription: A demo.\n---\nIntro line.\n\n## Stage 1: look\nLook at it.\n\n## Stage 2: do it\nDo it.\nTwice.\n\n## When to ask\nOnly about taste.\n";

    #[test]
    fn a_workflow_has_a_name_a_description_and_numbered_stages_with_their_own_text() {
        let w = parse(GOOD).unwrap();
        assert_eq!((w.name.as_str(), w.description.as_str()), ("demo-flow", "A demo."));
        assert_eq!(w.stages.iter().map(|s| (s.number, s.name.as_str())).collect::<Vec<_>>(), [(1, "look"), (2, "do it")]);
        assert_eq!(w.stages[1].text, "Do it.\nTwice.", "a stage ends at the next heading, and guidance after it is not part of it");
        assert!(w.body.contains("Intro line.") && w.body.contains("## When to ask"));
        assert_eq!(w.stage("2").map(|s| &s.name[..]), Some("do it"));
        assert_eq!(w.stage("  LOOK ").map(|s| s.number), Some(1));
        assert!(w.stage("3").is_none() && w.stage("nothing").is_none() && w.stage("").is_none() && w.stage("-1").is_none());
    }

    #[test]
    fn the_shipped_workflow_parses_and_has_its_stages_in_order() {
        let all = builtin();
        assert!(!all.is_empty(), "the built-in workflows must parse");
        let w = all.iter().find(|w| w.name == "rough-cut-from-narration").expect("the rough cut");
        assert!(w.stages.len() >= 5, "{:?}", w.stages.iter().map(|s| &s.name).collect::<Vec<_>>());
        assert!(w.stages.iter().all(|s| !s.text.is_empty()));
        assert!(w.description.len() < MAX_DESCRIPTION);
    }

    #[test]
    fn a_file_that_does_not_fit_is_refused_with_a_sentence_about_why() {
        for (text, want) in [
            ("", "begin with a line of three dashes"),
            ("no front matter\n## Stage 1: a\nx", "begin with a line of three dashes"),
            ("---\nname: x\ndescription: d\n## Stage 1: a\nx", "never closed"),
            ("---\ndescription: d\n---\n## Stage 1: a\nx", "no `name:`"),
            ("---\nname: Bad Name\ndescription: d\n---\n## Stage 1: a\nx", "lowercase letters"),
            ("---\nname: ok\n---\n## Stage 1: a\nx", "no `description:`"),
            ("---\nname: ok\ndescription: d\n---\njust words", "no stages"),
            ("---\nname: ok\ndescription: d\n---\n## Stage 2: a\nx", "numbered 1, 2, 3"),
            ("---\nname: ok\ndescription: d\n---\n## Stage 1: a\nx\n## Stage 3: c\ny", "numbered 1, 2, 3"),
            ("---\nname: ok\ndescription: d\n---\n## Stage 1:\nx", "no stages"),
        ] {
            let e = parse(text).expect_err(text);
            assert!(e.contains(want), "{want:?} not in {e:?}");
        }
        let long = format!("---\nname: ok\ndescription: {}\n---\n## Stage 1: a\nx", "d".repeat(401));
        assert!(parse(&long).unwrap_err().contains("longer than 400"));
        assert!(parse(&"x".repeat(MAX_BYTES + 1)).unwrap_err().contains("longer than 32 KB"));
        let many: String = (1..=13).map(|n| format!("## Stage {n}: s{n}\nx\n")).collect();
        assert!(parse(&format!("---\nname: ok\ndescription: d\n---\n{many}")).unwrap_err().contains("more than 12"));
    }

    #[test]
    fn odd_text_is_tidied_or_refused_and_never_panics() {
        let w = parse("\u{feff}---\r\nname: ok\r\ndescription: a\u{7}b\r\n---\r\n## Stage 1: look\u{0}here\r\nbody\r\n").unwrap();
        assert_eq!(w.description, "ab");
        assert_eq!(w.stages[0].name, "lookhere");
        for t in ["---", "---\n---", "---\nname\n---", "---\nname: \n---", "\u{0}\u{1}", "---\nname: é\ndescription: d\n---\n## Stage 1: x", "## Stage 1: x"] {
            let _ = parse(t);
        }
        assert!(parse("---\nname: ok\ndescription: d\n---\n## stage 1: lower case heading\nx").is_ok(), "the word stage is not case sensitive");
    }
}
