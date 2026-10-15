# edith-ai

The one crate the rest of Edith imports for AI (L3; roadmap F03 in
[docs/upcoming-features.md](../../docs/upcoming-features.md), design in section 4.8). It re-exports
everything from `edith-llm` (providers) and `edith-agent` (the agent), and holds what ties them to
the app:

- `AiSettings`: service, **default model** (a setting, shipped as Haiku 5.5), effort, reasoning
  summary, step limit, cost cap per thread, output limit. Stored in `edith-ai-settings.json` in the
  data directory (not upstream's preferences). A missing file is the defaults, a damaged one is the
  defaults plus a note, writes are atomic, and every value is forced into a sane range.
- `models`: `badges` (capabilities, context size, estimated price; nothing is hidden) and
  `resolve_model` (the preferred model, else the usual cheap-to-capable ones, else the first).
- `agent_config`: settings plus the chosen model's entry become the agent's configuration (a
  reasoning summary or effort is only asked for when the model supports it).
- `Assistant`: model, tools and settings, ready to run turns on a worker thread.
- `usage_summary`: the thread meter, "12.4k tokens, about $0.04".
- `model_for` (feature `net`, with `keychain` for Anthropic): the provider for a service id.

Features, off by default and off for the web build: `net`, `keychain`.
