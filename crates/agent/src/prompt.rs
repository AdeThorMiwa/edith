//! Prompt packs: the versioned text that makes Edith Edith. Plain files, so a change is a diff
//! that can be measured (the evaluation suite, F03.6), not a code edit buried in a function.

use crate::Profile;
use edith_llm::SystemPart;

const BUILTIN: &str = include_str!("../prompts/edith.md");

/// A parsed prompt pack: a version and named sections, in file order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptPack {
    pub version: String,
    pub sections: Vec<(String, String)>,
}

impl PromptPack {
    /// The pack shipped with the app.
    pub fn builtin() -> Self {
        Self::parse(BUILTIN)
    }

    /// Reads `# version: N` and `## name` sections. Text before the first section is ignored.
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
        for (_, body) in &mut sections {
            *body = body.trim().to_string();
        }
        sections.retain(|(_, b)| !b.is_empty());
        Self { version, sections }
    }

    /// The system prompt as parts, in a fixed order so the cached prefix stays cached: every
    /// section, then the profile note. The last part is marked for caching. Anything that changes
    /// from turn to turn (selection, transcript) goes in the user message instead.
    pub fn system_parts(&self, profile: Profile) -> Vec<SystemPart> {
        let mut text = String::new();
        for (name, body) in &self.sections {
            text.push_str(&format!("## {name}\n{body}\n\n"));
        }
        text.push_str(profile.note());
        vec![SystemPart { text, cache: true }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_pack_has_a_version_and_the_core_sections() {
        let p = PromptPack::builtin();
        assert_eq!(p.version, "1");
        let names: Vec<_> = p.sections.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["identity", "honesty", "working", "tool results"]);
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
    fn parsing_tolerates_odd_files() {
        let p = PromptPack::parse("preamble\n# version: 7\n## a\nx\n\n## empty\n\n## b\n  y  \n");
        assert_eq!(p.version, "7");
        assert_eq!(p.sections, vec![("a".to_string(), "x".to_string()), ("b".to_string(), "y".to_string())]);
        let p = PromptPack::parse("");
        assert!(p.sections.is_empty() && p.version == "0");
    }
}
