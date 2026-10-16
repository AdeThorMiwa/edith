# Format Packs

A **Format Pack** is a small text file that gives a kind of video its starting structure: the sections and beats, how much of the runtime each takes, and what each is for. You choose a pack and a length in the Planner and get the beats with their lengths already worked out.

Two packs ship with the program, **Mystery** and **Explainer**. You can write your own: no code is involved.

## Where packs live

- Shipped packs are built in.
- Yours go in a folder called `format-packs` next to your preferences (the same folder as `edith-narrator-voices.json`). Each file ends in `.toml`.
- A pack of yours (the Planner marks it "(custom)") with the same `id` as a shipped one **replaces** it.
- A file that does not check out is skipped, and the Planner says which and why. It never stops the others loading.

## The format

```toml
version = 1
id = "travelogue"                # lower-case letters, digits, - and _
name = "Travelogue"
summary = "A place, three encounters, and what stayed with you."
look_preset = "Cinema Look"      # optional: a look that suits it
title_style = "A place name and one odd detail."
typical_minutes = { min = 8, max = 15 }

[targets]                        # optional numbers for the coach
words_per_minute = 150
average_shot_seconds = 6

[[section]]
name = "Arrival"

[[section.beat]]
name = "First sight"
purpose = "What the place feels like in the first minute."
from = 0                         # percent of the runtime
to = 10
intensity = 6                    # 0 to 10
sound = "Street sounds, close."  # a note for the sound plan

[[section.beat]]
name = "Encounters"
purpose = "One person or moment each."
from = 10
to = 90
intensity = 5
intensity_end = 7                # ramps across the copies
repeat = { min = 2, max = 5, default = 3, name = "Encounter {n}" }

[[section]]
name = "Close"

[[section.beat]]
name = "What stayed"
from = 90
to = 100
intensity = 4
```

## Rules

- The beats go in order, each starting where the one before ended (a gap or overlap of up to half a percent is forgiven), the first at 0 and the last at 100.
- A beat that `repeat`s comes in several copies that share its time equally; you choose how many when you create the plan.
- A beat whose intensity ramps without repeating becomes one beat at its middle value.
- Every beat name is different, and a pack makes at most 40 beats.
- The lengths of the beats always add up to the length of the video exactly, to the tick.

`docs/format-pack.schema.json` describes the same format for tools that check files.
