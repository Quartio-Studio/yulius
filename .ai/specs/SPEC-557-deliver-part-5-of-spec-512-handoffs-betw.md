---
yulius-task: 594bf22e-eb0e-422c-a19c-441d5e6ac95a
status: in-progress
---

# SPEC-557: Deliver SPEC-512 part 5 (handoffs between sandboxes, #1106)

Status: in-progress
Date: 2026-11-20
Owner: Yulius task #539

## Goal

Deliver SPEC-512 part 5 into `dev`: tasks hand work to another sandbox behind an instance switch
(issue #1106), built by task #494 on `feat/494-marcin-s-five-issues-1412-1418-1413-1107` (head
a8c6d4a80). Task #494's review approved it at 0868ae83c in round 3; a8c6d4a80 only adds a refusal
that redacts before it cuts, plus a documentation line.

Part of #1106

## Context

Task #494's pull request #1480 (parts 1 to 3 of SPEC-512) merged into `dev`. The running Yulius
server (37b2b28b2) judges a later head of that task by a 3-way merge from #1480's merged head
(6bd0f463f). `dev` and that branch have since both appended migrations at the same spot of
`MIGRATIONS`, so `delivery_merge` refuses part 5 as conflicting with `origin/dev` although a
`git merge origin/dev` is already up to date (issue #1299, fixed on `dev` by #1469 but not yet in
the running server). A branch that starts from `dev` does not meet it, so this task carries the
head on its own branch, as task #538 did for part 4 (SPEC-556, PR #1487).

## Requirements

- **DLV-001** This branch's change against `dev` is SPEC-512 part 5: its requirements
  HOF-001..HOF-006 are in
  [SPEC-512 "Part 5"](SPEC-512-marcin-s-five-issues-1412-1418-1413-1107.md#part-5-tasks-hand-work-to-another-sandbox-1106),
  plus the part 4 fix it carries (`/task owner/name` in `telegram_intake.rs` `project_named`).
- **DLV-002** SPEC-512 is not edited here: task #494 owns it.
- **DLV-003** Migrations keep `dev`'s order with `0076_task_handoffs.sql` last, at schema
  version 76.
- **DLV-004** (Yulius CI, review round 1) A playground guest's Git page asks nothing the guest
  fence refuses; a refusal's audit row names its source task (`refused: <sandbox> #N to
  <target>: <reason>`), and a refused row whose target was not found lists the sandbox asked for.

## Plan

- [x] Branch carries task #494's part 5 head
- [x] Touched tests, clippy and repository checks pass
- [x] Evidence recorded and the head handed to an independent review
- [x] Round 1's findings answered and the head handed to review round 2

## Acceptance criteria

- [x] `git diff origin/dev..HEAD` holds part 5, its SPEC-512 Evidence and this spec (DLV-001,
  DLV-002).
- [x] The tests part 5 touches pass on this head (DLV-001, DLV-003).
- [x] This head is handed to an independent review (`yulius-reviewer`) that knows task #494's
  rounds.

## Test plan

The tests SPEC-512's part 5 Evidence names, run on this head: `cargo test -p yulius --test main
handoffs::`, `parking::a_handed_off_tasks_outcome_reaches_its_source` and `telegram::`, `cargo
test -p yulius-store --lib`, `cargo test -p yulius-app --lib handoffs::`, `npx e2e run
tests/handoffs.e2e.ts tests/git.e2e.ts tests/telegram.e2e.ts`, `cargo clippy --workspace
--all-targets -- -D warnings` and `python3 scripts/check_repository.py`. The whole gate is Yulius
CI's.

## Decisions

- Carry task #494's head by a plain merge instead of cherry-picking or re-applying its diff: the
  approved commits stay as they were, and `dev` (225bf38fc) is an ancestor of that head, so the
  merge is a fast-forward and nothing was resolved.
- No new ADR: part 5's design is ADR-0395, delivered with it.

## Evidence

Part 5's own evidence (each criterion's test, review rounds 1 to 3, approval at 0868ae83c and
what changed after it) is SPEC-512's
[Evidence, "Part 5 (#1106, handoffs)"](SPEC-512-marcin-s-five-issues-1412-1418-1413-1107.md#part-5-1106-handoffs).

On this branch (2026-11-20):

| Check | Result |
|---|---|
| `git fetch origin feat/494-…` and `git merge --no-edit a8c6d4a80` from `dev` 225bf38fc | fast-forward, nothing resolved; 41 files against `dev` (part 5 and its SPEC-512 Evidence), then this spec |
| Migration order | `MIGRATIONS` ends with 0075 (schema version 74), 0073 (75) and 0076 (76); pinned test `(76, 0076)`, `MIGRATIONS.len() == 76`; numbers unique (`check_repository.py`) |
| `cargo test -p yulius --test main -- handoffs:: parking::a_handed_off_tasks_outcome_reaches_its_source telegram::` | 44 passed (8 `handoffs::`, 1 `parking::`, 35 `telegram::`) |
| `cargo test -p yulius-store --lib` | 100 passed, 1 failed: `tests::a_large_step_history_backfills_quickly` ("the migration took 10.8s", host load average 65); alone it passed in 0.49 s. A known load flake on `dev`, tracked by #1484, outside this change |
| `cargo test -p yulius-app --lib handoffs::` | 2 passed |
| `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check` | clean |
| `python3 scripts/check_repository.py` | Repository contracts passed |
| `cd e2e && npx e2e run tests/handoffs.e2e.ts tests/git.e2e.ts tests/telegram.e2e.ts` | 19 passed (5, 8, 6) |
| `yulius security-scan` | disabled (Vidoc off for this project): nothing scanned, not a pass |

The whole gate is Yulius CI's, on the pull request's head.

### Yulius CI run 2d377ea3 at 5ba12d0d2: e2e failed, fixed test-first

`playground-guests.e2e.ts` "a guest sees their playground and nothing else" failed: the Git
page's new "Handoffs from other sandboxes" card asked `GET /api/sandboxes/sb_lab/handoffs`, a
route the guest fence (SPEC-410 GST-004) refuses. A guest never sets a sandbox's opt-out, so the
Git page draws the card only for a person who is not a guest. Reproduced locally before the fix
(1 failed, 8 passed); after it `npx e2e run tests/playground-guests.e2e.ts tests/git.e2e.ts
tests/handoffs.e2e.ts` 22 passed.

### Review round 1 (head bd46d12e1, CHANGES REQUESTED), answered test-first

The reviewer confirmed the claim about 0868ae83c..a8c6d4a80 (each field is redacted whole and
then cut by characters), found no blocker in authorization, the claim before creation,
migration 0076 or the docs, and found bd46d12e1 and SPEC-557 correct.

| Finding | Change | Test |
|---|---|---|
| Major (a8c6d4a80): the redact-before-cut fix had no test that fails without it | a test | `handoffs::a_secret_across_a_refusals_cut_leaves_no_part_of_it` (a secret starting 10 characters before the sandbox, title and prompt bounds (round 2): no `shop-token` in the page or the audit; fails with cut-then-redact: mutation, the prompt and title kept `…shop-token`) |
| Minor (pre-existing test, the only guard of bd46d12e1): `guestRefusals` was read before the mock answered (50 to 200 ms latency) | the mock counts `unanswered` requests; the test waits for 0 before reading | with the Git page's guest guard removed, `playground-guests.e2e.ts --grep "nothing else"` failed 5 runs of 5 (the reviewer saw 1 of 3 before) |
| Minor: a refusal's audit row on the target named no source (`task:N`, `refused: <reason>`) | `refused: <source sandbox> #N to <target>: <reason>`, as `audit_handoff` names the others | `handoffs::a_refusal_names_its_source_and_what_was_asked` (failed before) |
| Nit: a refusal with no target found listed the task's own repository as its target | the view shows the sandbox asked for | the same test (`target_sandbox` "nowhere"; was `acme/app`) |
| Nit: `harness`, `model`, `effort` and `mode` are outside the secret check | kept, as SPEC-512 records: short option strings a person reads on the request before approving | — |
| Nit: PR #1489's title said `feat(digest)` | retitled "feat(handoffs): tasks hand work to another sandbox, SPEC-512 part 5 (#1106), from task #494's branch" (100 characters) | `gh pr view 1489` |
| Nit: the changes entry did not name this delivery | "delivered by SPEC-557" | by reading |

After the fixes: `cargo test -p yulius --test main -- handoffs:: parking::a_handed_off_tasks_outcome_reaches_its_source`
11 passed; `npx e2e run` of `playground-guests`, `git`, `handoffs`, `telegram`, `playground`,
`initial-load`, `loading`, `mock-ready`, `editor` and `consistency` 96 passed; clippy
`-D warnings` and `fmt --check` clean; `check_repository.py` passed.

### Review round 2 (head abac589c8): APPROVED

Left one minor, answered in the test only: the sandbox field's secret started 5 characters before
its bound, so a cut there kept only `shop-`; it now starts 10 before. With cut-then-redact on the
sandbox field alone, the test fails (`target_sandbox` kept `…shop-token`); `handoffs::` 10 passed
after.
