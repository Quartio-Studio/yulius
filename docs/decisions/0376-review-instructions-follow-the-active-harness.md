# 0376. Review instructions follow the active harness

## Status

Accepted for SPEC-524.

## Context

Only Claude Code installs the named Yulius reviewer and has review hooks that select its model for the current round. Codex and Antigravity offer native subagents.

## Decision

The attempt briefing chooses the review instructions. Direct Claude Code to its registered reviewer, Codex to a fresh GPT-6.2 Astra reviewer, and Antigravity to a fresh Gemini 4 Neon reviewer. Use high effort. Codex starts a subagent with a fresh context. Antigravity adds a fresh swarm worker, whose model list includes `gemini-4-neon-high`. Shared delivery instructions refer to the briefing.

## Alternatives

Putting the model in each workflow would duplicate the policy and leave installed or customized workflows stale. Inheriting the writer's model would change the reviewer whenever a person selects another model for a task.

## Consequences

Repeat rounds keep their harness's review policy. The same independent-review and merge requirements apply across all three harnesses. Neon is also the default for a Gemini task without a model choice. Antigravity CLI 1.5.12 supports that choice for both supported Google credential types. Its changelog records removal of `/fast` in 1.1.0.
