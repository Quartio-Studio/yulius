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
built, reviewed and approved by task #494 on `feat/494-marcin-s-five-issues-1412-1418-1413-1107`
(head f7cde240e).

Part of #1107

## Context

Task #494's pull request #1480 (parts 1 to 3 of SPEC-512) merged into `dev`. The running Yulius
server (37b2b28b2) judges a later head of that task by a 3-way merge from #1480's merged head
(6bd0f463f). `dev` has since appended migration 0071 to `MIGRATIONS` at the spot where part 4
appends 0073, so `delivery_merge` refuses part 4 as conflicting with `origin/dev` although a
`git merge origin/dev` is already up to date (issue #1299, fixed on `dev` by #1469 but not yet in
the running server). A branch that starts from `dev` does not meet it, so this task carries the
approved head on its own branch.

## Requirements

- **DLV-001** This branch's change against `dev` is exactly SPEC-512 part 4, as approved: its
  requirements TGI-001..TGI-005 are in
  [SPEC-512 "Part 4"](SPEC-512-marcin-s-five-issues-1412-1418-1413-1107.md#part-4-the-personal-telegram-channels-intake-1107).
- **DLV-002** SPEC-512 is not edited here: task #494 owns it and continues with part 5.
- **DLV-003** Migrations keep `dev`'s order (0072 at version 71, 0071 at 72) with 0073 last.

## Plan

- [x] Branch carries task #494's approved part 4 head
- [x] Touched tests, clippy and repository checks pass
- [x] Evidence recorded and the head handed to an independent review

## Acceptance criteria

- [x] `git diff origin/dev..HEAD` holds part 4 and this spec only (DLV-001, DLV-002).
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
- No new ADR: part 4's design is ADR-0392, delivered with it.

## Evidence

Part 4's own evidence (each criterion's test, review rounds 1 and 2, approval at a7cc9d7ed and the
minors closed after it) is SPEC-512's
[Evidence, "Part 4 (#1107, intake)"](SPEC-512-marcin-s-five-issues-1412-1418-1413-1107.md#part-4-1107-intake).

On this branch (2026-11-20):

| Check | Result |
|---|---|
| `git fetch origin feat/494-…` and `git merge --no-edit f7cde240e` from `dev` c20e9807d | fast-forward, nothing resolved; 33 files against `dev` (part 4, its SPEC-512 Evidence, this spec) |
| Migration order | `MIGRATIONS` ends 0072, 0071, 0073; numbers unique (`check_repository.py`) |
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
