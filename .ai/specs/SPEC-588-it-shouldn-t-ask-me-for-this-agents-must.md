---
yulius-task: d72a43d2-fda5-4cf6-b8e4-d81011fe5c5f
status: done
---

# SPEC-588 Continue independent review under the task's existing authorization

## Goal

An agent fixes findings and obtains the independent approval needed for delivery without asking the task owner to authorize another review round.

Fixes #1593.

## Context

Task #570 twice raised a Needs You request for another independent review after its third round. The task already authorized delivery into dev on green CI. Both versions of the generated Git briefing offered round-count escalation, and the delivery and review skills capped re-review at three rounds. The briefing tests required the escalation wording. [Issue #1593](https://github.com/Quartio-Studio/yulius/issues/1593)

## Requirements

- **REV-001** For connected projects and playgrounds, the task authorization covers independent re-review of in-scope corrections until approval, including a fourth or later round.
- **REV-002** Delivery and review instructions agree on who acts when findings persist. The agent fixes its own findings test-first and can split unwieldy work within the authorized scope. It seeks human input only for a concrete action or decision outside its authority.
- **REV-003** Delivery still requires independent approval and the project's gates. Existing rules for interrupted reviewers, approved nits and filing out-of-scope findings apply.

## Plan

- [x] Trace the review authorization prompt to the instructions that produced it
- [x] Cover continued review in task briefings and bundled guidance
- [x] Align review instructions with the task’s existing authorization
- [x] Independently review the fix and resolve findings
- [x] Record the reviewed fix and verification evidence for green-CI delivery

## Acceptance criteria

- **AC-1** In each harness, a connected or playground task whose third reviewer requests changes receives instructions to fix and obtain independent re-review through approval. The review instruction gives no round-count reason to call `needs_you`. (REV-001)
- **AC-2** The Feature and Fix workflows, reviewer checklist, and installed review, verification and delivery skills all convey the same authorization. A credential or permission only the owner can supply can still be raised. (REV-002)
- **AC-3** A request-changes verdict still requires correction before merge. An approved head is delivered through the existing gate. (REV-003)

## Test plan

Exercise the generated briefings for all three harnesses and both project modes. Extend the existing cross-document review contract to catch the cap and the invitation to escalate. Run the affected Rust suites and repository checks, including generated skill parity. The defect is in the instructions delivered to the agent, so the tests inspect those instructions directly.

## Decisions

Keep the existing independent reviewer and merge gate. Three rounds may prompt the author to split work and may leave eligible cosmetic nits to file. The count grants no new reason to interrupt the owner. [ADR-0427](../../docs/decisions/0427-task-authorization-covers-independent-re-review.md)

## Evidence

- On the test-first commit `be6f1faa7`, nextest ran both regression tests against the previous instructions. Both failed. The connected briefing invited `needs_you` after three rounds, and the bundled delivery skill capped re-review.
- The corrected briefings, skills and built-in workflows passed all 58 scoped nextest cases. These cover all three harnesses in connected projects and playgrounds, the briefing size limit, the own-code finding rules and the independent review instructions.
- APM regenerated the repository skills. `apm audit` scanned 16 files and found no drift. The tests exercise the instructions delivered to agents. They use no live provider account.
- Independent review approved `ddf6bf359` with no findings. The reviewer reran the 58 scoped cases, `scripts/dev quick` and `git diff --check`. `scripts/dev security` reported that Vidoc was disabled for this project.
- `scripts/dev mutants --jobs 1` completed with 4 listed, 4 caught, 0 missed, 0 timed out, 0 unviable and 0 not reached. Its baseline ran 3,346 tests successfully and skipped two. The first attempts were interrupted or failed on temporary Unix socket path length. The completed campaign used a short disk-backed directory. The six-mode briefing regression alone also caught all four mutants.
- All 58 scoped tests passed after each clean merge of `origin/dev` at `c748520a4` and `f24514af2`. Hosted CI and the landed revision are recorded in the task's delivery evidence.
