# 0428. A goal turn continues its declared work

## Status

Accepted for [SPEC-589](../../.ai/specs/SPEC-589-it-shouldnt-ask-for-things-like-that-fix.md).

## Context

An owner who gave an agent a goal had to type “continue” when it reached post-merge verification. The task already held the instruction, an open plan step and access to verification evidence.

## Decision

The shared Stop hook returns a continuation to a goal task with actionable declared work. It names the open step, or directs the agent to finish its unfinished spec. An explicit human handoff, native goal evaluation, background work and a delivery wake keep their existing turn ownership. A run awaiting confirmation of its launched or typed native goal keeps its first Stop for the evaluator, using the same bounded wait as a recorded open goal. Each wait belongs to the authenticated attempt and its Stop time. A replacement invalidates the old wait before its opening turn can run. When a run opens with a verification instruction, a subscription move carries that instruction until the opening turn ends and Claude receives its native goal. Only pending or in-progress steps are actionable. A blocked-only plan shows the recorded dependency. The durable gate counter follows the work across capacity holds and subscription moves. Completing an open step or receiving fresh owner input restores the budget. A restored native goal or a machine wake continues with the existing count. A queue admission keeps its fresh or resumed launch mode and its instruction through a second wait for capacity. A subscription move claims the observed process-free goal pause. The database reads an older turn hold and its process in one snapshot. If a spec diagnostic exceeds the capacity and launch argument limit, the instruction directs the agent to the complete report in spec_check and to the remaining plan in steps_get. The launch and queue record distinguish a Stop instruction from owner text, even when the words match. The terminal records the order and time of a viewer’s submission and fingerprints directly entered text. When the provider’s editor completes or recalls text, the nonempty submit callback confirms it. The hook client generates one invocation ID per submit and reuses it on retries. The run remembers recent invocations and issued instructions, so a replay preserves a pending owner submission and a fresh edited copy of a previously acknowledged instruction can start new work. Legacy clients without an ID use the entered-text evidence. A later draft keeps its own input order. Charging a continuation and restoring the budget on step completion are ordered by the database. After live observations finish, Stop reads the plan and retry count under the plan writers’ lock. Plan writers release the lock before publishing to viewers. Viewers receive the resulting activity once the continuation decision is recorded. The agent records verification and completes the step through the normal task tools.

## Alternatives

Changing the notice wording would still leave the agent idle. Marking verification steps complete upon merge would discard their evidence requirements. Recognizing the words of individual step titles would miss equivalent work in another language or workflow.

## Consequences

A clean goal turn can continue across all three harnesses. A repeated failure to finish within the Stop budget carries the gate's explanation. Prompt-mode tasks still return control after answering their prompt.
