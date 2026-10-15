# edith-llm

Model providers for the Edith agent, behind one narrow interface (L3; roadmap F03 in
[docs/upcoming-features.md](../../docs/upcoming-features.md), design in section 4.8). It knows how to
talk to a language model and nothing about editing, so the agent (`edith-agent`) can be built and
tested without it.

- `ChatModel`: `models()` and `stream(request, cancel, sink)`.
- `types`: `Message` of `Part`s (`Text`, `ToolUse`, `ToolResult`, `Opaque`), `ToolSpec`, `Request`,
  `StreamEvent`, `StopReason`, `Usage`, `Capabilities`, `Price`.
- `anthropic`: the Messages API. `build_body`, `StreamAssembler` and `parse_models` are plain code
  (checked against recorded streams); `Anthropic` (feature `net`) is the HTTPS client over
  `edith-net`, with the key read from the keychain on every request (feature `keychain`) and never
  kept. Its errors have the exact key and key-shaped words removed.
- `scripted::ScriptedModel`: replays recorded streams for tests.

Features, off by default and off for the web build: `net` (the real client), `keychain`.

`cargo test -p edith-llm --features keychain -- --ignored real_anthropic` makes one tiny real call
with the stored key (a few tokens).
