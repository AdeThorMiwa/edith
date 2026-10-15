# edith-agent

The Edith agent (L3; roadmap F03 in [docs/upcoming-features.md](../../docs/upcoming-features.md),
design in section 4.8). It is Edith's craft: the loop, the prompt packs, how context is assembled,
the budgets, the safety profile and the records of every run. It talks to a model only through
`edith_llm::ChatModel` and to the editor only through `Environment`, so it is tested with a scripted
model and a fake editor, and can be improved without touching any provider.

- `Agent::run_turn(thread, context, text, cancel, sink)`: one user message in, a finished turn out.
  Every step is reported as an `AgentEvent`; the result is a `TurnReport` with a `Finish` (`Done`,
  `StepLimit`, `BudgetReached`, `Stopped`, `Refused`, `CutOff`, `Failed`).
- `Environment`: the tools the agent may call (`tools()` with a read-only flag, `run()`). The engine
  implements it; `testing::FakeEnvironment` is the fake.
- `Profile`: `ReadOnly` or `Propose`. A read-only thread is never offered, and never runs, a tool that
  changes the project.
- `PromptPack` (`prompts/edith.md`): versioned prompt text, assembled in a fixed order so the cached
  prefix stays cached. Context (selection, transcript) goes in the user message, never the system
  prompt.
- `Trace`: a JSON-lines record of a run. `Trace::replay_model()` reproduces the recorded thread with
  no network; this is the base of the evaluation suite.
- `eval` (F03.6): the evaluation suite. `Suite::builtin()` is the shipped `core` suite (`suites/core.json`);
  `grade(task, trace, context)` checks a run with code graders and needs no model; `Report` renders
  to markdown and `compare` shows two or more runs side by side. The engine runs it
  (`assistant.eval`); a prompt, tool-description or loop change ships with its report.

Rules the loop keeps, each with a test: the thread only grows (reasoning blocks come back exactly as
sent); every tool call is answered even when a turn ends early (refusal, cut-off, Stop, step limit,
budget), so the thread is always valid to send again; all answers to one reply go in one message;
tool results are data, never instructions; a request is retried only if it showed nothing yet.
