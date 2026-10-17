---
yulius-task: 995d1b5e-0316-4e75-8bdc-9ee68849708b
status: done
---

# SPEC-209: An agent's secret request shows on an open Secrets page at once

Status: done
Date: 2026-10-17
Owner: task #196
Scope authorization: task #196 (requested by MrFishPL): "Allow didnt show instantly but i needed
to refresh the page. Fix." The screenshot shows a Superagent that requested `api.digitalocean.com`
for `DO_PERSONAL_TOKEN`, while the open Agent secrets page still showed no request. Fixes #631.
Extends SPEC-121 (hosts agents ask for) and SPEC-129 (`.env` requests) over the UI event stream
(SPEC-002 §7.8).

## Goal

When an agent or a Superagent asks for a secret's hosts or for a `.env` copy, a person who already
has the Agent secrets page open sees the request, with its Allow and Dismiss buttons, without
reloading the page.

## Context

- **Symptom.** The Superagent called `secret_request_hosts` and printed the Secrets link. The
  person's open Agent secrets page still showed `DO_PERSONAL_TOKEN` with no waiting host and no
  Allow. The request appeared only after a reload.
- **Root cause.** `tool_secret_request_hosts` (`crates/yulius-app/src/vault/requests.rs`) and
  `ask_env_files` (`vault/env_files.rs`) store the request and audit it. Their only other signal
  is `flag_secret_wait` (`vault/users.rs`), which marks a task as needing attention and does
  nothing for a Superagent. `UiEvent` (`crates/yulius-contracts/src/events.rs`) has no event for
  secrets. `mountAgents` (`web/assets/pages/secrets.js`) loads the list once and listens to
  nothing, not even a stream `resync`. So nothing ever tells an open page that a secret changed.
- **Why tests missed it.** The Rust tests read the request back over the API. The e2e tests open
  the page after the mock already holds the request. No test kept a page open while a request
  arrived.
- **Same cause elsewhere.** The `.env` request from `secret_copy_to_env` and from a dev CD run
  (`cd_secret_values`) goes through `ask_env_files`, so it had the same gap.

## Requirements

- SRE-001. A new host request (`secret_request_hosts` adding at least one host) and a new or
  extended `.env` request (`ask_env_files` writing the row) publish `UiEvent::SecretsUpdated`
  (`secrets.updated`), from a task's agent and from a Superagent alike.
- SRE-002. The event carries only `sandbox_id`: the secret's sandbox, or `null` for a secret of
  every sandbox. It has no name, host or value. It is scoped like the secret. A sandbox secret's
  event reaches the people who can view that sandbox. An instance-wide secret's event reaches
  everyone signed in, since every sandbox's page lists it.
- SRE-003. An open Agent secrets page reloads its list quietly, keeping what it shows until the
  new list arrives. It reloads on `secrets.updated` for its sandbox or with a `null` sandbox, and
  on a stream `resync`. It stops listening when it unmounts or switches to Team secrets.
- SRE-004. A request that changes nothing (hosts already bound or already waiting, or the same
  `.env` requester again) publishes nothing.
- SRE-006. A live update keeps SPEC-121's guarantee that nothing is allowed unseen. When a
  card's waiting hosts change after the page has shown it, its Allow and Dismiss are drawn
  disabled for one second, with the note "The hosts asked for changed while this page was open."
  A click or Enter meant for the earlier buttons therefore cannot allow a host the person never
  saw.
- SRE-007. A live update waits while an answer is on its way. Otherwise it would bring back
  enabled buttons for a request about to be gone. The reload after the answer catches up.
- SRE-005. The web mock announces an agent's request as the server does, so e2e tests can drive
  it.

## Plan

- [x] Spec and issue for the stale Allow
- [x] Agent requests publish a secrets.updated event
- [x] Agent secrets page reloads on the event
- [x] Docs, gates and independent review

## Acceptance criteria

- With the Agent secrets page open on the mock, an agent's request for OLD_DEPLOY_KEY shows
  "asks to send it to" and the Allow button with no page reload.
- An event for another sandbox does not reload the page. A request made while the stream was
  down shows after the stream reconnects.
- On the server, a task agent's host request, a Superagent's host request and a `.env` request
  each publish exactly one `secrets.updated` event with the secret's scope. A request that
  changes nothing publishes none.

## Test plan

- `crates/yulius/tests/secrets.rs`: a new test subscribes to the event bus, covers SRE-001,
  SRE-002 and SRE-004 for a task agent's host requests, on a sandbox secret and on an
  instance-wide one. `crates/yulius/tests/secrets_env.rs` covers a `.env` request. Both fail
  before the fix because no event arrives.
- `crates/yulius/tests/cockpit.rs`: the Superagent's host request in the existing test also
  asserts the event. This is the reported case.
- `e2e/tests/secrets.e2e.ts`: a request that arrives while the page is open shows Allow with no
  reload. A request missed while offline shows after the reconnect. Another sandbox's event does
  not reload the list.
- Gates: `scripts/dev ci`, `scripts/dev security`, and the e2e suite.

## Decisions

- **Announce, then refetch.** The event says only "this sandbox's secrets changed", and the page
  reads `GET /api/sandboxes/{id}/agent-secrets` again. That read already applies who may see or
  manage what (`can_manage`). This follows `pulls.updated` (SPEC-108) and `update.updated`. A
  view inside the event would duplicate those access rules per viewer. No new ADR: this applies
  the event stream's existing pattern.
- **Publish where the request is stored, not in `flag_secret_wait`.** That function only flags
  tasks and exits early for a Superagent, which is the reported case. The event belongs next to
  the write that changed the row.
- **Agent requests only.** A person's own answers already reload their page. Announcing every
  person-side change, so that a second open tab follows too, is a broader change. Stale buttons
  there are already safe, because the server refuses an answer to a request that changed
  (SPEC-121).
- **A short settle time, not a confirmation step** (review round 1, M1). Re-rendering a card
  replaces its Allow with one that carries the new hosts. A click already on its way, or a
  keyboard user whose focus `keepFocus` moves to the same place, could then allow a host they
  never saw. That is the case SPEC-121's "answer what you saw" exists to refuse. A second of
  disabled buttons and a visible note cover the moment of the swap without adding a confirmation
  step to every request. Firefox uses the same one-second delay on its security prompts. Keeping
  the old buttons until the person acknowledged the change was rejected. The card would show
  hosts that no longer match the request, and the server would refuse the answer anyway.
- **Polling rejected.** A timer would add load on every open page and still lag, while the event
  stream is already there.

## Evidence

- Regression tests written first and seen failing without the fix (no `secrets.updated` arrived:
  `left: []`, `right: [Some("s1")]`):
  `secrets::an_agents_host_request_is_announced_to_open_secrets_pages`,
  `secrets_env::an_env_request_is_announced_to_open_secrets_pages`, and
  `cockpit::the_superagent_copies_allowed_secrets_into_its_repositorys_ignored_env` (the reported
  Superagent case). The e2e tests `an agent’s host request shows its Allow on the open page at
  once` and `a request made while the stream was down shows once it reconnects` failed on the
  missing Allow before the page fix. All pass with it.
- `scripts/dev ci`: exit 0, 2131 tests run, 2131 passed (18 skipped), with clippy, fmt, doctests
  and repository contracts.
- `cd e2e && npx e2e run`: 59 files passed (1 skipped), 608 tests passed (1 skipped). After
  merging `origin/dev` (#630 also changed the Secrets page), `tests/secrets.e2e.ts` was run again:
  17 passed. `scripts/check_repository.py` passed.
- Review round 1 (independent, fresh session): REQUEST CHANGES with two medium findings, both
  fixed test-first:
  - M1: a live re-render could put new hosts under a click. Fixed by SRE-006. The test `an
    Allow whose hosts just changed is off for a moment` failed before the fix (the new Allow was
    drawn enabled).
  - M2: the other-sandbox check could not fail. It now counts list reads (exactly one for this
    sandbox's event after another sandbox's).
  - Lows fixed: an answer in flight (SRE-007; `a live update waits for an answer on its way`
    failed before, 2 reads instead of 1), the test plan text, the mock giving a production secret
    a host request (it now refuses, and the e2e uses an instance-wide non-production secret), the
    store's topic list, and a doc comment's space.
  - Answered without change: an instance-wide event reaches every signed-in person but names
    nothing (SRE-002 by design), and a `.env` request naming N secrets sends N events (bounded,
    and the loader keeps only the newest read).
  - After the fixes, `tests/secrets.e2e.ts` passed 19 of 19. Unrelated to this change, dev's
    test "a failed copy shows no check…" (#630) fails its axe target-size check about 1 run in 5
    when run alone. It covers Team secrets, which this change does not touch, and is reported as
    #640.
- `scripts/dev security`: exit 0, no open critical or high finding. One low finding is in
  `requester()` (`vault/requests.rs`, joining askers on `", "`). This change does not touch that
  code, and its inputs (GitHub logins, sandbox slugs, `#N`) cannot contain `", "`.
