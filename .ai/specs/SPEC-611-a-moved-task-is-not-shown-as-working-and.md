---
yulius-task: 3a440d56-3c75-4dc8-a3de-d3bbccd904d6
status: done
---

# SPEC-611. Preserve idle Codex activity and place sessions by remaining quota

## Goal

A Codex task moved while idle still shows Waiting or Done and a finished task can be closed. The pool uses the Codex subscription with the most remaining quota when it places a new or resumed session.

## Context

On 21 November 2026, two idle Codex tasks appeared Working after a subscription move until another turn ended. Seven new tasks went to a subscription with 7% weekly quota left while another had 98%. Codex warns at the end of a turn when less than 10% remains.

[SPEC-447](SPEC-447-subscriptions-switch-by-hand-delete-one.md) reopens idle sessions without typing. At `3fc0e5976` on `origin/main`, a launch without a saved idle activity defaults to Working. [SPEC-590](SPEC-590-look-bro-the-taks-says-it-working-but-th.md) gives bare Codex resumes a Waiting default on `dev`. The supervisor still preserves a moved session's specific idle activity only for Prompt mode, although Codex also runs tasks recorded in Goal mode. [SPEC-001 ROT-004](SPEC-001-yulius-product.md) previously ranked subscriptions by weekly quota per hour until reset and live demand. A subscription close to its reset could win with little quota left.

## Requirements

- **CQS-001** A live idle Codex task moved into the same conversation without an opening turn retains Waiting or Done. The replacement run checks for background work before publishing its activity so the switch's working timer ends when there is none. A new turn, including a held continuation or reviewer recovery, sets Working. When the observed idle attempt or its activity changes before the switch claims it, the move returns a conflict and the current run stays in place. Existing switch cancellation and spawn failure paths settle the task as before.
- **CQS-002** Automatic Codex placement selects the eligible account with the highest remaining percentage in its tightest binding window. A known account with at least 10% left precedes unknown usage, which precedes a known account with less than 10% left. Equal known headroom uses the existing ranking to break ties. A passed reset follows existing usage semantics. Eligibility, explicit account choice, and the threshold that triggers moving an existing run stay as specified by ROT-003 and ROT-007.

## Plan

- [x] Specify idle moves and quota-based Codex placement
- [x] Preserve idle task activity across a subscription move
- [x] Place new and resumed Codex sessions by remaining quota
- [x] Preserve a Codex turn started while an idle move prepares
- [x] Verify regressions and obtain independent review

## Acceptance criteria

- **AC-1** Move Goal-mode Codex tasks in Waiting and Done to a second subscription. The same conversation opens with no prompt, the task list still shows it idle, and `working_since` is absent. The finished task can be closed without another turn. An interrupted turn or pending reviewer still opens with work.
- **AC-2** With Codex accounts at 93% and 2% weekly usage, both a new task and an unpinned resume use the account at 2%, even when the 93% account resets soon. The tightest applicable window takes precedence across both accounts, including when both have less than 10% left. Full accounts are excluded and explicit picks still work.
- **AC-3** Submit a new turn while the move of an idle Codex task is preparing. Once the turn starts, the earlier idle move returns a conflict and the same attempt keeps working.

## Test plan

Use the application lifecycle with the real PTY adapter and fake Codex to prove idle moves, the task list state, close, new task placement and Continue. Exercise headroom, resets, unknown usage and capacity exclusions in the rotation domain tests. Run the account move and related Codex tests, repository quick checks and the delivery gate. The focused regressions must fail before their fixes.

## Decisions

Reuse the supervisor's existing idle snapshot and choose activity from the final launch mode so work appended during the launch still takes precedence. Use Codex's least remaining percentage across binding windows to order eligible accounts. Reset urgency can break ties but cannot place a session onto an account with less known headroom. Keep the existing pool assessment, credentials and refresh paths.

Claim an idle move against the observed attempt, activity and task update time in the store. A changed task can be moved again after the caller rereads it. Preserve the newly started turn in place so a stale request never ends it.
[Decision 0449](../../docs/decisions/0449-use-codex-quota-headroom-and-preserve-idle-moves.md)

## Evidence

With the original pool policy at `13a632c40`, `codex::new_and_resumed_codex_sessions_choose_the_subscription_with_more_quota` chose `a-codex` for both runs. The domain regression `rotation::tests::codex_placement_compares_the_tightest_binding_window` chose `early-reset` over `more-quota`.

Nextest run `3b95b0a8-008d-4946-a84c-e674ad025d7d` passed all 26 rotation checks and the start and resume regression after the placement correction. The idle move regression still found a populated `working_since` after the move, before the supervisor correction.

Nextest run `129035a2-9d2c-4567-92ab-436f357d50b2` passed all 227 selected rotation, Codex, Claude Code move and subscription pool checks. The open-and-close test used a real PTY host with a fake Codex process and synthetic usage. The first independent review of `b4853b74` identified a new turn that could overtake the snapshot before the switch claimed the task.

Before adding the guarded claim, nextest run `ac1e240a-9f8c-4ce8-b01e-7f905cdfa511` reproduced a new working turn interrupted at both pause points. A move overtaken during classification reopened the task as Waiting. A move overtaken during switch preflight reopened the task as Done.

On `cfff0d94d6caadadebb0d4dc082d83af45d35e1c`, nextest run `20e96616-0fa4-4d23-81be-2055f7692d8f` passed all 210 selected Codex, subscription pool, rotation and SQLite checks. Both race tests kept the original Working attempt. The store test refused an earlier update time, a different idle activity and a different attempt, then accepted the observed state. The idle move still preserved Waiting and Done and the 2%-used subscription took both launches.

`scripts/dev quick`, `spec_check` and `git diff --check` passed. `scripts/dev security` reported Vidoc disabled. Independent re-review approved `cfff0d94d6caadadebb0d4dc082d83af45d35e1c` with no actionable findings.
