---
name: trellis-check
description: "Review the current task result and use actual application evidence only when needed to confirm that goal."
---

# Current Task Check

Use the dispatched goal, existing task context, and actual changed behavior to decide whether any further evidence is needed. This skill does not make review a mandatory additional phase.

Read only relevant implementation and callers. Fix defects that directly obstruct the current goal or necessary related behavior within the assigned scope; do not turn unrelated issues into new work.

An additional run is necessary only when it directly confirms a user-agreed current business outcome, uses the real application's normal business flow with the relevant real input or deliverable, and resolves a specific gap in the existing applicable evidence. If existing evidence already answers that outcome, perform zero additional runs; an actual delivery operation that supplies the evidence already completes this work without a separate test phase. Running a script, changing code or SHA, or starting a new chat is not a reason to rerun. Unit, mock, synthetic-fixture, static/style, coverage, historical-suite, and governance checks are excluded. If a real business flow fails, fix only that blocked flow within the assigned scope; do not require all-green results, a fixed matrix, or unrelated repairs. Necessary build/packaging remains available without added tests. Preserve actual permissions, data protections, accepted behavior, and the user's delivery standard.

Report the actual result and any specific remaining gap. Existing GPT/Codex responsibilities and submission permissions still apply; this skill grants no additional authority.
