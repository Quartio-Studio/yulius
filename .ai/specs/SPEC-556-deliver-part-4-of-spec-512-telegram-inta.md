---
yulius-task: 5e43fdfd-05f0-4af8-8ca7-3ea5ceb09f7c
status: done
---

# SPEC-556: Deliver SPEC-512 part 4 (Telegram intake, #1107)

Status: done
Date: 2026-11-20
Owner: Yulius task #538

## Goal

Deliver SPEC-512 part 4 into `dev`: the personal Telegram channel's intake (issue #1107, part 2),
built by task #494 on `feat/494-marcin-s-five-issues-1412-1418-1413-1107` (head f7cde240e). Task
#494's review approved it at a7cc9d7ed; f7cde240e adds the minors closed after that approval,
reviewed here for the first time.

Part of #1107

## Context

Task #494's pull request #1480 (parts 1 to 3 of SPEC-512) merged into `dev`. The running Yulius
server (37b2b28b2) judges a later head of that task by a 3-way merge from #1480's merged head
(6bd0f463f). `dev` has since appended migration 0071 to `MIGRATIONS` at the spot where part 4
appends 0073, so `delivery_merge` refuses part 4 as conflicting with `origin/dev` although a
`git merge origin/dev` is already up to date (issue #1299, fixed on `dev` by #1469 but not yet in
the running server). A branch that starts from `dev` does not meet it, so this task carries the
head on its own branch.

## Requirements

- **DLV-001** This branch's change against `dev` is SPEC-512 part 4 and the fixes of this task's
  review: its requirements TGI-001..TGI-005 are in
  [SPEC-512 "Part 4"](SPEC-512-marcin-s-five-issues-1412-1418-1413-1107.md#part-4-the-personal-telegram-channels-intake-1107).
- **DLV-002** SPEC-512 is not edited here: task #494 owns it and continues with part 5.
- **DLV-003** Migrations keep `dev`'s order (file 0072 at schema version 70, 0070 at 71, 0071
  at 72, 0074 at 73, 0075 at 74) with 0073 last, at 75.
- **DLV-004** (review round 1) A chat's intake messages are handled one at a time in the order
  sent; a whole poll answer (20) waits its turn; a message past that is not dropped silently: it
  is logged and the chat is told, once per quiet window. A waiting message runs only if the link
  it came through still holds with intake on when its turn comes (round 2). `/start` with intake
  on gets the intake help.

## Plan

- [x] Branch carries task #494's part 4 head
- [x] Touched tests, clippy and repository checks pass
- [x] Evidence recorded and the head handed to an independent review
- [x] A chat's intake lane holds a whole poll answer in order and tells what it drops
- [x] Round 1's findings answered and the head handed to review round 2
- [x] Dev merged: 0073 at schema version 75, the affected tests rerun
- [x] Round 2's findings answered and the head handed to review round 3

## Acceptance criteria

- [x] `git diff origin/dev..HEAD` holds part 4, round 1's fixes and this spec (DLV-001, DLV-002).
- [x] A full poll answer from one chat starts its tasks in order; one past the bound is answered;
  intake off or `/stop` ends what waits (DLV-004).
- [x] The tests part 4 touches pass on this head (DLV-001, DLV-003).
- [x] This head is handed to an independent review (`yulius-reviewer`) that knows task #494's
  rounds.

## Test plan

The tests SPEC-512's part 4 Evidence names, run on this head: `cargo test -p yulius --test main
telegram::` and `task_message::`, `cargo test -p yulius-store --lib`, `cargo test -p yulius --lib
wiring::telegram`, `cargo test -p yulius-app --lib -- telegram notify`, `npx e2e run
tests/telegram.e2e.ts`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt
--check` and `python3 scripts/check_repository.py`. The whole gate is Yulius CI's.

## Decisions

- Carry task #494's head by a plain merge instead of cherry-picking or re-applying its diff: the
  approved commits stay as they were, and `dev` is an ancestor of that head, so the merge is a
  fast-forward and nothing was resolved.
- No new ADR: part 4's design is ADR-0392, delivered with it; round 1's lane is added to it.
- The lane is a bounded queue per chat with one worker, not a lock per chat: spawned tasks race
  for a lock, so order was not guaranteed, and a queue's `try_send` tells a full lane at once.
  The worker removes its lane when it finds the queue empty, under the map's lock, so the map
  never grows with chats seen once and no message handed over meanwhile is lost. The bound is 20,
  the adapter's poll limit (a restart hands over at most one answer at a time); a message past
  it is answered at most once per quiet window (10 minutes), so a flooding chat cannot fill the
  shared send queue. Rejected: holding the poll while a lane is full (one chat would stall every
  other chat's `/stop` and link codes, review round 1 of task #494).

## Evidence

Part 4's own evidence (each criterion's test, review rounds 1 and 2, approval at a7cc9d7ed and the
minors closed after it) is SPEC-512's
[Evidence, "Part 4 (#1107, intake)"](SPEC-512-marcin-s-five-issues-1412-1418-1413-1107.md#part-4-1107-intake).

On this branch (2026-11-20):

| Check | Result |
|---|---|
| `git fetch origin feat/494-…` and `git merge --no-edit f7cde240e` from `dev` c20e9807d | fast-forward, nothing resolved; 33 files against `dev` (part 4, its SPEC-512 Evidence, this spec) |
| Migration order | at 9890d38d3, `MIGRATIONS` ended with files 0072 (schema version 70), 0070 (71), 0071 (72), 0073 (73); after the merge of `dev` eef33c931 below, 0074 (73), 0075 (74), 0073 (75); pinned test `(75, 0073)`, `MIGRATIONS.len() == 75`; numbers unique (`check_repository.py`) |
| `cargo test -p yulius --test main telegram::` | 30 passed |
| `cargo test -p yulius --test main task_message::` | 33 passed |
| `cargo test -p yulius-store --lib` | 98 passed |
| `cargo test -p yulius --lib wiring::telegram` | 11 passed |
| `cargo test -p yulius-app --lib -- telegram notify` | 3 passed |
| `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check` | clean |
| `python3 scripts/check_repository.py` | Repository contracts passed |
| `cd e2e && npx e2e run tests/telegram.e2e.ts` | 6 passed |
| `yulius security-scan` | disabled (Vidoc off for this project): nothing scanned, not a pass |

The whole gate is Yulius CI's, on the pull request's head.

### Review round 1 (head 9890d38d3, CHANGES REQUESTED), answered test-first

| Finding | Change | Test |
|---|---|---|
| Major: past five in flight a chat's message was dropped without a word; no test held the lane | a queue per chat with one worker, 20 waiting (a whole poll answer); past it, a log line and `INTAKE_FULL_REPLY` once per quiet window | `telegram::a_chats_whole_poll_answer_is_handled_in_order` (a held photo, then 19 texts in one answer: 20 tasks, in order; fails at bound 5: 6 of 21 answers; fails with a worker per message: the photo's task comes last); `telegram::a_message_past_the_chats_waiting_bound_is_answered` (photo held, 22 more: the chat is told, the 20 waiting still run, the hourly bound refuses the 21st task; fails on the old code: no answer) |
| Minor: SPEC-556 misstated the migration order | DLV-003 and the Evidence row corrected | by reading `crates/yulius-store/src/lib.rs` |
| Minor: SPEC-556 called f7cde240e the approved head | Goal says the approval was at a7cc9d7ed | by reading |
| Minor: `/start` with intake on said "turn on Telegram intake" | it answers `INTAKE_HELP` | `telegram::a_start_with_intake_on_gets_the_intake_help` (failed before) |
| Nit: `INTAKE_WAITING` counted the running message | it now counts only those waiting (the worker holds the running one) | by reading |
| Nit: the `lanes` map never shrank | the worker removes its lane when empty | by reading (`next_in_lane`) |
| Nit: PR title cut mid-word, scope `needs-you` | retitled "feat(telegram): Telegram intake, SPEC-512 part 4 (#1107), from task #494's branch" (81 characters) | `gh pr view 1487` |
| Nits SPEC-512 records as left (the bound's refusal says "tools"; label, `Named::Several`, `Ok(None)` untested) | left as task #494 recorded them; outside this delivery's findings | — |

After the fix: `telegram::` 33 passed; the three new tests 15 runs in a row without a failure;
`cargo test -p yulius-app --lib` 645 passed; clippy `-D warnings` and `fmt --check` clean;
`check_repository.py` passed. `telegram.e2e.ts` was not rerun: no web asset changed in this
round.

### Merge of `dev` eef33c931 (after round 1)

`dev` appended migrations 0074 and 0075 (schema versions 73 and 74, #1488) where part 4 appends
0073. Resolved keeping both: `dev`'s first, 0073 last at schema version 75; the pinned test and
`MIGRATIONS.len()` follow, and 0073's header comment and ADR-0392 say version 75. No other file
conflicted.

Tests on the merged head 27956906d (side worktree, own target): `telegram::` 33, `task_message::`
33, `yulius-store --lib` 100, `wiring::telegram` 11, `yulius-app --lib -- telegram notify` 3,
clippy `-D warnings` and `fmt --check` clean, `telegram.e2e.ts` 6 passed. `check_repository.py`
failed: the union merge put ADR-0392's index line after 0385; moved after 0390 (fe7360834),
then passed.

### Review round 2 (head 4799058c6, CHANGES REQUESTED), answered test-first

Round 1's findings were all confirmed resolved (the three lane mutations killed in the
reviewer's copy).

| Finding | Change | Test |
|---|---|---|
| Blocker: CI `quality` failed at `cargo doc`: `INTAKE_FULL_REPLY`'s doc linked the private `QUIET_MS` | plain backticks | `cargo doc --locked --workspace --all-features --no-deps` (failed before, passes) |
| Blocker: `dev` took schema versions 73 and 74 | the merge above: 0073 at 75, pinned `(75, 0073)`, `MIGRATIONS.len() == 75`, header comment and ADR-0392 | `cargo test -p yulius-store --lib` 100 passed |
| Minor: a waiting message ran under the link as it was at receipt, so intake off or `/stop` still started up to 20 tasks | `intake_in_turn`: the worker re-reads the chat's link before each message; same link with intake on runs it, intake off gets the help, no link the unlinked reply (both at most every 10 minutes), another link nothing | `telegram::intake_off_or_stop_ends_what_waits_in_the_lane` (a held photo with one waiting, then intake off; then on another chat `/stop`: the waiting ones start nothing and get the help and the unlinked reply; failed before the fix) |
| Nit: `docs/api.md` lacked the lane | the intake row says it | by reading |
| Nit: the Plan said "approved part 4 head" | reworded | by reading |
| Nit: the full-lane answer shares the chat's quiet slot with the help | kept: that slot is what bounds what one chat can put in the shared send queue; a suppressed notice is still logged | — |
| Nit: a worker that panics loses its waiting messages without an answer | kept: `telegram_intake` returns errors rather than panicking, and an error is answered; the next message starts a new lane (`Closed`) | — |

After the fix (head with fe7360834 and these documents): `telegram::` 34 passed; the three lane
tests 12 runs in a row without a failure; `cargo doc --locked --workspace --all-features
--no-deps`, clippy `-D warnings` and `fmt --check` clean; `check_repository.py` passed.
