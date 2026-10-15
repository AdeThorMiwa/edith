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

## Upstream-owned files this fork changes

Keep this table current. It is the whole list of places a merge can conflict.

| File | Change | Task |
|---|---|---|
| `apps/filmcraft/Cargo.toml` | `default = ["whisper", "neural-voices"]` (one line, with a trailing comment) | F01.1 |
| `apps/filmcraft-cli/Cargo.toml` | the same `default`, plus a `neural-voices` feature that forwards to the engine (3 lines), so MCP and CLI clients can transcribe and narrate | F01.1 |
| `crates/speech/src/whisper/tokenizer.rs` | `expect()` replaced by `filter_map`, plus a test of the byte table: a lint fix the new defaults surface (see the exception under Fork rules) | F01.1 |
| `Cargo.toml` | one workspace dependency line, `filmcraft-net` | F02.1 |
| `xtask/src/main.rs` | one `LAYERS` entry, `("net", 2)` | F02.1 |
| `docs/architecture.md` | `net` in the L2 line and the crate table | F02.5 |
| `crates/engine/Cargo.toml` | the `filmcraft-net` dependency and a `net` feature that forwards to it (3 lines) | F02.5 |
| `crates/engine/src/lib.rs` | `pub mod net_commands;` and `mod net_tests;` | F02.5 |
| `crates/engine/src/commands.rs` | `v.extend(crate::net_commands::commands());` in `build()` | F02.5 |
| `apps/filmcraft/Cargo.toml`, `apps/filmcraft-cli/Cargo.toml` | `net` added to `default`, plus a `net` feature (3 lines each, so the first row above changes too) | F02.5 |
| `apps/filmcraft/src/main.rs`, `apps/filmcraft-cli/src/main.rs` | one line each: `net_commands::init(...)`, so Offline mode is remembered | F02.5 |
