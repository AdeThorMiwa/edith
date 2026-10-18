//! The user's AI choices. A small JSON file, `edith-ai-settings.json`, in the data directory.
//! Reading is forgiving (a damaged file means defaults and a note, never a crash); writing is
//! atomic (a temporary file, then a rename), so a crash cannot leave half a file.

use edith_llm::Effort;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const SETTINGS_FILE: &str = "edith-ai-settings.json";

/// What is sent along with each question unless the person turns it off. Remembered between
/// sessions. The picture of the frame is on by default but only goes to a model that can see.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Chips {
    pub selection: bool,
    pub playhead: bool,
    pub transcript: bool,
    pub markers: bool,
    pub frame: bool,
}

impl Default for Chips {
    fn default() -> Self {
        Self { selection: true, playhead: true, transcript: false, markers: false, frame: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettings {
    /// Which provider to use (`anthropic`; others arrive with their connect buttons).
    pub service: String,
    /// The model for new threads. A setting, not a constant: change it any time. Empty means none
    /// is chosen yet (a provider with many models never gets one picked for the person).
    pub model: String,
    /// The model last used with each provider, so switching providers and back restores it.
    pub model_by_service: BTreeMap<String, String>,
    /// Reasoning effort, where the model supports it (`None`: the model's own default).
    pub effort: Option<Effort>,
    /// Ask the model for a short summary of its reasoning and show it.
    pub show_thinking: bool,
    /// What goes along with each question.
    pub chips: Chips,
    /// After the first answer, ask the model for a short title for the thread (one small extra request).
    pub auto_title: bool,
    /// Most model requests in one turn.
    pub max_steps: u32,
    /// Stop a turn once a thread's estimated cost reaches this many US dollars (`None`: no limit).
    pub max_cost_usd: Option<f64>,
    pub max_output_tokens: u32,
    /// How the assistant works in the next message: `ask` (look and explain) or `coach` (the same,
    /// with the coach's way of teaching). It is read when a message is sent, so it can change in
    /// the middle of a thread.
    pub mode: String,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            service: "anthropic".into(),
            model: "claude-haiku-5-5".into(),
            model_by_service: BTreeMap::new(),
            effort: None,
            show_thinking: true,
            chips: Chips::default(),
            auto_title: true,
            max_steps: 12,
            max_cost_usd: Some(1.0),
            max_output_tokens: 8192,
            mode: "ask".into(),
        }
    }
}

impl AiSettings {
    /// These settings with every value forced into a sane range. Settings come from a file anyone
    /// can edit, and from commands: nothing is trusted.
    pub fn sanitized(mut self) -> Self {
        let d = AiSettings::default();
        let word_ok = |s: &str| !s.is_empty() && s.len() <= 100 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-'));
        self.service = self.service.trim().to_string();
        self.model = self.model.trim().to_string();
        if !word_ok(&self.service) {
            self.service = d.service;
        }
        if !self.model.is_empty() && !word_ok(&self.model) {
            self.model = crate::endpoints::default_model(&self.service).to_string();
        }
        self.model_by_service.retain(|k, v| word_ok(k) && word_ok(v));
        while self.model_by_service.len() > 16 {
            self.model_by_service.pop_last();
        }
        if !matches!(self.mode.as_str(), "ask" | "coach" | "edit") {
            self.mode = d.mode;
        }
        self.max_steps = self.max_steps.clamp(1, 40);
        self.max_output_tokens = self.max_output_tokens.clamp(256, 64_000);
        self.max_cost_usd = match self.max_cost_usd {
            Some(c) if c.is_finite() && c > 0.0 => Some(c.min(10_000.0)),
            Some(_) => d.max_cost_usd,
            None => None,
        };
        self
    }

    /// Changes the provider, remembering the model used with the old one and restoring the one
    /// used with the new one (or that provider's starting model, which may be none).
    pub fn switch_service(&mut self, service: &str) {
        if self.service == service {
            return;
        }
        if !self.model.is_empty() {
            self.model_by_service.insert(self.service.clone(), self.model.clone());
        }
        self.service = service.to_string();
        self.model = self.model_by_service.get(service).cloned().unwrap_or_else(|| crate::endpoints::default_model(service).to_string());
    }

    /// Reads the settings from `dir`. A missing file is the defaults; a damaged one is the defaults
    /// plus a note to show.
    pub fn load(dir: &Path) -> (AiSettings, Option<String>) {
        let path = dir.join(SETTINGS_FILE);
        match std::fs::read(&path) {
            // Only a JSON object counts: serde would also read a bare array field by field.
            Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .filter(serde_json::Value::is_object)
                .and_then(|v| serde_json::from_value::<AiSettings>(v).ok())
            {
                Some(s) => (s.sanitized(), None),
                None => (AiSettings::default(), Some("The AI settings file was damaged, so the defaults are in use.".to_string())),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (AiSettings::default(), None),
            Err(_) => (AiSettings::default(), Some("The AI settings file could not be read, so the defaults are in use.".to_string())),
        }
    }

    /// Writes the (sanitized) settings to `dir`, atomically.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&self.clone().sanitized()).map_err(|_| "the settings could not be encoded".to_string())?;
        std::fs::create_dir_all(dir).map_err(|_| "the settings folder could not be created".to_string())?;
        let tmp = dir.join(format!("{SETTINGS_FILE}.tmp"));
        std::fs::write(&tmp, json).map_err(|_| "the settings could not be written".to_string())?;
        std::fs::rename(&tmp, dir.join(SETTINGS_FILE)).map_err(|_| "the settings could not be saved".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("edith-ai-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn the_default_model_is_haiku_and_a_cost_cap_is_on() {
        let s = AiSettings::default();
        assert_eq!((s.service.as_str(), s.model.as_str()), ("anthropic", "claude-haiku-5-5"));
        assert_eq!(s.max_cost_usd, Some(1.0));
        assert_eq!(s.clone().sanitized(), s);
    }

    #[test]
    fn the_frame_goes_along_by_default_and_the_choice_is_remembered() {
        let d = Chips::default();
        assert!(d.frame && d.selection && d.playhead && !d.transcript && !d.markers);
        let dir = tmp("chips");
        let s = AiSettings { chips: Chips { frame: false, markers: true, ..Chips::default() }, ..Default::default() };
        s.save(&dir).unwrap();
        assert_eq!(AiSettings::load(&dir).0.chips, s.chips);
        // an older file with no chips gets the defaults; a partial one keeps what it says
        std::fs::write(dir.join(SETTINGS_FILE), r#"{"model":"claude-opus-5-5"}"#).unwrap();
        assert_eq!(AiSettings::load(&dir).0.chips, Chips::default());
        std::fs::write(dir.join(SETTINGS_FILE), r#"{"chips":{"frame":false}}"#).unwrap();
        let c = AiSettings::load(&dir).0.chips;
        assert!(!c.frame && c.selection);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn switching_provider_remembers_each_providers_model() {
        let mut s = AiSettings::default();
        s.switch_service("openrouter");
        assert_eq!((s.service.as_str(), s.model.as_str()), ("openrouter", ""), "nothing is chosen for the person");
        s.model = "meta/free-model:free".into();
        s.switch_service("anthropic");
        assert_eq!(s.model, "claude-haiku-5-5", "the Claude choice is restored");
        s.model = "claude-sonnet-5-5".into();
        s.switch_service("openrouter");
        assert_eq!(s.model, "meta/free-model:free");
        s.switch_service("anthropic");
        assert_eq!(s.model, "claude-sonnet-5-5");
        let before = s.clone();
        s.switch_service("anthropic");
        assert_eq!(s, before, "switching to the same provider changes nothing");
        assert_eq!(s.clone().sanitized(), s);
    }

    #[test]
    fn settings_survive_a_save_and_a_load() {
        let dir = tmp("roundtrip");
        let s = AiSettings {
            model: "claude-sonnet-5-5".into(),
            effort: Some(Effort::High),
            show_thinking: false,
            max_steps: 5,
            max_cost_usd: None,
            ..Default::default()
        };
        s.save(&dir).unwrap();
        assert!(!dir.join(format!("{SETTINGS_FILE}.tmp")).exists(), "no temporary file is left behind");
        assert_eq!(AiSettings::load(&dir), (s, None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_damaged_or_unreadable_file_means_defaults() {
        let dir = tmp("damaged");
        assert_eq!(AiSettings::load(&dir), (AiSettings::default(), None));
        std::fs::create_dir_all(&dir).unwrap();
        for bad in ["not json", "{\"model\": 5}", "[]", "[\"anthropic\"]", "\"text\"", "null", ""] {
            std::fs::write(dir.join(SETTINGS_FILE), bad).unwrap();
            let (s, note) = AiSettings::load(&dir);
            assert_eq!(s, AiSettings::default(), "{bad:?}");
            assert!(note.is_some_and(|n| n.contains("damaged")));
        }
        // a directory where the file should be
        std::fs::remove_file(dir.join(SETTINGS_FILE)).unwrap();
        std::fs::create_dir(dir.join(SETTINGS_FILE)).unwrap();
        assert!(AiSettings::load(&dir).1.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn partial_files_fill_in_defaults_and_hostile_values_are_forced_sane() {
        let dir = tmp("hostile");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SETTINGS_FILE), r#"{"model":"claude-opus-5-5"}"#).unwrap();
        let (s, note) = AiSettings::load(&dir);
        assert_eq!((s.model.as_str(), s.max_steps, note), ("claude-opus-5-5", 12, None));
        let wild = AiSettings {
            service: "bad service!".into(),
            model: "x".repeat(500),
            max_steps: 0,
            max_output_tokens: u32::MAX,
            max_cost_usd: Some(f64::NAN),
            ..Default::default()
        }
        .sanitized();
        assert_eq!((wild.service.as_str(), wild.model.as_str()), ("anthropic", "claude-haiku-5-5"));
        assert_eq!((wild.max_steps, wild.max_output_tokens, wild.max_cost_usd), (1, 64_000, Some(1.0)));
        assert_eq!(AiSettings { max_cost_usd: Some(-3.0), ..Default::default() }.sanitized().max_cost_usd, Some(1.0));
        assert_eq!(AiSettings { max_cost_usd: Some(1e300), ..Default::default() }.sanitized().max_cost_usd, Some(10_000.0));
        assert_eq!(AiSettings { max_steps: 9999, ..Default::default() }.sanitized().max_steps, 40);
        // a bad model name falls back to the provider's own starting model, which may be none
        let odd = AiSettings { service: "openrouter".into(), model: "bad model!".into(), ..Default::default() }.sanitized();
        assert_eq!(odd.model, "");
        let many = (0..40).map(|i| (format!("s{i}"), "m".to_string())).chain([("bad key!".to_string(), "m".to_string())]).collect();
        assert_eq!(AiSettings { model_by_service: many, ..Default::default() }.sanitized().model_by_service.len(), 16);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
