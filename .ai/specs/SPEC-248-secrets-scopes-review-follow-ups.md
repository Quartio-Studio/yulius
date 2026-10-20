---
yulius-task: 8ff0e8ce-10f1-41f4-9f80-3f90e00917e6
status: in-progress
---

# SPEC-248: Secrets scopes: review follow-ups

Status: in-progress
Date: 2026-10-20
Owner: Yulius task #234
Scope authorization: task #234, the minor items the independent review of task #231
([SPEC-245](SPEC-245-secrets-scopes-project-global-personal.md),
[ADR-0186](../../docs/decisions/0186-agent-secrets-are-project-global-or-personal.md)) listed after
its third round and split out.

## Goal

Close the gaps the review of SPEC-245 left: no egress client can carry a frozen person for an
agent run, a run never keeps hashes of a previous owner's personal values after a hand-over, the
200-secret limit per project holds under concurrency, and the tests and documents say what the
code does.

## Context

- `EgressClient` (`crates/yulius-app/src/ports/egress.rs`) carried `person: Option<UserId>` and a
  `holder()` without callers. For an agent run it was the task's holder when the proxy credential
  was resolved; `client_person` (`vault/runtime.rs`) reads the task's current holder per request
  instead (SPEC-245 review round 2), so the field invited a misuse that would bring back the
  hand-over leak.
- A run's fingerprint file (`<data>/runs/<attempt>/scrub.json`) is written at launch for the
  task's holder. The Bash wrapper and the PostToolUse hook ask Yulius for current fingerprints
  first and read the file only when Yulius cannot be reached. A hand-over (SPEC-070,
  `pass_task_control`) changes the holder; the file kept the previous owner's salted hashes,
  contradicting SPEC-245 RES-003.
- The cap of 200 agent secrets reaching a project was counted in one store call and written in
  another (`switch_agent_secret`, `insert_agent_secret`); concurrent switch-ons or creations could
  pass it. The store runs every job on one database thread, so a count and a write inside one job
  are atomic.

## Requirements

- FUP-001. An agent run's egress client names no person: the policy reads its task's current
  holder per request; a CI job's names none; a Superagent's names its person.
  `EgressClient::holder()` is gone.
- FUP-002. When control of a task passes (SPEC-070), Yulius rewrites its live run's fingerprint
  file for the new holder, in place (a read-only bind into a contained session keeps seeing it),
  never following a link. A run whose launch computed its file before control passed gets it
  rewritten once it is live. Rewrites run one at a time, each for the task's holder as it is
  then, so two quick hand-overs end with the latest owner's file. A file that cannot be rewritten is logged; nothing else of the
  hand-over or the launch fails.
- FUP-003. Creating an agent secret and switching a global or personal one on count the secrets
  reaching the project in the same store transaction as the write and answer `Capacity` at 200;
  switching on one that already is on is never refused.
- FUP-004. Tests: the new owner's personal secret applies on a client resolved before the
  hand-over; a deleted task's proxy client resolves to no person (its owner's personal secret no
  longer applies, a project secret still does); the "no live session" helper of the message test
  says what it means.
- FUP-005. Documents: SPEC-245's Test plan and "Not tested" name the version-38 database the
  migration starts from; the ADR index lists 0186 after 0185; ADR-0186 and SPEC-245 describe the
  rewritten fingerprint file and the transactional cap.

## Plan

- [x] Spec and plan for the follow-ups
- [x] Agent egress clients carry no frozen person
- [x] A hand-over rewrites the run's fingerprint file
- [x] Hand-over and deleted-task tests, message test tidied
- [x] The 200-secret cap is checked in the store transaction
- [x] Documents corrected and the ADR index ordered
- [x] Gates green and review round 1 fixed
- [ ] Review round 2 approved and merged on green

## Acceptance criteria

- After a hand-over from alice to bob, the run's `scrub.json` is a valid fingerprint set that
  lists bob's personal secret and not alice's; so is it when control passed while the run was
  being launched.
- A client resolved before the hand-over gets bob's personal secret on its host; after the task
  is deleted, a client resolved before gets no personal secret but still a project one.
- Concurrent switch-ons of several personal secrets into a project with 199 leave exactly 200
  reaching it; the store refuses an insert or a switch-on at the cap and accepts switching on one
  that is already on.
- `EgressClient` has no `person` field and no `holder()`.

## Test plan

- Integration (`crates/yulius/tests/task_done.rs`, a real run): control passing while the spawn
  is held still leaves the new owner's fingerprints.
- Integration (`crates/yulius/tests/secrets_scopes.rs`): hand-over rewrites the fingerprint file;
  the hand-over test asserts the new owner's secret on the old client; a deleted task's client;
  concurrent switch-ons stop at 200.
- Store (`crates/yulius-store/src/vault.rs` tests): insert and switch refuse at the cap in the same
  transaction; an already-on switch is not refused.
- Existing proxy and scope tests cover the removal of the person field (compile and behavior).

## Decisions

- Rewrite the fingerprint file rather than correct RES-003: the hand-over already knows the task
  and its live attempt, the rewrite is a few lines and keeps the guarantee ADR-0186 states. In
  place, not by rename, since a contained session binds the file read-only by inode. A copy the
  run made of the file before the hand-over cannot be recalled; ADR-0186 says so.
- Move the person into `EgressCaller::Superagent` instead of documenting the field: the type then
  cannot express a person for an agent run or a CI job.
- Fold the cap into the store (`cap` argument) rather than keep the soft cap: the database thread
  serializes jobs, so one count query per write closes the race at no measurable cost. No new ADR;
  ADR-0186's consequence is amended.

## Evidence

Observed on the branch (2026-10-20), after merging `origin/dev` (b289f81f):

- `scripts/dev ci`: exit 0 (check, fmt, clippy, tests, docs, deps, macOS targets); nextest 2317
  passed, 17 skipped (on 10dea5ee, with the review round 2 fixes; 2316 on 345a14ff). The first run, before the dev merge, failed only
  `supervision::a_usage_limit_moves_the_session_to_the_next_account` (60 s timeout under the full
  suite's load): the open flaky issue #539, outside this change; it passed 3/3 alone.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^secrets_scopes::/)'`: 13
  passed (12 before review round 2). Red first: `a_hand_over_rewrites_the_runs_fingerprint_file`
  failed before the fix (the file still listed `MY_TOKEN`); `concurrent_switch_ons_stop_at_the_projects_limit` (20
  switch-ons on 4 workers into a project with 199) let 5, 7 and 4 through in three runs before the
  fix, and passed 5/5 after. The new hand-over assertions (bob's secret on the client resolved
  before) and `a_deleted_tasks_client_resolves_to_no_person` pin behavior already right since
  SPEC-245 review round 2; they passed on first run.
- Store tests (`cargo nextest run -p yulius-store vault::`): 14 passed, with
  `adding_and_switching_on_stop_at_the_cap` (insert and switch-on refused at the cap, an
  already-on switch not refused, switching off then on fits).
- Review round 1 (independent, fresh session; approved with minor findings, all fixed):
  `task_done::a_hand_over_during_a_launch_leaves_the_new_owners_fingerprints` (a real run whose
  spawn is held while control passes) failed before the fix ("timed out waiting for the new
  owner's fingerprints: Some([\"MY_TOKEN\"])") and passed 3/3 after; the rewrite opens with
  `O_NOFOLLOW | O_NONBLOCK` and checks the handle is a regular file; the store test asserts a
  project secret gets no switch row; SPEC-245 RES-002 names the rewritten file.
- Review round 2 (fresh session; two minor findings, fixed): `secrets_scopes::
  a_rewrite_follows_the_tasks_current_owner` (alice → bob → carol, then the first hand-over's
  rewrite) failed before the fix (no `CAROL_TOKEN`) and passes now that rewrites are serialized
  and read the task afresh; the launch test now waits for bob's own secret too, and a mutation
  computing the rewrite for nobody turned it red.
- FUP-001 has no red test: it removes a field and a method without callers; the compiler and the
  hand-over tests (which read the current holder) are its evidence.
- `scripts/dev security`: **not run** (exit 2): Vidoc answered HTTP 429, "This API key has used its
  50,000,000 tokens". Unrun, not passed; a person was asked to restore the quota (`needs_you`).
  The PR's CI `security` job is the live scan once it has quota.
- e2e: not run, no web UI change.
- Not tested: a contained session reading the rewritten file through its read-only bind (the test
  checks the inode stays the same); a fingerprint fallback read racing the rewrite (it would read
  invalid JSON and withhold output, failing closed).
- Size: about 240 changed lines outside tests and docs.
