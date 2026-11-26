---
yulius-task: ad63a510-886a-49f9-952f-39f81729f3e1
status: done
---

# SPEC-672: Cleanup of store errors, row helpers, MCP tool plumbing and the task group (audit L7a)

## Goal

Carry out findings F2, F5, F8, F4 (with L1 F8) and F6 of the November 2026 layering audit (L7a):
less code doing the same thing, one pull request per finding. Behaviour stays the same.

## Context

- The owner's cleanup charter and the audit report live outside the repository (task
  attachments `audit-2026-11-25/CHARTER.md` and `L7a-layering.md`). The charter treats this work
  as a chore with no spec; this file exists only because Yulius runs no CI for a task without a
  registered spec. It records the plan and the evidence, nothing else.
- Out of scope by the owner's request: F1 (migration squash), F3 (persistence traits), F7.

## Requirements

- CLN-001: Every change keeps behaviour, HTTP and MCP contracts and authorisation identical.
- CLN-002: Each finding is its own pull request into `dev`, delivered before the next starts.
- CLN-003: Tests of every touched binary pass unchanged, except tests of deleted code and the
  `status.js` scrape tests F4 replaces with a server-side test.

## Plan

- [x] F2: crate-private DbError with From<rusqlite::Error>; call() unwraps it once
- [x] F2: every store module uses ? instead of .map_err(db)
- [x] F2: store and dependent test binaries pass; PR opened
- [x] F2: independent review approves the exact head
- [x] F5.1+F8: shared MCP argument helpers (str_arg, opt_str, flag, schema, tool)
- [x] F5.1+F8: store row-parse helpers (text_as, ts, opt_ts) used by every *_from_row
- [x] F5.1+F8: tests pass; independent review approves
- [x] F4 PR1: TaskSummary carries the server-computed group, with a test
  (written as PR #1791, then reverted unmerged: see Decisions)
- [x] F4: checked and reported: the server-owned group would change what the sidebar shows
- [x] F5.2: one tool table derives COCKPIT/PROJECT/PERSON lists and descriptors
- [x] F5.2: authorisation tests pass unchanged; independent review approves
- [x] F6: typed contracts structs for the MCP results mcp_text renders
- [x] F6: tests pass; independent review approves

## Acceptance criteria

- Each finding above is merged into `dev` on green Yulius CI, or named under "Not done" in the
  closing summary with the reason.

## Test plan

- Per pull request: clippy and the whole test binary of every touched crate; `scripts/dev quick`;
  the e2e specs for the `status.js` change.

## Decisions

- A spec is written despite the charter's "no spec": without one Yulius starts no CI run and
  nothing can merge. It is kept to this plan and its evidence.
- F4 (the web UI reads the server's sidebar group) is not done. The audit made it conditional:
  "the server must publish `TaskUpdated` when [pull request state and browser requests] change".
  It does not. The Git overview refreshes `observed_pulls` without republishing the task, and
  the PR watch republishes a task only for a stall, news or a candidate change. Meanwhile the
  web UI applies `pulls.updated` and `browser.action_*` itself. Review of PR #1791 found more:
  - a parked row's own facts disagree with its verified `parked_needs_you` in three existing
    tests;
  - `TaskUpdated` events carry the unverified group of a parked task;
  - the summary's `yulius_wait` is wider than the facts'.
  So `status.js` on the server's group would show stale or different groups. By the owner's
  charter (behaviour stays the same; report instead of deciding), both pull requests stay
  undone. Part 1 alone would add a field nothing reads.


## Evidence

- F2 (PR #1771, merged at 088774d5d on green Yulius CI)
  - `DbError` replaces `db()`.
  - Store product 15,574 → 14,744 lines (−830).
  - Checks: `nextest -p yulius-store --features test-support` 179 passed; clippy clean.
  - Review: approved in two rounds.
- F5 step 1 and F8 (PR #1781, merged at 059d834b2)
  - `mcp_args` replaces the argument and descriptor helper copies.
  - The served descriptors are byte-identical: a temporary dump of all descriptor lists for all token scopes compared equal on dev and on the branch.
  - The row-parse helpers take store product from 14,734 to 14,638 lines.
  - Checks: `nextest -p yulius-app` 740 passed, `-p yulius-store` 179 passed, the whole `yulius::main` binary 2,828 passed.
- F4 (PR #1791, closed unmerged, reverted in e62cd25e8): not done; see Decisions.
- Part of F5 step 2 (PR #1794, merged at d941af88a)
  - `COCKPIT_TOOLS` is derived from `PROJECT_TOOLS`, `PERSON_TOOLS` and `COCKPIT_OTHER_TOOLS`. A temporary comparison of the sorted list with the old literal found the same 94 names.
  - The authorisation tests are unchanged and pass: `yulius-app` 740, eight MCP and Superagent integration files 213, the whole binary 2,829.
  - Mutation by hand: dropping `ability_report` from the list fails `mcp::tests`.
  - Not done: deriving `REMOTE_TOOLS` would reorder the remote `tools/list`.
- Part of F6 (PR #1801)
  - The steps tools answer a typed `PlanRecord`, with the same JSON and the same text.
  - Tests:
    - `blank_notes_and_titles_read_as_absent` failed on the first head;
    - `a_plan_record_keeps_its_wire_shape` fails if `skip_serializing_if` is added to `note`;
    - `yulius-app` and `yulius-contracts` 799 passed;
    - the whole `yulius::main` binary 2,829 passed.
  - Review: the reviewer's differential run of the old renderer against the new one found 0 differences over 900,000 random records.
  - Not done: the rest of F6, whose producers are in #651/#652/#653 paths, `session_read`, and the task brief.
- Security scan: Vidoc is disabled for this project (CI `security` skipped); not reported as passed.
