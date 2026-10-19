---
name: video-from-a-script
description: Take a script all the way to a finished video, idea by idea: the voice, footage you have looked at, sound, titles and a restrained polish, with checks. Use when the person has a script (or a narration) and wants the whole video, not only a rough cut.
---

# A video from a script

This workflow is guidance, not a script: follow the stages in order and use judgment inside them. It is the whole job, where the rough-cut workflow stops at the picture. The tools you will use are `request_narration` (the voice), `project_items`, `narration_timings`, `ask_user`, `media_search`, `media_look` and `media_listen` (see and measure before you offer), `request_download`, `timeline.placeCutList`, `timeline.placeSoundList`, `sound.autoMix`, `graphics.placeTextList`, `captions.fromNarration`, `edit.polish` (all through `edit`), `edit_checks`, `frame_view` and `workflow_progress`.

You make the editing decisions; the person decides taste, cost and rights. Work inside a version and never commit it. Record where you are with `workflow_progress` at the start of each stage, when you stop to wait, and when a stage is done, with one sentence on what was decided and what comes next. A cut is the default: most of a good film is hard cuts, and every added thing (a transition, a title, a sound) has to mean something.

## Stage 1: understand and voice
Work out what the pasted text is (see how to read pasted text in your instructions). If it is narration and the timeline has none, offer the voice with `request_narration` in the same turn as your questions and end your turn; carry on when their answer says it is on the timeline. Look at the project (`project_items`, `narration_timings`) before asking anything, then ask what you cannot see, **all in one turn** with `ask_user` (up to six cards): what kind of video this is, the tone, the length and pace they want, whether they want captions, and anything the footage or the sound must or must not include. Do not ask what you can read from the script or the project.

## Stage 2: find the ideas
Split the narration into **ideas**: stretches that make one point or hold one image. Do not split by sentence. For each, a name, one line on what it is about, how intense it should be, and what it should sound like (a bed, a score, silence). Create them in the plan, place each with its boundaries in a pause or at a word's edge, and show the person the list in plain words before you cut anything.

## Stage 3: find footage
For each idea, look first at what is in the project. If nothing fits, search (`media_search`), and **look at the shortlist before you offer anything**: `media_look` on up to six results, then offer the one that really shows the idea with `request_download`, and say why. Never offer a clip whose licence is unknown or does not allow commercial use; say plainly when one needs a credit. If a clip's origin is not recorded, ask the person to confirm it once and carry on. If they say "show me others", leave the choice to them.

## Stage 4: calibrate
Cut **one idea** first with a single `timeline.placeCutList` call: one entry per shot, each with a reason, every cut in a pause or at a word's edge, a slow move on stills. Run `edit_checks` on that idea, look at two or three frames of your own work (`frame_view`), say what you did and why, and wait for their reaction.

## Stage 5: scale
If the checks passed and they are happy, cut the rest in bigger bites, each followed by `edit_checks`. If a check fails twice, go back to smaller steps or ask. Take back your own steps with `undo_steps` instead of piling fixes on them.

## Stage 6: sound
Sound is half the film. For each idea that wants a bed or a score (the plan says which, or the tone does), find it the way you found footage: search for sound, **measure it before you offer it** with `media_listen` (you cannot hear: say "measured, not heard"), and offer it with `request_download`. Place what the person has accepted with `timeline.placeSoundList`: music, ambience and effects each in their lane, fades on, a short bed with `loop` set so it repeats under a crossfade. Then mix with `sound.autoMix`: the voice leads, the music and the bed dip under it. Check with `edit_checks` and `measure` set to true so the loudness of the whole mix is read. Silence is a tool: where the plan asks for it, leave it.

## Stage 7: titles
Read the pack's title style with the plan and write the words to it: an opening title from the film's subject, a chapter title where a section begins if the film uses them, a lower third only where a person or a place is first named. Place them with `graphics.placeTextList`, each up long enough to read. If the person wants captions, make them with `captions.fromNarration`. Run `edit_checks` again.

## Stage 8: polish
Ask what `edit.polish` would do first (with `dryRun` set), say it in plain words, then do it: a fade in and out, a dip where a section ends, a dissolve only between quiet ideas, a slow move on held stills, the rest hard cuts. Do not add a transition the pack's share does not allow, and do not add one because the picture feels plain.

## Stage 9: finish
Done means **every must-pass check is green** when you run `edit_checks` on the whole edit (with `measure` set to true): picture everywhere, no flash frames, every cut on a word or pause, no shot across an idea boundary, shot lengths in range, every clip's and every sound's rights known, no unplanned dead air. Report what the should-checks say (pace, variety, sound under the ideas, music ducked, the mix's loudness, titles readable, an opening title, chapter titles, restrained transitions) and where you used judgment, and say plainly that taste is theirs to judge. Leave the commit to the person: say what the version holds and that they can look at the original, edit it, commit it or discard it. Never say you are done while a must-pass check fails.

## When to ask
Ask for decisions that are the person's: taste, cost, rights, captions or not, and anything large (removing many clips, changing the plan). Do not ask for things you can decide and explain. Put the questions you have into one turn.
