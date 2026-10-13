# Upcoming features

> **Last reviewed:** 2026-10-12 (against upstream 0.7.2) · **Last updated:** 2026-10-13 · **Change:** F01 done (speech on by default, `upstream` remote, fork rules in fork.md, `CLAUDE.md` pointer); earlier: new file (direction for the creator layer: assistant, sourcing, planning, analysis, voices, look)

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
2. **Write and voice.** Write the narration per beat in the A/V script. Fix names in the pronunciation dictionary, audition takes, and run the voice through your channel voice so it sounds like *your* narrator.
3. **Source.** Each narration line owns a *slot*. The Source Hub fills slots in the background with licensed B-roll from free sources. You shortlist and choose. Nothing is accepted for you.
4. **Rough cut.** Ask for a rough cut. You get a *proposal*: cuts on sentence boundaries, B-roll placed, ambience under, music ducked. Preview it against your current cut, then accept all, part or none, as one undo step.
5. **Craft.** Add the film finish (grain, halation, diffusion), run the consistency checker, read the sound map and spotting sheet. The coach and your reference fingerprint show in plain numbers where the pacing drifts from the style you are aiming for.
6. **Deliver.** Loudness check, chapters, credits block, disclosure checklist, thumbnail frame.

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
| F06 | Narration upgrades: pronunciation, takes, channel voice | Must | M | F01 |
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
| `filmcraft-net` (`crates/net`) | L2 | HTTP client, cache, rate limits, Offline mode, secret store, OAuth/PKCE | F02 and every networked feature |
| `filmcraft-ai` (`crates/ai`) | L2 | Chat provider clients (Anthropic, OpenAI-compatible, Gemini, OpenRouter), streaming, message and tool types, ACP client | F03, F15, F21 |
| `filmcraft-sources` (`crates/sources`) | L2 | `MediaSource` trait, provider adapters, license normalization | F05, F25 |
| `filmcraft-analysis` (`crates/analysis`) | L2 | Pure metrics over frames, audio and sequences | F10 onward |
| Planner, provenance, lexicon, proposal-log types | L1 in `filmcraft-project` | Project data (undoable) | F04, F06, F08, F14 |
| Engine glue (`assistant.rs`, `hub.rs`, `planner.rs`, `proposals.rs`, …) | L4 in `crates/engine/src/` | Commands, jobs, tool loop | all |
| Panels | L5 in `crates/ui-egui/src/panels/` | UI, listed in `PanelKind` (`dock.rs`) | all |

Rules: add every new crate to `LAYERS` in `xtask/src/main.rs` (a crate without a layer fails `cargo xtask layers`) and any same-layer edge to `SAME_LAYER`. `cargo xtask wasm` checks every L0–L4 crate for the browser, so network crates must compile without their network feature, the pattern `crates/speech` uses (`download`, off for wasm). AI and network features are desktop-only, like speech today.

### 4.2 Network, secrets and OAuth

All network access goes through `filmcraft-net` (F02): the pure-Rust TLS stack already used for model downloads in `crates/speech` (`ureq` 3 + rustls + `rustls-rustcrypto` + the platform verifier), per-host rate limits, an on-disk cache, a request log with secrets redacted, a global Offline mode, and an OS-keychain secret store. OAuth is built from PKCE (S256), a loopback redirect receiver and the `open` crate the desktop app already uses to launch the browser.

### 4.3 Connecting an AI provider (F03)

The pane has a **Connect** button per provider. What it does depends on what each vendor allows third-party apps to do, so there are four lanes. Each provider card says in plain words how it bills ("uses your ChatGPT plan", "pay-as-you-go API", "free tier", "runs on your machine").

| Lane | What Connect does | Billing | Providers | Status |
|---|---|---|---|---|
| **A. Sign in (OAuth/PKCE)** | Opens the provider's page in your browser; you approve; FilmCraft receives a token or key on a loopback redirect. | per provider | **OpenRouter** (documented PKCE flow that returns a key you control; no app registration). **ChatGPT sign-in** (new, for partner apps; access for a personal app unconfirmed). Google (needs an OAuth client from Google Cloud). | OpenRouter available now; others to verify |
| **B. API key** | Opens the provider's console key page; you paste the key; it goes to the OS keychain. | pay-as-you-go, or free tier | Anthropic, OpenAI, Google Gemini (free tier through AI Studio), any OpenAI-compatible endpoint | available |
| **C. Local** | Detects Ollama or LM Studio on localhost. | free (your hardware) | Ollama, LM Studio, llama.cpp server | available |
| **D. Bring your own agent (ACP)** | Starts the vendor's own agent (Claude Code, Codex, Gemini CLI) over the Agent Client Protocol; *that agent* shows its own browser sign-in with your subscription. | your existing plan | Claude (Pro/Max), Codex, Gemini CLI | F03.4; policy-sensitive |

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

### F02 Network, secrets and OAuth foundation · Must · M · ⬜

**Needs:** F01 · **Unlocks:** F03, F05, F25.

**What it does.** One safe way for the app to reach the internet and hold credentials. Every networked feature uses it, so "no silent network calls", rate limits, caching, Offline mode and the usage meter exist in exactly one place.

**How it works.**
- **HTTP client** in `filmcraft-net`: timeouts, retries with backoff, per-host rate limits, an on-disk cache that honors ETag and Last-Modified, a descriptive User-Agent, streaming reads for server-sent events (LLM replies).
- **Offline mode**: a global switch (status bar and `net.setOffline`); every call fails fast with a clear error.
- **Request log** in the Events panel, secrets redacted.
- **Secret store**: OS keychain (macOS Keychain, Windows Credential Manager, Linux Secret Service). Secrets never enter `.fcproj`, logs, MCP output or screenshots. Only the UI can enter one; commands can test that one exists but never read it back.
- **OAuth building blocks**: PKCE, loopback redirect receiver on `127.0.0.1` with a random port, `state` check, single use, timeout.
- **Settings ▸ Connections**: every service and provider with status, Connect/Disconnect and usage.

**Commands.** `net.status`, `net.setOffline`, `net.log`, `secrets.has`, `secrets.delete`, `connections.list`.

**Done when.** A mock provider is called with a stored secret; Offline mode blocks it; the secret appears in no log; hostile responses (huge bodies, bad JSON, stalled connections) never panic.

**Notes.** Spike (D5): which keychain crate (license, maintenance, macOS behavior).

---

### Phase 1: first-video kit

### F03 Assistant pane: connect, chat, mentor · Must · L · yours · ⬜

**Needs:** F02 · **Unlocks:** F15, F21, and a home for every later feature's tools.

**What it does.** A docked panel, modeled on Zed's agent panel, where you connect an AI provider and talk to it about the project. The assistant can *see* what you see (selection, playhead, current frame, transcript around the playhead, the beat you are on). It starts **read-only**: it explains, critiques and answers editing questions about *your* timeline. It cannot change anything until F14 and F15.

**How it works.**
- **Connect** per provider card, in the four lanes of §4.3. Keys and tokens live in the keychain (F02).
- **Panel**: thread list, model picker, profile picker (Read-only now, Propose later), removable context chips (`@selection`, `@frame`, `@transcript`, `@beat`, `@markers`) so you always see what the model sees, streaming replies with Stop, tool-call cards, a usage meter, slash commands (`/explain-cut`, `/glossary`).
- **Read-only tools**: project and sequence inspect, markers, a transcript window, a Program monitor frame, `command_list` search. Same names and shapes as the MCP tools in `crates/automation/src/server.rs`, implemented in the engine layer.
- **Threading**: provider calls run on worker threads under `catch_unwind`; tool calls run on the session thread between frames; tokens arrive over a channel; long calls show in `jobs.list`.

**Steps.**
- F03.1 Provider abstraction, Connections UI, lanes B and C, and OpenRouter sign-in (A).
- F03.2 Chat panel, streaming, threads.
- F03.3 Context chips, read-only tools, mentor prompts.
- F03.4 ACP host lane (Claude Code, Codex, Gemini CLI) with their own sign-in and permission prompts.
- F03.5 ChatGPT and Google sign-in, once access is confirmed (D2).

**Where it lives.** `filmcraft-ai` (provider clients), `crates/engine/src/assistant.rs` (threads, tool loop), `panels/assistant.rs` and `PanelKind::Assistant`. Commands `assistant.connect|disconnect|send|cancel|threads|setModel`.

**Done when.** You can connect through at least two lanes, ask "why does the cut at 00:42 feel abrupt?" and get an answer that cites your actual timeline; disconnecting removes the secret; tests pass against a mock provider with no live network.

**Notes.** The mentor value is immediate for someone new to editing. Every later feature adds its tools here.

### F04 Provenance and license ledger · Must · M · ⬜

**Needs:** F01 · **Unlocks:** F05, F20, F25.

**What it does.** Every asset knows where it came from and what you may do with it: source, author, URL, license, attribution text, content hash, and whether it is synthetic (with model, prompt, seed). One click produces a credits block. An audit warns when something conflicts with the project's license policy, for example a non-commercial clip in a monetized project.

**How it works.** A `provenance` record per media item, stored with the project (undoable; D1) and a project-level policy (`commercial`, `attribution_ok`, `synthetic_ok`). Commands `provenance.get|set|list`, `provenance.credits {format}`, `provenance.audit`. The Source Hub and every importer write records automatically; manual imports can be tagged ("my footage", "licensed"). UI: a license badge in the Project panel, a Credits and Rights dialog, an audit step at export.

**Done when.** Import a clip with a CC BY license, see its badge, get its credit line in the credits block, and get an audit warning if the policy forbids it.

**Notes.** Disclosure of realistic synthetic media is a YouTube setting; the ledger supplies the timecodes. The ledger only encodes what a source says; it is not legal advice.

### F05 Source Hub and Find Media panel · Must · L · yours · ⬜

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
- F05.5 Local folder index.

**Where it lives.** `filmcraft-sources` (network behind a feature), engine commands `hub.providers|search|acquire|shortlist`, `panels/find_media.rs`.

**Done when.** A request returns candidates from at least three sources with correct license data; acquiring one writes the file, the ledger entry and the bin item; Offline mode and rate limits are honored; adapters pass against recorded responses.

**Notes.** Pexels, Pixabay and Freesound need free API keys. Terms differ (attribution display, caching, hosting); encode them in each adapter's metadata and re-check them (§9).

### F06 Narration upgrades · Must · M · ⬜

**Needs:** F01 · **Unlocks:** F16, F19.

**What it does.** Makes the built-in narration usable for real videos: names and terms pronounced correctly, several takes to choose from, direction per sentence, and a saved "channel voice" so every video has the same narrator.

**How it works.**
- **F06.1 Pronunciation dictionary.** A lexicon (project and global) of word → respelling or phonemes, applied before the phonemizer. Words missing from the built-in dictionary (CMUdict) are flagged *before* rendering; "fix this word" lets you edit and audition. Lives in `crates/tts-text` (`normalize.rs`, `phonemize.rs`) and the narration settings.
- **F06.2 Takes.** The vocoder's noise is seeded, so a seed can stand for a take. Generate N takes of a line, pick one, store its seed; re-rendering stays deterministic. The seed needs exposing in `tts.create` and `tts.edit`.
- **F06.3 Direction.** Per-sentence pace, pitch and pauses (extends the `[pause 1s]` marker), as chunk-level settings because the model takes pace per call.
- **F06.4 Channel voice.** A saved preset: base voice, pace, pitch and a track insert chain built from existing effects (EQ, compressor, de-esser, room reverb), plus an optional procedurally generated room-tone bed. Applied to the narration track, with A/B against the dry voice.

**Where it lives.** `crates/tts-text`, `crates/engine/src/narration.rs` and `Narration` in `filmcraft-project`, `panels/tts.rs`; presets through the mixer inserts.

**Done when.** A script with five proper nouns renders correctly after you fix them once; takes are repeatable; one click applies your channel voice to a new narration track.

**Notes.** Natural voices are US English only and have no pitch input (pitch is a post shift), per `crates/tts/README.md`. Room-tone samples must be generated in code or openly licensed with attribution (AGENTS.md §1).

### F07 Look Pack: film texture and the A24 stack · Must · L · ⬜

**Needs:** F01 (independent; can be pulled forward at any time).

**What it does.** The finishing layer that makes stock footage read as film: real film grain, halation, diffusion glow, gate weave, flicker and dust, anamorphic streaks, lens fringing, a one-click **A24 stack** with a single *film look* amount, and camera-move presets that bring stills to life.

**How it works.** Each item is a new video effect: definition in `crates/project/src/effect.rs`, CPU implementation in `crates/render/src/effects.rs`, a test in `crates/render/src/tests.rs` (see `docs/contributing.md` §6). Linear-light, premultiplied float, scaled by `px_scale`, and deterministic (seeded by frame index and a seed) so previews match exports. The GPU path follows later.
- F07.1 **Film Grain**: size, softness, brightness response, chroma, animated seed, stock-style presets with generic names. `crates/av1/src/grain.rs` already implements the AV1 film-grain synthesis (spec §7.18.3), an autoregressive, brightness-dependent model that is a strong, royalty-free basis. It is internal to the decoder, so reuse means exposing a small public API.
- F07.2 **Halation** (highlight threshold, radius, tint, strength).
- F07.3 **Diffusion** (soft bloom with lifted blacks).
- F07.4 **Film Gate** (weave, flicker, dust and scratches).
- F07.5 **Anamorphic streak**.
- F07.6 **Lens fringing** (radial).
- F07.7 **A24 stack** preset (through `presets.*`) with one *film look* macro slider and a letterbox option (2.39:1, 1.85:1).
- F07.8 **Camera moves for stills**: duration-aware keyframes (slow push-in, drift, handheld sway) from a command `motion.cameraMove`. Depth-map parallax is a later experiment.

**Done when.** Each effect has golden tests; the CPU path previews acceptably on the architect's Mac (a real-time GPU path may follow); the A24 stack turns a flat stock clip into a convincing film look in one click.

**Notes.** Implement from public descriptions only; do not copy GPL grain or halation code (AGENTS.md §2). Use generic names, not product trademarks. Today's catalog has only `Noise` (amount, color, grain size), and `Wonder Glow` can fake a bloom by hand.

---

### Phase 2: plan and understand

### F08 Planner core and Format Packs · Must · L · yours · ⬜

**Needs:** F01 · **Unlocks:** F09, F11, F13, F16, F24.

**What it does.** One structure for designing a video before and while you cut it: `Video → Sections → Beats → Lines/Shots`. A beat holds its *purpose* ("what changes here?"), intensity (0–10), target duration, narration text, visual intent, sound note, status and a link to a timeline range. A **Format Pack** supplies the starting structure for a kind of channel.

**How it works.**
- Data in `filmcraft-project`, stored with the project and undoable (D1). Commands `planner.create|addSection|addBeat|updateBeat|move|delete|applyPack|link|buildSkeleton|export|import`.
- `buildSkeleton` creates a range marker per beat and offline placeholder clips for unsourced shots (`file.newOfflineFile`); `clip.automateToSequence` can place clips at those markers.
- A **Format Pack** is a data file: beat template (names, purposes, share of runtime, intensity curve), default look preset, title style, sound-plan template, analysis targets. Packs are data, never code, so any channel type can be added later (D4).
- Ships with two baseline packs, **Mystery** and **Explainer** (§6).
- The Notes panel (`notes.rs`, `project.setNotes`) is the precedent: project-stored text with undo and agent access.

**Done when.** Create a treatment from the Mystery pack, edit beats, build a skeleton on the timeline, save and reopen the project, and get the same planner back; commands have hostile-parameter tests.

**Notes.** The planner is the shared backbone: Source Hub slots, the coach's targets, the sound plan and the rough-cut assistant all read it.

### F09 Planner views · Must · L · yours · ⬜

**Needs:** F08 (F05 for slots) · **Unlocks:** F16.

**What it does.** Different lenses on the same planner data, each in its own panel, all editing the same beats.

**Steps.**
- F09.1 **A/V script**: two columns (narration | visuals), one row per sentence, with slot thumbnails and timing; clicking a row seeks the timeline. The expected workhorse.
- F09.2 **Beat lane** above the timeline: beats as colored ranges with the intensity curve, planned versus actual duration.
- F09.3 **Beat board**: cards grouped by act, drag to reorder; reordering can reorder the sequence sections.
- F09.4 **Storyboard grid**: a thumbnail per shot with shot type, camera move and duration; unsourced shots show the visual intent as text.

F12 adds the *look bible* view and F13 the *sound plan* view.

**Done when.** Editing a line in the A/V script updates the beat board, the lane and the narration in the TTS panel; every widget has automation ids and state in `state.rs`.

### F10 Analysis core · Must · L · ⬜

**Needs:** F01 · **Unlocks:** F11, F12, F13, F18, F21, F23.

**What it does.** Measures footage and audio so later features can talk in numbers.

**How it works.** `filmcraft-analysis` is a set of pure functions over frames and audio buffers. The engine runs them as background jobs and caches results by content hash plus algorithm version (rebuildable, never in the project). Output is a versioned `AnalysisReport`.
- **Per shot**: mean luma, contrast, saturation, palette (5 colors in Lab), color temperature, sharpness, grain estimate, motion energy and a rough camera-move class, letterbox and aspect.
- **Per audio segment**: loudness (BS.1770, already in `audio-dsp`), loudness range, true peak, silence share, spectral balance, a speech/music/ambience guess, music beat grid.
- **Sequence level** (from the model, no decoding): shot-length distribution, cut density, J/L-cut share, transition use, narration words per minute, track occupancy.
- Reuses what exists: `SceneDetector` for cuts, the optical-flow code behind Morph Cut, the scopes maths.
- Commands `analysis.run|get|list|clear`.

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

**What it does.** Design a voice that is yours: pick base voices, blend them, audition the same sentence, and save the result as the channel's narrator.

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
| D2 | Order of the AI connection lanes (§4.3) | B (API key), C (local) and OpenRouter first; then D (ACP) for your Claude subscription; then ChatGPT and Google sign-in once access is confirmed. |
| D3 | Default free model for the assistant: Gemini free tier, OpenRouter free models, or local | Decide by testing the mentor prompts on each before F03.2 ships. |
| D4 | Format Pack file format and location | TOML for hand-editing plus a JSON Schema; a folder in the data directory; an explicit version field. |
| D5 | Keychain crate (license, maintenance, macOS behavior) | A spike at the start of F02. |
| D6 | Point `CLAUDE.md` at this file (one line in an upstream-owned file) | Yes, so every agent session starts from it. **Decided 2026-10-12: yes (done in F01.3).** |
| D7 | Crate naming | Keep the `filmcraft-` prefix for workspace consistency. |
| D8 | Default profile for new assistant threads | Read-only for a first thread; Propose once edit tools exist. |
| D9 | Reference analysis stance and UI wording | Files you supply only; no downloaders; say so in the UI. |
| D10 | Telemetry | None. Confirm. |

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
| OpenRouter's PKCE flow returns a user-controlled key, needs no registration, and its code is valid for ten minutes | F03.1 lane A | [OpenRouter OAuth PKCE](https://openrouter.ai/docs/use-cases/oauth-pkce) |
| "Sign in with ChatGPT" launched in early October 2026 for partner apps (scope `chatgpt.tokens.use.direct`); access for a personal app is unconfirmed | F03.5 | [IT Brief](https://itbrief.co.uk/story/openai-launches-chatgpt-sign-in-for-16-partner-tools), [WorkOS](https://workos.com/blog/sign-in-with-chatgpt-plan-usage-scope) |
| Gemini: an API key is the simple route; OAuth needs a Desktop OAuth client from Google Cloud; check the free-tier limits | F03.5 | [Gemini API OAuth quickstart](https://ai.google.dev/gemini-api/docs/oauth) |
| Pexels and Pixabay API limits and terms (Pexels defaults to 200 requests an hour; Pixabay 100 per minute; both ask for credit when displaying results) | F05 | [Pexels API requirements](https://help.pexels.com/hc/en-us/articles/900005852323), [Pixabay API docs](https://pixabay.com/api/docs/) |
| YouTube monetization rules target mass-produced, repetitive "inauthentic content", and realistic altered or synthetic content needs a disclosure | F04, F20, F25 | [Flocker](https://flocker.tv/posts/youtube-inauthentic-content-ai-enforcement/), [AIR Media-Tech](https://air.io/en/youtube-glossary/what-is-youtubes-ai-content-disclosure-policy) |
| ArtCraft is the same team's open-source AI image and video app that aggregates third-party generators; its license is unclear | F25 | [gittrend](https://gittrend.io/repo/storytold/artcraft), [Brightcoding](https://www.blog.brightcoding.dev/2026/10/08/storytoldartcraft-open-source-ide-for-controlled-ai-image-video) |
