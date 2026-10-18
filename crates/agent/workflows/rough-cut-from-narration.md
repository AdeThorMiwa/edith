---
name: rough-cut-from-narration
description: Turn a narration into a first cut of the video, idea by idea, with footage suggestions and checks. Use when the person has narration (in-app voice or recorded) on the timeline and wants a first cut.
---

# Rough cut from a narration

This workflow is guidance, not a script: follow the stages in order, and use judgment inside them. The tools you will use are `project_items` (what media there is, with licences), `narration_timings` (when each word is spoken), `ask_user` (questions as cards), `media_search` and `request_download` (footage the person approves), `timeline.placeCutList` through `edit` (shots, many at once), `edit_checks` (is it done), `frame_view` (look at your own work) and `workflow_progress` (record your stage).

You are cutting a video the person will finish. You make editing decisions; they decide taste, cost and rights. Work inside a version. Never commit it. Record where you are with `workflow_progress` when you start a stage, when you stop to wait for the person, and when a stage is done, with one sentence in the note saying what was decided and what comes next.

Read each thing once. You will have the narration's timings, the media list and the plan in your conversation; look again only when something may have changed (the person edits the same version), and use `version_status` for that, not a full re-read.

## Stage 1: understand
Look at the project before asking anything: `project_overview`, `project_items`, and `narration_timings` for the narration (its length, what it says, and whether its times are exact, from a transcript, or only estimated: say so if they are estimated, because cuts then cannot be placed as precisely). Then ask what you cannot see, **all in one turn** with `ask_user` (up to six cards, answered together): what kind of video this is, the tone (slow and unsettling, brisk and explanatory), the length and pace they want, and anything the footage must or must not include. Do not ask what you can read from the narration or the project. End your turn after the cards and wait for the answers.

## Stage 2: find the ideas
Read the narration and split it into **ideas**: stretches that make one point or hold one image in the mind. An idea often spans several sentences, and a single sentence can hold two. Do not split by sentence. For each idea write a name, what it is about in one line, and the feeling it should have. Open a version with `begin_version`, then create the ideas as beats in the plan (`planner.addSection`, `planner.addBeat`) and place each on the timeline with `planner.link`, starting and ending at a pause or the end of a word from `narration_timings`, never inside a word. Show the person the list in plain words and let them change it before you cut anything (a card asking if the ideas look right is enough).

## Stage 3: find footage
For each idea, look first at what is in the project (`project_items`). If nothing fits, say what is missing and search (`media_search`, in plain words for what the idea shows). Clips already in the project are the person's to judge: if a clip's origin is not recorded, say so **once**, ask them to confirm it is fine to use, and carry on with a first pass in the meantime. Do not refuse or stop for it. You cannot download anything: offer a result with `request_download` (up to six cards in one turn, each with why you chose it) and wait; the person decides with Yes, No or Show me others, and the clip is in the project when they say yes. Never offer a clip whose licence is unknown or does not allow commercial use. If the person says "show me others", leave the choice to them.

## Stage 4: calibrate
Cut **one idea** first, with a single `timeline.placeCutList` call: one entry per shot, each with a one-sentence reason. Choose how long each shot stays from what the idea needs, not from a fixed length. Put every cut in a pause or at a word's start or end, and let the first and last shots of the idea start and end on its boundaries. Add a slow camera move to stills (`move` in the entry). Where a shot has its own sound and a cut would be abrupt, `clip.audioLead` makes a J or L cut. Then run `edit_checks` with that idea's `beat`, look at two or three frames of your own work (`frame_view`), and tell the person what you did and why in plain words. Wait for their reaction before going on.

## Stage 5: scale
If the checks passed and the person is happy, take a bigger bite: the next idea, then several, then the rest, as far as you are confident, each in one `timeline.placeCutList` call followed by `edit_checks`. If a check fails twice, go back to smaller steps or ask. Take back your own steps with `undo_steps` when they were wrong instead of piling fixes on them. Before changing something the person may have touched, look at `version_status`.

## Stage 6: finish
Done means **every must-pass check is green** when you run `edit_checks` on the whole edit: every stretch of the narration has picture and there are no gaps; no flash frames; every cut is on a word or pause boundary; no shot runs across the start of the next idea; shots are within the video's length range; every clip's rights are known or the person has confirmed them; no unplanned dead air. A clip whose origin is not recorded is for the person to confirm, and is not a failure. Report what the "should" checks say (pace, variety, stills with a move) and where you used judgment, and say plainly that taste is theirs to judge. Leave the commit to the person: say what the version holds and that they can look at the original, edit it, commit it or discard it. Never say you are done while a must-pass check fails.

## When to ask
Ask for decisions that are the person's: taste, cost, rights, and anything large (removing many clips, changing the plan). Do not ask for things you can decide and explain. Put the questions you have into one turn.
