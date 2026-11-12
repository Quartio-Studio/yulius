# 0307. A refresh asks Claude about each stored limit with the model it binds, and the background round reads no model's allowance ahead

## Status

Accepted. Specified in [SPEC-473](../../.ai/specs/SPEC-473-subscriptions-page-refresh-now-one-and-a.md);
task #451. Refines [ADR-0301](0301-a-models-usage-credits-are-a-limit-of-that-model.md) (what ends
a model's limit, and what a usage reading keeps) and SPEC-001 ROT-009 (what a usage reading is).

## Context

ADR-0301 stores a limit Claude reports for one model as that model's weekly window, full until
the reset Claude named, and keeps it across usage readings, which cannot see it. Two things were
left open:

- A person who buys usage credits could not make Yulius use the subscription before that reset:
  nothing asked Claude again. A limit recorded without a reset was ended by the next provider's
  reading whether or not it still held.
- Nothing reads a model's allowance of usage credits before it is used up, so a task meets the
  limit as a refused request.

What Claude Code 2.1.322 does with its own responses (read from its binary, 2026-11-12): the
allowance comes as `anthropic-ratelimit-unified-7d_oi-*` headers on responses of the model that
has one; a refusal is HTTP 429 with the limit in `representative-claim` and its reset in
`unified-reset`, or with `error.details.error_code: "credits_required"` in the body; a request
that ran on credits beyond the plan still carries `unified-status: rejected` beside
`overage-status: allowed`; its own quota check is one request of one output token with the model
asked about. Its status line gives Yulius the 5-hour and weekly windows only, each "present only
while the API reports it and its resets_at has not passed".

The cost of asking with a model. The probe's request is about 25 input tokens and one output
token. At API list prices (`yulius_domain::pricing`) that is about $0.00003 with the cheapest
model (Haiku 4.5, $1 and $5 per million tokens) and about $0.00024 with the default one (Fable
5.2, $8 and $40). The round reads an idle subscription every 5 minutes: 2 016 requests a week,
about $0.06 with the cheapest model and about $0.48 with the default one, per subscription. A
request with the default model draws on that model's allowance, and once the allowance is used
up, on credits the person bought, or it is refused.

## Decision

- **A refresh asks about each stored limit with the model it binds.**
  `POST /api/admin/accounts/{id}/usage` reads the usage and then sends one request of one output
  token for each limit stored and in effect: with a model of each family whose own weekly window
  is used up (the default model for its own family, else the first one Yulius offers of the
  family), and with the default model for a limit of the whole subscription. Nothing is asked
  when no limit is stored. At most five provider requests, one after another, under one
  deadline of 25 s, shorter than the 30 s the web client waits.
- **The response's status says whether the model runs.** A response that ran ends the limit,
  whatever its headers say, and nothing is stored about the model's allowance. HTTP 429 that
  names a limit is recorded like a limit a session reports (`ReportedLimit::of_refusal`, the
  one rule the harness uses too): the window is full until the reset the answer names, so a
  limit recorded without a reset gets one, and an answer that names none keeps the stored one.
- **On this path the answer ends a limit without a reset, not the reading.** The refresh's
  reading keeps a model's window stored without a reset (`AccountUsage::keeping_asked`). The
  round's reading still ends it, as ADR-0301 decided.
- **An answer is applied under the usage lock, for the sign-in and the limit it was asked
  about.** A limit a session reported meanwhile is newer than the answer; a new sign-in's limits
  are not the previous one's.
- **A question Claude does not answer fails the refresh** with one sentence (502, kept as the
  usage note) and leaves the limit. A reading Claude will not give for now (not shared, or
  throttled) does not stop the questions; a refused sign-in or a failed request does.
- **The background round asks about no model's allowance ahead of its limit.** A task still
  meets the end of an allowance as one refused request, which the limit path (ADR-0027,
  ADR-0301) turns into a move past the cut turn or a wait until the reset.
- **The time a usage was read is the time both of its windows were read**
  (`AccountUsage::observed_at`): by the provider, by a session whose reading carried both, or
  with an answer that carried both. A session's reading of one window and a limit Claude reports
  no longer stamp it. So a card's age is the age of its windows, and the round, which skips a
  subscription read in the last 90 s, asks the provider for one whose sessions report one
  window.
- **"Refresh all" is the page's loop over that one route**, three subscriptions at a time.

## Alternatives

- **Read the allowance in the background with the default model**, for every subscription every
  5 minutes, so the pool leaves at 98 % of it. It costs eight times the cheapest model's request,
  drawn from the allowance it measures, also on subscriptions nobody runs that model on. Once
  the allowance is used up, every such request is refused or billed to credits the person
  bought. And the rule it would feed is wrong for those credits: a subscription whose owner
  bought them runs past 100 % of the allowance, and a 98 % rule would move tasks off it before.
  What it would save is one refused request per task at the end of an allowance. Rejected.
- **Ask with the default model in place of the cheapest one**, one request for all three
  windows. The same cost and the same refusals, and a subscription that cannot run the default
  model would get no reading at all. Rejected.
- **Read the allowance only for subscriptions with live runs of that model.** Those are the
  ones the round skips, because their sessions report; it needs a second schedule for the same
  saving. Rejected.
- **Store the allowance a model's answer carries.** Nothing reads it again, so one stored at 98
  to 99.9 % would keep the pool off the subscription for that model until the reset, with a card
  that reads ready and a refresh that asks nothing. Rejected (the spec's first version).
- **Go by `unified-status` instead of the response's status.** A request that ran on bought
  credits says `rejected` there; the limit would never end for the person this decision is for.
  Rejected.
- **Let the refresh's reading end a limit without a reset, as any reading does.** Claude would
  be asked about a limit already gone, and a question that failed could not leave it. Rejected.
- **A route that refreshes every subscription**, or the background round on demand. One long
  request whose result is all or nothing to the page, or a round whose back-off refuses the
  reading the person asked for. Rejected.
- **A time per usage window** in place of one for the snapshot. Exact, and `UsageWindow` is
  built in about forty places; the one rule above says the same for the two windows a card
  shows first. Rejected.
- **A check in the round for a missing window** (the first version). It reads again for good a
  window the provider does not have, and leaves the card's age wrong. Rejected for the rule on
  `observed_at`.

## Consequences

- A limit a person lifted ends at the refresh, and a task that waits for that model's reset
  resumes at once. A limit recorded without a reset gets its reset, or ends, by Claude's own
  answer.
- A refresh costs at most five requests: the usage endpoint, the cheapest model's headers, and
  one request per stored limit with the model it binds. A subscription with no stored limit
  costs what it did before.
- The end of a model's allowance still costs each task on that subscription one refused
  request. The pool does not see it coming, by decision.
- The header semantics are Claude Code 2.1.322's, read from its code and not from a live
  response. If they differ, a refresh fails with its sentence, or ends a limit that the next
  refusal records again at the cost of one refused request.
- A limit Claude reports no longer makes the usage read "just now". The round may read that
  subscription up to 90 s sooner than before; for a limit without a reset that brings ADR-0301's
  one retry forward by as much.
- A subscription whose sessions report one window costs one request of the cheapest model
  every 5 minutes while they run, as an idle subscription does.
- An identical limit reported again while Claude is being asked is not told from the one asked
  about: an answer "the model ran" ends it, and the next refusal records it again.
