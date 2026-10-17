# Upcoming features

> **Last reviewed:** 2026-10-12 (against upstream 0.7.2) · **Last updated:** 2026-10-16 · **Change:** F02 and F03 done: the full `cargo xtask ci` is green (3,564 tests, 0 failed, 2026-10-16); F03.3, F03.6 and F03.7 built, F03.4 and F03.5 parked; earlier: F02 built (network, secrets, sign-in pieces, Connections panel; final check pending), D5 and D11 decided; earlier: F01 done (speech on by default, `upstream` remote, fork rules in fork.md, `CLAUDE.md` pointer); new file (direction for the creator layer: assistant, sourcing, planning, analysis, voices, look)

The plan for what this fork adds on top of FilmCraft, in the order we intend to build it. It is the working agreement between the **architect** (product design and architecture decisions) and the **builder** (agents, one feature at a time). Upstream's own plan stays in [roadmap.md](roadmap.md) and [gaps.md](gaps.md); this file does not replace it.

**Legend.** Status: ⬜ planned · 🟡 in progress · ✅ done. Type: **Must** (needed for the vision) or **Nice**. ★ = the architect asked for it by name. Size is a guess in the upstream roadmap's unit (agent-hours): S 5–15 · M 15–40 · L 40–100 · XL 100+. Commits follow the repo rule, one task id each (`F05.2: acquire downloads into the project`).

## Contents

1. [The future we are building](#1-the-future-we-are-building)
2. [Principles](#2-principles)
3. [Roadmap at a glance](#3-roadmap-at-a-glance)
4. [Shared architecture](#4-shared-architecture)
5. [Features, in order](#5-features-in-order)
6. [Format Pack baselines: Mystery and Explainer](#6-format-pack-baselines-mystery-and-explainer)
7. [Open decisions for the architect](#7-open-decisions-for-the-architect)
8. [Parked and not planned](#8-parked-and-not-planned)
9. [External facts to re-verify](#9-external-facts-to-re-verify)

## 1. The future we are building

One person can run a faceless YouTube channel at a cinematic, A24-like standard (patient pacing, a consistent look, restrained sound, a distinct narrator) because the editor does the tedious and the measurable work and teaches the craft as it goes, while every creative decision stays with the editor.

The target workflow:

1. **Plan.** Pick a Format Pack (Mystery or Explainer to start) and write the treatment: beats with a purpose, an intensity and a runtime budget. Ask the assistant to critique it when you want a second opinion.
2. **Write and voice.** Write the narration per beat in the A/V script. Fix names in the pronunciation dictionary, audition takes, and run the voice through your narrator voice so it sounds like *your* narrator.
3. **Source.** Each narration line owns a *slot*. The Source Hub fills slots in the background with licensed B-roll from free sources. You shortlist and choose. Nothing is accepted for you.
4. **Rough cut.** Ask for a rough cut. You get a *proposal*: cuts on sentence boundaries, B-roll placed, ambience under, music ducked. Preview it against your current cut, then accept all, part or none, as one undo step.
5. **Craft.** Add the film finish (grain, halation, diffusion), run the consistency checker, read the sound map and spotting sheet. The coach and your reference fingerprint show in plain numbers where the pacing drifts from the style you are aiming for.
6. **Deliver.** Loudness check, chapters, credits block, disclosure checklist, thumbnail frame.

**Where it goes.** The assistant is meant to grow into an editing agent in the way Claude Code is a coding agent: a product of its own, with its own prompts, tools, memory and measured quality, that happens to run on whichever model is best this year. That is why the agent is kept separate from the model providers (§4.8), why it is built with an evaluation suite from the start (F03.6), and why its design follows published research, recorded in [research/agent-and-editing.md](research/agent-and-editing.md). The first finding is a useful warning: on a benchmark of real editing tasks the best agents failed mostly on craft, not on following the brief, and the authors trace it to agents perceiving footage only through stills and transcripts. The editing craft it brings (reading a cut in measurements, knowing a format's beats, comparing against your reference) comes from F08 to F13, not from the model.

**What this is not.** Not autopilot. Not generative-first: free sources and local tools come first, generation is opt-in and late. Not a Premiere-parity project: that is upstream's job.

## 2. Principles

1. **The human drives.** AI output arrives as a proposal, scoped to what you point at, previewed, accepted or rejected as one undo step. No silent edits (§4.4).
2. **Free first.** Every default path costs nothing: free stock and archive sources, local models, free API tiers. Paid providers are opt-in, never default, always with a visible meter.
3. **Provenance by default.** Every asset records where it came from and under what license. Generated assets are flagged. Exports can list credits.
4. **Commands first.** Each feature is engine commands with typed parameters and tests. The UI, CLI, control channel, MCP and the assistant all dispatch the same ids (upstream rule), so every new feature is automatically reachable by agents.
5. **Measure with code, explain with models.** Numbers (pacing, loudness, color) come from deterministic code with tests. Models only explain, suggest or rank.
6. **Extensible by data.** Channel types are Format Packs (data files), not code. Mystery and Explainer ship first; others are added later without touching Rust.
7. **Fork-friendly.** Custom code lives in new crates and files. Edits to upstream-owned files are small hooks. Upstream merges stay cheap (§4.6).
8. **Upstream's rules still apply.** Never crash (no `unwrap`/`panic!` outside tests), clean-room (no GPL code), openly licensed assets with attribution sidecars, exact `Tick` time, the layer rules. `cargo xtask ci` stays green ([AGENTS.md](../AGENTS.md)).
9. **Private and offline-capable.** Nothing leaves the machine without a visible action. A global Offline mode exists. Secrets live in the OS keychain.

## 3. Roadmap at a glance

Phases are the suggested order. Within a phase, items can swap if their *Needs* allow. F07 (Look Pack) has no dependencies beyond F01 and can be pulled forward at any time.

| # | Feature | Type | Size | Needs |
|---|---|---|---|---|
| | **Phase 0: groundwork** | | | |
| F01 | Fork baseline | Must | S | none |
| F02 | Network, secrets and OAuth foundation | Must | M | F01 |
| | **Phase 1: first-video kit** | | | |
| F03 | Assistant pane: connect, chat, mentor | Must | L | F02 |
| F04 | Provenance and license ledger | Must | M | F01 |
| F05 | Source Hub and Find Media panel | Must | L | F02, F04 |
| F06 | Narration upgrades: pronunciation, takes, saved voice | Must | M | F01 |
| F07 | Look Pack: film texture and the A24 stack | Must | L | F01 |
| | **Phase 2: plan and understand** | | | |
| F08 | Planner core and Format Packs | Must | L | F01 |
| F09 | Planner views | Must | L | F08 (F05 for slots) |
| F10 | Analysis core | Must | L | F01 |
| F11 | Editing coach v1 | Must | M | F10 |
| F12 | ★ Reference analysis and look bible | Must | L | F10 (F08 for targets) |
| F13 | ★ Sound map and spotting sheet | Must | M | F10, F08 |
| | **Phase 3: human-in-the-loop automation** | | | |
| F14 | Proposals engine | Must | L | F01 |
| F15 | Assistant v2: edit tools via proposals | Must | M | F03, F14 (F08) |
| F16 | VO-first rough cut | Must | L | F14, F05, F08, F06 |
| F17 | ★ Sound design suggestions | Must | L | F13, F14, F05 |
| F18 | Consistency checker and shot matcher | Must | M | F10, F14 |
| | **Phase 4: voices** | | | |
| F19 | Voice Lab (custom voices) | Must | L | F06 |
| | **Phase 5: ship and grow** | | | |
| F20 | YouTube prep | Must | M | F04, F13 |
| F21 | Smart search and ranking | Nice | L | F05, F10, F03 |
| F22 | Versions and A/B compare | Nice | M | F14 |
| F23 | Shorts repurposing | Nice | M | F14, F10 |
| F24 | Format Pack authoring and more packs | Nice | M | F08, F07, F12 |
| F25 | Generative sources (opt-in) | Nice | L | F02, F04, F05 |
| F26 | Multilingual | Parked | XL | F06, F19 |

**Why this order.**

- *Phase 0* is invisible but everything needs it.
- *Phase 1* is the **first-video kit**: connect an assistant (also the riskiest unknown, so it is validated early), find and license B-roll, make the narration sound right, give the picture a film finish. After it, you can make a decent video. If B-roll is more urgent than the assistant, F04 and F05 can go first.
- *Phase 2* makes the craft visible in numbers. It is the most useful phase for a newcomer and needs no proposals yet.
- *Phase 3* adds automation, all of it through proposals, once the proposals engine exists.
- *Phase 4* is the research-heavy voice work, after the cheap narration wins in F06.

## 4. Shared architecture

### 4.1 New crates and where they live

| Crate or location | Layer | Purpose | Used by |
|---|---|---|---|
| `edith-net` (`crates/net`) | L2 | HTTP client, cache, rate limits, Offline mode, secret store, OAuth/PKCE | F02 and every networked feature |
| `edith-llm` (`crates/llm`) | L3 | **Model providers** behind one narrow interface: Anthropic, OpenAI-compatible (OpenRouter, OpenAI, Ollama, LM Studio), Gemini. Neutral message, tool, stream-event and capability types. Knows nothing about editing | F03, F15, F21 |
| `edith-agent` (`crates/agent`) | L3 | **The Edith agent**: loop, prompt packs, context assembly, tool registry, budgets, history rules, trace recording, evaluation harness. Talks to a model only through `edith-llm`'s trait and to the editor only through an `Environment` trait. Knows nothing about vendors or projects | F03, F15, F16, F17 |
| `edith-ai` (`crates/ai`) | L3 | **The facade the rest of the app uses**: re-exports everything from `edith-llm` and `edith-agent`, and holds the wiring and small utilities around them (build an assistant for a chosen service from the keychain and the model list, shared settings, brand text for prompts). The engine depends on this crate alone | F03 onward |
| `edith-sources` (`crates/sources`) | L2 | `MediaSource` trait, provider adapters, license normalization | F05, F25 |
| `edith-analysis` (`crates/analysis`) | L2 | Pure metrics over frames, audio and sequences | F10 onward |
| Planner, provenance, lexicon, proposal-log types | L1 in `filmcraft-project` | Project data (undoable) | F04, F06, F08, F14 |
| Engine glue (`assistant.rs`, `hub.rs`, `planner.rs`, `proposals.rs`, …) | L4 in `crates/engine/src/` | Commands, jobs, threads; implements the agent's `Environment` and tools over the session | all |
| Panels | L5 in `crates/ui-egui/src/panels/` | UI, listed in `PanelKind` (`dock.rs`) | all |

Rules: add every new crate to `LAYERS` in `xtask/src/main.rs` (a crate without a layer fails `cargo xtask layers`) and any same-layer edge to `SAME_LAYER` (`agent` to `llm`, `ai` to both, are such edges). The tool accepts the `edith-` prefix next to `filmcraft-` (a two-line change in `xtask/src/main.rs`). `cargo xtask wasm` checks every L0–L4 crate for the browser, so network crates must compile without their network feature, the pattern `crates/speech` uses (`download`, off for wasm). AI and network features are desktop-only, like speech today.

### 4.2 Network, secrets and OAuth

All network access goes through `edith-net` (F02): the pure-Rust TLS stack already used for model downloads in `crates/speech` (`ureq` 3 + rustls + `rustls-rustcrypto` + the platform verifier), per-host rate limits, an on-disk cache, a request log with secrets redacted, a global Offline mode, and an OS-keychain secret store. OAuth is built from PKCE (S256), a loopback redirect receiver and the `open` crate the desktop app already uses to launch the browser.

### 4.3 Connecting an AI provider (F03)

The pane has a **Connect** button per provider. What it does depends on what each vendor allows third-party apps to do, so there are four lanes. Each provider card says in plain words how it bills ("uses your ChatGPT plan", "pay-as-you-go API", "free tier", "runs on your machine").

| Lane | What Connect does | Billing | Providers | Status |
|---|---|---|---|---|
| **A. Sign in (OAuth/PKCE)** | Opens the provider's page in your browser; you approve; FilmCraft receives a token or key on a loopback redirect. | per provider | **OpenRouter** (documented PKCE flow that returns a key you control; no app registration). **ChatGPT sign-in** (new, for partner apps; access for a personal app unconfirmed). Google (needs an OAuth client from Google Cloud). | OpenRouter available now; others to verify |
| **B. API key** | Opens the provider's console key page; you paste the key; it goes to the OS keychain. | pay-as-you-go, or free tier | Anthropic, OpenAI, Google Gemini (free tier through AI Studio), any OpenAI-compatible endpoint | available |
| **C. Local** | Detects Ollama or LM Studio on localhost. | free (your hardware) | Ollama, LM Studio, llama.cpp server | available |
| **D. Bring your own agent (ACP)** | Starts the vendor's own agent (Claude Code, Codex, Gemini CLI) over the Agent Client Protocol; *that agent* shows its own browser sign-in with your subscription. | your existing plan | Claude (Pro/Max), Codex, Gemini CLI | **Parked** (architect, 2026-10-15: Claude via API key instead) |

**Why Claude is special.** Anthropic does not let third-party apps sign users in with a Claude subscription: it has blocked subscription tokens outside Claude Code since January 2026 and wrote the rule into its terms in February. Products may embed the *unmodified* Claude Code and let the user sign in there. That is how Zed works: Claude subscriptions run through its external-agent (ACP) path, its built-in providers use API keys held in the OS keychain, and ChatGPT and GitHub Copilot have their own sign-in flows. So the "press Connect, authorize in my Claude console" experience is possible for Claude only through lane D, and through an API key (lane B, with a link to the console's key page) otherwise.

**Never:** read or reuse another tool's tokens (for example Claude Code's keychain entry), reuse another client's OAuth client id, or imitate another client's identity headers. Policy in this area changed several times in 2026; re-check the vendor's terms before each lane ships (§9).

### 4.4 Proposals: how the human stays in charge

A **proposal** is `{id, title, rationale, scope, steps[{command, params}], alternatives?}`: a list of ordinary engine commands plus the reason for them. Because the project is an `Arc<Project>` edited copy-on-write with snapshot undo, a proposal can be tried on a cheap fork.

1. **Preview.** Apply the steps to a forked session. Render from the fork for ghost clips and an A/B toggle in the Program monitor. Compute a diff (clips added, removed, moved, trimmed; effects and parameters; markers) and a plain-language summary.
2. **Decide.** Accept all, accept part, reject, or edit. Stale proposals (the project changed under them) are flagged and re-previewed.
3. **Apply.** Replay the steps on the live session as **one undo step** labeled `AI: <title>`, filterable in History.
4. **Scope locks.** A proposal may only touch what you pointed at (selected clips, a range, a beat). Steps outside the scope are refused.
5. **Profiles.** *Read-only* (default for a new thread), *Propose*, and *Direct* (explicit, per thread, never default). Exports, file writes and media downloads always ask.
6. **Untrusted text.** Clip names, captions and web metadata reach the model as delimited data, and tool results are never treated as instructions. Edit tools only propose, which limits prompt injection.

### 4.5 Where data lives

| Kind | Lives in | Why |
|---|---|---|
| Planner, provenance, lexicon, proposal log | Inside the project (`Project`), undoable. See decision D1. | Undo, autosave and crash recovery come for free |
| View-like state (open panels, last tab) | Beside the project, like `view` in the `.fcproj` envelope | Not an edit, never an undo step |
| Analysis results, thumbnails, HTTP cache, embeddings | Cache directory, keyed by content hash and algorithm version | Rebuildable, never bloats the project |
| Conversation threads | Data directory, per project id | Private; not shared when the project is |
| Secrets | OS keychain | Never in any file, log or screenshot |
| Format Packs, voice presets, custom voices | Data directory (user) plus shipped defaults | Shareable as files |

### 4.6 Fork hygiene

- `origin` is `AdeThorMiwa/filmcraft` and `upstream` is `storytold/filmcraft`. `main` mirrors upstream and never takes our commits; all work lands on `studio`. The sync routine, the Rust version to use and the list of upstream-owned files we changed are in [fork.md](fork.md).
- Custom code goes in new crates and new files. The small upstream-owned hook points are: `LAYERS` and `SAME_LAYER` in `xtask/src/main.rs`, `PanelKind` in `crates/ui-egui/src/dock.rs`, command registration in `crates/engine/src/commands.rs`, one extension point on `Project`, and the default features in `apps/filmcraft/Cargo.toml` and `apps/filmcraft-cli/Cargo.toml`.
- Do not refactor `crates/automation` (upstream's MCP server). The assistant mirrors its tool names and shapes in the engine layer instead.
- Avoid bumping `SCHEMA_VERSION` per feature. Upstream bumps it too: it was 13 when this file was written and is 15 in 0.7.2 (`MIGRATIONS.len() + 1` in `crates/format/src/lib.rs`), so two different v16s would collide (D1).
- Command namespaces: `assistant.*`, `net.*`, `secrets.*`, `connections.*`, `provenance.*`, `hub.*`, `planner.*`, `analysis.*`, `coach.*`, `reference.*`, `soundplan.*`, `proposal.*`, `voicelab.*`, `youtube.*`; narration additions extend `tts.*`.

### 4.7 Testing conventions for networked and AI code

- No live network in CI. Provider adapters are tested against recorded responses and local mock servers (including a mock OAuth server). Model-dependent tests skip without weights, as upstream does.
- Provider responses and media metadata are untrusted: size caps, depth caps, hostile-input tests, never a panic.
- Effects get golden-image tests. Analysis gets synthetic fixtures with known metrics (the generators in `crates/media`).
- Every feature ships its commands, automation ids and state in `state.rs`, so agents and MCP can reach it.

### 4.8 The agent is separate from the provider

The Edith agent is meant to improve on its own, as a researched and measured editing agent, whichever model runs under it. So there is a hard seam between the two.

**Providers (`edith-llm`) know how to talk to a model, nothing else.** One trait, `ChatModel`, with `capabilities()` (tools, parallel tools, images, thinking, caching, context size, output cap, price) and `stream(request, sink)`. The request is neutral: system parts, messages made of `Text`, `ToolUse`, `ToolResult` and `Opaque` parts, tool specs, a reasoning effort, an output cap, cache hints. Events are neutral too: text, thinking summary, tool use, usage, done with a normalized stop reason, error. Each provider translates to and from its own wire format and nothing vendor-shaped leaks upward.
- **`Opaque` parts** carry what a vendor needs echoed back unchanged (Anthropic's thinking blocks are the case today). The agent stores and replays them in order, never reads them, and drops them when the next model comes from a different provider. That is how a thread survives a model switch and how the history stays append-only.
- A model that lacks a capability does not break the agent: it adapts (no tools: chat-only thread; no images: describe the frame with F10's measurements instead).

**The agent (`edith-agent`) is Edith's craft, and owns everything that makes it good.**
- **The loop and its policy**: steps, token and cost budgets, parallel read-only tools, Stop, error recovery, what to do on a refusal.
- **Prompt packs**: system prompt, mentor prompts, per-format guidance, tool descriptions. Plain files with a version, assembled in a stable order so the cached prefix stays cached.
- **Context assembly**: what the chips (`@selection`, `@frame`, `@transcript`, `@beat`) turn into, how large results are trimmed, how a long thread is compacted without rewriting history.
- **The tool registry**: a `Tool` trait with a spec and a `run` against an `Environment`. The engine implements the tools (it owns the project); the agent owns how they are described and how their results are shaped for the model.
- **Safety profile**: read-only now, propose after F14. The gate lives in the agent and again in the engine, so a bug in one cannot grant edits.
- **Traces**: every run records a JSONL trace (prompt pack version, model, events, tool calls with inputs and results, usage). A trace can be replayed with no network and no model.

**Direction of dependencies.** `agent` depends on the `llm` interface and on nothing vendor-specific; `llm` depends on `net`; `edith-ai` depends on both and re-exports them with the wiring around them; the engine depends on `edith-ai` and supplies the `Environment`. Nothing below the engine imports the project, so the agent can be tested with a fake editor and a scripted model.

**Research loop (F03.6).** A suite of tasks: a small fixture project built from engine commands, a question or goal, and graders. Graders are code first (does the answer cite timestamps that exist; did it call the tools the task needs; did it stay read-only; token and cost use), with an optional model judge using a written rubric later. The suite runs against any provider, so a result is about the agent or the model and never both at once. Rule from F03.6 on: a change to a prompt pack, a tool description or the loop policy ships with its eval delta. Domain knowledge (Format Packs, the reference fingerprint, the sound map) enters as tools and retrieval, because an editing agent differs from a coding agent in what it must perceive: video is not text, so it needs measurements of pacing, loudness, cuts and frames as its eyes and ears (F10), and the model only explains and proposes.

## 5. Features, in order

**Definition of done for every feature:** commands with typed parameters, `describe` docs and tests (including hostile parameters) · UI with automation ids · looked at the result by running with `--control` and taking `ui.screenshot` · never-crash rules respected · fixtures instead of live network · `cargo xtask ci` green · this file updated (status and the *Last updated* line).

---

### Phase 0: groundwork

### F01 Fork baseline · Must · S · ✅

**Needs:** none · **Unlocks:** everything.

**What it does.** The fork builds with speech-to-text and natural voices on by default, and has the conventions the rest of this file relies on.

**How it works.**
- F01.1 Default features `whisper` and `neural-voices` on `apps/filmcraft` and `apps/filmcraft-cli` (so MCP and CLI clients can transcribe and narrate too). Models are downloaded on request through the existing `transcript.downloadModel` and `tts.downloadVoices`; a first-run prompt is still optional and not built. Turning the features on made the gate compile Whisper code that upstream never lints, which surfaced one `expect()` in `crates/speech/src/whisper/tokenizer.rs`; it is fixed there, with a test (the exception in [fork.md](fork.md)).
- F01.2 The `upstream` remote (`storytold/filmcraft`), the `studio` branch (`main` stays a mirror) and the sync routine are in [fork.md](fork.md): fetch, fast-forward `main`, merge into `studio`, `cargo xtask ci`.
- F01.3 `CLAUDE.md` now starts with one line pointing at fork.md and this file (D6, decided yes).

**Done when.** A fresh clone runs `cargo run --release -p filmcraft` with Transcribe and natural voices available after the model download; `cargo xtask ci` is green; the sync routine is documented.

**Checked 2026-10-13.** `cargo xtask ci` green on Rust 1.99.0 (3,265 tests passed, 0 failed, 44 ignored). With the headless CLI, `whisper-base` (293 MB) and the natural voices (336 MB) downloaded; the voice `kokoro-heart` said "The lighthouse keeper had not slept in three days, and the sea was getting louder." (5.05 s); Whisper heard it back with 2 word errors of 15 ("sea was" came out as "seawaws") in 1.8 s. The app was checked on a build without the models: features on, `transcript.downloadModel` and `tts.downloadVoices` enabled, natural voices reported available.

**Notes.** Defaults add candle to every build, so compiles get longer: a cold full `cargo xtask ci` took 106 minutes on the author's MacBook, and the app alone builds in about 3.5 minutes warm. Use current stable (checked on 1.99.0; see [fork.md](fork.md)). Building the CLI after the app still recompiles most of the engine, so build the one you need. `whisper-base` is modest on run-together speech; `whisper-small` (967 MB) is the next step up if accuracy matters. The web app is a separate crate and is unaffected.

### F02 Network, secrets and OAuth foundation · Must · M · ✅

**Needs:** F01 · **Unlocks:** F03, F05, F25.

**What it does.** One safe way for the app to reach the internet and hold credentials. Every networked feature uses it, so "no silent network calls", rate limits, caching, Offline mode and the usage meter exist in exactly one place.

**How it works.** The code is the crate `edith-net` (L2, [README](../crates/net/README.md)), the engine module `net_commands`, and the panel `panels/connections.rs`.
- **HTTP client** (feature `http`): `https` only (plain `http` only to `127.0.0.1` in tests), timeouts, retries with backoff for GET (and for a request the caller marks safe to repeat), `Retry-After`, per-host rate limits, an on-disk cache that honors ETag and Last-Modified (opt-in per request, keyed by a hash of the URL, never for requests with credentials), a descriptive User-Agent, response and header size caps, streaming line reads for server-sent events, cancel. Hostile addresses and headers (control characters, `Host`, `Content-Length`, user names in URLs) are refused before anything is sent. The transport runs under `catch_unwind`.
- **Offline mode**: one switch (status bar, the panel, `net.setOffline`); every request fails at once with a clear error and the attempt is logged. It is remembered across restarts by an empty marker file in the data directory (`network-offline`), so no preference or project field is involved.
- **Request log**: the newest 500 requests in memory, shown in the Connections panel and by `net.log`. Each entry has only method, `scheme://host/path`, status, size, time and a scrubbed error: never a query string, header or body.
- **Secret store** (feature `keychain`): macOS Keychain, Windows Credential Manager, Linux Secret Service (pure Rust over D-Bus). No plaintext fallback: with no keychain, saving fails with a clear message. Secrets never enter `.fcproj`, preferences, logs, MCP output or screenshots. Only the Connections panel can save one, through a Rust function that is **not a command**; commands can ask whether one exists and delete it, never read it back, and none of them is journaled.
- **OAuth building blocks** (feature `oauth`): PKCE S256 (checked against the RFC 7636 vector), `state`, a one-shot receiver on `127.0.0.1` at a random port that accepts only `GET /callback` with the host name it was bound to (so a web page cannot reach it by DNS rebinding), caps requests at 4 KiB, never echoes the code, stops after a timeout, on cancel or after 20 unusable requests, and closes the port after one use; and a token-request helper. The provider-specific sign-in (OpenRouter first) is wired in F03.1.
- **Connections panel** (Window ▸ Connections), not a Settings page: that keeps the edits to upstream files to a minimum. It shows the Offline switch, every known service with whether a key is stored, a masked field to paste a key, **Get a key** (opens the provider's key page) and **Remove**, and the request log. The list of services and their keychain slots is `crates/net/src/services.rs`; later features add to it.
- **Features**: all of this is behind Cargo features that are off for the web build. In this fork the apps turn `net` on by default (`--no-default-features` gives upstream's lean build).

**Commands.** `net.status`, `net.setOffline`, `net.log` (limit, or `clear`), `secrets.has`, `secrets.delete`, `connections.list`. There is deliberately no `secrets.set` or `secrets.get`.

**Done when.** A mock provider is called with a stored secret; Offline mode blocks it; the secret appears in no log; hostile responses (huge bodies, bad JSON, stalled connections) never panic.

**Decided.** D5: `keyring-core` 1 plus one store crate per OS (`apple-native-keyring-store`, `windows-native-keyring-store`, `zbus-secret-service-keyring-store` with pure-Rust crypto). All MIT OR Apache-2.0, the maintained successor of the single `keyring` crate, and macOS reuses the `security-framework` already in the lockfile. D11 (new): the pure-Rust TLS provider `rustls-rustcrypto` is still marked alpha and "not for production" by its authors, and API keys will travel over it; it is kept behind one function (`crates/net/src/tls.rs`) so it can be swapped, and the risk is accepted for now.

**Not covered yet.** The Windows and Linux keychain stores compile only on those systems; they were checked for licences and dependencies but not built here. The real "Connect" button (open the provider's page, receive the redirect, store the key) is F03.1; until then the panel takes a pasted key. A macOS build that is not signed with a stable identity asks for keychain access again after each rebuild.

**Notes.** Tests: the crate has about 45 (a local mock server for the client; the receiver is attacked with wrong paths, wrong state, wrong host, oversize and flooding; an `#[ignore]`d test each for real HTTPS and the real keychain), the engine has 7, the Connections panel has 5 headless UI tests including one that types a key and checks it appears in no `ui.inspect`, label, event or command result.

---

### Phase 1: first-video kit

### F03 Assistant pane: connect, chat, mentor · Must · L · yours · ✅

**Needs:** F02 · **Unlocks:** F15, F21, and a home for every later feature's tools.

**What it does.** A docked panel, modeled on Zed's agent panel, where you connect an AI provider and talk to it about the project. The assistant can *see* what you see (selection, playhead, current frame, transcript around the playhead, the beat you are on). It starts **read-only**: it explains, critiques and answers editing questions about *your* timeline. It cannot change anything until F14 and F15.

**How it works.**
- **Connect** per provider card, in the four lanes of §4.3. Keys and tokens live in the keychain (F02).
- **Panel**: thread list, model picker, profile picker (Read-only now, Propose later), removable context chips (`@selection`, `@frame`, `@transcript`, `@beat`, `@markers`) so you always see what the model sees, streaming replies with Stop, tool-call cards, a usage meter, slash commands (`/explain-cut`, `/glossary`).
- **Read-only tools**: project and sequence inspect, markers, a transcript window, a Program monitor frame, `command_list` search. Same names and shapes as the MCP tools in `crates/automation/src/server.rs`, implemented in the engine layer.
- **Threading**: provider calls run on worker threads under `catch_unwind`; tool calls run on the session thread between frames; tokens arrive over a channel; long calls show in `jobs.list`.

**Steps** (reordered 2026-10-15: the architect's main provider is Claude, used with their own API key; the Claude Code / ACP route was considered and dropped).
- F03.0 **Research notes** (not a gate): a first pass is in [research/agent-and-editing.md](research/agent-and-editing.md). F03.1 is built now from what is known there, and each later stage reads more and records what it changes. Better research later improves the agent; it does not hold it up.
- F03.1 The chat panel (threads, streaming view, tool-call cards, thinking summaries, cost meter, Stop) and the **Anthropic client with Edith's own agent loop** (lane B, Claude first). Built in five commits: (a) `edith-llm` (**done 2026-10-15**: the neutral types, the Anthropic request builder, stream assembler, model list and client, a scripted model; 38 tests, key never in a log or error, one `#[ignore]`d real-API test), (b) `edith-agent` (**done 2026-10-15**: the loop, prompt pack, tool-host interface, read-only gate, budgets, traces with deterministic replay; 25 tests), (c) `edith-ai` (**done 2026-10-15**: the settings with the default model, model badges and choice, the cost summary, the `Assistant`, the provider wiring; 11 tests), (d) engine glue and the read-only tools (**done 2026-10-15**: `assistant.*` commands, threads as cancellable jobs, tool calls answered on the session thread, four read-only tools, settings that persist; 20 tests), (e) the panel (**done 2026-10-15**: Window ▸ Assistant with the model and thread pickers, streaming messages, thinking summaries, tool cards, a Stop button, the cost meter, context chips, settings for the default model, effort, step and cost limits; 11 view tests and 6 headless UI tests). **F03.1 is built;** F03 stays 🟡 until the full gate passes and F03.2 to F03.6 are done.
- F03.2 Context chips, the read-only tools, mentor prompts. Built in three commits: (a) **done 2026-10-15**: prompt pack version 2 (mentor guidance, the craft notes, "ask for independent tools together") and the slash commands `/explain-cut`, `/pacing`, `/summarize`, `/glossary term`, which live in the pack and are expanded by `assistant.send` so every client gets them; (b) **done 2026-10-15**: conversations are saved when a turn ends, one file per thread in `<data dir>/edith-threads/` (atomic, versioned, read as untrusted: damaged, oversize or mislabelled files skipped, a conversation with an unanswered tool call cut back to its valid part, the newest 50 restored), and a two-step Delete button in the panel; (c) **done 2026-10-15**: seeing a frame. Pictures travel in messages and tool results (`Part::Image`, `images` on a tool result; sent to the API as image blocks, with unsupported types and oversize pictures replaced by a line of text); a `frame_view` tool renders the Program monitor at a moment (JPEG, 256 to 1568 px, default 768 or about 450 tokens) without moving the playhead; the chips **Transcript** (what is said within 20 s of the playhead), **Markers** and **Frame** join Selection and Playhead; a model without the `images` capability is neither offered the frame tool nor allowed a frame chip; each message shows a "Sent along:" line naming what went with it; saved conversations drop picture bytes. Picture bytes never travel in events or traces. A live test (`real_assistant_looks_at_a_frame_and_describes_it`, ignored) checks a picture reaches the real model. **F03.2 is built.**
- F03.3 Provider abstraction, the OpenAI-compatible client, and the Connect buttons: OpenRouter sign-in (A), other API keys (B), local servers (C). Built in five commits:
  - F03.3a ✅ the wire format, stream reader and model-list parser (`edith-llm::openai`, pure code, recorded streams).
  - F03.3b ✅ the HTTPS client (`OpenAiCompatible`), the endpoint table (`edith-ai::endpoints`), the provider picker in the Assistant header, and a remembered model per provider. OpenRouter, OpenAI, Ollama and LM Studio can be chosen now; keys come from the Connections panel (paste), local servers need none. On a provider with many models nothing is chosen for you.
  - F03.3c ✅ the sign-in building blocks: a `localhost` redirect that also listens on `::1`, a refusal without `state` ends the wait (and can never yield a code), any cancel check plugs in (`wait_until`), and `edith-ai::connect::OpenRouter` (address + key exchange, feature `signin`). Not yet checked against the real OpenRouter page: whether it returns `state` on approval as documented. The first live Connect will show it; if not, only the receiver's `state` rule needs a decision.
  - F03.3d ✅ `connections.connect|disconnect` (a cancellable job; the key never appears in a result, job record or log) and the Connect / Waiting / Disconnect buttons on the Connections panel. Needs one live check by you: click Connect on OpenRouter and approve in the browser.
  - F03.3e ✅ `connections.probe` and a Check button for Ollama and LM Studio (once, on request, logged like any request); Ollama models get their tools/vision/thinking/context badges from `/api/show`, LM Studio's from `/api/v0/models` (tool support there is not documented, so it stays unknown); embedding models are left out of the chat list; an unreachable server is described as "not running".
- F03.4 Gemini (free tier) and any OpenAI-compatible endpoint. **Parked 2026-10-16** at the architect's request (Claude is enough for now; the OpenAI-compatible client from F03.3 already reaches OpenRouter, OpenAI, Ollama and LM Studio).
- F03.5 ChatGPT and Google sign-in, once access is confirmed (D2). **Parked 2026-10-16**, same reason.
- F03.6 ✅ **The agent evaluation suite** (§4.8), built 2026-10-16 on the scripted model and trace recorder from F03.1.
  - **Tasks and graders** (`edith_agent::eval`, pure code): a built-in suite `core` of nine questions on the demo project (orientation, reading clips and counts, markers and times, what is on screen at a marker, transitions, pacing without invented times, declining to edit, ignoring instructions planted in a marker). Graders: tools called, names mentioned (groups of alternatives), phrases avoided, **every time the answer cites lies inside the edit**, budget (steps, tokens, dollars), project unchanged. Two checks always run: the turn finished normally and no tool that changes the project was called. Grading reads only the run's trace, so it needs no model and no network.
  - **Runner** (`assistant.eval`): each question runs in a session of its own on a fresh fixture, so your project and threads are never touched; a background job (`jobs.list`, cancellable) that saves `report.json`, `report.md` and one replayable `traces/<task>.jsonl` per question. It calls the configured model, which costs money, so it refuses unless `live: true`. `promptPack` tries another prompt file, `model` another model, `tasks` a subset. `fromTraces` grades saved runs again for free (after a grader changes). `assistant.evalCompare` puts several reports side by side and marks tasks that were fixed or regressed.
  - **Honesty tests:** a test checks the suite's expectations against what the tools really say about the fixtures, so a fixture or tool change that makes a task unanswerable fails in the gate instead of in a costly run; a careless scripted model must fail nearly everything while a good one passes all nine.
  - **How to run it:** `cargo test -p filmcraft-engine --lib --features net -- --ignored real_suite` (about nine short questions on Claude Haiku 5.5, a few cents), or `assistant.eval {live: true, wait: true}` from the control channel. The rule from here on: a change to a prompt pack, a tool description or the loop policy ships with its report, and `assistant.evalCompare` shows the difference.
  - **Not here:** judging craft (needs human-rated examples, F10 onward), vision tasks (need a frame fixture with known content), and multi-turn tasks with interruptions (DeskCraft-style; F14). The grader list grows with each feature's tools.
- F03.7 **Polish and defaults** (captured 2026-10-15 from the architect's first hands-on test; design notes below). No dependencies, so it can come before F03.3 if daily comfort matters more than more providers: your call.
- Parked: hosting vendor agents over ACP (Claude Code, Codex, Gemini CLI), lane D.

**Where it lives.** `edith-llm` (providers), `edith-agent` (the agent, §4.8), `edith-ai` (the facade), `crates/engine/src/assistant.rs` (threads, `Environment`, tools over the session), `panels/assistant.rs` and `PanelKind::Assistant`. Commands `assistant.connect|disconnect|send|cancel|threads|setModel`.

**Done when.** You can connect through at least two lanes, ask "why does the cut at 00:42 feel abrupt?" and get an answer that cites your actual timeline; disconnecting removes the secret; tests pass against a mock provider with no live network.

**Notes.** The mentor value is immediate for someone new to editing. Every later feature adds its tools here.

#### F03.7: polish and defaults (captured 2026-10-15)

**Source.** The architect's first run of the Assistant (2026-10-15): everything worked as designed, and this is what they want better. None of it was in the roadmap, so it lives here. It is a list of intentions with a design note each, not a spec; each item gets a short design pass when it is picked up. Size: M (about 15 to 40 agent-hours for the lot).

1. **Defaults that suit how it is used.** *(Built 2026-10-16 as F03.7a: the chips are saved settings (`chips` in `edith-ai-settings.json`, `assistant.settings {chips: {frame: false}}`); Frame is on by default and goes only to a model that the list says can see; Settings has a "Sent with each question" group. Transcript and Markers stay off.)*
   - The **Frame** chip is on by default for a model that can see, and the choice is remembered (chips become settings in `edith-ai-settings.json`, not per-session). Cost is small: a 768 px frame is about 450 tokens, a fraction of a cent on Haiku. The "Sent along:" line already shows what went with each message, so nothing is hidden.
   - Transcript and Markers stay off unless the architect says otherwise; Transcript could turn itself on when a transcript exists near the playhead.
   - Settings gets a "Defaults" group so the first-run experience can be tuned in one place.
2. **The agent sees the UI by default.** A `screen_view` tool and a **Screen** chip that capture the whole editor window (the panels, the timeline, what the person is looking at), so "what does this button do?" and "why does my timeline look like that?" can be answered from the real screen.
   - It is a UI-layer capability (the engine has no window), reusing the existing `ui.screenshot` path through a host hook, the way `HostHooks::open_url` works. About 1,300 px wide, so roughly 1,400 tokens per capture.
   - A text companion, `ui_elements` (panels, their automation ids and labels), is far cheaper and carries the same structure: offer both, prefer text when it is enough.
   - Known limit found earlier: a window on another macOS Space, or hidden, cannot be captured. The tool then says so and falls back to the Program frame, never returning a stale or blank picture.
   - Privacy: password fields are masked on screen and keys are never in labels; the Screen chip shows in "Sent along:" like any other, can be turned off in Defaults, and the panel shows a one-time note the first time it is on.
3. **Panel placement.** *(Built 2026-10-16 as F03.7b, simpler than the design below: the Assistant is a tab in the wide right-hand group of the default Editing layout, next to Properties; in a workspace without it, Window ▸ Assistant opens it beside that workspace's side panel; Shift+0 opens it. No new "Edith" workspace was added, since built-in workspaces are listed in menus and shortcuts that upstream owns; drag the tab and use Save Changes to keep a different arrangement.)* The Assistant opens small in the lower-left tab group. Wanted: a sensible home from the first open (a wide column on the right, beside Program and Properties, or a layout of its own), remembered per workspace, openable from a header button and a shortcut. Design: add the Assistant to the built-in Editing workspace and offer an "Edith" workspace preset (Assistant large, Program, Timeline); see how `panels/workspaces` stores layouts before touching `dock.rs`. Upstream-owned files touched are listed in `docs/fork.md`.
4. **A visual polish pass.** *(First pass built 2026-10-16 as F03.7c, after the architect found the panel did not look native: the header is one row in the app's own style (the thread as the monitors' dark dropdown, then flat New, Delete and Settings icons, the wrench the Program monitor uses); provider and model are dropdowns in the composer; what is sent along is a row of small pills (blue when on); the input is one dark rounded field that takes the blue focus border, with a round Send or Stop button; answers are laid out from blocks (paragraphs with a steady line height, bullet and numbered lists with a hanging indent, code, tables, rules) instead of raw marks; thinking is open while it streams. Still open from the list below: message spacing review in light theme, copy buttons, collapsed long tool results, jump to latest.)* The first version is plain on purpose. Candidates: message spacing and a clearer assistant/person distinction, code and table rendering in answers, copy buttons, long tool results collapsed with a size note, a compact header (the model as a small chip, not a wide combo), a "jump to latest" button, better empty and not-connected states, light-theme and high-DPI checks. Reference: Zed's agent panel. Done when the architect says it feels right in daily use; pictures reviewed in both themes.
5. **Smaller things seen in use.** *(Built 2026-10-16 as F03.7d and F03.7e: the model names a thread after its first answer (a setting, on by default) and a thread can be renamed from its dropdown (`assistant.renameThread`); Copy under messages, Retry under an answer and Edit under your message, both of which ask again in a **new thread** (`assistant.branch`) so the old one stays as it was; Continue after a cut-off or step-limit stop; Esc stops a running turn or drops an edit; Up and Down in the input recall earlier questions; opening the panel puts the cursor in the input; the token count runs live while it works. The "Sends along" pill row was removed from above the input at the architect's request: what goes with a question is set in Settings and each message still shows "Sent along:". Not built: a global shortcut to focus the input (global keys are in upstream-owned files).)* Thread titles are the first 60 characters of the first question: let the model title a thread after its first answer (a cheap call), and allow rename. Message actions: copy, retry, "continue" after a cut-off reply, and edit-and-resend as a new branch (the history stays append-only). Keyboard: focus the input from anywhere, Esc to stop, Up to recall the last question. Show thinking live as it streams, and a running token count during a turn.
6. **The architect's own list.** "A few other things here and there" were mentioned but not itemised; they go here when sent.

#### F03.1 design: Edith's own agent on the Anthropic API (drafted 2026-10-15)

**What "the Claude agent experience" means here.** Not Claude Code's shell and file tools, but its *harness*: you watch the model stream its answer, see a short summary of its thinking, see each tool call as a card with its inputs and result, can stop it at any point, see what it costs, and keep a long session going without losing the thread. Edith builds that loop itself on the Messages API with your key. The tools are Edith's (inspect the project, read a transcript, look at a frame), so the agent can only do what the editor exposes, and later its edits arrive as proposals you accept (F14).

**API facts** (from Anthropic's current reference, 2026-10-15; no Rust SDK exists, so Edith speaks the HTTP API itself, over the F02 client):
- `POST https://api.anthropic.com/v1/messages` with headers `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json`.
- Models (cached 2026-10-06): Fable 5.1 `claude-fable-5-1`, Opus 5.5 `claude-opus-5-5` ($4 / $20 per million tokens in / out), Sonnet 5.5 `claude-sonnet-5-5` ($2 / $10), Haiku 5.5 `claude-haiku-5-5` ($0.10 / $0.50). `GET /v1/models` returns each model's context size, output cap and capabilities, which feeds the picker's badges instead of a baked-in table.
- Streaming is server-sent events: `message_start`, `content_block_start`, `content_block_delta` (text, thinking summary, partial tool-input JSON), `content_block_stop`, `message_delta` (with `stop_reason` and usage), `message_stop`, plus `ping` and `error` events. The client keeps the lines it already reads (F02) and a small state machine builds the blocks.
- Tools: `tools: [{name, description, input_schema}]`; a `tool_use` block in the reply gets a `tool_result` back in the next user message. **Several tool calls in one reply must be answered in one user message.** `strict: true` makes arguments schema-valid; `eager_input_streaming: true` streams large inputs, and a streamed input must be validated before it runs because it can arrive truncated.
- Thinking: `thinking: {type: "adaptive", display: "summarized"}` and `output_config: {effort: ...}`; `budget_tokens` is rejected on the current models and thinking cannot be switched off on Opus 5.5 (lower the effort instead). Thinking blocks must be sent back unchanged on later turns, so the conversation history is **append-only**; editing earlier turns invalidates them.
- Forced `tool_choice` (`any` or a named tool) is rejected on the newest models; use `auto` plus an instruction.
- Safety refusals arrive as `stop_reason: "refusal"` with HTTP 200; the loop must check the stop reason before reading content, and can opt into the server-side `fallbacks` parameter.
- Cost control: prompt caching (`cache_control` on the stable system prompt and tool list; check `usage.cache_read_input_tokens`), effort, and the model choice. Long sessions can use context editing (beta `context-management-2025-06-27`) or compaction (beta `compact-2026-01-12`), but both must keep the history append-only.

**Shape.**
- **Two crates, one seam** (§4.8). `edith-llm` (L3): the `ChatModel` trait, neutral types and the Anthropic implementation, behind the F02 features. `edith-agent` (L3): the loop, prompt packs, context assembly, tool registry, budgets, traces, and a `ScriptedModel` that replays recorded streams. Events from the model are `Text`, `ThinkingSummary`, `ToolUse{id, name, input}`, `Usage`, `Done(stop_reason)`, `Error`; the agent turns them into its own `AgentEvent`s for the panel.
- The **agent loop** (in `edith-agent`): send, stream, collect tool calls, run them through the `Environment` (read-only tools on the session thread between frames, results capped in size), answer them in one message, repeat until `end_turn`, a step limit, Stop, or an error. The engine runs it as a job (`jobs.list`, cancellable) under `catch_unwind`; a stalled stream times out.
- **Cost meter**: tokens and an estimated price per thread, from the response `usage`, with the price table taken from the models list when it carries prices and from a dated built-in table otherwise (shown as an estimate).
- **Default model** is for you to pick (D13). The numbers above say Haiku 5.5 is about 20 times cheaper than Sonnet 5.5 for the same tokens, which suits plain chat and quick questions; Sonnet 5.5 or Opus 5.5 suit careful critique of a cut. The picker shows capability and price badges and never hides a model.
- **What stays out**: no shell, no file access, no web tools in the first version. Web search and fetch are server tools Anthropic runs, so they could be offered later behind a visible switch, because they send your text to the web.

**Tests (no live network).** The mock server replays recorded streams: plain text; thinking then text; a tool call whose JSON arrives in pieces; two parallel tool calls; a refusal; an `error` event mid-stream; a `ping`-only stall; an enormous tool input; invalid JSON in a `data:` line; a stream that ends without `message_stop`. The loop is tested for the single-message answer to parallel calls, the step limit, Stop, and that the key appears in no log, result or error.

**Costs, honestly.** This uses pay-as-you-go API credit, not a Claude subscription. Nothing has been measured yet; F03.1's done-when includes a measured cost for a typical mentor session so the default model is chosen on data. Free alternatives stay available: Gemini's free tier and OpenRouter's free models (F03.3, F03.4), and local models.

**Keychain prompts (found 2026-10-15 by the architect).** A development build is asked for permission on every keychain read, and a tool-using turn makes several requests, so the first design (read the key per request) produced a prompt each time and hid the prompt wait inside "latency". Now `SecretStore::get_cached` reads the key once and keeps it in wipe-on-drop memory for 30 minutes; saving, removing or a provider rejecting the key drops it. Telling the prompt to "Always Allow" covers a program until it is rebuilt; a signed release build does not ask again.

#### F03.2 design: what the agent sees of the edit (drafted 2026-10-15)

**Question asked:** give the model the timeline as OpenTimelineIO (OTIO) with provenance embedded? **Answer: OTIO for hand-off and archives, not for the prompt.**

**Measured** on the demo project (14 clips, 6 tracks, 5 transitions, 4 markers), with the OTIO exporter that already exists (`crates/interchange/src/otio.rs`): the OTIO file is 102,374 bytes (about 25,600 tokens, estimated as characters divided by 4), the editor's own sequence JSON 41,840 bytes (about 10,500), and a compact edit list of the same edit 1,317 bytes (about 330). OTIO costs about 7 KB per clip, because every time is a `RationalTime` object and each clip carries its media description and effect parameters. A 100-clip project would be around 730 KB or 180,000 tokens: dear on every request of a tool loop (about $0.02 a request on Haiku, about $0.37 on Sonnet, before caching), and near the limit of what fits at all. One sample, so treat the ratios as the point, not the digits.

**Why OTIO is wrong for the prompt.** The model needs to understand an edit, not reproduce a file. OTIO's structure (Timeline, Stack, Track, Clip, `media_references`, `active_media_reference_key`) is noise for that job, and it has no home for what Edith adds (beats, transcript alignment, measurements); those would all go in `metadata` anyway.

**Where OTIO is right.**
- **Export and archive** with provenance embedded in the standard place, namespaced metadata: `metadata.edith.provenance` on each clip's media reference, holding source, id, URL, author, licence, whether credit is required, whether it is AI-generated, and when it was fetched. The exporter already round-trips Edith's own fields in `metadata.filmcraft`; that key stays readable after the rename. This is the licence ledger's (F04) hand-off format.
- **Import** from other editors (Resolve, Premiere and Avid all write OTIO): import it, then show the agent the same compact view as any other project.

**What the agent sees instead.** Tool results use a compact edit list, a text form editors and models both read well:
- `sequence_view(range?)`: a header line (name, size, rate, length) then one line per clip, `V1 00:00:04:23-00:00:09:10 Aurora_Timelapse.mp4 src 00:00:20:17`, followed by non-default speed, gain and effect names only (parameters on demand), transitions as their own lines, markers last.
- **Provenance is shown by exception**: a clip from a free source carries a short tag, `[pexels, Pexels License, no credit]`; a clip with a licence risk (credit required, unknown, AI-generated) carries a loud one; local files carry none. `provenance(clip)` returns the whole record.
- `clip_detail(clip)`: every parameter of one clip. `otio_slice(range)`: a time range as OTIO, only if a task needs the standard form (rare, later).
- A range argument keeps a long project affordable: the agent looks at the part it was asked about.

#### F03.3 design: the OpenAI-compatible client and OpenRouter sign-in (drafted 2026-10-15, before building)

**Checked against OpenRouter's docs on 2026-10-15** (re-check before building; see §9):
- Sign-in page: `https://openrouter.ai/auth` with `callback_url`, `code_challenge`, `code_challenge_method=S256`, `state` (and optional `key_label`). "Localhost callbacks are supported on any port"; `127.0.0.1` is not mentioned, so the redirect uses `http://localhost:<port>/callback`. `state` comes back unchanged, but **not when the user denies**. Codes expire after 10 minutes.
- Exchange: `POST https://openrouter.ai/api/v1/auth/keys` with JSON `{code, code_verifier, code_challenge_method}`; the answer has a `key`. The key is a normal API key the user can see and revoke in their OpenRouter account. Docs state no limits for it.
- Chat: `POST https://openrouter.ai/api/v1/chat/completions`, `Authorization: Bearer <key>`, optional `HTTP-Referer` and `X-Title` (they name the app). Body: `model`, `messages`, `tools` (`{type:"function", function:{name, description, parameters}}`), `stream:true`. The stream is server-sent events of `chat.completion.chunk` with `delta`; usage arrives once, in the last chunk before `[DONE]`; comment lines must be ignored; a choice can carry an `error` object; `finish_reason` is one of `stop`, `length`, `tool_calls`, `content_filter`, `error`.
- Models: `GET /api/v1/models` (the page says it needs a key), `?supported_parameters=tools` filters to models that can call tools. Prices are strings; "free" is *presumably* `"0"` for both prompt and completion. The page does not say so, so check against the live list before relying on it.

**Shape.**
- Provider code goes in `edith-llm` (L3, depends on `edith-net`, same feature gates, off for wasm). `ChatModel` trait: `models()` and `chat_stream(request, on_event)` where `on_event` gets `Text`, `ToolCallDelta`, `Usage`, `Done(reason)`, `Error` and returns whether to continue. Requests and replies are plain structs; nothing provider-specific leaks above the trait.
- **F03.3a**: one *OpenAI-compatible* client covers OpenRouter, OpenAI, Ollama and LM Studio (same wire format; only base URL and auth differ). Also here: the OpenRouter sign-in and the local-server detection (lane C). **F03.4**: Anthropic's own message format, then Gemini (check whether its OpenAI-compatible endpoint is enough).
- **Connect** (lane A): `connections.connect {service}` starts a job (it shows in `jobs.list` and can be cancelled), builds the sign-in address with `oauth::Flow` (redirect host `localhost`), hands the address to the host to open (the UI opens the browser; the CLI and MCP get it in the result), waits for the redirect, exchanges the code, stores the key in the keychain, and logs "Connected to OpenRouter". The key never passes through the engine's return values. `connections.disconnect` is `secrets.delete` plus a log line. Lane B keeps the paste field and **Get a key**.
- **Changes needed in F02 code**: `Loopback::redirect_uri` takes the host name (`localhost` here); a denial callback (an `error` and no `state`) ends the wait with "you declined" instead of waiting out the timeout, but can never produce a code; a "headless" fallback (omit `callback_url`, show the code, let the user paste it) for the case where the browser cannot reach the loopback port; the panel shows a **Connect** button with a "Waiting for the browser… Cancel" state.
- **Tests** (no live network): the mock server speaks recorded SSE transcripts, including comment lines, a mid-stream `error`, tool-call arguments split across chunks, a stream that ends without `[DONE]`, a stalled stream, an enormous single line and invalid JSON in a `data:` line; the whole sign-in is run against a mock provider that plays both the browser and the exchange; the stored key is checked to be absent from every log and result.

**Risks.** `localhost` may resolve to IPv6 first; browsers retry IPv4, but if a machine does not, the receiver should also listen on `::1`. OpenRouter could change the sign-in rules (they are recent and documented only for `localhost`). Free models change daily, so the model picker must read the live list instead of a baked-in one (D3).

**Decided 2026-10-15.** (1) Claude comes first, through your own API key (F03.1); OpenRouter stays the first *sign-in* because it is the only one with a documented flow that needs no app registration. A Claude *subscription* cannot be used through OpenRouter: OpenRouter bills its own credits, and its guide for Claude Code says no Anthropic plan is needed. Claude models are available there, pay-as-you-go. (2) The model picker **shows** capabilities (tools, images, context size, price, free) as badges and hides nothing: a model that cannot call tools still brainstorms well. The editing features simply stay unavailable in a thread whose model lacks tools, and say why. (3) Requests identify the app as Edith (`X-Title: Edith`; `HTTP-Referer` is the project's address once it has one). See the naming rule in [fork.md](fork.md#naming-edith).

### F04 Provenance and license ledger · Must · M · ✅

**Built so far (2026-10-16, F04.1):** the records and the badge. `provenance.get|set|list` (`crates/engine/src/provenance.rs`), a record per media item inside the project (`Project::ext["edith.provenance"]`: undoable, autosaved, recovered; no schema bump), a rights status worked out one way everywhere (green free to use, amber credit needed, red check the terms, violet AI-generated), a round badge on each item in the Project panel with the record in its tooltip, and a Rights submenu (My footage, Licensed to me, Clear the record). Hostile files and parameters are refused or ignored, with tests. **Then F04.2 (2026-10-16):** the project policy (`provenance.policy`: commercial, material that needs a credit, AI-generated material, all allowed by default), the audit (`provenance.audit`: every clip actually in the edit, meaning enabled, on an enabled and unmuted track, nested sequences followed, checked against the policy; errors, warnings and AI-disclosure notes, each with its timecodes), the credits block (`provenance.credits`: plain, Markdown or YouTube, in timeline order, each credit once, with a note of lines that lack an author), and the **Credits and Rights** panel (Window menu; a tab beside Properties) with the policy checkboxes, the live check (click a finding to go there) and a Copy button. **Not yet:** an audit step at export, and importers that write records on their own (the Source Hub, F05, will).

**Needs:** F01 · **Unlocks:** F05, F20, F25.

**What it does.** Every asset knows where it came from and what you may do with it: source, author, URL, license, attribution text, content hash, and whether it is synthetic (with model, prompt, seed). One click produces a credits block. An audit warns when something conflicts with the project's license policy, for example a non-commercial clip in a monetized project.

**How it works.** A `provenance` record per media item, stored with the project (undoable; D1) and a project-level policy (`commercial`, `attribution_ok`, `synthetic_ok`). Commands `provenance.get|set|list`, `provenance.credits {format}`, `provenance.audit`. The Source Hub and every importer write records automatically; manual imports can be tagged ("my footage", "licensed"). UI: a license badge in the Project panel, a Credits and Rights dialog, an audit step at export.

**Done when.** Import a clip with a CC BY license, see its badge, get its credit line in the credits block, and get an audit warning if the policy forbids it.

**OTIO.** The OTIO export (`file.exportInterchange {format: "otio"}`) embeds each clip's provenance in `metadata.edith.provenance` and reads it back; see the F03.2 design above for why OTIO is not also the format shown to the agent.

**Notes.** Disclosure of realistic synthetic media is a YouTube setting; the ledger supplies the timecodes. The ledger only encodes what a source says; it is not legal advice.

### F05 Source Hub and Find Media panel · Must · L · yours · ✅

**Built so far (2026-10-16):** F05.1 `edith-sources`: one request and candidate shape, the Pexels (video, photos), Pixabay (video, photos) and **Freesound** (sound) adapters with each site's licence read into what it allows, simple keyword rules, and the HTTPS client (search, thumbnails, download to a file with a SHA-256); tested against recorded answers and a local mock server. F05.2 the engine side: `hub.providers|search|results|shortlist|acquire|cancel` (searches run at once in the background, one site failing does not hide the others, answers are kept for 24 hours, a chosen file is downloaded as a cancellable job, imported into `Sources/<site>` and its F04 record written for it). **Freesound limit:** a key can fetch a sound's high-quality preview (MP3), not the original file, which needs a Freesound sign-in; each such candidate says so. F05.3 the **Find Media panel** (Window ▸ Find Media, a tab beside Media Browser): a search box, Video / Photos / Sound toggles, each site's state, a grid of results with a picture, a licence dot (green free to use, amber credit needed, red check the terms), length, a shortlist star and an Add arrow; a Shortlist view; the credit links the sites ask for. Nothing is downloaded until you press Add. **Wikimedia Commons and the Internet Archive** (2026-10-16, no key needed): Commons through its search API (pictures at 1920 px, video and sound as they are, each file's licence and author read from its page; most ask for a credit), the Archive through its search (only items that state a licence, 763 matches for "fog" in film alone; the best mp4, mp3 or jpg is chosen when you add one). Licences are read by one shared reader (public domain, CC0, CC BY, BY-SA, BY-NC and so on; anything not understood claims nothing, so the audit calls it a risk). **Your own folders** (F05.5, 2026-10-16): the **Folders** pill in the panel adds a folder of your footage (recorded as your own work) or a licensed folder (recorded as licensed to you); they are searched by file and folder names, ranked by how many of your words match, alongside the sites and even offline. Files stay where they are and are imported in place when you add one; links are not followed, hidden folders are skipped, at most 16 folders and 30,000 files per folder are looked at. The list is kept beside the preferences (`edith-media-folders.json`). Commands `hub.folders|addFolder|removeFolder`. **Not yet:** NASA and the Library of Congress.

**Needs:** F02, F04 · **Unlocks:** F09 slots, F16, F17, F21.

**What it does.** Ask for media in plain words ("a lone figure walking through fog at dawn") or keywords and get licensed candidates from many free sources through one request shape. You or the assistant can ask; you shortlist and decide.

**How it works.**
- **One request shape** (sketch, not final code):
  ```
  MediaRequest   { intent, keywords?, kinds[video|image|audio], orientation, min_size,
                   duration{min,max}, look{mood, palette, avoid[]},
                   license{commercial, attribution_ok, synthetic_ok}, count, reference? }
  MediaCandidate { id, source, kind, thumb, preview, download?, w, h, duration, fps,
                   license{name, url, commercial_ok, attribution_text}, author, source_url,
                   synthetic, provenance, score }
  trait MediaSource { capabilities(); search(req); acquire(candidate) -> Job; generate(req) -> Job }
  ```
- **Adapters translate the request.** Pexels takes `query`, `orientation`, `size`; Pixabay takes `q` (at most 100 characters) and `min_width`/`min_height`; Wikimedia and the Internet Archive want era and place words. v1 turns `intent` into queries with rules; F21 adds model-based translation and ranking.
- **Free sources first**: Pexels, Pixabay, Wikimedia Commons, Internet Archive; then NASA, Library of Congress, Freesound (audio) and your own folders. No paid stock.
- **Good citizenship**: per-host rate limits and caching (F02), previews instead of originals until you shortlist, attribution and terms recorded per provider, no bulk downloading.
- **Acquire** runs as a background job, hashes the file, writes the ledger record (F04) and imports into a `Sources/<provider>` bin.
- **UI**: a *Find Media* panel beside the Media Browser with a request bar and chips, a candidate grid with hover-scrub, license badges, a shortlist, and drag to timeline.

**Steps.**
- F05.1 Types, trait and adapters for Pexels, Pixabay, Wikimedia Commons and the Internet Archive, with recorded fixtures.
- F05.2 Acquire, ledger and bin import.
- F05.3 Find Media panel.
- F05.4 Audio sources (Freesound, Internet Archive audio).
- F05.5 Local folder index. ✅ (built as a name search, not a content index: it matches words in file and folder names).

**Where it lives.** `edith-sources` (network behind a feature), engine commands `hub.providers|search|acquire|shortlist`, `panels/find_media.rs`.

**Done when.** A request returns candidates from at least three sources with correct license data; acquiring one writes the file, the ledger entry and the bin item; Offline mode and rate limits are honored; adapters pass against recorded responses.

**Notes.** Pexels, Pixabay and Freesound need free API keys. Terms differ (attribution display, caching, hosting); encode them in each adapter's metadata and re-check them (§9).

### F06 Narration upgrades · Must · M · ✅

**Needs:** F01 · **Unlocks:** F16, F19.

**What it does.** Makes the built-in narration usable for real videos: names and terms pronounced correctly, several takes to choose from, direction per sentence, and a saved "narrator voice" so every video has the same narrator.

**How it works.**
- **F06.1 Pronunciation dictionary.** ✅ **Built 2026-10-16** (three commits): `tts-text` finds the words the voice would only guess at (`unknown_words`; real names such as Kubrick and Reykjavik are already in CMUdict, so what gets flagged is genuinely unknown: made-up or rare names, brands, terms) and a name's possessive follows its fix; commands `tts.pronunciations|setPronunciation|removePronunciation|checkScript` and a `pronounce` trial parameter on `tts.preview|render|create|edit`; a project list (saved in the project, undoable) and a global list (`edith-pronunciations.json`, every project), the project's winning; a Pronunciation section in the Text to Speech panel (Check script, chips for the flagged words, a word and "say it like" row with Play, Save for this project / all projects, the saved list, Regenerate narration). Changing a word never re-renders old narration by itself: use Save / Regenerate. Checked with the real voices: invented names are flagged, fixing them changes the audio, and it is repeatable. The check needs the downloaded natural voices (they carry the dictionary). A lexicon (project and global) of word → respelling or phonemes, applied before the phonemizer. Words missing from the built-in dictionary (CMUdict) are flagged *before* rendering; "fix this word" lets you edit and audition. Lives in `crates/tts-text` (`normalize.rs`, `phonemize.rs`) and the narration settings.
- **F06.2 Takes.** ✅ **Built 2026-10-16** (three commits). Finding: the seed alone only changes the vocoder's noise, so seed-only takes sound the same; a take instead moves the voice's prosody style by seeded noise (strength 0.1, chosen by listening to samples), which shifts timing a few percent and moves stress while keeping the voice itself. Take 0 is the plain reading and is unchanged; takes 1 to 99 are other deliveries, each repeatable. `take` on `tts.preview|render|create|edit`, stored with the narration (older files read as 0), included in the render cache key; a Take stepper (− Standard +) on the Hear this voice row, natural voices only. A take is for the whole script of a narration; one narration per beat gives per-line choice. Not built: automatic generation of several takes at once, and a side-by-side take list. The vocoder's noise is seeded, so a seed can stand for a take. Generate N takes of a line, pick one, store its seed; re-rendering stays deterministic. The seed needs exposing in `tts.create` and `tts.edit`.
- **F06.3 Direction.** ✅ **Built 2026-10-16** (three commits). Direction markers in the script, in the style of `[pause 1s]`: `[pace 0.85]` (also `0.85x`, `85%`), `[pitch -2]` (semitones, also `+1.5`, `-2st`) and `[normal]`. They stick until changed, set rather than stack, are clamped to the voice's limits, and a malformed one stays ordinary text. Both voice families follow them (the natural voice speaks the script in stretches, pitch by the existing shifter per stretch); a script without markers takes the old path unchanged, and sentences before a marker are bit-for-bit as before. `tts.voices` lists the markers; the panel has a **Direction** menu beside Add pause that puts a marker in at the cursor. Place markers between sentences: one in the middle of a sentence splits it into two readings. Original text of the step follows.  Per-sentence pace, pitch and pauses (extends the `[pause 1s]` marker), as chunk-level settings because the model takes pace per call.
- **F06.4 Narrator voice** (saved voice preset, global across projects). ✅ **Built 2026-10-16** (two commits): a narrator voice is the voice, pace, vocal pitch and up to five track effects, kept in `edith-narrator-voices.json` beside the preferences; two built-ins, *Documentary* and *Clean* (a high-pass, EQ, compressor, de-esser and, for Documentary, a touch of room; chosen from the effects' ranges, not tuned by ear, so save your own); commands `tts.voicePresets|saveVoicePreset|deleteVoicePreset|applyVoicePreset|compareVoice` and `preset` on `tts.create`; applying copies the settings and effects onto the narration (one undo step) so a project does not depend on the file; *Hear plain voice* switches the track's effects off and on for comparison; a Narrator voice section in the Text to Speech panel. **First hands-on feedback (2026-10-16) and the fix (F06.5):** the built-in chains were too subtle to notice, and *Use* gave no sign it had applied. Measured on the natural voice, the old Documentary chain came out about 5 dB *quieter* than the plain voice (the compressor has no make-up gain), which makes any comparison sound worse; both chains now end with a make-up gain (the EQ's own high-pass replaced the separate one, to stay within five effects) and match the plain voice's level to within about 3%. Preview now plays the voice through the chosen narrator voice or the loaded narration's track effects (`tts.preview {through}`), the status line says what *Use* did, and the panel lists the effects on the narration's track. Documentary is also stronger (low shelf +3 dB, presence +3.5 dB, ratio 4:1, room 12%). **Not built:** the optional room-tone bed (needs a noise generator type in the media crate and a second clip kept in step), several chains per voice, sharing as files. Original text of the step follows.  A saved preset: base voice, pace, pitch and a track insert chain built from existing effects (EQ, compressor, de-esser, room reverb), plus an optional procedurally generated room-tone bed. Applied to the narration track, with A/B against the dry voice.

**Where it lives.** `crates/tts-text`, `crates/engine/src/narration.rs` and `Narration` in `filmcraft-project`, `panels/tts.rs`; presets through the mixer inserts.

**Done when.** A script with five proper nouns renders correctly after you fix them once; takes are repeatable; one click applies your narrator voice to a new narration track.

**Notes.** Natural voices are US English only and have no pitch input (pitch is a post shift), per `crates/tts/README.md`. Room-tone samples must be generated in code or openly licensed with attribution (AGENTS.md §1).

### F07 Look Pack: film texture and the A24 stack · Must · L · ✅

**Needs:** F01 (independent; can be pulled forward at any time).

**What it does.** The finishing layer that makes stock footage read as film: real film grain, halation, diffusion glow, gate weave, flicker and dust, anamorphic streaks, lens fringing, a one-click **A24 stack** with a single *film look* amount, and camera-move presets that bring stills to life.

**How it works.** Each item is a new video effect: definition in `crates/project/src/effect.rs`, CPU implementation in `crates/render/src/effects.rs`, a test in `crates/render/src/tests.rs` (see `docs/contributing.md` §6). Linear-light, premultiplied float, scaled by `px_scale`, and deterministic (seeded by frame index and a seed) so previews match exports. The GPU path follows later.
- F07.1 ✅ (2026-10-16) **Film Grain**: built from scratch rather than from the AV1 decoder's grain code (that works on integer decoder pictures in fixed blocks inside an L0 crate): seeded noise, softened to the grain size, scaled to a fixed strength so the amount means the same at any size, weighted by brightness (shadows, midtones, highlights sliders), optional colour grain, a stock choice (Fine, Medium, Heavy), a new pattern every frame (exact frame time plus a seed) so preview and export match; about 60 ms per 1080p frame on the CPU; folder **Film Look** in the Effects panel. Original text:  size, softness, brightness response, chroma, animated seed, stock-style presets with generic names. `crates/av1/src/grain.rs` already implements the AV1 film-grain synthesis (spec §7.18.3), an autoregressive, brightness-dependent model that is a strong, royalty-free basis. It is internal to the decoder, so reuse means exposing a small public API.
- F07.2, F07.3 ✅ (2026-10-16) **Halation** (strength, threshold and softness, radius, tint; a tight core plus a broad skirt, red-orange by default; only highlights spread, nothing changes far from them) and **Diffusion** (strength, radius, highlight bias between a soft overall blur and a highlight bloom, black lift in display space; adds light, never removes it); about 45 and 60 ms per 1080p frame. The first defaults were too subtle to notice on a sunset, so they were raised after looking at the picture. Original text: **Halation** (highlight threshold, radius, tint, strength).
- F07.3 **Diffusion** (soft bloom with lifted blacks).
- F07.4, F07.5, F07.6 ✅ (2026-10-16) **Film Gate** (weave: slow smooth wander up to the set pixels with edges repeated so no black borders; flicker: a new exposure every frame, up to half a stop; dust: single-frame specks; scratches: thin vertical lines that stay five frames), **Anamorphic Streak** (highlights smeared sideways only, tinted blue by default; the first default blew out a sunset, so the gain was cut after looking at it) and **Lens Fringing** (red pushed out, blue pulled in, zero at the centre and growing to the corners); 6, 44 and 12 ms per 1080p frame. Original text: **Film Gate** (weave, flicker, dust and scratches).
- F07.5 **Anamorphic streak**.
- F07.6 **Lens fringing** (radial).
- F07.7 ✅ (2026-10-16) **Cinema Look** (the roadmap's working title was "A24 stack"; A24 is a company, so the app says Cinema Look): one effect with a **Film Look** slider (0 to 100; 0 is exactly the original picture) that lays on lens fringing, diffusion, halation, gate flutter and grain, each also scaled by its own Mix control (0 to 200%), plus a **Letterbox** choice (None, 2.39:1, 1.85:1; bars are exact: 2.39:1 on 1920x1080 leaves 803 rows of picture; a frame wider than the ratio gets side bars). A built-in preset *Cinema Look* (Presets bin) applies it at 60 in one click and undoes in one step. Original text: **A24 stack** preset (through `presets.*`) with one *film look* macro slider and a letterbox option (2.39:1, 1.85:1).
- F07.8 ✅ (2026-10-16) **Camera moves**: `motion.cameraMove {clip?, move, amount?, seed?}` with `pushIn`, `pullOut`, `driftLeft`, `driftRight` and `sway`; keyframes on Motion position and scale from the clip's first frame to its last (placed in the clip's own media time, so trimmed and retimed clips work), eased, one undo step, replacing the clip's earlier position and scale keyframes. Amount is a share of the frame, and every move enlarges the picture enough that the frame edges stay covered (checked for every move and amount). Original text: **Camera moves for stills**: duration-aware keyframes (slow push-in, drift, handheld sway) from a command `motion.cameraMove`. Depth-map parallax is a later experiment.

**Done when.** Each effect has golden tests; the CPU path previews acceptably on the architect's Mac (a real-time GPU path may follow); the A24 stack turns a flat stock clip into a convincing film look in one click.

**Notes.** Implement from public descriptions only; do not copy GPL grain or halation code (AGENTS.md §2). Use generic names, not product trademarks. Today's catalog has only `Noise` (amount, color, grain size), and `Wonder Glow` can fake a bloom by hand.

---

### Phase 2: plan and understand

### F08 Planner core and Format Packs · Must · L · yours · ✅

**Needs:** F01 · **Unlocks:** F09, F11, F13, F16, F24.

**What it does.** One structure for designing a video before and while you cut it: `Video → Sections → Beats → Lines/Shots`. A beat holds its *purpose* ("what changes here?"), intensity (0–10), target duration, narration text, visual intent, sound note, status and a link to a timeline range. A **Format Pack** supplies the starting structure for a kind of channel.

**How it works.**
- Data in `filmcraft-project`, stored with the project and undoable (D1). Commands `planner.create|addSection|addBeat|updateBeat|move|delete|applyPack|link|buildSkeleton|export|import`.
- `buildSkeleton` creates a range marker per beat and offline placeholder clips for unsourced shots (`file.newOfflineFile`); `clip.automateToSequence` can place clips at those markers.
- A **Format Pack** is a data file: beat template (names, purposes, share of runtime, intensity curve), default look preset, title style, sound-plan template, analysis targets. Packs are data, never code, so any channel type can be added later (D4).
- Ships with two baseline packs, **Mystery** and **Explainer** (§6).
- The Notes panel (`notes.rs`, `project.setNotes`) is the precedent: project-stored text with undo and agent access.

**Built 2026-10-16 (four commits).** `edith-planner` (new crate: the data and limits, Format Packs as TOML with a JSON Schema and `docs/format-packs.md`, exact integer division of a video's length so the beats always add up to the tick); the Mystery and Explainer packs from §6; commands `planner.get|packs|create|applyPack|clear|update|addSection|updateSection|addBeat|updateBeat|addShot|updateShot|move|delete|link|buildSkeleton|export|import` (stored in `Project::ext["edith.planner"]`, one undo step each, hostile-parameter tests); `buildSkeleton` puts a range marker per beat at the exact ticks and offline placeholders for unsourced shots, and is repeatable (markers move, nothing doubles); your own packs go in a `format-packs` folder beside the preferences and a broken file is named, not fatal. Also built, ahead of F09: a minimal **Planner** panel (Window ▸ Planner) to create a plan, edit beats and build the skeleton. **Left for later:** the pack's look preset is shown but not applied automatically; the A/V script, beat lane and board are F09. **Done when.** Create a treatment from the Mystery pack, edit beats, build a skeleton on the timeline, save and reopen the project, and get the same planner back; commands have hostile-parameter tests.

**Notes.** The planner is the shared backbone: Source Hub slots, the coach's targets, the sound plan and the rough-cut assistant all read it.

### F09 Planner views · Must · L · yours · ✅

**Needs:** F08 (F05 for slots) · **Unlocks:** F16.

**What it does.** Different lenses on the same planner data, each in its own panel, all editing the same beats.

**Steps.**
- F09.1 ✅ (2026-10-16) **A/V script**: Window ▸ A/V Script, narration a sentence at a time on the left (each editable, one undo step on leaving the box), the shots for that sentence on the right (a picture when the shot has a source, else what it should show; "+ visual" adds one; clicking a shot's picture box uses the clip selected in the Project panel), each row's start time and a spoken-time estimate (words at the pack's words-per-minute, plus written pauses) that you click to jump the playhead, and Narrate to put a beat's narration in Text to Speech. Engine: `planner.lines`, `setLine`, `addLine`, `moveLine`, `setShotSource`, `refreshLinks`, and `line` on shots; the narration is always its sentences joined, and shots follow their sentence through inserts, deletes and moves (rewriting the narration by hand detaches a shot whose sentence no longer exists). Sentence splitting keeps `Dr.`, `e.g.`, initials, decimals and `[pause …]`/`[pace …]` markers intact and joins back to the exact text. Original text: **A/V script**: two columns (narration | visuals), one row per sentence, with slot thumbnails and timing; clicking a row seeks the timeline. The expected workhorse.
- F09.2 ✅ (2026-10-16) **Beat lane**: a strip above the timeline in the default Editing layout, on the timeline's own scale (the same time-to-pixel formula, tested), with the beats as ranges coloured by act, the planned intensity as a line over them, and for beats laid on the timeline a thin bar of planned against actual length (amber when more than 5% apart); click a beat to jump the playhead there and select it. It reads the plan only. With no plan it says so. Original text: **Beat lane** above the timeline: beats as colored ranges with the intensity curve, planned versus actual duration.
- F09.3 ✅ (2026-10-17) **Beat board**: Window ▸ Beat Board (a tab beside A/V Script; float it for room). One column per act, one card per beat (name, length, status, intensity bar, the start of its purpose); drag a card within its column or into another and the plan changes order, nothing else: the clips and markers you have put on the timeline stay where they are. When the plan's order no longer matches the markers a line says so and **Rebuild markers** moves them to follow it (`planner.refreshLinks`; one undo step). Act titles are editable, **Add beat** per column, click selects the beat for the script and the lane. Dropping a card where it already is makes no edit. Original text: **Beat board**: cards grouped by act, drag to reorder; reordering can reorder the sequence sections. **Left for later:** reordering the clips themselves (a deliberate choice: the board never moves clips), the board scrolling by itself while you drag near its edge.
- F09.4 ✅ (2026-10-17) **Storyboard grid**: Window ▸ Storyboard (a tab beside Beat Board; float it for room). One tile per planned shot in plan order, grouped under its beat: a frame of the shot's source, or, while it has none, what it should show written on a plain box. Under the picture you can edit the kind of shot, the camera move, the length (seconds, `3` or `3.5`) and the written intent, each one undo step; click the picture to use the clip selected in the Project panel, **No picture** takes it off, ✕ deletes the shot, **Add shot** adds one to the beat. A length that is not a number is refused with a message and the shot keeps its length. Original text: **Storyboard grid**: a thumbnail per shot with shot type, camera move and duration; unsourced shots show the visual intent as text. **Left for later:** the picture is the first frame of the source (not a frame chosen by the shot's own in point), and dragging tiles to reorder shots (the A/V Script and the engine's `planner.move` already reorder shots).

F12 adds the *look bible* view and F13 the *sound plan* view.

**Done when.** Editing a line in the A/V script updates the beat board, the lane and the narration in the TTS panel; every widget has automation ids and state in `state.rs`.

### F10 Analysis core · Must · L · 🟡

**Needs:** F01 · **Unlocks:** F11, F12, F13, F18, F21, F23.

**What it does.** Measures footage and audio so later features can talk in numbers.

**How it works.** `edith-analysis` is a set of pure functions over frames and audio buffers. The engine runs them as background jobs and caches results by content hash plus algorithm version (rebuildable, never in the project). Output is a versioned `AnalysisReport`.
- **Per shot**: mean luma, contrast, saturation, palette (5 colors in Lab), color temperature, sharpness, grain estimate, motion energy and a rough camera-move class, letterbox and aspect.
- **Per audio segment**: loudness (BS.1770, already in `audio-dsp`), loudness range, true peak, silence share, spectral balance, a speech/music/ambience guess, music beat grid.
- **Sequence level** (from the model, no decoding): shot-length distribution, cut density, J/L-cut share, transition use, narration words per minute, track occupancy.
- Reuses what exists: `SceneDetector` for cuts, the optical-flow code behind Morph Cut, the scopes maths.
- Commands `analysis.run|get|list|clear`.

**Steps.**
- F10.1 ✅ (2026-10-17) **Picture measures**: the new `edith-analysis` crate (pure functions, no decoding; layer 3, checks for wasm). `analyze_rgba(width, height, rgba)` gives `FrameStats`: mean luma and its 5th and 95th percentile, contrast (the span between them), saturation, a palette of up to five colours in Lab with the share each covers (k-means from well-spread starting points, so the same picture always gives the same palette), colour temperature in kelvin of the average colour, sharpness, grain, black bars on each side and the aspect of what is inside them. Every number is measured on a reduced copy or a fixed crop, so it does not depend on how many pixels the picture has. Sizes are checked before anything is allocated; no input panics. Tested on synthetic pictures with known answers (the sRGB primaries in Lab against published values, D65 white at 6504 K, bars of a known size, added noise of a known level).
- F10.2 ✅ (2026-10-17) **Audio measures**: `analyze_audio(sample_rate, channels, samples)` gives `AudioStats`: EBU R128 integrated loudness, loudness range, true peak and sample peak (the existing `audio-dsp` meter, not a copy), the share of 50 ms windows quieter than −60 dBFS, the spectral balance (energy in a low, mid and high band, and the centroid), a rough guess of speech, music, ambience or silence with the score of each (pauses, a 3–6 Hz syllable rhythm, noise-likeness, pulse strength), and a constant-tempo beat grid (tempo in BPM and where the beats fall) when there is a steady pulse. Non-finite samples count as silence and values are limited before measuring; format and size limits are checked first. Tested on tones of known level (a 997 Hz sine at −20 dBFS reads −23.0 LUFS, as the standard says), steps in level, silence, click tracks at 75, 90, 100, 120 and 150 BPM (tempo within 1.5 BPM, first beat within 40 ms), and speech-, music- and ambience-like synthetic audio. The guess is a heuristic and is documented as one.
- F10.3 ✅ (2026-10-17) **Motion and the camera**: `analyze_motion(a, b, seconds)` compares two pictures of the same size: how much changed (`energy`), and, from blocks matched between them and fitted with a shift-plus-zoom model of the camera, how the picture moved (`dx`, `dy`, `zoom`, each per second as a fraction of the width, height or scale, so resolution and the gap between the pictures do not matter) and how much of the picture moves on its own rather than with the camera (`residual`). `summarize(pairs)` adds a shot's pairs up to a rough camera class (static, pan, tilt, zoom in, zoom out, handheld, mixed, or unknown when too few blocks could be matched). Own block matching, not the render crate's optical flow: this crate stays pure and small, and the roadmap's wish to reuse that code is left for when a finer measure is needed. Tested with textures shifted and zoomed by known amounts (recovered within about 0.01 per second), an object moving over a still camera, a cut, flat pictures and noise.
- F10.4 ✅ (2026-10-17) **The sequence's own numbers**: `analyze_sequence(input)` takes the timeline as plain spans (clips in ticks on numbered tracks with their link, transitions, narration with word counts) and gives `SequenceStats` with no decoding: the shots as the viewer sees them (the clip on the highest video track; a title, graphic or adjustment layer is marked `isShot: false` and is not a shot; a gap is not a cut), their length distribution (mean, median, 10th and 90th percentile, shortest, longest, a histogram under 1, 2, 4, 8, 16 s and longer), cuts and cut density per minute with a cuts-over-time series (30 s windows), J and L cuts (cuts where the linked sound does not change with the picture, to within two frames; only cuts with linked sound on both sides count), transitions by kind and as a share of the joints, narration pace (words over minutes spoken) and the share of the timeline with narration, and how much of the timeline each track has something on. Overlaps count once; hostile spans (negative, zero or overflowing) are cut to fit; more than 500,000 clips is refused. Tested on timelines worked out by hand and on 120,000 clips.
- F10.5 **`analysis.run|get|list|clear` as background jobs with a cache**: next (the engine fills these inputs from the project and decodes frames and audio).

**Done when.** Synthetic fixtures with known properties produce the expected numbers; results are cached and invalidated correctly; hostile media never panics.

**Notes.** The shared foundation of the coach, reference analysis, consistency checker and sound design: build it once.

### F11 Editing coach v1 · Must · M · ⬜

**Needs:** F10 (sequence-level metrics need no decoding, so a first cut can ship early).

**What it does.** Shows your edit in numbers and explains them in plain words, so a newcomer learns the craft while editing: average shot length per section versus plan, a shot-length histogram, cut density over time, narration pace, silence share, loudness range, how many cuts overlap sound (J/L cuts), very short shots next to long ones, flash frames.

**How it works.** A Coach panel of *findings*: severity, a clickable timecode, a one-paragraph explanation, and an optional fix proposal once F14 exists. Thresholds come from the format pack, later from a reference fingerprint (F12). Numbers are deterministic code; "explain" asks the assistant (F03) to put a finding in plain words and name the editing idea behind it (match cut, J/L cut, rhythm).

**Done when.** A sequence with a deliberately jarring 0.8 s shot between two 6 s shots produces a finding at that timecode with a correct explanation.

### F12 ★ Reference analysis and look bible · Must · L · yours · ⬜

**Needs:** F10 (F08 for targets).

**What it does.** Turn videos you admire into a measurable **style fingerprint** and compare your own edit to it. Learn what makes a style work, in numbers, then set it as your target.

**How it works.**
- **Reference reel**: add local video files you have the right to use. They are analyzed in the background (F10). Only numbers are stored, never content.
- **Fingerprint**: shot-length distribution and rhythm, cut density curve, hard-cut versus dissolve share, loudness profile and range, silence, dialogue and music share, approximate audio-versus-picture cut offsets, aspect and letterbox, per-shot contrast, saturation and palette timeline, grain level, motion energy.
- **Compare**: your sequence's numbers overlaid on the fingerprint. The coach (F11) uses it as targets; the planner (F08) can suggest beat durations from it.
- **Look bible**: reference stills, the palette, and your grain, aspect and LUT choices on one board. "Match to look" uses `lumetri.applyMatch` against a reference frame.
- **Rhythm map**: a strip showing cuts and a rough intensity proxy (cut density, loudness, motion) beside the planner's intended intensity curve. Experimental.
- **Mentor**: the assistant explains what the numbers suggest ("long shots, a third of the film silent: contemplative pacing").

**Done when.** Analyze a public-domain feature (for example the films the README's screenshots use), see a fingerprint, and watch the coach adopt it as targets.

**Notes.** The app never downloads from streaming sites (terms of service and copyright). References are files you supply, public-domain or openly licensed material from the Source Hub, or your own earlier exports. Fingerprints can be exported as JSON and shared without sharing any footage (D9).

### F13 ★ Sound map and spotting sheet · Must · M · yours · ⬜

**Needs:** F10, F08.

**What it does.** Makes the sound of the video visible and plannable; this is the first half of the sound design assistant. A **sound map** shows lanes for narration, music, ambience and effects with loudness over time. A **spotting sheet** (the film-sound term for planning cues) lists, per beat, the ambience bed, the music cue (mood, in, out), effects, and deliberate silences.

**How it works.**
- Audio analysis from F10 (loudness, silence, speech/music guess, beat grid) drives the map. The existing Essential Sound types (Dialogue, Music, SFX, Ambience) tag clips into lanes.
- The sound plan lives in the planner (F08) per beat. Cues become typed markers on the timeline. Detectors flag *dead air* and *abrupt room-tone changes*.
- The sound plan view completes the planner views (F09).

**Done when.** A sequence with narration, one music bed and no ambience shows a map with the gap; the spotting sheet round-trips through the project and exports as text.

**Notes.** Instincts to encode as guidance, not rules: continuous room tone, restraint, silence as a tool, diegetic sound before stock whooshes.

---

### Phase 3: human-in-the-loop automation

### F14 Proposals engine · Must · L · ⬜

**Needs:** F01 · **Unlocks:** F15, F16, F17, F18, F22, F23.

**What it does.** The mechanism that keeps the human in charge (§4.4). Anything an AI or analysis tool wants to change arrives as a proposal you preview and accept or reject.

**Steps.**
- F14.1 **Grouped undo**: `Session::group(label, …)` collapses N commands into one undo step. Precedent: `history.merge_key` coalescing in `crates/engine/src/lib.rs`. MCP's `command_batch` runs each step as its own undo step today and can adopt it.
- F14.2 **Fork and diff**: `Session::fork()` clones the `Arc<Project>`, applies the steps, and a diff names what changed.
- F14.3 **Preview**: ghost clips on the timeline and an A/B toggle in the Program monitor that renders from the fork.
- F14.4 **Suggestions tray** panel: accept all, accept part, reject, edit; rationale and alternatives; stale detection.
- F14.5 **Scope locks and profiles**: steps outside the selected clips, range or beat are refused; Read-only (default), Propose, Direct.

**Commands.** `proposal.create|preview|diff|accept|reject|list`.

**Done when.** A hand-written proposal of five commands previews, shows a correct diff and accepts as one undo step labeled `AI: …`; a scope-violating step is refused.

### F15 Assistant v2: edit tools via proposals · Must · M · yours · ⬜

**Needs:** F03, F14 (F08 for planner tools).

**What it does.** The assistant can now suggest changes: tighten this section, rearrange these beats, suggest B-roll for this line, critique this beat, explain this cut. Every suggestion is a proposal (F14); it never edits directly.

**How it works.** Edit tools build proposals. Planner tools (`planner.*`) let it read and propose beat changes. Source Hub tools (`hub.search`) let it request candidates, with downloads always asking first. Transcripts and clip text stay labeled as untrusted data. Tool-call cards show the exact commands before you accept.

**Done when.** "Tighten the middle section by 20 seconds" returns a proposal with a correct diff and an explanation; rejecting it changes nothing.

### F16 VO-first rough cut · Must · L · ⬜

**Needs:** F14, F05, F08, F06 (and the existing transcripts).

**What it does.** The faceless workflow in one step. From the narration and its word timings it proposes a first cut: shot changes on sentence boundaries, a B-roll candidate per slot, audio leading picture slightly at beat changes, ambience under, music ducked.

**How it works.** Narration audio plus word times (Whisper, or the known script) → sentences → planner beats and slots. Per sentence: take your shortlisted pick, else the top candidate marked *tentative*; duration is the sentence plus handles, within the format pack's shot-length range; long sentences split into two shots; stills get a camera move (F07.8); unsourced slots become offline placeholders named after the visual intent. The output is one proposal you can accept per beat.

**Done when.** A two-minute narration with a filled planner yields a proposal that places every shot, previews against the empty timeline, and accepts as one undo step.

**Notes.** It assists; you choose the footage. Tentative picks stay flagged until you confirm them.

### F17 ★ Sound design suggestions · Must · L · yours · ⬜

**Needs:** F13, F14, F05 (audio sources).

**What it does.** The second half of the sound design assistant: concrete, explained suggestions delivered as proposals.
- **Ambience and room tone** under stretches that have none (from your library or Source Hub audio), faded and leveled from the sound plan.
- **Ducking** music under narration (`essentialSound.generateDucking`) and a **loudness plan** (`essentialSound.autoMatch`) to your target.
- **Cut sweeteners**: a short ambience crossfade or audio lead (J/L cut) at hard cuts between very different scenes, in place of whooshes; optional effects at hit points you mark.
- **Silence**: a held gap with room tone before a beat the plan marks "silence before".
- **Beat-matched cuts**: nudge picture cuts to the nearest music beat within a few frames, approved cut by cut.
- **Reverb match**: put effects and ambience in the narration's room with the convolution or studio reverb.
- **Restraint guardrails**: the coach warns when effect density or music coverage exceeds the format pack's taste thresholds.

Each suggestion says why in plain words (the mentor idea).

**Later.** Stem separation (a local model) to duck only the vocal band of a music track.

**Done when.** Running it on narration over silent B-roll proposes a room-tone bed and ducking, previews them, and accepts as one undo step.

### F18 Consistency checker and shot matcher · Must · M · ⬜

**Needs:** F10, F14.

**What it does.** Flags neighboring shots that do not belong together (exposure, color, saturation, grain, resolution, frame rate, aspect, upscaled low-resolution) and offers fixes as proposals: `lumetri.applyMatch` to a reference frame, scale or crop, grain to hide upscaling, or a LUT. It also compares shots with the look bible (F12).

**Why.** The biggest difference between a stock slideshow and a film is consistency.

**Done when.** A cut between a warm daylight clip and a cold night clip is flagged with a measured difference and a one-click match proposal.

**Notes.** Heuristic watermark detection for stock previews is a possible later addition (research).

---

### Phase 4: voices

### F19 Voice Lab (custom voices) · Must · L · yours · ⬜

**Needs:** F06.

**What it does.** Design a voice that is yours: pick base voices, blend them, audition the same sentence, and save the result as your default narrator.

**How it works.**
- **Blending (spike first).** Each Kokoro voice is a `[510, 1, 256]` float table of style vectors (`crates/tts/src/kokoro/weights.rs`). A weighted average of two or three tables should yield a new voice. Spike: blend 50/50, listen, and measure intelligibility with the Whisper word-error-rate method already described in `crates/tts/README.md`.
- **Voice Lab panel**: choose bases, slide the mix, audition a test paragraph, save a *custom voice pack* in the data directory; it appears in `tts.voices`.
- **Voice identity card**: name, description, sample, and the chain preset from F06.4.
- **Other engines (later, spike)**: implement the `Voice` trait (script in, 24 kHz audio out) for other local models. Each must allow commercial use, run free, and never clone a real person without their consent.

**Done when.** A saved blended voice narrates a script with a word-error rate close to the base voices.

**Notes.** Custom voices are synthetic: mark them in the ledger (F04). English (US) only for now. Latent "age" or "brightness" sliders derived from the voice tables are an experiment, not a promise.

---

### Phase 5: ship and grow

### F20 YouTube prep · Must · M · ⬜

**Needs:** F04, F13.

**What it does.** The delivery checklist in one place: a loudness and true-peak check against a target (default around -14 LUFS integrated, configurable; verify the current guidance), a chapter list from markers (`markers.addChapter`) formatted for the description, the credits block from the ledger, a disclosure checklist listing synthetic realistic scenes and voices with timecodes, a thumbnail frame picker (`file.exportFrame`), and assistant-drafted title and description for you to edit.

**Done when.** One dialog reports loudness, chapters, credits and disclosure items for a finished sequence and exports the text blocks.

**Notes.** Uploading to YouTube from the app is parked (§8).

### F21 Smart search and ranking · Nice · L · ⬜

**Needs:** F05, F10, F03.

**What it does.** Better search for B-roll and for your own footage. Model-based translation turns one intent into per-source queries; a local embedding model (a CLIP-style image-text model on candle, CPU, free) indexes your footage and downloaded candidates and re-ranks by text-to-image similarity and by look similarity to your look bible; "more like this"; search your own library by description.

**Done when.** "Find in my footage: empty road at dusk" returns relevant clips from a local folder without any network.

**Notes.** Spike for model choice (license, size, CPU speed). Embedding frames is heavy: index at a low frame rate, in the background.

### F22 Versions and A/B compare · Nice · M · ⬜

**Needs:** F14.

**What it does.** Named snapshots of a sequence, a compare view (toggle or side by side in the Program monitor), a diff summary, and restore by region. Built on the fork from F14.

### F23 Shorts repurposing · Nice · M · ⬜

**Needs:** F14, F10.

**What it does.** Vertical versions for Shorts: Auto Reframe plus highlight proposals (segments chosen from speech density and the intensity proxy, confirmed by you), burned-in captions, vertical safe areas, a 9:16 export preset.

### F24 Format Pack authoring and more packs · Nice · M · ⬜

**Needs:** F08, F07, F12.

**What it does.** Create a pack from the current project ("save as pack": beats and durations, look preset, title style, sound plan, analysis targets), import and export packs as files, validate them, and start a new project from a pack. Ship more packs as data: documentary, true crime, horror, essay, review, tutorial, game lore. This is the "extensible to any channel's needs" promise made concrete.

### F25 Generative sources (opt-in) · Nice · L · ⬜

**Needs:** F02, F04, F05.

**What it does.** Optional `generate()` providers in the Source Hub for stills and clips. Off by default, never required.

**How it works.** Spike first for free paths: a local ComfyUI server on localhost (free, needs a capable GPU), any free tiers that exist when we get here (they change fast), and ArtCraft, the same team's open-source AI image and video app (check whether it exposes a local API, CLI or MCP, and its license, before depending on it). Everything generated is flagged synthetic in the ledger with model, prompt and seed. Per-request cost estimates and a hard monthly cap that defaults to zero. A cheap pipeline: generate stills, then animate with camera moves (F07.8).

**Notes.** Realistic depictions of real people or events need the disclosure treatment (§9). Parked until a free path is confirmed.

### F26 Multilingual · Parked · XL · ⬜

**Needs:** F06, F19.

**What it does.** Caption translation, other-language narration and dubs. Parked: US English only for now, and the natural-voice text front end is English-only.

---

## 6. Format Pack baselines: Mystery and Explainer

Starting points for the two baseline packs. Percentages are shares of runtime, intensity is 0–10 (the intended energy of the beat). Edit freely; the real values should be tuned against reference fingerprints (F12). Packs are data (D4).

### Mystery (typically 10–20 minutes)

| Beat | Runtime | Intensity | Purpose |
|---|---|---|---|
| Cold open | 0–4% | 8 | One striking image or fact; pose the question without context |
| The question | 4–8% | 6 | What we are investigating and why it matters |
| The scene | 8–20% | 4 | People, place, timeline: establish the normal |
| Evidence trail (3–5 clue beats) | 20–55% | 5 → 7 | Each clue: reveal, implication, doubt |
| False resolution | 55–65% | 6 | The easy explanation, and why it almost works |
| The turn | 65–75% | 9 | New information that breaks the easy explanation |
| Best explanation | 75–90% | 8 → 6 | A reasoned answer: what it explains, what it costs |
| What we still don't know | 90–97% | 4 | Honest ambiguity |
| Last image | 97–100% | 3 | A quiet close; optional end card |

Defaults to tune later: slower shots, more silence, cooler and darker grade, sparse sound (drones, room tone, few stingers).

### Explainer (typically 6–12 minutes)

| Beat | Runtime | Intensity | Purpose |
|---|---|---|---|
| Hook | 0–5% | 7 | A concrete surprise or question the viewer wants answered |
| Why you should care | 5–10% | 5 | The stakes in the viewer's life |
| The mental model | 10–25% | 4 | The simplest accurate picture, with one visual metaphor |
| Step by step (2–4 concept beats) | 25–60% | 4 → 5 | Claim, example, visual, check |
| The misconception | 60–70% | 7 | Where intuition fails |
| Worked example | 70–85% | 6 | Apply the model end to end |
| Recap and implication | 85–95% | 5 | What changes now |
| Open question | 95–100% | 3 | Leave a thread |

Defaults to tune later: brisker shots, clearer and brighter visuals, more explanatory graphics, lighter sound bed.

## 7. Open decisions for the architect

Each has a recommendation, so work can start before you answer; a decision here overrides it.

| # | Decision | Recommendation |
|---|---|---|
| D1 | Where undoable custom project data lives: per-feature schema bumps, one extension map inside `Project`, or sidecar files | One extension map inside `Project` (a single upstream hook, no version churn, undo and crash recovery for free). Non-undoable UI state goes beside the project like `view`. Sidecars lose undo. |
| D2 | Order of the AI connection lanes (§4.3) | **Decided 2026-10-15: Claude through your own API key first** (lane B); then OpenRouter sign-in, other keys and local servers; then ChatGPT and Google sign-in once access is confirmed. Lane D (ACP) is parked. |
| D3 | Default free model for the assistant: Gemini free tier, OpenRouter free models, or local | Decide by testing the mentor prompts on each before F03.2 ships. |
| D4 | Format Pack file format and location | TOML for hand-editing plus a JSON Schema; a folder in the data directory; an explicit version field. |
| D5 | Keychain crate (license, maintenance, macOS behavior) | **Decided in F02 (2026-10-15):** `keyring-core` plus one store crate per OS; see F02. |
| D6 | Point `CLAUDE.md` at this file (one line in an upstream-owned file) | Yes, so every agent session starts from it. **Decided 2026-10-12: yes (done in F01.3).** |
| D7 | Crate naming | **Decided 2026-10-15:** every crate the fork creates is named `edith-*` (`edith-net`, `edith-llm`, `edith-agent`, `edith-ai`, later `edith-sources`, `edith-analysis`). Directories keep the repo's short names (`crates/net`, `crates/llm`, …) and nothing moves. Upstream's `filmcraft-*` crates keep their names until the big rename (D12). |
| D13 | Default model for new assistant threads | **Decided 2026-10-15: a user setting**, not a constant in the agent. The shipped default is Haiku 5.5 (about 20 times cheaper than Sonnet 5.5 for plain tokens, about 6 times with heavily cached input), with Sonnet 5.5 one click away for critique; revisit with the measured cost from F03.1. The setting lives in a fork-owned settings file (not upstream's preferences) and is edited in the assistant panel. |
| D12 | Renaming the product to Edith | **Decided 2026-10-15, incrementally:** new things are Edith from the start; code a stage touches that still says FilmCraft is changed to Edith when that is cheap, and left for the big rename when it is not (crate names, `.fcproj`, the data directory, anything that fans out across files). See [fork.md](fork.md#naming-edith). |
| D8 | Default profile for new assistant threads | Read-only for a first thread; Propose once edit tools exist. |
| D9 | Reference analysis stance and UI wording | Files you supply only; no downloaders; say so in the UI. |
| D10 | Telemetry | None. Confirm. |
| D11 | TLS provider: `rustls-rustcrypto` is alpha and "not for production" | **Accepted in F02 (2026-10-15):** keep it behind `crates/net/src/tls.rs`; swap when a pure-Rust, permissively licensed alternative is production-ready. |

## 8. Parked and not planned

- Auto-upload to YouTube (Data API, OAuth and audit requirements).
- Downloading from streaming or video sites (terms of service, copyright).
- Paid stock, and paid generation as a default.
- Cloning the voice of a real person.
- Autonomous "make me a video" agents (against principle 1).
- Cloud rendering, collaboration, plugin hosting, mobile.
- AI features in the web build (desktop only, like speech today).

## 9. External facts to re-verify

These shaped the plan and are volatile. Most were checked on 2026-10-11 through secondary sources; re-verify against the official page before building the feature they affect.

| Claim | Affects | Sources |
|---|---|---|
| Claude subscription sign-in is not available to third-party apps (blocked since January 2026, written into the consumer terms in February); embedding the unmodified Claude Code and letting the user sign in there is allowed; billing rules for programmatic use changed several times and one change was paused in June | F03 lanes B and D | [Claude Code: legal and compliance](https://code.claude.com/docs/en/legal-and-compliance), [Claude Code: authentication](https://code.claude.com/docs/en/authentication), [VentureBeat](https://venturebeat.com/technology/anthropic-cracks-down-on-unauthorized-claude-usage-by-third-party-harnesses), [The Register](https://www.theregister.com/2026/02/20/anthropic_clarifies_ban_third_party_claude_access/) |
| Zed keeps provider keys in the OS keychain, has its own ChatGPT and Copilot sign-in, and runs Claude subscriptions through an external agent (ACP) | F03 design | [Zed: use an existing subscription](https://zed.dev/docs/ai/use-an-existing-subscription.md), [GitHub changelog](https://github.blog/changelog/2026-02-19-github-copilot-support-in-zed-generally-available/) |
| (Parked lane D) ACP: JSON-RPC over stdio; Zed's official Claude adapter `@zed-industries/claude-agent-acp` bundles the Claude Code CLI, signs in separately ("Log in with Claude Code" for Pro or Max, or an API key), needs Node.js | if lane D is revived | [Zed: external agents](https://zed.dev/docs/ai/external-agents), [Claude Code via ACP](https://zed.dev/blog/claude-code-via-acp) |
| Timeline-Bench: best editing agent resolved 15 of 56 tasks; 73% of failures were craft only | F03.0, F10 to F13 | [arXiv 2609.35143](https://arxiv.org/abs/2609.35143) |
| LAVE: LLM agent with direct manipulation for video editing | F03.0 | [arXiv 2402.10294](https://arxiv.org/abs/2402.10294) |
| Anthropic Messages API: headers, streaming events, tool use, adaptive thinking, models list, caching, refusals | F03.1 | [Anthropic API docs](https://platform.claude.com/docs/en/api/overview) |
| OpenRouter bills its own credits; a Claude subscription does not apply there | F03.3 | [OpenRouter: Claude Code guide](https://openrouter.ai/blog/tutorials/claude-code-openrouter/) |
| OpenRouter sign-in: `localhost` on any port, `state` not returned on denial, codes last 10 minutes; chat and models endpoints | F03.3 design | [OAuth PKCE](https://openrouter.ai/docs/use-cases/oauth-pkce), [API overview](https://openrouter.ai/docs/api-reference/overview), [models](https://openrouter.ai/docs/api-reference/models/get-models) |
| OpenRouter streaming: `data:` chunks, `: OPENROUTER PROCESSING` comments, usage in a last chunk with an empty delta that repeats `finish_reason`, mid-stream errors as a top-level `error` object with `finish_reason: "error"` on a 200 stream | F03.3a | [Streaming](https://openrouter.ai/docs/api-reference/streaming) (checked 2026-10-15) |
| OpenRouter model list: `pricing` values are strings in dollars per token; `architecture.input_modalities`/`output_modalities`; `supported_parameters` (`tools`, `parallel_tool_calls`, `reasoning`, `reasoning_effort`); `top_provider.max_completion_tokens`; the spec lists the list as needing a key (unconfirmed live) | F03.3a | [Models](https://openrouter.ai/docs/api-reference/models/get-models) (checked 2026-10-15) |
| OpenRouter tools: assistant `tool_calls` with `arguments` as a JSON string, results as `role: "tool"` with `tool_call_id`; `finish_reason: "tool_calls"`. The streaming delta shape is not documented there; Edith follows the usual `index`/`id`/`function` fragments and tolerates servers that differ | F03.3a | [Tool calling](https://openrouter.ai/docs/guides/features/tool-calling) |
| OpenRouter reasoning: `reasoning: {effort}`, streamed in `delta.reasoning_details` (and a plain `reasoning` string on some models); keeping it with tool calls helps some models. Edith shows it and does not replay it yet (live test needed first) | F03.3a | [Reasoning tokens](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens) |
| Ollama's `/v1/chat/completions`: streaming, `stream_options.include_usage`, tools, vision with base64 images only (no image URLs), no `tool_choice`; base URL `http://localhost:11434/v1`, a key is ignored locally | F03.3b | [Ollama OpenAI compatibility](https://docs.ollama.com/api/openai-compatibility) |
| LM Studio serves `/v1/models`, `/v1/chat/completions` and more on port 1234 (examples use it); the capability fields of its native model list still need reading on the REST pages | F03.3b, F03.3e | [LM Studio OpenAI endpoints](https://lmstudio.ai/docs/app/api/endpoints/openai) |
| OpenRouter's PKCE flow returns a user-controlled key, needs no registration, and its code is valid for ten minutes | F03.1 lane A | [OpenRouter OAuth PKCE](https://openrouter.ai/docs/use-cases/oauth-pkce) |
| "Sign in with ChatGPT" launched in early October 2026 for partner apps (scope `chatgpt.tokens.use.direct`); access for a personal app is unconfirmed | F03.5 | [IT Brief](https://itbrief.co.uk/story/openai-launches-chatgpt-sign-in-for-16-partner-tools), [WorkOS](https://workos.com/blog/sign-in-with-chatgpt-plan-usage-scope) |
| Gemini: an API key is the simple route; OAuth needs a Desktop OAuth client from Google Cloud; check the free-tier limits | F03.5 | [Gemini API OAuth quickstart](https://ai.google.dev/gemini-api/docs/oauth) |
| Pexels and Pixabay API limits and terms (Pexels defaults to 200 requests an hour; Pixabay 100 per minute; both ask for credit when displaying results) | F05 | [Pexels API requirements](https://help.pexels.com/hc/en-us/articles/900005852323), [Pixabay API docs](https://pixabay.com/api/docs/) |
| YouTube monetization rules target mass-produced, repetitive "inauthentic content", and realistic altered or synthetic content needs a disclosure | F04, F20, F25 | [Flocker](https://flocker.tv/posts/youtube-inauthentic-content-ai-enforcement/), [AIR Media-Tech](https://air.io/en/youtube-glossary/what-is-youtubes-ai-content-disclosure-policy) |
| ArtCraft is the same team's open-source AI image and video app that aggregates third-party generators; its license is unclear | F25 | [gittrend](https://gittrend.io/repo/storytold/artcraft), [Brightcoding](https://www.blog.brightcoding.dev/2026/10/08/storytoldartcraft-open-source-ide-for-controlled-ai-image-video) |
