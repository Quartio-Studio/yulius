---
yulius-task: ae2a5492-10b9-4841-86aa-4598bdb91b6a
status: in-progress
---

# SPEC-273: Less noise from the Vidoc security job

Status: in-progress
Date: 2026-10-22
Owner: Yulius task #262
Scope authorization: task #262 (person's request, evidence from project quartio-v2 tasks #7–#13).
Fixes #850. Amends SPEC-195, SPEC-217 and SPEC-162 (ADR-0133); decision
[ADR-0202](../../docs/decisions/0202-vidoc-repeats-match-by-place-and-an-incomplete-scan-blocks-as-infrastructure.md).

## Goal

The `security` job fails only on what someone has to act on: a reworded repeat of an accepted risk
passes, an answer Vidoc cut short is retried rather than failing, a scan that could not finish says
so as infrastructure, and one failing job never hides another's result or files issues for a run
whose tests failed.

## Context

Evidence (quartio-v2, 2026-10-21/22): 47 failed `security` runs, 69 failing findings; 59% real,
22% false positives ignored with the reviewer's agreement, **19% repeats**: an ignored or accepted
finding raised again in new wording on unchanged code. About 12 of ~50 `[[ignore]]` entries exist
only for a rewording; ADR 0017's accepted risk ("write-confirmation decision made by an LLM judge")
kept coming back. Two runs failed with no finding (a chunk hit `stop_reason: max_tokens`, its files
unscanned; #11 at 179ce81 and d9ae76c), Vidoc 503s made the scan skip (exit 78) twice, and a failed
`security` job cancelled `quality` ("stopped early: job security failed"), so there was no test
result for that commit. The person added: issues were filed while the tests had not passed
(screenshot: issues #837–#841 filed by a run whose `quality` and `e2e` failed).

Root causes (reproduced in code, see Evidence):

- `scripts/vidoc_scan.py` `apply_ignores`: an ignore matches only when its `title` is a
  case-insensitive substring of the finding's title. Vidoc rewords titles between scans
  (SPEC-217, SPEC-262), and it is never told which risks were accepted.
- `review` / `parse_answer`: an answer with `stop_reason: max_tokens` has truncated JSON, is
  "unreadable" three times and then raises `Failed` (exit 1). `Client.post`: 5xx, 408/409/429 and
  network failures past 5 attempts raise `Usage` (exit 2, "not run"), which `--ci` turns into the
  skip code; a skipped job passes the merge gate (ADR-0133), so code merged unscanned.
- `crates/yulius-app/src/ci/exec.rs` `ci_execute_inner`: fail-fast; any failed job sends the shared
  sibling cancel ("stopped early: job X failed") to every job, `security` included both ways.
- `ci/exec.rs` `ci_job`: `ci_follow_findings` runs when the `security` job ends, before (and
  whatever) the other jobs end.

## Requirements

- NOI-001 (repeats by place). An `[[ignore]]` may name where the accepted code is: `function`
  (the name of the enclosing function, class or impl) and/or `line` (the flagged line when it was
  triaged). Besides the title substring match (unchanged), an ignore whose `path` glob matches the
  finding's file matches a finding of any wording when (a) its `function` equals the function
  enclosing the finding's line (the nearest definition line above it: `fn`, `def`, `func`,
  `function`, `class`, `struct`, `enum`, `trait`, `impl`, `interface`, `module`), or (b) its `line`
  is within 10 lines of the finding's line, or (c) Vidoc answered the finding as that ignore's
  accepted risk (NOI-002). (a)–(c) hold only on code the branch did not touch: a finding within 3
  lines of a changed line, in a function the branch changed anywhere, within 10 lines of a changed
  line of code (only lines above every definition, such as imports, may move code unjudged),
  without a valid line, or in a new file, matches by title only, so a change to accepted code is
  judged again (review rounds 1 and 2). A definition is a code line that starts (after
  indentation and modifiers) with one of those keywords, so comments and docstrings are none.
  In a file with no such line (C, shell, SQL), any change in the file makes a finding touched.
- NOI-002 (Vidoc is told). Each request lists, as data, the ignores whose `path` matches a file of
  its chunk, each with an id (`A<n>`), path, place, title and reason. Vidoc is asked to report a
  weakness that is one of them with `"accepted": "A<n>"` instead of as a new finding. An id that
  names no ignore, or one whose path does not match the finding's file, is disregarded.
- NOI-003 (nearest ignore). Each finding not ignored is printed (and written in `--report`) with
  the most similar ignore of its file (title similarity, `difflib` ratio, as a percentage) and
  whether it is in the same function, so the task and the reviewer see a likely repeat.
- NOI-004 (split on max_tokens). A chunk whose answer stops at `max_tokens` is split in half (by
  files, or a single file's line range) and each half reviewed, at most 3 levels deep; findings of
  the halves count as usual.
- NOI-005 (incomplete). A chunk that cannot be reviewed for a transient reason (5xx, 408, 409, 429
  or a network failure or timeout past the retries, `max_tokens` at the deepest split, an answer
  still unreadable after 3 tries, the deadline) leaves its files **unscanned** and the scan
  **incomplete**: exit 3, listing the files. Open failing findings still win (exit 1). Not run
  (exit 2) stays for what only a person fixes: no key, a refused key, no quota (402, or a 429 whose
  message says the key is used up), a malformed config,
  no base, an old Python, and for the byte budget.
- NOI-006 (CI incomplete). With `--ci`, an incomplete scan exits `$YULIUS_CI_INCOMPLETE_CODE` (75,
  `EX_TEMPFAIL`, given to every job) after printing one `::yulius-incomplete <text>` line naming the
  unscanned files; without that variable (an older instance) it exits the skip code, as before. A
  job that exits 75 ends **error** with the reason `incomplete: <text>` (or a generic one), so the
  commit status reads `Error: incomplete: …`, the merge stays blocked until a rerun passes, and it
  is never a failed finding. It does not stop the other jobs.
- NOI-005a (quota). A 429 whose message says the key is used up ("used its", "out of quota",
  "insufficient credits", "billing") is no quota, not run; any other 429 is a passing limit.
- NOI-007 (no cancel across the scan). A failed `security` job does not cancel the run's other jobs,
  and another job's failure does not cancel the `security` job; both run to the end and report.
  Fail-fast between the other jobs and the run's own cancel (superseded, stopped) are unchanged.
  While the others run, the commit status says the security job failed
  (`Running on Yulius: 0 of 3 jobs passed, security failed`).
- NOI-008 (issues after the tests). Findings outside the change are filed or reopened as issues when
  the run ends, and only when every other job of the run passed or was skipped; otherwise the
  security job's output says how many wait for a run whose other jobs pass. Closing issues whose
  code is gone from the integration branch is unchanged.
- Unchanged: critical and high findings anywhere and medium ones in the change fail the scan; an
  ignore still needs the independent reviewer's agreement.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec SPEC-273 and ADR for repeats, incomplete scans and job independence
- [ ] Reworded repeats of an ignore pass: location match, Vidoc's accepted answer, nearest ignore shown
- [ ] A chunk cut at max_tokens is split; Vidoc 5xx/timeouts end the scan incomplete, naming the files
- [ ] CI shows an incomplete job as error 'incomplete: files' in the status; merge stays blocked
- [ ] A failed security job no longer cancels the other jobs, nor they it
- [ ] Findings outside the change are filed as issues only when the run's other jobs passed
- [ ] Docs, briefing, changes entry and every gate green
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Merged into dev on green CI

## Acceptance criteria

- A reworded high finding in an ignored function (or near an ignored line) of unchanged code
  passes; the same on a changed line fails; a finding Vidoc marks `accepted` passes only with a
  valid id for its file; a real new finding ("Weak randomness in session id") in an unrelated place
  still fails, with its nearest ignore and similarity printed.
- A fake Vidoc answering `max_tokens` for a two-file chunk is asked again for each half and the scan
  passes; one that always answers `max_tokens` ends incomplete (exit 3) naming the file; a 503 past
  the retries ends incomplete, not failed or skipped; `--ci` exits 75 with `::yulius-incomplete`,
  or the skip code without `YULIUS_CI_INCOMPLETE_CODE`.
- A CI job exiting 75 ends `error` with `incomplete: …` in the run reason and commit status
  (`error`), and the merge gate is a failure.
- A run where `security` fails and `quality` sleeps then passes: `quality` ends passed; a failed
  `quality` leaves `security` passed.
- A run whose other job failed files no finding issue and says so; the same findings are filed once
  the other job passes.

## Test plan

- `scripts/test_vidoc_scan.py` (stdlib unittest, fake Vidoc on loopback): new `Repeats` and
  `Incomplete` cases; existing outage, deadline and unreadable cases updated to the new verdicts.
- `crates/yulius/tests/local_ci.rs`: exit 75 ends error with the reason and status; security and
  another job do not cancel each other; filing waits for the other jobs.
- `crates/yulius-domain` unit test for the incomplete reason text.
- `scripts/dev ci` (the full gate) and `scripts/dev security` (or recorded as not run).

## Decisions

See ADR-0202. In short:

- Incomplete is the existing **error** state with an `incomplete:` reason, not a new run state:
  error already means "could not run, infrastructure", blocks the merge, and is shown everywhere;
  a new state would touch the store, contracts, web UI and every match for the same gate rule.
- Exit 75 (`EX_TEMPFAIL`) via `YULIUS_CI_INCOMPLETE_CODE`, like the skip code (ADR-0133): an
  instance that does not know it gets the old skip, never a failure.
- Place matching applies only outside the changed lines; title matching stays everywhere. A
  similarity threshold alone would hide new weaknesses in the same file, so similarity is shown,
  never decisive.
- The `security` job is exempt from fail-fast in both directions; fail-fast between other jobs
  stays (it frees the CI lane, SPEC-145).
- Filing moves to the run's end so it can see the other jobs; closing gone issues stays (it reads
  only the integration branch).
