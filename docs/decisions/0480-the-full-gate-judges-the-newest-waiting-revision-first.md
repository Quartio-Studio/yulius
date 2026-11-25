# 0480. The full gate judges the newest waiting revision first

## Status

Accepted. Specified in [SPEC-653](../../.ai/specs/SPEC-653-staging-the-full-gate-judges-the-newest.md)
(NWF-001 to NWF-007); task #637. Supersedes the order of
[0411](0411-staging-changes-through-a-queue-of-tested-merged-revisions.md)'s "the full gate on
exactly that commit" (one candidate run per request, oldest first). Keeps the rest of 0411, the
first-parent attribution of [0450](0450-attribute-integration-test-failures-in-first-parent-order.md)
and [0477](0477-people-and-a-schedule-ask-merge-first-staging-for-the-gated-head.md)'s requests of
no task.

## Context

The merge-first queue judged its `testing` requests oldest first, each with a full run of its
own, one run at a time. In a busy project the verdict on the newest revision waited behind every
older one. In quartio-v2 on 2026-11-25, with 15-minute runs, nine runs went back to back and a
revision's verdict came 45 to 89 minutes after its merge. Two runs judged revisions a green
revision already contained (task #402 was told its gate failed although a green revision held its
change), and three found again the failure the run before had reported. Over four days, a
failing verdict came a median of 18 minutes after its merge (worst 89) and the gate was green
again after a median of 97 minutes.

## Decision

- **A green revision answers what it contains.** First-parent history is linear, so a revision
  contains every revision below it. A `testing` request at or below a revision whose full gate
  passed (no job skipped) needs no run. When that revision is not its own, the request becomes
  `inherited` with that revision and run in its note, so `pick` never deploys its untested
  revision and includes it with the next verified deploy containing it. Its own failing verdict,
  if it had one, is not reported.
- **Newest first.** The candidate run goes to the newest request above every revision the full
  gate judged or is judging.
- **Older requests under a red revision.** One whose red revision above brings no failure the
  judged revision below did not have needs no run: `inherited`. When the red revision brings a new
  failure (a test, a job without identified tests, the run's own failure), the range from the
  judged revision below it to it is bisected over its waiting requests: the middle one is judged,
  then the half the verdict leaves. That is at most `ceil(log2(k + 1))` runs for `k` waiting
  requests, and never more than one run per request. It goes before newer requests.
- **No owner before the range is narrowed.** The red revision's own request stays `testing`, and
  the gate history stops its replay before that revision while its range holds a waiting request.
  So no task is told it broke the gate, and no repair owner is named, for a failure another merge
  may have brought.
- **Unrequested merges are not narrowed.** As before, a failure belongs to the first judged
  revision showing it, and that merge's owner owns the repair.
- **One full-gate run at a time.** A candidate is queued only while no candidate or
  integration-branch run of the sandbox is active, and a running candidate is never cancelled for a
  newer request.
- **Pure decisions.** `yulius_domain::staging_gate::judging` takes first-parent points (position,
  newest verdict, whether a request waits) and returns the covered, inherited and unresolved
  positions and the next run. The queue reads at most 512 revisions below the verified head per
  round. The gate history uses the same function for its stop.

## Alternatives

- **Run only the newly failing tests on the revisions in between.** Fewer minutes per narrowing
  step, but Yulius runs each project's own `[[ci.jobs]]` commands and has no generic way to run one
  named test of any project's suite. A test filter would be a new contract for every project's
  jobs. Full runs over requested revisions need nothing new and cost at most a logarithm of the
  waiting requests, never more than today.
- **Report the whole range and wake every merge in it.** Simpler, but it would wake tasks that did
  not break anything and name no single repair owner, which is less than the queue told before.
- **Judge newer requests before narrowing.** It gives the newest verdict sooner, but a newer
  revision of a red branch stays red until the repair lands, and the repair cannot start before
  its owner is named. Merges every few minutes would also starve the narrowing for ever.
- **Two full-gate runs side by side.** Two suites on one sandbox share its CPU budget: each run
  takes longer, and load-sensitive tests fail more often (seen in the yulius project in 2026-11).
  With the newest judged first, a second lane would mostly judge older revisions whose answer the
  newest gives anyway. The time saved comes from runs not spent.
- **Cancel the running candidate for a newer request.** A run near its end is thrown away, and
  under steady merges no verdict ever comes.

## Consequences

- Replayed at its real times (`staging_gate::tests::morning`), the 2026-11-25 morning spends no
  run on `0d26f0ad9` or `3346ca6a5`, reports the new failure at `867dfe486` at 05:52 instead of
  06:21 and the one at `5d1294696` at 06:51 instead of 07:21, with 5 runs instead of 8 by 07:37.
- A request whose revision a green revision contains is told `inherited` ("needs no run of its
  own") and then `included`, instead of `gate_failed` on a failure already fixed.
- A newest revision that brings a new failure over several waiting requests reports it a few runs
  later than its own verdict, to name the right merge. Without waiting requests in its range (the
  usual case) it reports at its own verdict.
- A request's own verdict that would have failed on a transient failure fixed below a later green
  revision is never seen. The green revision is what staging deploys.
