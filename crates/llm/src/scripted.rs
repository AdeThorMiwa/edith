//! A model that replays recorded streams, so the agent can be tested with no network and no key.

use crate::anthropic::StreamAssembler;
use crate::{Cancel, ChatModel, LlmError, ModelInfo, Request, Response, StreamEvent};
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, PoisonError};

enum Turn {
    /// A recorded Anthropic stream (`event:` and `data:` lines), replayed line by line.
    Sse(String),
    /// A ready-made reply and the events to announce for it.
    Reply(Vec<StreamEvent>, Response),
    Fail(LlmError),
    /// Some events arrive, then the stream fails.
    Partial(Vec<StreamEvent>, LlmError),
}

/// Plays back the turns it was given, one per request, and remembers every request it received.
#[derive(Default)]
pub struct ScriptedModel {
    turns: Mutex<VecDeque<Turn>>,
    seen: Mutex<Vec<Request>>,
    models: Vec<ModelInfo>,
}

impl ScriptedModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_models(mut self, models: Vec<ModelInfo>) -> Self {
        self.models = models;
        self
    }

    /// The next request is answered by replaying this recorded stream.
    pub fn push_sse(&self, text: impl Into<String>) -> &Self {
        self.turns.lock().unwrap_or_else(PoisonError::into_inner).push_back(Turn::Sse(text.into()));
        self
    }

    pub fn push_reply(&self, events: Vec<StreamEvent>, response: Response) -> &Self {
        self.turns.lock().unwrap_or_else(PoisonError::into_inner).push_back(Turn::Reply(events, response));
        self
    }

    pub fn push_error(&self, error: LlmError) -> &Self {
        self.turns.lock().unwrap_or_else(PoisonError::into_inner).push_back(Turn::Fail(error));
        self
    }

    /// The next request delivers `events` and then fails with `error`, as a dropped connection does.
    pub fn push_partial(&self, events: Vec<StreamEvent>, error: LlmError) -> &Self {
        self.turns.lock().unwrap_or_else(PoisonError::into_inner).push_back(Turn::Partial(events, error));
        self
    }

    /// Every request received so far, oldest first.
    pub fn requests(&self) -> Vec<Request> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Turns not yet played.
    pub fn remaining(&self) -> usize {
        self.turns.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

impl ChatModel for ScriptedModel {
    fn provider(&self) -> &str {
        "anthropic"
    }

    fn models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(self.models.clone())
    }

    fn stream(&self, request: &Request, cancel: &Cancel, sink: &mut dyn FnMut(StreamEvent)) -> Result<Response, LlmError> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner).push(request.clone());
        if cancel.load(Ordering::SeqCst) {
            return Err(LlmError::Cancelled);
        }
        let turn = self.turns.lock().unwrap_or_else(PoisonError::into_inner).pop_front();
        match turn {
            None => Err(LlmError::Unavailable("the scripted model has no more turns".to_string())),
            Some(Turn::Fail(e)) => Err(e),
            Some(Turn::Partial(events, e)) => {
                for ev in events {
                    sink(ev);
                }
                Err(e)
            }
            Some(Turn::Reply(events, response)) => {
                for e in events {
                    sink(e);
                }
                Ok(response)
            }
            Some(Turn::Sse(text)) => {
                let mut asm = StreamAssembler::new();
                for line in text.lines() {
                    if cancel.load(Ordering::SeqCst) {
                        return Err(LlmError::Cancelled);
                    }
                    asm.feed_line(line, sink)?;
                }
                asm.finish()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::stream_fixtures as f;
    use crate::{Message, Part, StopReason, Usage};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    fn cancel() -> Cancel {
        Arc::new(AtomicBool::new(false))
    }

    #[test]
    fn it_plays_turns_in_order_and_remembers_the_requests() {
        let m = ScriptedModel::new();
        m.push_sse(f::start(3) + &f::text_block(0, &["one"]) + &f::end("end_turn", 1))
            .push_sse(f::start(3) + &f::text_block(0, &["two"]) + &f::end("end_turn", 1));
        let mut r1 = Request::new("m");
        r1.messages.push(Message::user_text("first"));
        let a = m.stream(&r1, &cancel(), &mut |_| {}).unwrap();
        let mut r2 = r1.clone();
        r2.messages.push(Message::user_text("second"));
        let b = m.stream(&r2, &cancel(), &mut |_| {}).unwrap();
        assert_eq!((a.message.text().as_str(), b.message.text().as_str()), ("one", "two"));
        assert_eq!(m.requests().len(), 2);
        assert_eq!(m.requests()[1].messages.len(), 2);
        assert_eq!(m.remaining(), 0);
        assert!(matches!(m.stream(&r1, &cancel(), &mut |_| {}), Err(LlmError::Unavailable(_))));
    }

    #[test]
    fn ready_made_replies_and_errors_work_and_cancel_wins() {
        let m = ScriptedModel::new();
        let resp = Response {
            message: Message { role: crate::Role::Assistant, parts: vec![Part::Text { text: "hi".into() }] },
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            model: "m".into(),
        };
        m.push_reply(vec![StreamEvent::Text("hi".into())], resp).push_error(LlmError::Overloaded);
        let mut seen = Vec::new();
        let r = m.stream(&Request::new("m"), &cancel(), &mut |e| seen.push(e)).unwrap();
        assert_eq!((r.message.text().as_str(), seen), ("hi", vec![StreamEvent::Text("hi".into())]));
        assert_eq!(m.stream(&Request::new("m"), &cancel(), &mut |_| {}).unwrap_err(), LlmError::Overloaded);
        let stop = Arc::new(AtomicBool::new(true));
        m.push_sse("irrelevant");
        assert_eq!(m.stream(&Request::new("m"), &stop, &mut |_| {}).unwrap_err(), LlmError::Cancelled);
    }

    #[test]
    fn stopping_mid_replay_is_cancelled_not_a_short_reply() {
        let m = ScriptedModel::new();
        m.push_sse(f::start(3) + &f::text_block(0, &["a", "b", "c"]) + &f::end("end_turn", 1));
        let stop = cancel();
        let flag = stop.clone();
        let mut n = 0;
        let r = m.stream(&Request::new("m"), &stop, &mut |e| {
            if matches!(e, StreamEvent::Text(_)) {
                n += 1;
                if n == 2 {
                    flag.store(true, Ordering::SeqCst);
                }
            }
        });
        assert_eq!(r.unwrap_err(), LlmError::Cancelled);
    }

    #[test]
    fn a_partial_turn_delivers_its_events_then_fails() {
        let m = ScriptedModel::new();
        m.push_partial(vec![StreamEvent::Text("half".into())], LlmError::Overloaded);
        let mut seen = Vec::new();
        let r = m.stream(&Request::new("m"), &cancel(), &mut |e| seen.push(e));
        assert_eq!((r.unwrap_err(), seen), (LlmError::Overloaded, vec![StreamEvent::Text("half".into())]));
    }
}
