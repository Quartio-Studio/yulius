# 0443. Hold automated Codex input at the rate-limit model picker

## Status

Accepted for [SPEC-603](../../.ai/specs/SPEC-603-codex-s-rate-limit-prompt-must-not-switc.md).

## Context

Codex 0.170.0 can show a picker after a turn near a subscription limit. It initially selects a cheaper model. The Enter that follows a Yulius message can accept that item and change both the model and its effort.

## Decision

Set `notice.hide_rate_limit_model_nudge` in the generated Codex configuration. At the terminal writer, hold automated pastes and submits whenever the live Codex screen shows the model picker. A PR notice can be retried by its existing watch after the picker goes. Record an in-flight paste in its run directory before writing it. A successful Enter clears that record. If submission is interrupted, recovery waits for a confirmed empty composer. A viewer can choose to keep their current model with the native controls.

## Alternatives

Suppressing the reminder alone leaves already running sessions exposed. Sending a key sequence to select “Keep current model” depends on the current selection and could type into the composer if the screen changed.

## Consequences

New and resumed Codex sessions start with the reminder hidden. Sessions showing the picker defer automated input until it is dismissed. The screen check uses the text rendered by the pinned CLI. A future renderer that changes the picker needs new replay evidence.
