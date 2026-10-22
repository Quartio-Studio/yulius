# 0202. Vidoc repeats match by place, and an incomplete scan blocks as infrastructure

## Status

Accepted. Specified in [SPEC-273](../../.ai/specs/SPEC-273-vidoc-security-job-in-yulius-ci-too-much.md);
task #262, issue #850. Amends [ADR-0133](0133-a-ci-job-that-checked-nothing-ends-skipped.md) (a
scan that Vidoc's outage cut short is no longer skipped) and
[ADR-0172](0172-medium-findings-in-the-change-block-and-the-rest-become-issues.md) (issues are filed
at the run's end, after the other jobs passed).

## Context

In quartio-v2 (2026-10-21/22) a fifth of the failing `security` findings were repeats: an ignored
or accepted finding raised again in new wording on unchanged code, because an `[[ignore]]` matched
only a title substring. Two runs failed with no finding when an answer stopped at `max_tokens`
(truncated JSON read as unreadable), Vidoc 503s turned the scan into a skip (which passes the merge
gate with nothing scanned), and fail-fast let a failed `security` job cancel `quality`, so the
commit had no test result. Issues for findings outside the change were filed while the run's tests
failed.

## Decision

- **An ignore can name its place.** `[[ignore]]` takes optional `function` and `line`. Outside the
  lines the branch changed, an ignore whose `path` matches also matches a finding in the same
  enclosing function (nearest definition line above it), within 10 lines of its `line`, or that
  Vidoc itself answered as that ignore's accepted risk (`"accepted": "A<n>"`, the ignores of a
  chunk's files being listed in the request). On changed lines only the title matches, so a change
  to accepted code is judged again. Each open finding is shown with its most similar ignore and the
  similarity, for the task and the reviewer.
- **Truncation is split, not failed.** An answer stopped at `max_tokens` splits its chunk in half,
  at most 3 levels deep.
- **Incomplete is infrastructure, and it blocks.** A chunk still not reviewed for a transient
  reason (5xx, 408/409/429, network or timeout past the retries, `max_tokens` at the deepest split,
  an answer unreadable 3 times, the deadline) makes the scan incomplete: exit 3 locally; in CI exit
  `$YULIUS_CI_INCOMPLETE_CODE` (75, `EX_TEMPFAIL`) with one `::yulius-incomplete <files>` line. Yulius
  ends a job that exits 75 in the existing **error** state with the reason `incomplete: …`, so the
  status reads `Error: incomplete: …`, the merge is blocked until a rerun passes, and it is never a
  failed finding. Without the variable (an older instance) the scan exits the skip code as before.
  Not run (exit 2, skipped) stays for what only a person can fix: no key, a refused key, no quota,
  a malformed config, no base.
- **The scan runs beside the tests, not against them.** A failed `security` job does not cancel the
  other jobs and theirs do not cancel it. Fail-fast among the other jobs is unchanged.
- **Issues follow passing tests.** Findings outside the change are filed and reopened when the run
  ends, only when every other job passed or was skipped.

## Alternatives

- **A new `incomplete` run state.** The clearest name, but it touches the store, the contracts,
  the web UI and every exhaustive match for the same gate rule as error. Error already means
  "could not run" and carries the reason to the commit status.
- **Fail the job (exit 1) when incomplete.** The status then says Failed, which reads as a finding
  and is what the person asked to stop.
- **Keep skipping on outages.** Code merges unscanned whenever Vidoc is down; the person asked that
  the merge stay blocked until a rerun passes.
- **Match repeats by title similarity.** A new weakness in the same file often shares words with an
  accepted one (another SSRF, another missing check); a threshold would hide it. Similarity is
  shown, never decisive.
- **Match by place everywhere.** A change to accepted code would never be judged again.
- **Drop fail-fast entirely.** It frees the CI lane when `quality` fails long before `e2e` ends
  (SPEC-145); only the scan, short and independent, is exempt.

## Consequences

- Reworded repeats on unchanged code pass without new ignore entries; reviewers write `function`
  (or `line`) into an ignore so it survives rewording, still with their agreement.
- A Vidoc outage blocks merges of pull requests (on an instance with this change) until a rerun
  passes, and the status says it is infrastructure.
- The place heuristic is textual: a finding just after a function's end can count as inside it.
  It applies only to code the branch did not touch, in a file the ignore names.
- A run whose tests fail files no finding issues; a later green run files them.
