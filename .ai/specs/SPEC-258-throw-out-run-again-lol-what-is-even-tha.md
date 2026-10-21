---
yulius-task: fe5ab035-1780-4403-a5c7-491bfc667106
status: in-progress
---

# SPEC-258: Run again leaves the run cards for the run's own view

Status: in-progress
Date: 2026-10-21
Owner: Yulius task #244
Scope authorization: the person's request (task #244): "throw out run again lol what is even that",
with a screenshot of the CI page's run list, a "Run again" button on every card. After the answer
(the button is the only way for a person to test a commit again) they asked to finish the task.
Issue #774.

## Goal

The CI page's run list stops repeating "Run again" on every finished run. A person can still test
a commit again, from the run's own view, where they look at what failed.

## Context

- **Today (SPEC-001 DLV-006, SPEC-238).** `pages/ci.js` `runCard` puts "Cancel" on every queued or
  running card and "Run again" on every other card, for people with write access. The run's view
  (`showRun`, `?run=<id>`) has only "In Pipeline" in its head.
- **Why Run again exists.** A cancelled run is never retried by the scheduler
  (`yulius_domain::ci::plan`), and a flaky failure needs the same commit tested again. Agents have
  `ci_rerun`; people only have this button. `POST /ci/runs/{id}/rerun` returns the new run.
- **The screenshot.** Four cards in a row, each with "Run again" and the open chevron: on a list of
  mostly passed runs the button is noise, and its meaning is not obvious.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| RRA-001 | A run card in the CI list shows no "Run again", whatever its state. "Cancel" stays on queued and running cards (stopping a run is urgent). | `ci.e2e.ts` |
| RRA-002 | The run's view offers, to people with write access, "Run again" on a finished run (passed, failed, error, cancelled, skipped) and "Cancel" on a queued or running one, in its head beside "In Pipeline". "Run again" queues the commit anew ("PR #218 queued again.") and opens the new run's view; "Cancel" asks the same confirmation as on the card and redraws the view cancelled. A refusal is an error toast, as before. | `ci.e2e.ts` |
| RRA-003 | People with read access see neither button in the run's view. | `ci.e2e.ts` |
| RRA-004 | Nothing else changes: the server and its API, `ci_rerun`, the CD tab. No WCAG violations. | e2e, gate |

## Plan

- [x] Read the product context, docs and earlier sessions on the topic
- [x] Investigate the code, data or sources and collect evidence
- [x] Answer with the evidence, the options and a recommendation
- [x] Spec SPEC-258 written and validated
- [ ] Run cards show no Run again; the run view offers Run again and Cancel (e2e first)
- [ ] Docs and changes log updated
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green (scripts/dev ci, e2e, security)

## Acceptance criteria

- No run card in the mock's studio CI list has a "Run again" button; the running dev card keeps
  "Cancel".
- The view of a failed run has "Run again"; clicking it toasts "queued again" and shows the new
  run's view at its own address.
- The view of a running run has "Cancel"; confirming it shows the run Cancelled with "Run again".
- A reader's run view has no "Run again" and no "Cancel".

## Test plan

`e2e/tests/ci.e2e.ts` against the mock UI: the list without the button, Run again and Cancel from
the run's view, the reader's view, axe on the view with its buttons. Then `scripts/dev ci` and
`scripts/dev security`.

## Decisions

- **Move, not delete.** Deleting the button leaves a cancelled or flaky run's commit untestable by
  a person until a new commit comes. The run's view is where a person decides to retry (they just
  read the log), so it holds the action; the list stays a list.
- **Cancel stays on the card too.** A run to stop is found in the list and stopping it should not
  take a detour; there is at most one active run per reference, so it does not repeat down the list.
- **Run again opens the new run.** The rerun is a new run with its own id; staying on the old one
  would show a finished run after the person asked for a fresh one. Back returns to the old run.
- No ADR: a placement of an existing action within one page, recorded here.

## Evidence

To be filled at the end.
