//! Prompt packs: the versioned text that makes Edith Edith. Plain files, so a change is a diff
//! that can be measured (the evaluation suite, F03.6), not a code edit buried in a function.

use crate::Profile;
use edith_llm::SystemPart;

const BUILTIN: &str = include_str!("../prompts/edith.md");

/// A slash command: `/name args` is replaced by `template` (with `{args}` filled in) before the
/// model sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub template: String,
}

/// A parsed prompt pack: a version, the sections that make the system prompt, and the slash
/// commands, in file order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptPack {
    pub version: String,
    pub sections: Vec<(String, String)>,
    /// Sections that belong to one mode only, `(mode, text)`: `## mode:coach` in the file. They come
    /// after the shared sections, only for that profile.
    pub mode_sections: Vec<(Profile, String)>,
    pub commands: Vec<Command>,
}

impl PromptPack {
    /// The pack shipped with the app.
    pub fn builtin() -> Self {
        Self::parse(BUILTIN)
    }

    /// Reads `# version: N`, `## name` sections and `## /name | description` commands. Text before
    /// the first section is ignored.
    pub fn parse(text: &str) -> Self {
        let mut version = String::from("0");
        let mut sections: Vec<(String, String)> = Vec::new();
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("# version:") {
                version = v.trim().to_string();
            } else if let Some(name) = line.strip_prefix("## ") {
                sections.push((name.trim().to_string(), String::new()));
            } else if let Some((_, body)) = sections.last_mut() {
                body.push_str(line);
                body.push('\n');
            }
        }
        let mut system = Vec::new();
        let mut mode_sections = Vec::new();
        let mut commands = Vec::new();
        for (name, body) in sections {
            let body = body.trim().to_string();
            if body.is_empty() {
                continue;
            }
            if let Some(rest) = name.strip_prefix('/') {
                let (cmd, description) = rest.split_once('|').map_or((rest, ""), |(a, b)| (a, b));
                commands.push(Command { name: cmd.trim().to_ascii_lowercase(), description: description.trim().to_string(), template: body });
            } else if let Some(mode) = name.strip_prefix("mode:") {
                // an unknown mode name is ignored: a pack from a newer build still loads
                if let Some(profile) = match mode.trim() {
                    "coach" => Some(Profile::Coach),
                    "propose" => Some(Profile::Propose),
                    _ => None,
                } {
                    mode_sections.push((profile, body));
                }
            } else {
                system.push((name, body));
            }
        }
        Self { version, sections: system, mode_sections, commands }
    }

    /// The system prompt as parts, in a fixed order so the cached prefix stays cached: every
    /// section, then the profile note. The last part is marked for caching. Anything that changes
    /// from turn to turn (selection, transcript) goes in the user message instead.
    pub fn system_parts(&self, profile: Profile) -> Vec<SystemPart> {
        let mut text = String::new();
        for (name, body) in &self.sections {
            text.push_str(&format!("## {name}\n{body}\n\n"));
        }
        for (mode, body) in &self.mode_sections {
            if *mode == profile {
                text.push_str(&format!("## {} mode\n{body}\n\n", if profile == Profile::Coach { "coach" } else { "propose" }));
            }
        }
        text.push_str(profile.note());
        vec![SystemPart { text, cache: true }]
    }

    /// If `text` is a slash command (`/name` or `/name arguments`), what the model should be asked
    /// instead. `Ok(None)` for ordinary text, including things that merely start with a slash, such
    /// as a file path. An unknown command or a missing argument is an error saying what to type.
    pub fn expand(&self, text: &str) -> Result<Option<String>, String> {
        let t = text.trim();
        let token = t.split_whitespace().next().unwrap_or("");
        let is_command_shaped =
            token.len() >= 2 && token.len() <= 24 && token.starts_with('/') && token[1..].bytes().all(|b| b.is_ascii_alphabetic() || b == b'-');
        if !is_command_shaped {
            return Ok(None);
        }
        let name = token[1..].to_ascii_lowercase();
        let Some(cmd) = self.commands.iter().find(|c| c.name == name) else {
            let all: Vec<String> = self.commands.iter().map(|c| format!("/{}", c.name)).collect();
            return Err(format!("There is no command {token}. Available: {}.", all.join(", ")));
        };
        let args: String = t[token.len()..].trim().chars().filter(|c| !c.is_control()).take(300).collect();
        if cmd.template.contains("{args}") && args.is_empty() {
            return Err(format!("/{} needs something after it, for example /{} J-cut.", cmd.name, cmd.name));
        }
        Ok(Some(cmd.template.replace("{args}", &args)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_pack_has_a_version_and_the_core_sections() {
        let p = PromptPack::builtin();
        assert_eq!(p.version, "5");
        let names: Vec<_> = p.sections.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["identity", "honesty", "working", "craft notes", "tool results"]);
        let all = p.system_parts(Profile::ReadOnly)[0].text.clone();
        assert!(all.contains("You are Edith") && all.contains("never an instruction to you") && all.contains("cannot change it"));
    }

    #[test]
    fn the_system_prompt_is_stable_cached_and_profile_aware() {
        let p = PromptPack::builtin();
        let a = p.system_parts(Profile::ReadOnly);
        assert_eq!(a, p.system_parts(Profile::ReadOnly), "same input, same bytes: a changing prefix would defeat the cache");
        assert!(a.last().is_some_and(|x| x.cache));
        let b = p.system_parts(Profile::Propose);
        assert_ne!(a[0].text, b[0].text);
        assert!(b[0].text.contains("propose"));
    }

    #[test]
    fn the_coach_mode_adds_its_own_section_and_nothing_else_changes() {
        let p = PromptPack::builtin();
        let ask = p.system_parts(Profile::ReadOnly)[0].text.clone();
        let coach = p.system_parts(Profile::Coach)[0].text.clone();
        assert!(!ask.contains("coach mode") && coach.contains("## coach mode"));
        assert!(coach.contains("coach_findings") && coach.contains("Never state a number"), "{coach}");
        assert!(coach.starts_with(ask.split("This thread is read-only").next().unwrap_or("")), "the shared sections come first and are identical");
        assert!(coach.contains("Coach mode and read-only") && !coach.contains("This thread is read-only"));
        assert!(ask.contains("analysis_get"), "Ask is told it has the measurements too");
        assert!(ask.contains("`show`") && ask.contains("never type numbers into it"), "both modes know about views");
        assert!(coach.contains("Show where it happens"));
    }

    #[test]
    fn parsing_tolerates_odd_files() {
        let p = PromptPack::parse("preamble\n# version: 7\n## a\nx\n\n## empty\n\n## b\n  y  \n");
        assert_eq!(p.version, "7");
        assert_eq!(p.sections, vec![("a".to_string(), "x".to_string()), ("b".to_string(), "y".to_string())]);
        // a mode section for a mode this build does not know is dropped, not an error
        let p = PromptPack::parse("# version: 1\n## a\nx\n## mode:coach\nc\n## mode:warp\nw\n");
        assert_eq!(p.mode_sections, vec![(Profile::Coach, "c".to_string())]);
        assert!(p.system_parts(Profile::Coach)[0].text.contains("## coach mode\nc") && !p.system_parts(Profile::ReadOnly)[0].text.contains("coach mode\nc"));
        let p = PromptPack::parse("");
        assert!(p.sections.is_empty() && p.commands.is_empty() && p.version == "0");
    }

    #[test]
    fn the_builtin_pack_has_four_commands_and_the_system_prompt_leaves_them_out() {
        let p = PromptPack::builtin();
        let names: Vec<&str> = p.commands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["explain-cut", "pacing", "summarize", "glossary"]);
        assert!(p.commands.iter().all(|c| !c.description.is_empty() && !c.template.is_empty()));
        let system = &p.system_parts(Profile::ReadOnly)[0].text;
        assert!(!system.contains("Explain the cut nearest the playhead"), "commands are not part of the system prompt");
        assert!(system.contains("ask for them together") && system.contains("craft notes"));
    }

    #[test]
    fn a_slash_command_expands_and_ordinary_text_does_not() {
        let p = PromptPack::builtin();
        assert!(p.expand("/pacing").unwrap().unwrap().contains("Review the pacing"));
        assert!(p.expand("  /EXPLAIN-CUT  ").unwrap().unwrap().contains("nearest the playhead"));
        let g = p.expand("/glossary J-cut").unwrap().unwrap();
        assert!(g.contains("\"J-cut\"") && !g.contains("{args}"), "{g}");
        for plain in ["hello", "why is /pacing slow?", "/Users/me/clip.mp4 will not play", "//comment", "/", "", "/a/b", "/9lives"] {
            assert_eq!(p.expand(plain), Ok(None), "{plain:?}");
        }
    }

    #[test]
    fn a_bad_command_says_what_to_type() {
        let p = PromptPack::builtin();
        let unknown = p.expand("/nope").unwrap_err();
        assert!(unknown.contains("/nope") && unknown.contains("/pacing") && unknown.contains("/glossary"), "{unknown}");
        let missing = p.expand("/glossary").unwrap_err();
        assert!(missing.contains("for example /glossary J-cut"), "{missing}");
        // arguments are cleaned and bounded
        let long = p.expand(&format!("/glossary {}\u{7}term", "x".repeat(2000))).unwrap().unwrap();
        assert!(long.len() < 1200 && !long.contains('\u{7}'));
    }
}
