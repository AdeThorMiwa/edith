# Research notes: agents and AI video editing

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-15 · **Change:** new file (first pass: what the published work says, and what it means for the Edith agent)

The Edith agent is designed from published research, not from habit. This file is where that is recorded: what was read, what it says, what we decided because of it, and how sure we are. It starts small and grows with each stage; a design decision in [upcoming-features.md](../upcoming-features.md) that rests on research links here.

**How to read the confidence column.** *Read* means the abstract or paper was read directly. *Lead* means only a title or a snippet from a reference list was seen: do not build on it until it has been read.

## First pass (2026-10-15)

| Work | What it says | Confidence | What it means for Edith |
|---|---|---|---|
| **Timeline-Bench**, arXiv 2609.35143 (Sep 2026) | 56 real editing tasks from raw footage to final cut. 16 agents (frontier models inside coding-agent harnesses such as Codex, Claude Code, OpenCode). Best resolved 15 of 56 (26.8%), average 14.0%. Human editors preferred the reference edit in 83.5% of judgments. Of 771 unresolved runs, 562 (about 73%) failed *only* the quality test: the agents delivered what the brief asked for but not good craft. The authors say the agents see footage "through stills and transcripts" and check renders for defects, not craft. The quality test was calibrated on 2,582 blind judgments by 43 editors. The best run used "curated editorial guidance". | Read (abstract) | The gap is craft, and perception is the bottleneck. This is the case for F10 to F13: give the agent measurements (pacing, loudness, cut rhythm, frame content) as its eyes and ears. It is also the case for editorial guidance as a first-class asset (Format Packs, prompt packs), since the best run had it. And it says the F03.6 suite must include human-calibrated judgments of craft, not only code checks. |
| **LAVE**, IUI 2024, arXiv 2402.10294 | An LLM agent for video editing that plans and executes actions from a free-form goal, next to direct manipulation the user can always use. The system turns footage into language descriptions so the model can work with it. Study of 8 participants, novice to proficient; the authors report on sense of co-creation. | Read (abstract) | Keep the agent and the manual editor side by side, never agent-only. A text description of footage is a workable first perception layer (we already have transcripts and scene detection), but see Timeline-Bench for its limit. |

## Leads to read as the related stage proceeds

| Lead | Why it matters | Needed for |
|---|---|---|
| VideoAgent (editing framework; 30+ editing agents, claims 87 to 95% orchestration success on a VideoEdit benchmark) | Orchestration of many specialist tools; check what "success" measures. | F16, tool design |
| Hierarchical Screenwriter / Director / Editor agents with Mashup-Bench | Role split between planning and cutting; metrics for visual continuity and audio alignment. | F08, F16 |
| CutVerse (arXiv 2605.19484) | Scores the *trajectory* of actions in Premiere and After Effects, not only the export. | F03.6 graders |
| DeskCraft (arXiv 2606.03103) | Tasks with mid-turn clarification and user interruption, and feedback after the agent reports done. 538 tasks; the best model scored 31.6% standard and 27.6% interactive. | F03.6 tasks, F14 proposals |
| VideoDiff (arXiv 2502.10190) | Human-AI co-creation with *alternatives*. | F14, F22 |
| EditDuet, CutClaw, AssistEditor | Multi-agent editing; music-synchronized long-form editing; GUI-agent editing of professional software. | F16, F17 |
| "What Does Context Compression Cost an Agent?" (arXiv 2608.16370); "A Survey of Context Engineering for LLMs" (arXiv 2507.13334); "Memory for Autonomous LLM Agents" (arXiv 2603.07670); Anthropic, "Effective context engineering for AI agents" | What long sessions lose when compacted, and how to manage context and memory. | F03.1 loop, history rules |
| CentaurEval (arXiv 2512.04111) | Measures the human's contribution to a human-agent team. | F03.6, the "human drives" rule |

## Method

Each stage is designed from what is known here and then built; it does not wait for more reading. As a stage proceeds, its open design questions are listed, the relevant leads are read, and what changed is written down here with the sources and a confidence. Better research later improves the agent; it never holds a stage up. Evaluation follows what the work above agrees on: grade the steps an agent took as well as its result; include mid-task interruptions and feedback; keep code checks separate from human-calibrated judgments of craft.

## Open questions this file should answer next

1. What perception does an editing agent need beyond stills and transcripts, and which of those can be plain measurements (F10) rather than a vision model?
2. How should editorial guidance be encoded so a model uses it: prompt text, retrieved examples from the reference fingerprint, or tools that check a rule?
3. What is the smallest human-rated set that calibrates a craft judge for our formats, and who rates it (the architect)?
4. Which context-management strategy keeps a long editing session coherent without rewriting history?
