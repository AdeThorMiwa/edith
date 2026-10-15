//! The loop: ask the model, run the tools it calls, answer them, repeat, until it is done or a
//! limit is reached. Every way a turn can end leaves the thread valid to send again.

use crate::context::{ContextBlock, clip_text, user_message};
use crate::environment::{Environment, Profile};
use crate::prompt::PromptPack;
use edith_llm::{Cancel, ChatModel, Effort, LlmError, Message, Part, Price, Request, Response, StopReason, StreamEvent, ToolSpec, Usage};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub model: String,
    pub effort: Option<Effort>,
    /// Ask the model for a summary of its reasoning to show.
    pub thinking_summary: bool,
    pub max_output_tokens: u32,
    /// Most model requests in one turn.
    pub max_steps: u32,
    /// Tool calls run per reply; any more are answered with an error asking for fewer.
    pub max_tools_per_reply: usize,
    pub max_tool_result_chars: usize,
    pub max_context_block_chars: usize,
    /// Stop a turn once this many tokens (in and out, all turns of the thread) have been used.
    pub max_total_tokens: Option<u64>,
    /// Stop a turn once the estimated cost of the thread reaches this many US dollars.
    pub max_cost_usd: Option<f64>,
    pub price: Option<Price>,
    pub profile: Profile,
    /// Waits before each retry of a request that failed before any output (overloaded, rate limited, timeout).
    pub retry_delays: Vec<Duration>,
}

impl AgentConfig {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            effort: None,
            thinking_summary: false,
            max_output_tokens: 8192,
            max_steps: 12,
            max_tools_per_reply: 16,
            max_tool_result_chars: 20_000,
            max_context_block_chars: 20_000,
            max_total_tokens: None,
            max_cost_usd: None,
            price: None,
            profile: Profile::ReadOnly,
            retry_delays: vec![Duration::from_secs(1), Duration::from_secs(3)],
        }
    }
}

/// A conversation. It only grows: nothing in it is edited or removed, because the model's
/// reasoning blocks must come back exactly as they were sent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    pub messages: Vec<Message>,
    /// Tokens used by the whole thread so far.
    pub usage: Usage,
}

impl Thread {
    pub fn new() -> Self {
        Self::default()
    }

    /// Cuts the conversation back to its longest valid beginning, and says how many messages were
    /// dropped. A thread read from a file anyone can edit may hold a tool call that was never
    /// answered, or answers in the wrong place; the provider rejects such a conversation on every
    /// later request, so only the part that is valid is kept. Valid means: every tool call is
    /// answered, once, by the very next message (a user message holding all the answers in order),
    /// and no answer refers to a call that was not made.
    pub fn repair(&mut self) -> usize {
        let mut keep = 0usize;
        let mut i = 0usize;
        let mut open: Vec<String> = Vec::new();
        while let Some(m) = self.messages.get(i) {
            let uses: Vec<&str> = m.parts.iter().filter_map(|p| if let Part::ToolUse { id, .. } = p { Some(id.as_str()) } else { None }).collect();
            let answers: Vec<&str> =
                m.parts.iter().filter_map(|p| if let Part::ToolResult { tool_use_id, .. } = p { Some(tool_use_id.as_str()) } else { None }).collect();
            let ok = if !open.is_empty() {
                // this message must answer exactly the open calls, in order
                m.role == edith_llm::Role::User && answers == open.iter().map(String::as_str).collect::<Vec<_>>() && uses.is_empty()
            } else {
                answers.is_empty() && (uses.is_empty() || m.role == edith_llm::Role::Assistant)
            };
            if !ok {
                break;
            }
            open = uses.iter().map(|s| s.to_string()).collect();
            i += 1;
            // a point where nothing is waiting for an answer is a place the thread can end
            if open.is_empty() {
                keep = i;
            }
        }
        let dropped = self.messages.len() - keep;
        self.messages.truncate(keep);
        dropped
    }

    /// Estimated cost of the thread in US dollars, if the model's price is known.
    pub fn estimated_cost(&self, price: Option<&Price>) -> Option<f64> {
        price.map(|p| p.estimate(&self.usage))
    }
}

/// Why a turn ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Finish {
    Done,
    /// The step limit was reached with work still pending.
    StepLimit,
    /// The token or cost budget was reached.
    BudgetReached,
    Stopped,
    /// The provider declined to answer.
    Refused,
    /// The reply was cut off by the output limit.
    CutOff,
    Failed {
        message: String,
        retryable: bool,
    },
}

/// What the person sees as a turn unfolds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// What the person typed (context blocks are not repeated), first in every turn, so a recorded
    /// or streamed thread reads as a whole conversation.
    UserMessage {
        text: String,
    },
    StepStarted {
        step: u32,
    },
    /// The text and thinking shown for this step should be discarded: it is being asked again.
    StepRestarted,
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolStarted {
        id: String,
        name: String,
    },
    ToolCall {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        id: String,
        name: String,
        content: String,
        is_error: bool,
    },
    Usage {
        reply: Usage,
        total: Usage,
    },
    Notice {
        text: String,
    },
    Finished {
        finish: Finish,
    },
}

/// The result of a turn: why it ended, how many requests it made, and the model's replies in order
/// (enough to replay the turn with no network).
#[derive(Clone, Debug, PartialEq)]
pub struct TurnReport {
    pub finish: Finish,
    pub steps: u32,
    pub replies: Vec<Response>,
}

pub struct Agent<'a> {
    pub model: &'a dyn ChatModel,
    pub env: &'a dyn Environment,
    pub pack: PromptPack,
    pub config: AgentConfig,
}

fn sleep_cancelable(total: Duration, cancel: &Cancel) -> bool {
    let end = std::time::Instant::now() + total;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return false;
        }
        let now = std::time::Instant::now();
        if now >= end {
            return true;
        }
        std::thread::sleep((end - now).min(Duration::from_millis(25)));
    }
}

/// The unanswered tool calls of `message`, answered with `text` as errors, as one user message.
fn answer_all(message: &Message, text: &str) -> Option<Message> {
    let results: Vec<Part> = message
        .parts
        .iter()
        .filter_map(|p| {
            if let Part::ToolUse { id, .. } = p { Some(Part::ToolResult { tool_use_id: id.clone(), content: text.to_string(), is_error: true }) } else { None }
        })
        .collect();
    (!results.is_empty()).then(|| Message::tool_results(results))
}

impl Agent<'_> {
    fn over_budget(&self, thread: &Thread) -> bool {
        let c = &self.config;
        let tokens = thread
            .usage
            .input_tokens
            .saturating_add(thread.usage.output_tokens)
            .saturating_add(thread.usage.cache_read_tokens)
            .saturating_add(thread.usage.cache_write_tokens);
        c.max_total_tokens.is_some_and(|m| tokens >= m) || c.max_cost_usd.zip(thread.estimated_cost(c.price.as_ref())).is_some_and(|(m, cost)| cost >= m)
    }

    /// Runs one turn: adds the person's message (with its context blocks) to the thread, then loops
    /// until the model is done or a limit is reached. Never panics; every outcome is a [`Finish`].
    pub fn run_turn(&self, thread: &mut Thread, context: &[ContextBlock], text: &str, cancel: &Cancel, sink: &mut dyn FnMut(AgentEvent)) -> TurnReport {
        self.run_turn_as(thread, context, text, text, cancel, sink)
    }

    /// [`Agent::run_turn`] where the model is asked `text` but the conversation shows `shown`: a
    /// slash command appears as the person typed it while the model gets its expansion.
    pub fn run_turn_as(
        &self,
        thread: &mut Thread,
        context: &[ContextBlock],
        text: &str,
        shown: &str,
        cancel: &Cancel,
        sink: &mut dyn FnMut(AgentEvent),
    ) -> TurnReport {
        let c = &self.config;
        thread.messages.push(user_message(context, text, c.max_context_block_chars));
        sink(AgentEvent::UserMessage { text: shown.to_string() });
        // The gate: tools that would change the project are neither offered nor run in a read-only thread.
        let all = self.env.tools();
        let offered: Vec<&(ToolSpec, bool)> = all.iter().filter(|(_, read_only)| c.profile.allows(*read_only)).collect();
        let specs: Vec<ToolSpec> = offered.iter().map(|(s, _)| s.clone()).collect();
        let system = self.pack.system_parts(c.profile);
        let mut report = TurnReport { finish: Finish::Done, steps: 0, replies: Vec::new() };

        let finish = 'turn: loop {
            if report.steps >= c.max_steps.max(1) {
                break Finish::StepLimit;
            }
            if cancel.load(Ordering::SeqCst) {
                break Finish::Stopped;
            }
            report.steps += 1;
            sink(AgentEvent::StepStarted { step: report.steps });
            let mut request = Request::new(c.model.clone());
            request.system = system.clone();
            request.messages = thread.messages.clone();
            request.tools = specs.clone();
            request.effort = c.effort;
            request.thinking_summary = c.thinking_summary;
            request.max_output_tokens = c.max_output_tokens;

            let mut attempt = 0;
            let response = loop {
                let mut delivered = false;
                let result = self.model.stream(&request, cancel, &mut |e| {
                    let ev = match e {
                        StreamEvent::Text(text) => AgentEvent::Text { text },
                        StreamEvent::Thinking(text) => AgentEvent::Thinking { text },
                        StreamEvent::ToolUseStarted { id, name } => AgentEvent::ToolStarted { id, name },
                        StreamEvent::ToolUseFinished { .. } | StreamEvent::Usage(_) => return,
                    };
                    delivered = true;
                    sink(ev);
                });
                match result {
                    Ok(r) => break r,
                    Err(LlmError::Cancelled) => break 'turn Finish::Stopped,
                    // Only a request that produced nothing is asked again, so the person never sees duplicated text.
                    Err(e) if e.retryable() && !delivered && attempt < c.retry_delays.len() => {
                        sink(AgentEvent::Notice { text: format!("{e} Trying again.") });
                        if !sleep_cancelable(c.retry_delays.get(attempt).copied().unwrap_or_default(), cancel) {
                            break 'turn Finish::Stopped;
                        }
                        attempt += 1;
                    }
                    Err(e) => break 'turn Finish::Failed { message: e.to_string(), retryable: e.retryable() },
                }
            };
            if attempt > 0 {
                sink(AgentEvent::StepRestarted);
            }

            // The reply goes into the thread exactly as received.
            thread.messages.push(response.message.clone());
            thread.usage.add(&response.usage);
            sink(AgentEvent::Usage { reply: response.usage, total: thread.usage });
            let stop = response.stop_reason.clone();
            report.replies.push(response.clone());

            match stop {
                StopReason::Refusal => {
                    if let Some(m) = answer_all(&response.message, "Not run: the provider declined this reply.") {
                        thread.messages.push(m);
                    }
                    break Finish::Refused;
                }
                StopReason::MaxTokens => {
                    if let Some(m) = answer_all(&response.message, "Not run: the reply was cut off before this call was complete.") {
                        thread.messages.push(m);
                    }
                    break Finish::CutOff;
                }
                StopReason::PauseTurn => continue,
                StopReason::ToolUse => {}
                StopReason::EndTurn | StopReason::StopSequence | StopReason::Other(_) => {
                    if let Some(m) = answer_all(&response.message, "Not run: the reply ended without asking for tool results.") {
                        thread.messages.push(m);
                    }
                    break Finish::Done;
                }
            }

            // Run the tools: answers to all of this reply's calls go back in ONE message.
            let mut stopped = false;
            let mut results: Vec<Part> = Vec::new();
            for (i, part) in response.message.tool_uses().enumerate() {
                let Part::ToolUse { id, name, input, input_error } = part else { continue };
                sink(AgentEvent::ToolCall { id: id.clone(), name: name.clone(), input: input.clone() });
                let out = if cancel.load(Ordering::SeqCst) {
                    stopped = true;
                    crate::ToolOutput::error("Not run: the person stopped this turn.")
                } else if i >= c.max_tools_per_reply {
                    crate::ToolOutput::error(format!("Not run: more than {} tool calls in one reply. Ask for fewer at a time.", c.max_tools_per_reply))
                } else if let Some(e) = input_error {
                    crate::ToolOutput::error(json!({"INVALID_JSON": e}).to_string())
                } else if !offered.iter().any(|(s, _)| s.name == *name) {
                    if all.iter().any(|(s, _)| s.name == *name) {
                        crate::ToolOutput::error(format!("Not run: {name} changes the project and this thread is read-only."))
                    } else {
                        let names: Vec<&str> = offered.iter().map(|(s, _)| s.name.as_str()).collect();
                        crate::ToolOutput::error(format!("Unknown tool {name}. Available: {}.", names.join(", ")))
                    }
                } else {
                    self.env.run(name, input, cancel)
                };
                let content = clip_text(&out.content, c.max_tool_result_chars);
                sink(AgentEvent::ToolResult { id: id.clone(), name: name.clone(), content: content.clone(), is_error: out.is_error });
                results.push(Part::ToolResult { tool_use_id: id.clone(), content, is_error: out.is_error });
            }
            if !results.is_empty() {
                thread.messages.push(Message::tool_results(results));
            }
            if stopped || cancel.load(Ordering::SeqCst) {
                break Finish::Stopped;
            }
            if self.over_budget(thread) {
                break Finish::BudgetReached;
            }
        };
        sink(AgentEvent::Finished { finish: finish.clone() });
        report.finish = finish;
        report
    }
}
