# 0389. A reattach finishes its hand-over before a limit moves the run

## Status

Accepted for [SPEC-547](../../.ai/specs/SPEC-547-stabilize-restart-of-a-reattached-run-at.md), issue #1286.

## Context

Replaying a usage limit requests a subscription move. Reattaching an attempt that still holds the previous owner's MCP servers also requests a restart. Under load, the pool claimed the switch first. The hand-over lost the status update and stopped the run during the pool's switch.

## Decision

Reconciliation takes priority for that task before it attaches the host and starts transcript replay. Replay can record the limit while reconciliation checks the owner and completes any required hand-over. The deferred pool move then checks that the reporting attempt is still attached before it acts on that limit. The startup capacity loop shares this lock when it acts on a persisted pause. A contended pass continues to the next task and requests a prompt recheck. Once it can proceed, it verifies the pause ID against the task's open pause.

## Alternatives

Increasing the test deadline would leave the task stopped. Ignoring a failed hand-over would risk leaving the previous owner's servers in the session.

## Consequences

When a hand-over replaces the attempt, its replacement continues the interrupted turn with the new owner's servers. If the previous subscription cannot take the interrupted turn, the hand-over uses an eligible alternative. With none, its original preflight refusal stops the old host. A model refusal with no known reset is recorded against the reporting account before the move so the existing retry bound still applies. Replay records a model selected inside the CLI with the reporting attempt so the replacement can announce its return to the configured model. When the original attempt stays attached, the pool can still move it or pause it for capacity.
