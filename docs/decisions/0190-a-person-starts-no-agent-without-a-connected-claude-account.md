# 0190. A person starts no agent without a connected Claude account

## Status

Accepted. Specified in [SPEC-246](../../.ai/specs/SPEC-246-onboarding-requires-a-connected-claude-a.md);
task #232. Refines SPEC-001 ROT-010 (a start without capacity waits) and SPEC-051/SPEC-097
(onboarding); builds on [0187](0187-an-empty-repository-gets-its-first-commit-when-onboarding-starts.md).

## Context

- An early user's onboarding on a fresh instance broke, "probably because I didn't connect Claude
  and the first job failed". Nothing in first-run told them a Claude account was needed first.
- With no account, a task's start found nothing to run on and paused `waiting_capacity` with "no
  subscription has capacity left", the words of an exhausted pool; onboarding moved to `preparing`
  and waited there without saying what it waited for or where to fix it. Only the Superagent's
  start told the case apart, as a plain `conflict`.
- Agents are started by people (New task, Continue, Start now, Start onboarding, a browser import,
  the Superagent) and by machinery (a chain, a rule, an automation, a scheduled time, the resume
  loop), which has nobody to answer.

## Decision

- A Claude account is usable when it has a credential and is `ok` or `limited`, the pool's own
  health rule (`account_gate::agent_accounts`). `GET /api/me` carries `instance.claude_connected`.
- Every start of an agent a person asks for is refused before it changes anything while no account
  is usable: HTTP 409 with the new code `claude_account_required` (`AppError::AccountRequired`),
  whose message says no Claude account is connected (or that every one must sign in again) and that
  an instance administrator connects one in Subscriptions (`/admin/accounts`). Writing a prepared
  task is not a start.
- `start_onboarding` checks first, so a browser import without an account adds the sandbox and
  leaves onboarding `pending`, with no task and no first commit; the sandbox page shows the step.
- A start by machinery still waits `waiting_capacity` (ROT-010) and starts by itself once an
  account is connected; its notice says that no Claude account is connected (or that every one
  must sign in again) and where to connect one, and its summary carries `waits_for_account`, so
  the task strip, sidebar and time label say it waits for a Claude account, not for capacity.
- A start whose chosen account has lost its credential ends `failed_no_account` (new
  `FailReason::NoAccount`), not `failed_precondition`.
- The web UI shows "Connect Claude" as the first, required step while `claude_connected` is false:
  on Home in place of the playground drop, template and Import, in the composer, and in the
  onboarding gate, where Start onboarding (Retry onboarding after a failed start) waits for it. A
  `claude_account_required` toast offers Connect Claude. The page reads `/api/me` again on
  `account.updated` and when it comes back into view.

## Alternatives

- **Refuse machinery starts too.** A chain or automation would end failed with nobody told, the
  failed job this removes; waiting with a clear notice resumes by itself.
- **A Claude step in the setup wizard.** The sign-in is the Subscriptions dialog's PTY login, which
  another task changes in parallel; one sign-in flow linked from every first step stays simpler,
  and Home is where every first-run path begins.
- **Match the error message in the UI.** Wording changes; a code is a contract.
- **Retry failed onboarding by itself once connected.** A person asked for it before; offering Retry
  onboarding keeps them in control, and a waiting (paused) one already resumes by itself.

## Consequences

- No agent job a person starts on a fresh instance fails or waits silently for want of an account;
  the refusal is immediate and points to Subscriptions.
- Integration tests that started tasks or onboarding with no account now seed one; a test that
  wants a start to wait without a process uses a subscription with no capacity left
  (`support::exhausted_subscription`).
- The playground's server endpoints are unchanged (they run no agent); its UI waits for the
  connection because its first task would be refused.
