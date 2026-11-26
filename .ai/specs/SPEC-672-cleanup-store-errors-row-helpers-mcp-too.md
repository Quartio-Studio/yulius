---
yulius-task: ad63a510-886a-49f9-952f-39f81729f3e1
status: in-progress
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
- [ ] F5.1+F8: shared MCP argument helpers (str_arg, opt_str, flag, schema, tool)
- [ ] F5.1+F8: store row-parse helpers (text_as, ts, opt_ts) used by every *_from_row
- [ ] F5.1+F8: tests pass; independent review approves
- [ ] F4 PR1: TaskSummary carries the server-computed group, with a test
- [ ] F4 PR1: independent review approves
- [ ] F4 PR2: status.js reads the group; JS derivation and scrape tests deleted
- [ ] F4 PR2: e2e and tests pass; independent review approves
- [ ] F5.2: one tool table derives COCKPIT/PROJECT/PERSON lists and descriptors
- [ ] F5.2: authorisation tests pass unchanged; independent review approves
- [ ] F6: typed contracts structs for the MCP results mcp_text renders
- [ ] F6: tests pass; independent review approves

## Acceptance criteria

- Each finding above is merged into `dev` on green Yulius CI, or named under "Not done" in the
  closing summary with the reason.

## Test plan

- Per pull request: clippy and the whole test binary of every touched crate; `scripts/dev quick`;
  the e2e specs for the `status.js` change.

## Decisions

- A spec is written despite the charter's "no spec": without one Yulius starts no CI run and
  nothing can merge. It is kept to this plan and its evidence.
