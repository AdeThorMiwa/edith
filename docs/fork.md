# Working in this fork

> **Last reviewed:** 2026-10-12 · **Last updated:** 2026-10-15 · **Change:** F02 hook points listed (was: new file (branches, upstream sync routine, Rust version, fork rules, speech defaults, build notes))

This checkout is `AdeThorMiwa/filmcraft`, a fork of [`storytold/filmcraft`](https://github.com/storytold/filmcraft). The fork adds a creator layer on top of the editor (assistant, sourcing, planning, analysis, voices, look; see [upcoming-features.md](upcoming-features.md)) and keeps merging upstream's work. This page is how the two stay compatible. [AGENTS.md](../AGENTS.md) and [contributing.md](contributing.md) still apply in full.

## Branches and remotes

| Name | What it is | Rule |
|---|---|---|
| `main` | An exact mirror of upstream's `main` | Never commit to it. It only ever fast-forwards. |
| `studio` | Upstream plus the fork's features | All our work lands here. Build and run from it. |
| `origin` | `AdeThorMiwa/filmcraft` on GitHub (SSH) | Our fork. |
| `upstream` | `storytold/filmcraft` (HTTPS), tracking `main` only | Fetch only; we never push there. |

Why a mirror: GitHub's *Sync fork* button is a clean fast-forward only while `main` holds none of our commits, and `git log main..studio` then lists exactly what the fork has added.

The `upstream` remote is already set up in this checkout. On a new clone: `git remote add -t main upstream https://github.com/storytold/filmcraft.git`.

## Syncing with upstream

Sync at the start of each feature, after each upstream release, and before touching an area upstream is also changing. Upstream moves fast: between 2026-10-11 and 2026-10-12 it landed 378 commits (about 25 hours), including about 600 changed lines in `crates/engine/src/commands.rs`. Small, frequent merges are far easier than one large one.

```sh
git fetch upstream
git switch main
git merge --ff-only upstream/main   # refuses if main holds a commit of ours
git push origin main                # or press Sync fork on GitHub first; then this is a no-op
git switch studio
git merge main
cargo xtask ci                      # the gate; fix whatever the merge broke here
```

- **Conflicts** can only be in a file we changed ([the list below](#upstream-owned-files-this-fork-changes)). Take upstream's side of the surrounding code, then re-apply our line. Never resolve by taking ours wholesale.
- **`--ff-only` refuses** only if a commit landed on `main` by mistake. Move it to `studio` first (`git switch studio && git cherry-pick <sha>`), then make `main` match upstream again.
- **A red gate after a clean merge** is upstream's or an interaction with our code. Run the failing test on `main` first; if it fails there too, it is upstream's.

## Rust version

Use current stable (1.99.0 on 2026-10-12; the repo needs 1.95 or newer). Upstream's GitHub checks do not run the Rust gate (their `lint / lint` check is the packaging lint), so `cargo xtask ci` runs only on contributors' machines, and the lint set moves with each Rust release. On 1.96 the gate fails on three recent upstream lines (`nonminimal_bool`, which Clippy 1.97 moved out of its default lints). Run `rustup update stable` before blaming a red gate on a change.

## Fork rules

The short version of [section 4.6 of the roadmap](upcoming-features.md#46-fork-hygiene).

- New code goes in new crates and new files.
- Touch an upstream-owned file in at most one line per feature, and only at these hook points: `LAYERS` and `SAME_LAYER` in `xtask/src/main.rs`, `PanelKind` in `crates/ui-egui/src/dock.rs`, command registration in `crates/engine/src/commands.rs`, one extension point on `Project`, the default features in `apps/filmcraft/Cargo.toml` and `apps/filmcraft-cli/Cargo.toml`, and one pointer line in `CLAUDE.md`.
- Exception: a never-crash or lint fix that the fork's defaults surface in feature-gated upstream code. Upstream's own gate never compiles Whisper or Kokoro code (both are off by default there), so it can hide violations of its own rules. Keep the fix minimal, add a test, list it in the table below, and offer it upstream so it drops out of the fork after a sync.
- Do not refactor `crates/automation` (upstream's MCP server). The assistant mirrors its tool names and shapes in the engine layer instead.
- Do not bump `SCHEMA_VERSION` per feature. It was 13 on 2026-10-11 and 15 on 2026-10-12 (`MIGRATIONS.len() + 1` in `crates/format/src/lib.rs`), so a v16 of ours and a v16 of theirs would collide. Custom project data goes in one `extensions` map (decision D1 in the roadmap).
- Command namespaces for fork commands: `assistant.*`, `net.*`, `secrets.*`, `connections.*`, `provenance.*`, `hub.*`, `planner.*`, `analysis.*`, `coach.*`, `reference.*`, `soundplan.*`, `proposal.*`, `voicelab.*`, `youtube.*`. Narration additions extend `tts.*`.
- Commits: one task id each (`F01.1: …`), only green states (`cargo xtask ci`), each ending with the `Co-Authored-By` trailer.

## Speech and natural voices (on by default)

`cargo run --release -p filmcraft` builds with Whisper speech-to-text and the natural voices (Kokoro-82M). The code is in the build; the model files are not. They download when asked:

| What | Command | Size | Lands in |
|---|---|---|---|
| Whisper `whisper-base` (the default; `whisper-tiny` and `whisper-small` are listed too) | `transcript.downloadModel` | about 293 MB | `<data dir>/models` |
| Natural voices (Kokoro-82M, 9 US English voices) | `tts.downloadVoices` | 336 MB | `<data dir>/models` |

`<data dir>` is `~/Library/Application Support/FilmCraft` on macOS (`FILMCRAFT_DATA_DIR` overrides it). Models never go into the repo or a project file.

- `transcript.models` and `tts.voices` report whether this build has each feature (`available`) and whether the files are on disk (`installed`).
- Upstream's lean build is `--no-default-features`.
- Tests never download. Tests that need weights run only when `FILMCRAFT_MODELS_DIR` / `FILMCRAFT_KOKORO_DIR` point at them, and skip otherwise.
- Candle makes the first build longer (the app took 3 min 31 s on an Apple-silicon Mac on 2026-10-12, with 378 upstream commits to compile as well). Later builds reuse it.
- The web app and `cargo xtask wasm` are unaffected: the wasm check selects only L0 to L4 and web crates, which do not see the app's defaults.

## Building the app and the CLI

Alternating `cargo build -p filmcraft` and `cargo build -p filmcraft-cli` rebuilds the engine and about a dozen other crates each time (the CLI after the app took 2 min 51 s). The defaults are not the cause: 29 shared crates (`log`, `indexmap`, `chrono`, …) resolve different features for the two binaries, and Cargo keys the build on that. Build the one you need and run its binary from `target/release/`, rather than switching back and forth.

## Naming: Edith

The whole app is becoming **Edith**; the full rename is not done yet. Until it is, the rule is simple and applies to every stage:

1. **New things are Edith.** Every crate the fork creates is named `edith-*` (`edith-net`, `edith-llm`, `edith-agent`, `edith-ai`, …). Directories keep the repo's convention, the short name (`crates/net`, `crates/llm`, `crates/agent`, `crates/ai`), so nothing moves. New text, docs, HTTP headers (`User-Agent`, `X-Title`), the keychain service and any file the fork writes say Edith. The constants live in one place, `crates/net/src/brand.rs`.
2. **Code a stage touches that still says FilmCraft is changed to Edith, when that is cheap**: strings, comments, local names and docs inside the function, block or file section the stage is already editing. List the upstream-owned file in the table below.
3. **When it is not cheap, leave it for the big rename.** That means crate names `filmcraft-*` and the Rust paths `filmcraft_*`, the `.fcproj` extension, the app id, the data directory, and anything that would fan out across many files. A crate cannot be renamed halfway (every dependent `Cargo.toml` and `use` line changes with it, in files upstream edits daily), and the data directory holds downloaded models (about 620 MB), preferences and autosaves, so it needs a migration that moves the folder. After the big rename, merging upstream needs a step first: run the same rename script over upstream's tree, then merge.
4. **No sweeps.** A stage never renames something it does not otherwise touch, so a merge from upstream stays small.

## Upstream-owned files this fork changes

Keep this table current. It is the whole list of places a merge can conflict.

| File | Change | Task |
|---|---|---|
| `apps/filmcraft/Cargo.toml` | `default = ["whisper", "neural-voices"]` (one line, with a trailing comment) | F01.1 |
| `apps/filmcraft-cli/Cargo.toml` | the same `default`, plus a `neural-voices` feature that forwards to the engine (3 lines), so MCP and CLI clients can transcribe and narrate | F01.1 |
| `crates/speech/src/whisper/tokenizer.rs` | `expect()` replaced by `filter_map`, plus a test of the byte table: a lint fix the new defaults surface (see the exception under Fork rules) | F01.1 |
| `Cargo.toml` | workspace dependency lines for the fork's crates (`edith-net`, `edith-llm`, `edith-agent`, `edith-ai`) | F02.1 |
| `xtask/src/main.rs` | `LAYERS` entries (`("net", 2)`, `("llm", 3)`, `("agent", 3)`, `("ai", 3)`), `SAME_LAYER` edges (`agent`/`ai` to `llm`, `ai` to `agent`), and the prefix rule accepts `edith-` next to `filmcraft-` (two lines) | F02.1, F02.7, F03.1 |
| `docs/architecture.md` | `net` in the L2 line and the crate table | F02.5 |
| `crates/ui-egui/src/dock.rs` | `PanelKind::Connections` and `PanelKind::Assistant` (variant, `ALL` length, title: 4 lines each; also `Rights`, "Credits and Rights", and `FindMedia`, "Find Media", a tab after Media Browser in the default layout); `Assistant` and `Rights` in the right-hand tab group of the default Editing layout, and its Window-menu shortcut Shift+0 (2 lines) | F02.6, F03.1, F03.7b |
| `crates/project/src/lib.rs` | one field on `Project`, `ext` (an open map for the fork's undoable data, empty and unwritten when unused, so no schema bump), and its initialiser (D1) | F04.1 |
| `crates/engine/src/lib.rs`, `commands.rs` | `pub mod provenance;`, `pub mod rights;` and their test modules; `v.extend(...::commands())` for each (6 lines) | F04.1, F04.2 |
| `crates/engine/src/project_tools.rs` (`project_subset`) | one line: a saved subset keeps only the provenance records of the items it keeps | F04.1 |
| `crates/tts-text/src/phonemize.rs`, `lib.rs`; `crates/tts/src/lib.rs`, `neural.rs` | F06.1: `unknown_words` and the possessive of an overridden word (tts-text); `Pronunciations`, `UnknownWord`, a defaulted `Voice::synthesize_with`, `check_words` (tts); the package-loading code in `neural::load` moved into `loaded_for` so the word check shares it; `Pronunciations::checked_entry` and `lexicon_only` (the dictionary alone, without the voice model) | F06.1a, F06.1b |
| `crates/engine/src/narration.rs`, `lib.rs`, `commands.rs` | F06.1b: the synthesis cache key and `render` take the pronunciation list (`cache_key`, `render`, `synthesize` and the three places that call them), `tts.preview`/`tts.render` list a `pronounce` parameter; `pub mod pronounce;` and its test module, `pub pronounce: pronounce::Channel` on `Session` (with its initialiser), `v.extend(crate::pronounce::commands())` | F06.1b |
| `crates/ui-egui/src/panels/tts.rs`, `mod.rs` | F06.1c: `pub mod tts_words;`; in `tts.rs` a call to the Pronunciation section under the text box and the handling of its events (audition, regenerate, status): about 20 lines | F06.1c |
| `crates/ui-egui/src/panels/mod.rs`, `project_views.rs` | `pub mod rights;`, `pub mod rights_panel;` and `pub mod find_media;` (and their dispatch lines); three call sites in the Project panel: the rights badge on a card and in a list row, and the Rights submenu in an item's menu | F04.1 |
| `crates/engine/src/lib.rs`, `commands.rs`, `Cargo.toml` | F05: `pub mod hub;` and its test module, `pub hub: hub::Hub` on `Session` (with its initialiser), `v.extend(crate::hub::commands())`, the `edith-sources` dependency and `edith-sources/keychain` in the `net` feature; F05.5 adds `pub mod hub_local;` and `v.extend(crate::hub_local::commands())` (2 lines) | F05.2, F05.5 |
| `crates/ui-egui/src/lib.rs` (`logic`) | one line: `self.session.pump_hub()` finishes media downloads every frame | F05.2 |
| `Cargo.toml`, `xtask/src/main.rs` | the `edith-sources` workspace dependency and its layer (`sources`, L3) | F05.1 |
| `crates/net/src/http.rs`, `services.rs` | `Client::download` (stream a body to a file, hashed, renamed only when whole) and the `freesound` service | F05.1 |
| `crates/ui-egui/Cargo.toml` | the `edith-sources` dependency (plain data for the Find Media panel), its test-only network features, `zeroize`, and the `jpeg` feature of the `image` crate so result thumbnails decode | F05.3 |
| `xtask/src/main.rs` (`ci`) | the test step runs with `FILMCRAFT_DATA_DIR` pointing at an empty folder under `target/`, so downloaded models and saved preferences on the developer's machine cannot change test results (the two `tts_ui` tests failed on a machine with the Kokoro voices installed) | F03.7g |
| `crates/ui-egui/src/lib.rs` (`show_panel`) | the Assistant and the Credits and Rights panel open beside a side panel (Properties, Essential Sound or Graphics, Lumetri Color) in a workspace that lacks them (one match arm) | F03.7b |
| `crates/ui-egui/src/panels/mod.rs` | `pub mod connections;`, `pub mod assistant;`, `pub mod assistant_view;` and one `match` arm each for the two panels | F02.6, F03.1 |
| `crates/ui-egui/src/lib.rs` | one line after `job_status` (the status-bar Offline switch) and one in `logic()` (`session.pump_assistant()`, every frame) | F02.6, F03.1 |
| `crates/ui-egui/src/i18n/{es,ja,zh-cn,pt-br}.tsv` | lines appended at the end for the Connections and Assistant panels (23 each in es, ja, zh-cn for each; the panel titles in pt-br); the tests insist every `tl!` string is translated in those languages | F02.6, F03.1 |
| `crates/ui-egui/tests/chrome_ui.rs` | the panel-extent helper ignores `status.*` elements (the status bar's own, now including the Offline switch) | F03.1 |
| `crates/ui-egui/Cargo.toml` | dev-dependencies (`filmcraft-engine` with `net`, `edith-net` with `keychain`, `edith-ai`) so the UI tests use the in-memory secret store, a scripted model and a mock sign-in server (`testserver`) | F02.6, F03.1, F03.3d |
| `crates/engine/Cargo.toml` | the `edith-net` and `edith-ai` dependencies and a `net` feature that forwards to them (4 lines), plus a dev-dependency on `edith-net` with `testserver` for the sign-in tests | F02.5, F03.1, F03.3d |
| `crates/engine/src/lib.rs` | `pub mod net_commands;`, `pub mod assistant;`, `mod assistant_tools;`, the test modules, and one `Session` field (`assistant`) with its initialiser | F02.5, F03.1 |
| `crates/engine/src/commands.rs` | `v.extend(crate::net_commands::commands());` and `v.extend(crate::assistant::commands());` in `build()` | F02.5, F03.1 |
| `apps/filmcraft/Cargo.toml`, `apps/filmcraft-cli/Cargo.toml` | `net` added to `default`, plus a `net` feature (3 lines each, so the first row above changes too) | F02.5 |
| `apps/filmcraft/src/main.rs`, `apps/filmcraft-cli/src/main.rs` | two lines each: `net_commands::init(...)` (Offline mode is remembered) and `assistant.set_dir(...)` (the assistant's settings) | F02.5, F03.1 |
