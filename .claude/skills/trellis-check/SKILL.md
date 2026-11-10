---
name: trellis-check
description: "Review the current task result and use actual application evidence only when needed to confirm that goal."
---

# Current Task Check

Use the dispatched goal, existing task context, and actual changed behavior to decide whether any further evidence is needed. This skill does not make review a mandatory additional phase.

Read only relevant implementation and callers. Fix defects that directly obstruct the current goal or necessary related behavior within the assigned scope; do not turn unrelated issues into new work.

Use existing evidence first. Only when the current software goal still needs confirmation, run the actual application through that path and report the observed result. Do not add or run unit, mock, regression, governance, lint, formatting, or type-only checks. Do not require an all-green suite or a full-repository review. Preserve build requirements, actual permissions, business data protections, and the user's accepted behavior.

Report the actual result and any specific remaining gap. Existing GPT/Codex responsibilities and submission permissions still apply; this skill grants no additional authority.
