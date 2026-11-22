---
yulius-task: 704a2d0c-2d19-49bd-9816-fcb5b0ffa2b6
status: in-progress
---

# SPEC-613. An idle Codex session takes messages after a restart

## Goal

Deliver a message to an idle Codex session that survived a server restart. Give the owner an actionable Needs you when the terminal cannot safely receive it.

## Context

On 22 November 2026, task #596 had an idle prompt after reattachment. Six sends were refused and the failed CI notice for PR #1678 never reached the task. The failing screen condition is unknown. The writer in SPEC-603 refuses an unresolved alternate grid. The PR watcher preserves undelivered event keys but can repeat a refusal indefinitely.

## Requirements

- CID-001. On an indeterminate attached Codex grid, request a child redraw without sending input. Check the current attempt and the screen again before each paste and Enter. Preserve viewer geometry, draft protection and the durable paste hold.
- CID-002. Bound a redraw probe to two seconds of waiting for terminal output. If the screen is still unreadable, record an actionable Needs you in the same send attempt. Preserve any question already open. A private message needs the owner to resend it or resolve its request. A repaired screen releases a retryable machine notice.
- CID-003. An owed PR event stays untold until it is submitted. Retry a refused current event on the next watcher pass (normally 20 seconds). A second refusal of the same event keys raises the result and reason to the owner. An unreadable screen or visible picker raises a held machine notice on its first refused send. Check the current event and attempt on retry. A known picker holds both paste and Enter.

## Plan

- [x] Specify bounded recovery for unreadable Codex input
- [x] Recover an idle composer through the attached PTY
- [x] Raise undeliverable messages on the task and retain PR news
- [x] Verify restart, retry and picker protection
- [ ] Restore the PTY size on reconnect and keep private messages actionable
- [ ] Approve the fix for merge into dev and a staging request

## Acceptance criteria

1. An idle host reattached to a new server accepts a message after its unresolved snapshot is repainted.
2. A redraw showing a picker, partial screen or empty grid receives no automated input. A complete composer accepts the message.
3. An unreadable terminal produces a Needs you within the probe bound. A held CI or merge notice is retried or escalated and is recorded as delivered only after Enter succeeds.

## Test plan

Exercise the supervisor, PTY and watcher with synthetic Codex grids, an idle host surviving two server generations, and a real task store. Assert actual submitted text, picker refusal, restored size and persisted owner requests. Run the affected binaries, quick checks and configured security scan. Use the reviewed head's Yulius CI for delivery.

## Decisions

Use the attached terminal and its current child output as the source of truth. A stale transcript or prompt-looking text alone cannot rule out a picker. [Decision 0451](../../docs/decisions/0451-repaint-an-unreadable-codex-terminal-before-typing.md)

## Evidence

- On the original writer, the reattached idle host received zero notices. An unreadable grid produced no Needs you and a second failed write of the same PR event also produced no request. All three new regressions failed for these reasons.
- `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^task_message::.*codex/)'` passed all 13 tests. The idle PTY survived server-runtime replacement and received the CI notice after a SIGWINCH repaint. Cancellation restored the size. The unreadable-screen tests bound the call to three seconds, including the task-store operation, and verify the request and subsequent single delivery. Picker and in-flight-paste tests covered both sides of Enter.
- The fixtures use a synthetic foreground terminal and a legacy-format screen with unknown redraw provenance. The original screen condition in task #596 remains unknown.
- `scripts/cargo nextest run --no-fail-fast -p yulius -p yulius-app -p yulius-store --lib --bin yulius --test main` passed all 4,087 selected tests across five binaries. The default profile skipped 24 tests. `scripts/dev quick` passed. `scripts/dev security` reported disabled by the project.
- Independent review of `cb84c4d86` found two recovery defects and a missing assertion. Disconnecting the PTY during the probe left the viewer at 121×40 instead of 120×40. Repairing a screen let PR news discard the outstanding private-message request. Both regressions failed on that reviewed head.
- Manually removing first-refusal picker escalation made the strengthened picker test fail with “picker refusal did not raise PR news”. The escalation was restored.
