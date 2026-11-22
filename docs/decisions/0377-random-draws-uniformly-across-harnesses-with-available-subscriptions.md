# 0377. Random draws uniformly across all harnesses with available subscriptions

## Status

Accepted on 2026-11-19 (`F 2026-11-19 Random harness`: the person's request in task #502, "i dont
know why but i dont see random harness feature anymore. It should be randomised between all added
harnesses and every harness should have same chance to get selected, even if onehas much more
subscribtions (unless subscribtion is finished or there is 0 subscribtons for the harness).
Implement"). Specified in [SPEC-520](../../.ai/specs/SPEC-520-i-dont-know-why-but-i-dont-see-random-ha.md),
fixing [#1437](https://github.com/Quartio-Studio/yulius/issues/1437). Supersedes the no-draw part
of [ADR-0368](0368-a-tasks-agent-is-set-never-drawn.md) and the two-harness draw of
[ADR-0361](0361-antigravity-is-a-third-harness-picked-at-random-by-default.md); ADR-0368's rule
that a task's agent is fixed once created, and that a running task agent's unadorned follow-up
keeps the calling task's agent, stands.

[ADR-0422](0422-cli-and-superagent-task-creation-defaults-to-codex.md) supersedes the default draw for direct CLI and Superagent task creation.

[ADR-0426](0426-the-new-task-composer-defaults-to-codex.md) supersedes the initial Random selection in the browser.

## Context

- Task #496 ("make sure harnesses doesnt change themselves between tasks (like claude -> gemini)
  because it's unpredictable. A harness is set for a task") asked for a task's harness to stay
  fixed rather than switching unexpectedly, and SPEC-514 (ADR-0368) removed the random harness
  feature altogether instead of keeping `Random` while locking each task's assigned harness.
- Before SPEC-514, SPEC-505 slice E (ADR-0361) drew only between Claude Code and Antigravity
  (excluding Codex), used `roll % len` on one `u8` (biased when `len == 3`: `256 % 3 == 1`), and
  checked `AgentAccounts::Usable` (`status.healthy()`), which counts `Limited` and `>= 98%`
  exhausted accounts as usable. A harness whose subscriptions were all finished could still be
  drawn and pause while other harnesses had capacity.

## Decision

- **All harnesses, equal weight per harness.** A task created without an explicit harness (`""` or
  `"auto"`) and without a `model` draws uniformly (`1 / k`, unbiased via rejection sampling over
  `CryptoPort::random_bytes()`, accepting bytes `< 256 - (256 % k)`) among the `k` harnesses in
  `Harness::ALL` (`ClaudeCode`, `Codex`, `Antigravity`) that currently have at least one healthy
  signed-in subscription with available capacity for that harness's default model (or its fallback
  family), as judged by the rotation policy (`!usage.exhausted_for(now, rotation::THRESHOLD,
  family)` and not a bare `Limited` status without usage). A harness with five subscriptions and a
  harness with one subscription each get one equal vote (`1 / k`).
- **Finished or absent pools excluded; all-limited fallback.** A harness with zero subscriptions,
  only unsigned/`relogin`/`error` subscriptions, or whose subscriptions are all finished is left
  out while any other harness has capacity. When every connected harness's subscriptions are
  currently finished (`k == 0`) while `m >= 1` harnesses have healthy (`AgentAccounts::Usable`)
  subscriptions waiting for a window reset, the draw picks uniformly (`1 / m`) among those `m`
  harnesses so the task queues in `waiting_capacity` instead of failing for a missing Claude
  account. With no healthy signed-in subscription anywhere (`m == 0`), it falls back to
  `Harness::ClaudeCode`, refused with 409 `AccountRequired`.
- **Fixed for life and recorded.** Once drawn at creation, the harness is stored on the task for
  its entire lifetime and recorded in `task_harness_draws` (`harness_choice: "random"`, analytics
  JSON and CSV); explicit or model-implied harnesses report `"chosen"`.
- **Caller-harness follow-ups.** A running task agent's `task_create` or `automation_create` that
  omits `harness` (and `model` on `task_create`) keeps the calling task's harness (`chosen`);
  explicit `harness: "auto"` and calls without a calling task (Superagent, Global Superagent,
  Remote MCP) draw.
- **Automations and UI.** Automations accept and default to `auto` (`Random` in the editor,
  drawing per run) and preserve their stored harness on partial `PUT` updates that omit `harness`;
  stored `claude`, `codex`, and `antigravity` automations stay as saved. The composer and Settings
  default to `Random` (`Random agent`) when no model is chosen, and `mock.js` mirrors the server.

## Alternatives

- **Weight the draw by subscription count or remaining headroom**: directly contradicts the
  person's requirement that every harness have the same chance even when one has many more
  subscriptions, and would bias cross-harness comparisons toward whichever provider has more seats.
- **Keep `AgentAccounts::Usable` (`status.healthy()`) as the sole filter**: includes `Limited` and
  `>= 98%` exhausted subscriptions, picking a finished harness that immediately pauses while other
  harnesses have spare capacity.
- **Refuse creation when every connected harness is temporarily `Limited`**: breaks Yulius's
  existing pool contract (ROT-010) where tasks created against a temporarily full pool wait for the
  next window reset instead of failing with 409.
- **Single-byte `u8 % 3` without rejection sampling**: gives index `0` (`ClaudeCode`) `86 / 256`
  against `85 / 256` for `Codex` and `Antigravity`.
- **Migrate stored `claude` automations back to `auto`**: cannot distinguish automations migrated
  by 0068 from ones explicitly created for Claude Code since; leaving stored rows untouched is safe
  and reversible in the editor.

## Consequences

- Every connected harness with non-exhausted capacity gets an exact `1 / k` share of default tasks
  and `Random` automation runs, labelled `harness_choice: random` in task views and analytics.
- When a provider's subscriptions all hit their limit, new random tasks route automatically to the
  remaining harnesses until capacity returns, without moving any already-created task across
  providers.
