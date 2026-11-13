# V1 bugs and gaps

The first end-to-end test of Edith on a real script (2026-10-19, one project, one 29-message conversation) is the baseline for **v1** of the app. Everything the person hit, and everything found by reading that conversation afterwards, is listed here so nothing slips through. Each item has: what happened, the evidence, the likely cause, the fix we agreed on or propose, and a test that must exist before it is closed. Status is `open`, `fixing` or `done (commit)`.

Rules for this file: one item per problem; never delete an item, close it with the commit and its test; new findings from the next e2e run go at the bottom of their section. Items marked **(hypothesis)** have a suspected cause that has not been confirmed in code yet; confirm before fixing.

Item numbers are not in order of importance. Items marked "(checklist)" are audits, not bugs. Evidence comes from the saved conversation (`~/Library/Application Support/FilmCraft/edith-threads/3.json`: 9,319 events, 199 model calls, 349 tool calls) and from what the person reported.

## Priority order

This is a suggested order, for the person to change.

1. **Export (U6, U7, B10):** small fixes, and they silently ruin a publish (no captions, no credits).
2. **Cost (C1 to C4):** it makes every other test expensive.
3. **Reliability (R1 to R6):** the editor stops answering, Yes cannot be clicked, turns stop mid-edit, the step limit.
4. **Behaviour (B1 to B11):** what she asks, how she picks, how the workflow starts, stills, captions, sound.
5. **Interface (U1 to U5):** Shift+Enter, cards off screen, disable Yes at once.
6. **New features (F1).**

Sections are lettered C (cost), R (reliability), B (behaviour), U (interface), F (features); items keep their numbers when others are closed.

---

## C. Cost and context

The whole conversation cost about 4 USD for one short video. Nothing in it is unusual: it is what the current design does.

### C1. The context sent on every model call is about 150,000 tokens, and it only grows  ·  open
- **Evidence:** over 199 model calls the totals were 29.8 million cached-read tokens, 1.38 million cache-write tokens, 97,000 output tokens: an average of about 150,000 tokens read per call. One conversation of 29 messages ran for the whole job; no earlier part was ever dropped or summarised.
- **What is in it** (measured from the saved events): 349 tool calls; **63 pictures** kept for ever (42 from `media_look`, 21 from `frame_view`); **114 opaque thinking blocks totalling 324 KB** carried in every later request (the largest single part, bigger than all tool calls and answers together); text of tool results about 200 KB (`media_search` 55 KB over 31 calls, `edit_checks` 39 KB over 12 calls at about 3.2 KB each, `narration_timings` 15 KB, `sequence_view` 13 KB, `command_list` 10 KB, `project_items` 10 KB); the full workflow text (6.5 KB) and the system prompt on every call.
- **Likely cause:** the conversation is replayed whole on every step, with no budget on what a finished stage keeps.
- **Fix (strategy, to be agreed before building):**
  1. **Do not carry old thinking.** Drop opaque thinking blocks from every earlier turn (keep only those of the step in progress, which the API requires). Expected to remove the largest part.
  2. **Pictures expire.** A picture is kept only until the next model call that follows the one that looked at it; the words about it stay. Never more than 6 pictures in the context.
  3. **Old tool results shrink.** After a stage is recorded with `workflow_progress`, the tool results of that stage are replaced by a one-line stub ("searched for X: 6 found, offered 2") in later requests. The saved conversation keeps the full text for the person; only what is sent shrinks.
  4. **Result sizes.** `edit_checks` returns only the verdict and the failing or noted checks (passing ones as one line); `media_search` lists 6 results with the tags cut to 5; `narration_timings` returns a range, not everything, after the first read; `command_list` is queried by word only.
  5. **Stage boundary summaries.** When a workflow stage is done, the conversation so far is summarised into a note of at most about 600 words (plan, choices made, items placed, what is left) and the older turns are no longer sent. The note is saved in the project with the workflow progress, so it also survives a restart.
  6. **A running cost line** in the panel (this conversation so far, and what the next call will cost), so the person sees it climbing.
  7. **Cheaper model for the looking work** is a later option (a smaller model for `media_look` shortlisting); not part of v1.
- **Test:** a scripted 40-step run where the request size of step 40 is under 40% of what it is today, no picture older than one call is sent, no thinking block from an earlier turn is sent, and the stage summary contains the choices the person made. A replay test that the saved conversation still shows everything.

### C2. The per-question budget stopped the work three times  ·  open
- **Evidence:** three turns ended `budget_reached`, nine ended `step_limit`, one `stopped`. The person had to type "continue" or "Please continue" 10 times.
- **Cause:** defaults of 12 steps and 1 USD a question, and a workflow takes dozens of steps.
- **Fix:** see R4 (step limit off by default). The cost cap stays as an optional safety, but is off by default as well, and is replaced by the running cost line in C1; a warning (not a stop) at a limit the person sets.
- **Test:** with limits off a 60-step scripted run finishes in one turn; with a limit set it stops and says so once.

### C3. A turn can fail on a damaged stream and cannot be retried  ·  open
- **Evidence:** one turn ended `failed`: "the reply was not understood: the stream ended inside a part of the reply" (`retryable: false`).
- **Fix:** treat a stream that ends early as retryable (once, automatically, from the step's start), and say so in one line. Never leave the person with a dead turn after a network blip.
- **Test:** scripted model that cuts a reply in two; the turn completes on the retry.

### C4. The same lookups are made again and again  ·  open
- **Evidence:** `media_search` for "book pages turning close up dark" four times, "telescope observatory dome night scientist laboratory microscope" four times, "supernova explosion nebula space animation" twice; `project_items` 11 times, `sequence_view` 12, `clip_detail` 14, `edit_checks` 12.
- **Fix:** a search made earlier in the conversation with the same words and kind answers from what was found (and says so), unless the person asked for others. The prompt tells her to look again only when the project changed.
- **Test:** the second identical search makes no network request.

---

## R. Reliability

### R1. "The editor did not answer in time": about 26 tool calls failed this way  ·  open  (hypothesis)
- **Evidence:** 26 tool results were the error "the editor did not answer in time" (`media_listen` 9, `edit` 4, `media_search` 3, `request_download` 2, `workflow_progress` 2, `clip_detail` 2, `request_narration` 1, `project_items` 1, `project_overview` 1, `edit_checks` 1, `sequence_view` 1, `analysis_get` 1). The person saw her say that the editor "isn't answering right now", that the narration card timed out, and that she could not tell whether it reached them. This is also what "it sometimes stops mid-edit" looks like from outside.
- **Likely cause:** every tool call is answered on the app's own thread by `pump_assistant`, which runs once per drawn frame (`crates/ui-egui/src/lib.rs:2020`). A tool call waits 60 seconds (`TOOL_WAIT`, `crates/engine/src/assistant.rs:61`) and then gives up. When the window is not being drawn (another Space or app in front, minimised, the Assistant tab not showing so its repaint request is not made, or one slow frame such as a long sound decode), no frame runs and the call times out. The repaint request that keeps frames coming (`crates/ui-egui/src/panels/assistant.rs:274`) is made only from inside the assistant panel's drawing code.
- **To confirm:** log the time between frames while a turn is running and the window is in the background; reproduce by switching Space during a turn.
- **Fix:** (a) a tool request wakes the app (a repaint request made from the thread that sends it, not from the panel); (b) a call that has not been answered is **not** abandoned after 60 seconds while the turn is still running and not stopped: it keeps waiting, and the panel shows "waiting for the editor"; (c) the slow steps (sound decode in `media_listen`, picture rendering) never block a frame for more than a few milliseconds; (d) when the person comes back, everything that waited is answered in order.
- **Test:** a session that is not pumped for 90 seconds and then is: the call is answered, not failed. A scripted turn where the pump is paused mid-turn finishes after it resumes.

### R2. The Yes button on a card could not be clicked at times; "Show me others" had to be used instead  ·  open  (hypothesis)
- **Evidence:** the person had to use "Show me others" six times (cards c20, c21, c22, c25, c26, c27, each answered "they will look at other options themselves in Find Media") to get clips, and said: "for some reason the card was acting up". In one turn she offered 7 downloads, in others 5, 4, 4.
- **Likely cause:** `can_answer` is `live && not answered && !running` (`crates/ui-egui/src/panels/assistant_cards.rs:133`). While the thread counts as running (the model carrying on after offering a card, or a turn stuck after a timeout from R1), Yes and the choices are disabled, while "Show me others" is enabled by a different condition. The person is then pushed to the one button that still works.
- **Fix:** a card is answerable whenever it is open, whether or not the model is still talking; answering while she works is queued and sent when her turn ends. "Yes" and "Show me others" use the same rule. Also: a turn should end when she has put cards on the screen (the prompt says so; enforce it in the engine: after cards are shown, no further tool call is run until the person answers).
- **Test:** UI test with a running thread: Yes is clickable and starts the download. Engine test: after `request_download`, further tool calls in that turn are refused with "wait for the person".

### R3. A turn stops mid-edit and the person has to type "continue"  ·  open
- **Evidence:** 10 turns of the 29 were just "continue" or "Please continue". "So why stop? Continue please." Causes seen in the log: the step limit (9), the cost limit (3), a failed stream (1), and the timeouts in R1.
- **Fix:** C2, C3, R1, R4. When a turn does end for a reason other than "done", the panel says why in one plain line with a button that continues, and Edith continues by herself where it is safe (not after a limit the person set).
- **Test:** each stop reason has a test that the panel names it and that Continue resumes with the same version.

### R4. The step limit cannot be turned off, and is on by default  ·  open
- **Evidence:** the setting is a number from 1 to 40, default 12 (`crates/ai/src/settings.rs:69`, `crates/ui-egui/src/panels/assistant.rs:512`, engine `maxSteps` accepts only 1 to 40). There is no "off". The person asked for it to be off by default.
- **Fix:** `maxSteps` accepts `null` meaning no limit; the default is null; the settings control has an on/off switch and a number when on. The per-question cost cap also defaults to off (see C2). A hard internal ceiling (500 steps in one turn) remains as a runaway guard, and says so if hit.
- **Test:** settings round trip for on and off through the command and through the UI control; with off, a 60-step scripted turn completes; with on at 5, it stops at 5 and says so; the setting is saved and restored.

### R5. A search result from earlier in the conversation can no longer be used  ·  open
- **Evidence:** errors such as "no result pixabay:video-10339 in this session: search with media_search first" for `media_look`, and the same for `request_download` and `media_listen` of Wikimedia results.
- **Likely cause:** the hub finds a candidate only in the **latest** search, the shortlist and pending downloads (`Hub::candidate`), so a new search drops the earlier results she was still working from.
- **Fix:** keep the candidates of every search made in the conversation (capped at about 300, oldest out), so an id she was given stays valid until the project closes.
- **Test:** search A, search B, then `media_look` an id from A works.

### R6. Questions rejected for a label that is too long cost a round trip  ·  open
- **Evidence:** two `ask_user` calls refused: "each option needs a label of 1 to 80 characters". Same for a `reason` over 300 characters on a cut, and a `workflow_progress` note over 300.
- **Fix:** cut over-long labels, reasons and notes to the limit and say so in the result instead of refusing (the text is only a label).
- **Test:** an 150-character label is accepted, shortened, and the card shows it.

---

## B. Behaviour

### B1. The workflow is too visible and must be started by the person  ·  open
- **What the person wants:** it should work like skills in Claude Code: subtle, found by Edith when the request calls for it, never a button the person has to press. No header or strip telling the person which workflow and stage she is in; they only need to see that it is working.
- **Evidence:** the first message of the conversation was "Let's use the “video-from-a-script” workflow." (the person pressing Workflows ▸ video-from-a-script); the panel shows a workflow strip with the stage; her messages say "I'm in stage 1 of the script workflow".
- **Fix:** (a) remove the Workflows starter list from the panel and the workflow strip (keep a quiet way to see progress in the conversation's menu for debugging); (b) the prompt lists the available workflows by name and one line each, and tells her to load the one that fits the request without announcing it; (c) she never says "stage", "workflow" or its name to the person unless asked; she says what she is doing in plain words; (d) progress is still recorded (`workflow_progress`) for resuming, invisibly; (e) pasting a script into a project with nothing in it is enough to start.
- **Test:** an evaluation task: a pasted script in a new project loads `video-from-a-script` with no mention of it by the person, and the answer contains none of "stage", "workflow". UI test: no Workflows control and no strip.

### B2. She asks what kind of video it is before there is a script  ·  open
- **Evidence:** her first questions were "How do you want to give me the script?", "What kind of video is it, and what tone should it have?", "Roughly how long should it run?", "Do you want captions burned in from the narration?", asked with nothing in the project. We agreed these are only asked when she has the script and cannot work them out from it.
- **Fix:** with no script and nothing in the project, she says, in one sentence, that she is ready for the script, and asks nothing. Once there is a script she works out the kind, the tone and the length from it and asks only what she could not (at most the one or two that matter most), and captions are asked at the end, not at the start.
- **Test:** an evaluation task: the workflow requested with an empty project produces no `ask_user` call; a task with a script: the questions asked are not about things the script states.

### B3. She almost always takes the first result  ·  open  (partly looked at)
- **Evidence:** 30 of 34 download offers were Pixabay results, and the person noticed "it always uses the first result". Check of the log: searches list results in the order the hub returns them; `media_look` was used 12 times for 31 searches.
- **Likely cause:** results are interleaved source by source, so rank 1 of every source comes first; with only one source answering (see B4), rank 1 is always that source's first hit; the titles are tag lists ("alone, person, man, people, guitar, sing, lonely…"), so there is little to choose on besides position; she looks at pictures only when the prompt tells her to and not for every shortlist.
- **Fix:** (a) the hub ranks results by how many of the search words match the tags and title before interleaving; (b) a result's line shows the five best-matching tags, not all of them; (c) she looks at up to six results for every idea (prompt, and the workflow stage says it is not optional), and offers the one that best shows the idea, not the first; (d) she never offers more than one clip per idea per turn unless the person asked for options.
- **Test:** a fake search where the best match is rank 4: ranking puts it first; an evaluation task where a scripted assistant must `media_look` before every `request_download`.

### B4. Every video came from Pixabay  ·  open
- **Evidence:** the search result lines say: "pexels failed (no pexels key is stored; add one in Connections), pixabay 7 found, wikimedia 0 found, archive 0 found, nasa 0 found, local 0 found." for the first search and the same pattern in later ones.
- **Causes (two):** (1) no Pexels key is stored, so the other keyed source for video is silent. (2) **Wikimedia, the Internet Archive and NASA found nothing for any long descriptive search**, because they match every word of the search (`query::keywords` allows 8 words and the Archive joins them with AND). Pixabay matches on tags, so it always answers. Only 2 of 31 searches used any source but Pixabay in practice.
- **Fix:** (a) searches to the open sources use the **three or four most distinctive words**, not eight, and if that finds nothing they retry with fewer; (b) the hub shows, per search, which sources answered and which found nothing, and she tells the person when a source is silent because of a missing key (once, with where to add it); (c) the panel's Connections shows "Pexels: add a key for more video" prominently; (d) the result list takes at most 3 from any one source before taking from others, so one source cannot fill it.
- **Test:** a recorded long query returns results from NASA and Wikimedia after the retry; the result list never has more than 3 from one source when others have results.

### B5. She uses only video, never pictures with a move  ·  open
- **Evidence:** 20 searches were for video, 9 for sound and **2 for pictures**. The person: "you know you can use picture with effect (ken burns, etc) so you're not limited."
- **Fix:** the workflow and the prompt say that a still with a slow move (the cut list's `move`) is a normal, often better, shot, especially for abstract ideas, archive material, places, diagrams and anything stock video does badly; for each idea she searches video **and** pictures (one search can ask for both kinds), and the card says which it is. `media_search` accepts both kinds at once.
- **Test:** evaluation task: for an idea that stock video cannot show, a scripted assistant that offers a picture with a move passes; one that gives up fails.

### B6. The sound was not audible and not melodic  ·  open
- **Evidence:** the person: "the sound is not melodic. And I can't hear it 95% of the time… we need something melodic playing more visibly in the background that gives the video the dreamy tone"; "the piano you added is not audible".
- **Likely causes:** the auto-mix sets music at -22 LUFS against narration at -16 and then ducks by 15 dB under the voice, so under continuous narration the music sits near -37 LUFS: inaudible. Also she picked ambient drones (`media_listen` reads no tune) for a request for melody.
- **Fix:** (a) default targets raised (music -20, ambience -26) and the duck depth reduced to about 9 dB by default (the pack can set it); (b) `sound.autoMix` reports the level of the music **under the voice** in LUFS and says plainly if it is below -32 ("you will barely hear it"); (c) she picks sound against the tone asked for, and when the person asks for melody she looks for a tune (a pulse and a changing pitch, from `media_listen`), not a drone; (d) a new should-check: music is audible under the voice (at least 12 LU under the voice and at least -34 LUFS).
- **Test:** `sound.autoMix` on a voice and a score leaves the score at least -34 LUFS during speech; the check fails when it is below.

### B8. Ken Burns on a still that is smaller than the frame, or the wrong shape, shows black bars  ·  open
- **What the person said:** they told her to use Ken Burns or similar on still images, and it "wasn't able to".
- **Evidence:** the four Pixabay stills in the project are 1280×853, 1280×853, 1280×848 and 1280×960; the frame is 1920×1080. `placeCutList` with `move: "pushIn"` and `motion.cameraMove` returned success, but they set the scale to 100% to 115% of the picture's own size and assume the picture already fills the frame (stated in `crates/engine/src/camera_moves.rs`). A test of the same call: a 3000×2000 still fills the frame; an 800×1200 portrait is 55% black; a 640×480 still is 85% black. Edith then fixed each by hand with `effects.setParam` keyframes at 152% to 175% scale, using about 20 calls (and one of her calls to hand-key left a clip with no animation: `keyframes: 0` on clip 260 for the second half of a split).
- **Cause:** the camera move does not first fit the picture to the frame. She also had no way to know a still was too small until she looked at a frame.
- **Fix:** (a) `motion.cameraMove` and the cut-list `move` first **fill the frame** (cover: scale so the picture covers it, cropping the overflow) and then add the move on top, so any still of any size and shape gets a clean move; (b) an option `fit: "fill" | "fit"` (default fill) for when the whole picture must show, with a blurred copy behind for the bars; (c) when a still is much smaller than the frame (under about 60% of its width) she is told its size in `media_search` results and `project_items` and says so before using it; (d) a still that is split across two cuts keeps one continuous move.
- **Test:** for each of four sizes and shapes, no black at the start, middle or end of a pushIn, pullOut and drift; a still cut in two entries has one continuous move.

### B9. Captions have no punctuation and break in odd places  ·  open
- **Evidence:** in the project, the narration's word list has no punctuation at all ("Welcome", "to", …, "channel", "Let's"), so captions read "Welcome to the first video on this new / channel Let's get into what today's" and "question Activities like music books meditation psychedelic". Sentences run together and a line breaks between "today's" and "video".
- **Cause:** the voice's word times (`SpokenWord`) hold only the spoken word; the commas and full stops of the script are dropped, and the caption grouping does not know where sentences end (the sentence number is kept, but not used).
- **Fix:** keep the script's punctuation with each word (a trailing `, . ? ! ; :` and dashes stay on the word), and make the caption grouping break at sentence ends first, at commas second, and only then at the line length, keeping a line from ending on a small word like "the" or "today's".
- **Test:** captions made from a narration with punctuation keep it, and no caption spans two sentences unless it has to for length.

### B10. Licences that need a credit are used but never credited  ·  open
- **Evidence:** the final project uses two tracks that require attribution: "Rogue Planet (mystery ambient)" by SondreDrakensson (CC BY 4.0, Freesound) and "The Long Dark" by Scott Buckley (CC BY 3.0, Wikimedia). The provenance records say credit is required, and Edith said so in chat ("Open: Rogue Planet credit"), but nothing puts a credit in the video, and no description text is produced. Publishing without the credit breaks the licence.
- **Fix:** (a) when a clip that needs a credit is placed, Edith adds it to a credits list kept with the project (the provenance already holds `creditText`); (b) at the end she offers, as a card: "credits as end text in the video" (a Credits card made with `graphics.placeTextList`, role `note` or a new `credits`), "credits as text for the video description" (copied or written next to the export), or both; (c) a new **must-pass check**: every used clip whose licence requires a credit has its credit in the video or in the description file, or the person has said they will add it themselves.
- **Test:** a project using a CC BY sound fails the check until the credit exists; the credit text matches the record.

### B11. Clicks in the sound after the audio ends, around 3:30  ·  open
- **Evidence:** the person reported "there are clicks after the audio ends at around 0:3:30". In the project the Music lane has a planned silence from 213.5 s to 214.9 s (3:33.5 to 3:34.9): the first music clip ends at 213.5 s and the second starts at 214.9 s, with the narration and the looped drone around it. Edith's own guess in chat was a cause for the clicks (a cut in sound with no fade), then she changed levels; whether the clicks are gone was never checked.
- **Likely cause:** a sound clip that ends or starts without a fade, or a loop seam of the drone at that point, leaves a click when the level jumps; `placeSoundList` adds fades by default, but the clips changed afterwards by hand (gain and level edits) may have lost them.
- **To confirm:** render the mix around 3:25 to 3:36 and look at the waveform for a jump; find which clip ends or starts there.
- **Fix:** every sound clip edge, however it was made, ends and starts with a short fade (at least 10 ms) unless it butts against another with a crossfade; `edit_checks` gets a should-check "no hard sound edges" that lists any clip edge with a level jump; `sound.autoMix` repairs them.
- **Test:** a clip with no fade is flagged; after the repair the jump at the edge is below a threshold in the rendered mix.

### B6b. The music has to be audible all the way through, not only in the pauses  ·  open
- **What the person said:** "I need it to be audible at all time throughout the video, not just when the narrator stops talking." Edith's answer was to remove the dip under the voice and set the piano to a steady level "about 5 dB below the voice. That's a guess from the numbers, because I can't hear it."
- **Fix:** see B6 (audible under the voice, a check for it). Add: a mix reading in the report of the music level **during speech** and **in the pauses**, so she is not guessing; and a way for the person to say "louder" or "quieter" once and have it kept as the mix's music level (so a later `sound.autoMix` does not undo it).
- **Test:** `sound.autoMix` keeps a person's stated music level; the report quotes both levels.

### B7. First-turn behaviour when the person types something that is not the script  ·  open
- **Evidence:** turn 2 was her list of questions; turn 6 and 7 were the person saying "I didn't see anything" and "no video at all, just blank screen?": after the voice was made nothing appeared on the timeline picture for a long time.
- **Fix:** after the voice is placed she says in one line what is on the timeline and what happens next; the picture is cut before sound and titles. If the timeline has no picture she does not describe the project as having a video.
- **Test:** evaluation: after Yes on the voice card the next message names what is on the timeline.

---

## U. Interface

### U1. Shift+Enter does not start a new line in the chat box  ·  open
- **Evidence:** reported; the input is a multiline field (`crates/ui-egui/src/panels/assistant.rs:925`) but Enter sends and Shift+Enter is not treated as a new line.
- **Fix:** Enter sends, Shift+Enter (and Alt+Enter) inserts a line break; a paste with line breaks stays as typed. A script is pasted into this box, so this matters.
- **Test:** UI test: Shift+Enter adds a newline and does not send; Enter sends; the draft keeps its lines.

### U2. The question boxes run off the screen, so Send answers is out of sight  ·  open
- **Evidence:** the Send answers button is not visible until the assistant panel is made full screen.
- **Likely cause:** the cards are drawn inside the transcript's scroll area under the last card with no minimum space and no pinned footer; when several cards are open the button is below the visible part and the scroll does not follow it.
- **Fix:** the group's Send answers bar is **pinned to the bottom of the transcript** (above the input), always visible while cards are open, with "n of m answered"; cards scroll above it; when new cards arrive the transcript scrolls to the first unanswered card.
- **Test:** UI test at a small panel size (300 by 500) with six cards: the Send button's rectangle is inside the panel.

### U3. Yes should be disabled the moment it is pressed  ·  open
- **Evidence:** requested. At present the button stays active while the download starts, so it can be pressed twice.
- **Fix:** on press the card goes to "Adding…" at once (button disabled, spinner), then "Added" or the error; the same for the voice card and for Send answers.
- **Test:** UI test: two quick clicks start one download; the second does nothing.

### U4. Cards need a state that matches what happened  ·  open
- **Evidence:** the person asked to see "the cards for the other ones again (for some reason the card was acting up)".
- **Fix:** a card that was answered with "Show me others" stays visible as such and can be reopened (Undo for a card that has not been sent); the panel has a Cards list for the conversation so none is lost.
- **Test:** a card answered with Others can be turned back into an open card before sending.

### U5. The assistant panel header and strip add noise  ·  open
- **Fix:** with B1 done the workflow strip goes; the rest of the panel's chrome is looked at again in the next e2e run (what the person did not need to see).

---

### U6. Captions are left out of the exported video unless the person finds an export setting  ·  open
- **Evidence:** the person exported and the subtitles were missing. Edith had made 57 captions (`captions.fromNarration`, track "Narration", enabled, styled). The export screen's Captions ▸ Export Options defaults to **None** (`burn_captions: false`, `crates/export/src/lib.rs:559`, `crates/ui-egui/src/panels/export_mode.rs:557`); the other choices are "Burn Captions Into Video" and "Create Sidecar File". Edith had asked "Do you want captions burned in?", the answer was yes, and nothing carried it to the export setting.
- **Not a renderer bug:** a scratch test with the same commands exported a frame with the option off (no text) and on (clean white caption with outline near the bottom).
- **Fix:** (a) when the sequence has captions the export default becomes "Burn Captions Into Video"; (b) if captions exist and the option is "None", the export screen shows "57 captions will not appear in this video" with a one-click switch; (c) when Edith makes captions she says once that they are burned in at export, and offers the `.srt` sidecar too (YouTube's own subtitles are often better); (d) the answer to "captions?" is remembered with the project so export uses it.
- **Test:** UI test: default flips when captions exist; the warning shows; engine test: export with the project's choice includes the caption pixels.

### U7. Export audit: what Edith added versus what an export includes  ·  open (checklist, from reading the project)
Checked on the autosaved project (`Untitled-2026-10-19_12-35-56.fcproj`): titles (4, track "Titles"), captions (57), Music, Ambience and Effects lanes, 5 transitions on picture, picture, sound and captions all end at 262.7 s, no track muted or hidden, no in/out range set. All of those are in the sequence and will export. Things that will **not** be in the exported video, which the person may not expect:
1. Captions unless burned in (U6).
2. **Credits for licences that require one** (B10).
3. Markers ("Why this shot", "Why this sound"), the plan, the workflow notes: working notes, by design.
4. An export made while a version is open exports the version (what is on the timeline), which is what the person sees; no problem found, but the export screen should say "exporting: version <title>" so it is clear.

---

## A. Architecture (decided changes that touch many items)

### A1. Everything of a project should live in one folder named after the project  ·  open (design decided 2026-10-20)
- **Request:** when a project is created, `<parent>/<Project Name>/` holds all of it: the saved project, exports, transcripts, conversation history, memory files, downloaded and generated media, auto-saves.
- **Today (code survey):** a project is a single `.fcproj`; downloads, narrations, recordings and exports land flat in its parent folder; previews in `FilmCraft Previews/<stem>`; auto-save in `Auto-Save/`; proxies beside the original media; conversations in the app data folder (`edith-threads/`, shared by a copied project); an unsaved project keeps its files in the data folder or `tmp`; media paths are absolute, so a copied project breaks. Transcripts, plan, credits and versions are already inside the project file.
- **Layout and method:** see the plan file (`binary-mixing-toast.md`, "Project folder"): `Media/`, `Proxies/`, `Previews/`, `Auto-Save/`, `Exports/`, `Assistant/` beside `<name>.fcproj`; New Project asks for a name and a parent (default `~/Movies/Edith`, remembered) and creates the folder at once; each writer goes through one funnel function (`media_dir`, `default_export_dir`, `auto_save_dir`, `dir_for_project`, `scratch_paths`, the assistant store directory); media inside the folder stored as relative paths; Save As copies the whole folder with a new project key and asks "Copy the Assistant chat into the new project?" (default yes). Own footage stays a reference in place, with a "Copy into project" action. Global things stay global (models, preferences, libraries, crash-recovery journal).
- **Closes or simplifies:** the global 50-thread cap and project-key scoping, the Save As conversation sharing, "unsaved project" file handling, U7 (exports in one known place), and the portable-summary step of the chat plan.
- **Existing projects:** moved by hand by the person, no migration code.
- **Test:** new project creates the folder; every kind of file lands in its subfolder; moving the folder and opening it finds the media; Save As copies the folder with a new key and honours the chat prompt; an unsaved state does not exist; the crash-recovery journal still finds the project.

## F. New features

### F1. Reference a clip or a point on the timeline from the chat  ·  open
- **Request:** "a way to click a clip or a point on the timeline and reference it in the assistant chat."
- **Proposal:** (a) a right-click item on a clip, a marker or a time on the ruler: **Ask Edith about this**, which adds a chip to the message box ("clip: Ocean_Sunset at 0:12–0:18", "point: 0:38"); (b) while the chat box is focused, clicking a clip adds its chip; (c) selected clips and the playhead already have chips: make the chip show what it holds and let it be removed; (d) the chip is sent as the same context blocks the chips already send (selection, playhead), with the clip's id so she never has to guess.
- **Test:** UI test: the menu adds a chip; the sent message carries the clip id and the time.

---

## What the first run got right (keep)
- The voice-it card made the narration and put it on the timeline.
- Word-exact cutting and the checks held: she cut in pauses and ran `edit_checks` repeatedly.
- Sources: Openverse and Freesound music were found and offered with licences; `media_listen` and `media_look` were used (14 and 12 times).
- Cards answered together as one message worked for questions.

## E2E v1 acceptance
v1 is declared when one run on a script of about a minute, from an empty project, finishes **without** the person typing "continue", without a timeout error, with the cost shown and under a limit the person chose, with video **and** picture shots from more than one source, with sound the person can hear, titles and a polish, and with no item in the sections above still open. The run is saved (conversation file) and read afterwards the same way this one was.
