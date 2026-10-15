//! The assistant: a model, the editor's tools and the user's settings, ready to run turns.

use crate::AiSettings;
use edith_agent::{Agent, AgentConfig, AgentEvent, ContextBlock, Environment, Profile, PromptPack, Thread, TurnReport};
use edith_llm::{Cancel, ChatModel, ModelInfo, Price, Usage};
use std::sync::Arc;

/// The agent settings for `settings` and the chosen model: reasoning summaries and effort are only
/// asked for when the model supports them, the output limit never exceeds the model's, and the
/// price (for the cost cap and the meter) comes from the model's own entry.
pub fn agent_config(settings: &AiSettings, info: Option<&ModelInfo>, profile: Profile) -> AgentConfig {
    let s = settings.clone().sanitized();
    let mut c = AgentConfig::new(s.model.clone());
    let caps = info.map(|i| &i.capabilities);
    c.effort = s.effort.filter(|_| caps.is_some_and(|c| c.effort));
    c.thinking_summary = s.show_thinking && caps.is_some_and(|c| c.thinking);
    c.max_output_tokens = match caps.map(|c| c.max_output_tokens).filter(|n| *n > 0) {
        Some(limit) => s.max_output_tokens.min(u32::try_from(limit).unwrap_or(u32::MAX)),
        None => s.max_output_tokens,
    };
    c.max_steps = s.max_steps;
    c.max_cost_usd = s.max_cost_usd;
    c.price = info.and_then(|i| i.price.clone());
    c.profile = profile;
    c
}

/// "12.4k tokens, about $0.003" for the thread's meter. The price is an estimate, so the text says
/// "about"; without a known price only the tokens are shown.
pub fn usage_summary(usage: &Usage, price: Option<&Price>) -> String {
    let total = usage.input_tokens.saturating_add(usage.output_tokens).saturating_add(usage.cache_read_tokens).saturating_add(usage.cache_write_tokens);
    let count = if total >= 1000 { format!("{:.1}k", total as f64 / 1000.0) } else { total.to_string() };
    match price.map(|p| p.estimate(usage)) {
        Some(cost) if cost >= 0.01 => format!("{count} tokens, about ${cost:.2}"),
        Some(cost) => format!("{count} tokens, about ${cost:.4}"),
        None => format!("{count} tokens"),
    }
}

/// A model, tools and settings. Cheap to share (`Arc`); the engine runs turns on a worker thread.
pub struct Assistant {
    pub model: Arc<dyn ChatModel>,
    pub env: Arc<dyn Environment>,
    pub pack: PromptPack,
    pub config: AgentConfig,
}

impl Assistant {
    pub fn new(model: Arc<dyn ChatModel>, env: Arc<dyn Environment>, settings: &AiSettings, info: Option<&ModelInfo>, profile: Profile) -> Self {
        Self { model, env, pack: PromptPack::builtin(), config: agent_config(settings, info, profile) }
    }

    /// Runs one turn on `thread`. See [`Agent::run_turn`].
    pub fn run_turn(&self, thread: &mut Thread, context: &[ContextBlock], text: &str, cancel: &Cancel, sink: &mut dyn FnMut(AgentEvent)) -> TurnReport {
        self.run_turn_as(thread, context, text, text, cancel, sink)
    }

    /// Runs one turn asking the model `text` while the conversation shows `shown` (slash commands).
    pub fn run_turn_as(
        &self,
        thread: &mut Thread,
        context: &[ContextBlock],
        text: &str,
        shown: &str,
        cancel: &Cancel,
        sink: &mut dyn FnMut(AgentEvent),
    ) -> TurnReport {
        Agent { model: &*self.model, env: &*self.env, pack: self.pack.clone(), config: self.config.clone() }
            .run_turn_as(thread, context, text, shown, cancel, sink)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edith_agent::testing::FakeEnvironment;
    use edith_agent::{Finish, ToolOutput};
    use edith_llm::scripted::ScriptedModel;
    use edith_llm::{Capabilities, Effort, Message, Part, Response, Role, StopReason, StreamEvent};
    use std::sync::atomic::AtomicBool;

    fn info(thinking: bool, max_out: u64, price: bool) -> ModelInfo {
        ModelInfo {
            id: "claude-haiku-5-5".into(),
            display_name: "Haiku".into(),
            capabilities: Capabilities {
                tools: true,
                parallel_tools: true,
                images: true,
                thinking,
                effort: thinking,
                caching: true,
                context_tokens: 1_000_000,
                max_output_tokens: max_out,
            },
            price: price.then(|| Price { input_per_mtok: 0.1, output_per_mtok: 0.5, cache_read_per_mtok: None, source: "t".into() }),
        }
    }

    #[test]
    fn the_config_follows_what_the_model_can_do() {
        let s = AiSettings {
            effort: Some(Effort::High),
            show_thinking: true,
            max_output_tokens: 50_000,
            max_steps: 7,
            max_cost_usd: Some(0.25),
            ..Default::default()
        };
        let c = agent_config(&s, Some(&info(true, 128_000, true)), Profile::ReadOnly);
        assert_eq!((c.effort, c.thinking_summary, c.max_output_tokens, c.max_steps, c.max_cost_usd), (Some(Effort::High), true, 50_000, 7, Some(0.25)));
        assert!(c.price.is_some() && c.profile == Profile::ReadOnly);
        // a model that cannot think or take an effort: neither is asked for; its output cap is respected
        let c = agent_config(&s, Some(&info(false, 8_000, false)), Profile::Propose);
        assert_eq!((c.effort, c.thinking_summary, c.max_output_tokens, c.profile), (None, false, 8_000, Profile::Propose));
        assert!(c.price.is_none());
        // unknown model info: nothing optional is assumed
        let c = agent_config(&s, None, Profile::ReadOnly);
        assert_eq!((c.effort, c.thinking_summary), (None, false));
        // hostile settings are sanitized on the way in
        let c = agent_config(&AiSettings { max_steps: 0, model: "bad model".into(), ..Default::default() }, None, Profile::ReadOnly);
        assert_eq!((c.max_steps, c.model.as_str()), (1, "claude-haiku-5-5"));
    }

    #[test]
    fn the_cost_meter_says_about_and_scales_its_precision() {
        let p = Price { input_per_mtok: 2.0, output_per_mtok: 10.0, cache_read_per_mtok: None, source: "t".into() };
        let u = Usage { input_tokens: 10_000, output_tokens: 2_400, ..Default::default() };
        assert_eq!(usage_summary(&u, Some(&p)), "12.4k tokens, about $0.04");
        assert_eq!(usage_summary(&Usage { input_tokens: 2_000, output_tokens: 240, ..Default::default() }, Some(&p)), "2.2k tokens, about $0.0064");
        assert_eq!(usage_summary(&Usage { input_tokens: 1_000_000, ..Default::default() }, Some(&p)), "1000.0k tokens, about $2.00");
        assert_eq!(usage_summary(&Usage { input_tokens: 850, ..Default::default() }, None), "850 tokens");
    }

    #[test]
    fn an_assistant_runs_a_whole_turn_through_the_facade() {
        let model = Arc::new(ScriptedModel::new());
        let call = Response {
            message: Message {
                role: Role::Assistant,
                parts: vec![Part::ToolUse { id: "t1".into(), name: "markers".into(), input: serde_json::json!({}), input_error: None }],
            },
            stop_reason: StopReason::ToolUse,
            usage: Usage { input_tokens: 100, output_tokens: 20, ..Default::default() },
            model: "claude-haiku-5-5".into(),
        };
        model.push_reply(vec![], call);
        model.push_reply(
            vec![StreamEvent::Text("Marker at 0:42.".into())],
            Response {
                message: Message::assistant_text("Marker at 0:42."),
                stop_reason: StopReason::EndTurn,
                usage: Usage { input_tokens: 150, output_tokens: 10, ..Default::default() },
                model: "claude-haiku-5-5".into(),
            },
        );
        let env = Arc::new(FakeEnvironment::new().tool("markers", true, |_| ToolOutput::ok("0:42 chapter")));
        let a = Assistant::new(model.clone(), env, &AiSettings::default(), Some(&info(true, 128_000, true)), Profile::ReadOnly);
        let mut thread = Thread::new();
        let mut events = Vec::new();
        let r = a.run_turn(&mut thread, &[], "where are my markers?", &Arc::new(AtomicBool::new(false)), &mut |e| events.push(e));
        assert_eq!((r.finish, r.steps), (Finish::Done, 2));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::Text { text } if text.contains("0:42"))));
        // settings reached the request
        let req = &model.requests()[0];
        assert_eq!(req.model, "claude-haiku-5-5");
        assert!(req.thinking_summary, "the default settings ask for a reasoning summary and this model supports it");
        assert!(usage_summary(&thread.usage, a.config.price.as_ref()).contains("tokens, about $"));
    }
}
