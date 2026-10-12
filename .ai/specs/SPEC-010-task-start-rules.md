---
yulius-task: b74b1901-c035-461c-97f1-3c84bf6954aa
status: draft
---

# SPEC-010: tasks start by a logical rule

Status: draft
Date: 2026-10-12
Owner: Yulius task #15
Scope authorization: user request: "Now we can chain tasks but we want actually even more
advanced rules, based on logical sentences. For example, start a task if it's true that task X
and Y are both finished. Etc."

## Goal

A task can wait for a **start rule**, a logical sentence over the state of other tasks of its
sandbox, for example `#12 done and #13 done` or `#12 done or (#13 failed and not #14 running)`.
It starts by itself the moment the sentence is true. A chain ("Run after #N", TSK-012) is the
special case `#N done`.

## Context

- TSK-012 (`crates/yulius-app/src/chains.rs`): one predecessor per chained task, stored in
  `task_chains.after_task_id`; `advance_chains(predecessor)` starts successors when it is Done
  and marks them "needs you" when it ended otherwise.
- ADR-0012 records chains. Tasks are numbered per sandbox, so rules name tasks as `#N`.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| RUL-001 | A rule is text: atoms `#N` or `#N <state>` with states `done` (default; alias `finished`), `merged`, `ended` (stopped, failed, closed or deleted), joined by `and`, `or`, `not` (also `&&`, `\|\|`, `!`) and parentheses; `not` binds tightest, then `and`, then `or`. | `yulius-domain` rule tests |
| RUL-002 | A rule is refused with a message naming the problem when it is empty, longer than 500 characters, deeper than 8 levels, has more than 16 atoms, or is malformed. | rule tests |
| RUL-003 | Evaluation is three-valued: a task that has not ended and is not in the asked state makes its atom unknown. The task starts when the rule is true, waits while unknown, and needs a person (reason shown) when it is false for good. | rule tests, chain tests |
| RUL-004 | Every task named by a rule must exist in the same sandbox, and must not be the task itself. | API test |
| RUL-005 | A rule is evaluated again whenever any task it names changes, and on the background tick. | integration test |
| RUL-006 | `after_task_id` keeps working unchanged and means `#N done`. A rule and `after_task_id` together are refused. | API test |
| RUL-007 | Task summaries show the rule; Start now and Cancel stay available; the composer offers "When a rule is true…". | API and e2e tests |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] A start rule parses and evaluates as a logical sentence (pure domain)
- [ ] A chained task waits on its rule and starts when it is true (store, advancer)
- [ ] Start rules can be set through the API and shown on tasks
- [ ] The composer offers a start rule
- [ ] Review, docs, ADR and delivery

## Acceptance criteria

- `#1 done and #2 done` starts the task only after both are Done; `#1 done or #2 done` after
  either; a rule that can no longer be true puts the task under Needs you.
- Existing chains behave exactly as before.
- `scripts/dev ci` passes.

## Test plan

Domain unit tests for parsing, precedence, limits and three-valued evaluation; a store test for
the dependency table; app/integration tests for and/or/not starting and blocking; an API test for
validation; an e2e test for the composer.

## Decisions

- Text syntax stored as given; the AST is reparsed, so there is one source of truth.
- Dependencies are stored in `task_chain_deps` so an event on any named task finds the waiting
  task; `after_task_id` stays the first named task (branch base, briefing).
- Three-valued evaluation avoids a separate "can this still happen" analysis.
