# 0428. A goal turn continues its declared work

## Status

Accepted for [SPEC-589](../../.ai/specs/SPEC-589-it-shouldnt-ask-for-things-like-that-fix.md).

## Context

An owner who gave an agent a goal had to type “continue” when it reached post-merge verification. The task already held the instruction, an open plan step and access to verification evidence.

## Decision

The shared Stop hook returns a continuation to a goal task with actionable declared work. It names the open step, or directs the agent to finish its unfinished spec. An explicit human handoff, native goal evaluation, background work and a delivery wake keep their existing turn ownership. Only pending or in-progress steps are actionable. A blocked-only plan shows the recorded dependency. The durable gate counter follows the work across capacity holds and subscription moves. Completing an open step or receiving fresh owner input restores the budget. A restored native goal or a machine wake continues with the existing count. After live observations finish, Stop reads the plan and retry count under the plan writers’ lock. Plan writers release the lock before publishing to viewers. Viewers receive the resulting activity once the continuation decision is recorded. The agent records verification and completes the step through the normal task tools.

## Alternatives

Changing the notice wording would still leave the agent idle. Marking verification steps complete upon merge would discard their evidence requirements. Recognizing the words of individual step titles would miss equivalent work in another language or workflow.

## Consequences

A clean goal turn can continue across all three harnesses. A repeated failure to finish within the Stop budget carries the gate's explanation. Prompt-mode tasks still return control after answering their prompt.
