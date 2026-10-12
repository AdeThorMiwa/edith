# Working in this fork

> **Last reviewed:** 2026-10-12 · **Last updated:** 2026-10-12 · **Change:** new file (branches, upstream sync routine, fork rules)

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

## Fork rules

The short version of [section 4.6 of the roadmap](upcoming-features.md#46-fork-hygiene).

- New code goes in new crates and new files.
- Touch an upstream-owned file in at most one line per feature, and only at these hook points: `LAYERS` and `SAME_LAYER` in `xtask/src/main.rs`, `PanelKind` in `crates/ui-egui/src/dock.rs`, command registration in `crates/engine/src/commands.rs`, one extension point on `Project`, the default features in `apps/filmcraft/Cargo.toml` and `apps/filmcraft-cli/Cargo.toml`, and one pointer line in `CLAUDE.md`.
- Do not refactor `crates/automation` (upstream's MCP server). The assistant mirrors its tool names and shapes in the engine layer instead.
- Do not bump `SCHEMA_VERSION` per feature. It was 13 on 2026-10-11 and 15 on 2026-10-12 (`MIGRATIONS.len() + 1` in `crates/format/src/lib.rs`), so a v16 of ours and a v16 of theirs would collide. Custom project data goes in one `extensions` map (decision D1 in the roadmap).
- Command namespaces for fork commands: `assistant.*`, `net.*`, `secrets.*`, `connections.*`, `provenance.*`, `hub.*`, `planner.*`, `analysis.*`, `coach.*`, `reference.*`, `soundplan.*`, `proposal.*`, `voicelab.*`, `youtube.*`. Narration additions extend `tts.*`.
- Commits: one task id each (`F01.1: …`), only green states (`cargo xtask ci`), each ending with the `Co-Authored-By` trailer.

## Upstream-owned files this fork changes

Keep this table current. It is the whole list of places a merge can conflict.

| File | Change | Task |
|---|---|---|
| none yet | | |
