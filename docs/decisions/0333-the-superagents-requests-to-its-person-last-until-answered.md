# 0333. The Superagent's requests to its person last until answered

## Status

Accepted. Specified in
[SPEC-487](../../.ai/specs/SPEC-487-the-superagent-can-do-everything-its-per.md) (XRQ-001 to
XRQ-007), task #469, slice 8a. Supersedes "a request lives as long as its call" in
[0227](0227-the-superagent-starts-tasks-in-other-sandboxes-only-on-its-persons-approval.md) and
[0286](0286-the-superagent-changes-tasks-in-other-sandboxes-on-its-persons-approval.md), and
takes up the alternative 0227 left for later ("stored requests, answered at any time, with a
status tool"). Every other part of them holds: the panel, the card read to its end, only a
person's click answers, approval runs the same-sandbox tool as the person, the bound of 3.
Follows [0330](0330-the-superagent-does-what-its-person-may-do.md).

## Context

A request ended with the Superagent's call: after 4 minutes, at Esc, at a client timeout or at a
restart of Yulius. People were away, came back and found nothing to approve; the Superagent asked
again, the same task three times in three minutes to add one sentence. A decision only the
person makes ("may I merge tonight?") had no place at all: it went into reports nobody read
(walls 6 and 9 of task #469). And to find a task in another sandbox the Superagent probed numbers
one by one.

## Decision

- **A request stays until the person answers it or the Superagent withdraws it.** The call
  still waits up to 4 minutes; then, or when the call is dropped, it answers
  `{"outcome":"waiting","request":<id>}` and the card stays on the panel. Answered later, it runs
  as it would have, and nothing is audited until then. A cancelled call
  (`notifications/cancelled`) still withdraws it.
- **Kept across a restart.** The waiting requests and the newest 200 outcomes are written whole to
  `superagent-requests.json` beside the database at every change (a temporary file renamed into
  place), as armed merges are, and put back at a start; one whose target sandbox is gone is
  dropped. No migration: they are runtime state, few and short-lived.
- **The Superagent reads, amends and withdraws its own.** `sandbox_requests` lists its waiting
  requests and its 20 latest outcomes (approved with what was done, declined, failed with why,
  withdrawn). `sandbox_request_amend` replaces the text and options of a waiting create, message
  or question in place, checked as a new request is, keeping its id; a change of start or a stop
  is withdrawn and asked again. An amended request takes the next revision: its card is drawn
  again, and the person's answer names the revision it showed, so text changed after they read
  it never runs on their click (409). The amendment is audited, without its text. `sandbox_request_withdraw` removes one (audited `withdrawn`).
  Another Superagent's request is not found.
- **`person_ask {question}`** leaves a decision on the Superagent's own panel: the question read
  to its end, **Yes** / **No**; the answer (`yes` or `no`) is the call's or is read later. Nothing
  runs on it.
- **`sandbox_tasks_list {sandbox}`** lists another sandbox's tasks as `tasks_list` does, without
  asking (it only reads), for a sandbox the person can work in.

## Alternatives

- **A longer wait.** Node's 300 s header timeout bounds a call; a person away for an hour is
  still away. Rejected.
- **Store requests in the database.** A migration and a sweep for state that lives minutes to
  hours; the JSON file beside it already keeps armed merges and pull request notices the same
  way. Rejected.
- **An expiry (a day, a week).** A request nobody wants is withdrawn by its Superagent or
  declined in one click; an expiry would bring back "lost while away". Not added.
- **`person_ask` as a notification.** A notification is read and forgotten; the panel keeps the
  question until it is answered, beside the session that asked.

## Consequences

- A person who comes back finds every request waiting, and the Superagent finds how each ended
  instead of asking again.
- At most 3 requests wait per person and source sandbox; a fourth is refused with "amend or
  withdraw one".
- A request can be approved long after it was asked; approval checks the target's rights and
  state at that moment, as before, so a stale request fails with why rather than doing harm.
