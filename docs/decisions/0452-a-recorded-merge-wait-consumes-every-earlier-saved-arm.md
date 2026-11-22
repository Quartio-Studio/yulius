# 0452. A recorded merge wait consumes every earlier saved arm

## Status

Accepted for SPEC-615 (issue #1564).

## Context

`armed-merges.json` keeps armed merges across restarts. The ordinary merge queue ends a decision
in memory, then tries to remove the entry (a failure is only logged) and records the final wait
as a finished `wait`/`merge_armed` span stamped with the activation's `armed_at`. The startup
restore compared a saved entry with that span only when the span carried a singleton receipt
(SPEC-558). After a refused clear, a decided ordinary activation was armed again for a live task,
and the next pass decided it a second time over the real outcome.

## Decision

The restore reads the task's latest finished merge wait. When the read or the verdict's detail
fails, the entry stays saved and the task stays unreconciled (it reads as armed from the file and
arm changes are refused) until a later restore reads it. Otherwise a saved entry whose `armed_at`
is not later than the wait's start is consumed, whatever queue decided it: the entry is cleared
and nothing is armed. A later entry is a new activation and is restored with its own `armed_at`.
This holds because only a final decision records a merge wait, stamped with the activation it
decided, and every activation is stamped after the task's latest merge wait.

## Alternatives

Making the ordinary queue keep its verdict pending until the file clear succeeds, as
`finish_singleton` does, would keep an agent waiting on a decision already made because of a
file-system error, and would still need the restore rule for a crash between the two writes.
Consuming without reading the detail would turn a damaged singleton receipt into a silent
release, which SPEC-558 keeps recoverable.

## Consequences

The verdict span is the durable record of a decided activation for both queues. A saved entry
from before activation stamps were recorded (`armed_at` 0) is consumed by any recorded merge
wait of its task; such entries predate SPEC-084.
