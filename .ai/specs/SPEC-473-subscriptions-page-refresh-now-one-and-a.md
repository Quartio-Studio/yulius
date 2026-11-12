---
yulius-task: ce6a43f9-05c3-42d5-b364-6fd9d09e4ab9
status: draft
---

# SPEC-473: The Subscriptions page reads usage and limit state again on request

Status: draft
Date: 2026-11-12
Owner: Yulius task #451
Scope authorization: the person's requirement of 2026-11-11 (`F 2026-11-11 limits` in SPEC-001),
split out of task #449 ([SPEC-464](SPEC-464-we-still-have-healthy-subscribtionsbut-w.md),
[ADR-0301](../../docs/decisions/0301-a-models-usage-credits-are-a-limit-of-that-model.md)): "The
Subscriptions page gets a manual refresh, per subscription and for all: it re-reads usage and
limit state now, and each card says how old its reading is and which window (5-hour or week) is
exhausted."

## Goal

An instance administrator presses Refresh on a subscription, or Refresh all, and the card shows
what Claude says now: the usage windows, and whether a limit Claude reported still holds. Every
card says how old its reading is and which window is used up, worked out when it is shown.

## Context

The person's evidence (production, 2026-11-11, UTC):

- (a) A card read "Limit reached", "This week 100% used", "5-hour window —", "Updated 3 min
  ago"; a minute later the row read `ok` with the week at 61 %.
- (b) Two subscriptions at 100 % of their 5-hour window (resets 17:10 and 19:10) read `ok`.
- (c) Two subscriptions with live runs had no 5-hour reading at all, "so nothing saw the limit
  coming".

What exists after SPEC-464:

- `POST /api/admin/accounts/{id}/usage` (`App::refresh_account_usage`) asks the provider for one
  subscription's 5-hour and weekly windows. The page calls it by itself for cards whose reading
  is older than 10 minutes, again at every redraw of the list. There is no button.
- A limit Claude reported for one model is stored as that family's weekly window
  (`weekly_fable`, `weekly_opus`, `weekly_sonnet`), full until the reset Claude named. No reading
  carries the allowance of usage credits, so nothing ends such a limit before its reset: a person
  who buys credits cannot make Yulius use the subscription again. A model's limit recorded
  without a reset is ended by the next provider's reading, whether or not it still holds.
- The probe (`crates/yulius/src/wiring/usage.rs`) reads `anthropic-ratelimit-unified-5h-*` and
  `-7d-*` from a one-token request on the cheapest model.
- The stored `status` is written at each reading. A card follows a reset only at the next
  reading, and its age ("Updated 3 min ago") sits in the card's foot.
- The background round skips a subscription whose stored usage was observed in the last 90 s.
  A session's reading refreshes that time also when it carries one window only
  (`App::session_usage` keeps the other as stored).

Verified against the installed Claude Code 2.1.322 (strings of its binary, 2026-11-12):

- It reads `anthropic-ratelimit-unified-{5h,7d,7d_oi,overage}-{utilization,reset,
  surpassed-threshold,status}` (`[["five_hour","5h"],["seven_day","7d"],
  ["seven_day_overage_included","7d_oi"],["overage","overage"]]`), and
  `anthropic-ratelimit-unified-{status,reset,representative-claim,overage-status,overage-reset,
  overage-disabled-reason,fallback}`. Utilization is a fraction, a reset is Unix seconds.
- A refusal is HTTP 429. Its claim (`representative-claim`: `five_hour`, `seven_day`,
  `seven_day_opus`, `seven_day_sonnet`, `seven_day_overage_included`) names the limit and
  `unified-reset` its reset. A model refused for usage credits has the claim
  `seven_day_overage_included` or the body's `error.details.error_code: "credits_required"`.
  A 429 with neither is a throttle, not a usage limit.
- A request that ran may still say `unified-status: rejected` with `overage-status: allowed`:
  it ran on usage credits beyond the plan (`isUsingOverage`). What says the model runs is the
  HTTP status.
- Its own quota check is one request of `max_tokens: 1` with the model it asks about.
- Its status line input carries `rate_limits.five_hour` and `.seven_day` only, each "present
  only while the API reports it and its resets_at has not passed". So a session cannot report
  a model's allowance, and a session's reading may lack a window: by reading, that is how
  evidence (c) comes about. Production was not looked at.

Related work: SPEC-007 (usage shown all the time, the header probe), SPEC-172 (the folded card's
glance), SPEC-447 ("Run #N on…", which already judges by windows when the menu opens), SPEC-463
(the card's editable lines and their focus rules). No other task works on these files
(`scopes_list`, 2026-11-12).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| RFR-001 | The probe asks Claude about one model: one request of one output token with that model, read as a **model's answer**. A response that ran (2xx) says the model runs, with its weekly allowance of usage credits when the headers carry it (`7d_oi`: utilization as percent, reset). HTTP 429 with a claim or `credits_required` is a refusal: what it binds and its window come from the claim by the rule the harness uses (one function in the domain), its reset from `unified-reset`; a credits refusal that claims another window takes its reset only from `7d_oi-reset`. HTTP 401 is a refused sign-in; a 429 without a claim, any other status, a timeout and an unreadable answer are a failure that says nothing about the limit. | to be filled |
| RFR-002 | A model's answer changes the stored usage of that model's family (`AccountUsage::record_model_answer`). **Ran:** a limit of the whole subscription ends (`limit_reached`), and the family's window becomes the allowance the answer read when that is below 100 %, else nothing: a model that runs with its allowance used up runs on credits a person bought. **Refused:** recorded like a limit a session reports (`record_limit`), so a window stored without a reset gets the answer's; an answer that names no reset keeps the stored one. The subscription's status and the pool follow at once (`record_account_usage`). | to be filled |
| RFR-003 | `POST /api/admin/accounts/{id}/usage` reads the usage as before and then asks about each limit stored for the subscription: with a model of each family whose own weekly window is used up, and with the default model when a limit of the whole subscription is in effect (the first offered model of the family, `OFFERED_CLAUDE_MODELS`). Nothing is asked when no limit is stored. At most five provider requests per call, one after another, each bounded by the probe's timeout. | to be filled |
| RFR-004 | A failure of either step answers 502 with one plain sentence (no HTTP code, no credential) and stores it as the usage note (`last_error`, `usage: …`); what was stored about the limit stays. A refused sign-in says so. A sign-in that may not read the usage endpoint and gets no headers (`NotShared`) is no failure: the limits are still asked about. | to be filled |
| RFR-005 | A limit recorded while Claude is being asked is kept: the answer is applied under the usage lock only when the limit it was asked about is still the one stored. | to be filled |
| RFR-006 | The background round reads a subscription whose stored usage lacks the 5-hour or the weekly window, or holds one past its reset, even when a session refreshed it moments ago; its own pace (5 minutes after a reading, the back-off after a failure) still bounds it. | to be filled |
| RFR-007 | The background round does not ask about a model's allowance ahead of its limit; the cost and the reasons are ADR-0307. | ADR-0307 |
| RFR-008 | One function says what a subscription's stored usage means now (`limitsNow`, `web/assets/usage.js`): whether the default model can run (a limit of the subscription in effect, or the 5-hour, weekly or default model's window at 100 % and not past its reset), which windows are used up with their resets, and the reading's age. The card's state, the Status panel's counts and health line and "Run #N on…" use it. A stored `limited` whose window has reset reads ready, and a stored `ok` with a binding window at 100 % reads "Limit reached". | to be filled |
| RFR-009 | The card (`accountUsage`: Subscriptions page, Status panel, Dashboard) starts its body with the reading's age: "Usage read 3 min ago", "Usage read just now", "Usage not read yet"; one older than 10 minutes is marked with an icon and words, "Usage read 25 min ago · may be out of date". The foot no longer carries the age. | to be filled |
| RFR-010 | Under the age, one line per used-up window says which and when it resets: "5-hour window used up · resets at 5:50 PM", "This week used up · resets Friday 10:00 AM", "Fable this week used up · resets …" (also Opus, Sonnet); without a reset "… used up · Claude did not say when it resets". A limit of the subscription with no window at 100 % reads "Claude reported a usage limit · resets …" or "Claude reported a usage limit without saying when it resets". A window of a model other than the default one gets its line and does not make the card read "Limit reached". | to be filled |
| RFR-011 | A window past its reset is shown as not read since ("Reset since this reading", "—", a hatched bar), in the card's rows and in the folded glance, never as 100 % used. | to be filled |
| RFR-012 | A shown card follows the clock without a reading or a redraw of the list: its state, lines, rows and age are worked out again every second while it is on the page, and drawn again only when they changed. A name or email being edited, the keyboard focus and a running refresh are kept. | to be filled |
| RFR-013 | Each signed-in subscription's card on the Subscriptions page has **Refresh** ("Refresh usage of {name}" to a screen reader). While it reads, the button is disabled and reads "Reading…" (`aria-busy`); then the card is drawn again with the new reading and its age, the keyboard still on the button. A failure is a toast "Couldn’t refresh {name}: {sentence}", and the card shows what the server stored. A subscription that needs a sign-in has no Refresh: its card offers "Sign in again". | to be filled |
| RFR-014 | **Refresh all** in the page's head reads every signed-in subscription, three at a time, each card drawn again as its answer arrives. It ends with one toast: "Usage read again for {n} subscriptions." or "{k} of {n} subscriptions could not be read: each card says why.". It is disabled while it runs and when no subscription is signed in. | to be filled |
| RFR-015 | The page's own reading of a card older than 10 minutes happens once per subscription per visit of the page, not at every redraw of the list. | to be filled |
| RFR-016 | Only instance administrators get the buttons (the page and the route are theirs already); the Status panel and the Dashboard show the age and the used-up windows to everyone who sees usage there. No WCAG violation on the page, with a card reading and with a stale one. | to be filled |

## Plan

- [ ] Read the product context and related work, then write and validate the spec
- [ ] The probe reads a model's answer: its allowance, or the refusal's claim and reset
- [ ] A model's answer ends or dates the stored limit of its family
- [ ] The refresh asks Claude about each stored limit and says why when it cannot
- [ ] The background round reads a window the sessions do not report, and ADR-0307 says why it does not read an allowance ahead
- [ ] Each card works out its state, used-up windows and reading age when it is shown
- [ ] Refresh on each card and Refresh all on the Subscriptions page
- [ ] The API, architecture, product and changes documents say what the refresh does
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- A subscription stored as out of Fable's allowance until a reset, whose default model now
  answers: after `POST …/usage` the view has no `weekly_fable`, the status is `ok`, and exactly
  one request went out with the default model.
- The same subscription stored without a reset, refused with the claim
  `seven_day_overage_included` and a reset: the view's `weekly_fable.resets_at` is that reset,
  the status stays `limited`, and a provider's reading afterwards keeps the window.
- The default model's request fails (a server error, then a refused sign-in): 502 with one
  sentence without "HTTP", the stored window unchanged, the sentence kept as the usage note.
- A subscription with no stored limit: no request with the default model.
- A limit a session records while Claude is being asked is still stored after an answer that
  the model runs.
- A subscription whose sessions report only the weekly window gets its 5-hour window from the
  round.
- On the page: a card read 2 minutes ago says so at the top of its body; one read 3 days ago is
  marked "may be out of date"; the foot holds no age.
- A card whose 5-hour window is at 100 % with a reset two seconds ahead reads "Limit reached"
  and "5-hour window used up · resets at …", and stops doing so when the reset passes, with no
  request made. A stored `ok` with the 5-hour window at 100 % reads "Limit reached", and the
  Status panel counts it "at the limit".
- Refresh on a card: the button reads "Reading…" while the request is held, then the card reads
  "Usage read just now" and the button has the keyboard. A refused refresh shows the server's
  sentence in a toast and leaves the card as the server has it.
- Refresh all: every signed-in subscription is read once, the one that needs a sign-in is not,
  and one toast says how it went.
- axe finds no violation on the Subscriptions page in these states, nor on the Dashboard and
  the Status panel.

## Test plan

- Unit, `wiring::usage`: the answer of a model from status, headers and body (ran with and
  without an allowance; each claim; `credits_required` with another claim; a 429 without a
  claim; 401; 403; 500; odd header values), and the request the probe sends (the model, one
  token), against the local mock server the existing probe tests use.
- Unit, `yulius-domain`: the claim's scope and window (moved from the harness, whose tests keep
  passing); `record_model_answer` (ran, ran with an allowance below and at 100 %, refused with
  and without a reset, a limit of the subscription, a family without a window); which families
  are asked.
- Integration, `crates/yulius/tests/usage_refresh.rs`: the real `ProviderUsage` pointed at a
  local mock server that answers by path and model and counts requests, behind the HTTP API:
  the acceptance cases above, plus an Opus window asked with an Opus model. A fake probe that
  records a limit during the request for RFR-005.
- Integration, `subscription_pool.rs`: the round reads a window the sessions do not report.
- e2e, `subscriptions.e2e.ts` (mock mode): the age and the stale mark; the used-up lines for
  each window, a limit without a reset and another model's window (a unit test of `limitsNow`
  in the page, and the cards); a reset that passes while the page is shown, with the mock's
  request count unchanged; Refresh with the mock's answer held, a refusal, and a limit the
  refresh ends; Refresh all with one failing; a name being edited survives a clock redraw;
  axe in each state. The mock's `POST …/usage` can be held, refused and counted.
- Existing suites around the change: `account_usage`, `accounts`, `usage_credits`,
  `subscription_pool`, `subscription_moves`, `ui_contract`; e2e `subscription-names`,
  `subscription-touch`, `consistency`, `loading-shapes`.
- Gates: `scripts/dev ci`, `cd e2e && npx e2e run`, `scripts/dev security`.

## Decisions

- **One change.** The buttons, the limit state and the card's wording are one capability: the
  refresh's result is only visible on the card, and the card's truth depends on what the
  refresh stores. The background round's rule (RFR-006) is included because it is the person's
  evidence (c) and six lines.
- **Refresh all is the page's loop over the one route, not a second route.** Each card shows
  its own progress and failure, one slow subscription holds nobody else up, and no new contract
  is needed; an API client loops the same way. Rejected: `POST /api/admin/accounts/usage` (a
  long request whose result is all or nothing to the page), and sharing the background round
  (its back-off would refuse the reading the person asked for).
- **The limit is asked about with the model it binds** (ADR-0307): a full Opus window with an
  Opus model, a limit of the whole subscription with the default model. Only the model's own
  response says whether it runs.
- **What says the model runs is the HTTP status, not `unified-status`.** A request that ran on
  bought credits carries `rejected` with `overage-status: allowed`. Assumed (reversible): the
  header semantics are read from Claude Code 2.1.322's code, not from a live response.
- **An allowance read at 100 % on an answer that ran is not stored.** Stored, it would keep the
  model off the subscription until its reset, which is the defect this change removes. Below
  100 % it is stored like any reading, so the pool leaves at 98 % of it.
- **A failed question leaves the limit and fails the refresh** (502), also after the usage
  reading itself succeeded and was stored: the person asked for the limit state, and a 200
  would read as "still limited".
- **No server-side debounce of the manual refresh.** The route is the administrators', each
  call is bounded, the button is disabled while its request runs, and the page's own reading is
  now once per visit (RFR-015). Rejected: returning the stored state for a reading younger than
  some seconds (the person asked to read now).
- **The card's state is the page's computation, the stored `status` stays the server's.** The
  API keeps `status` as written at the last reading (documented as such); `limitsNow` uses it
  only for `relogin`, `pending_login` and `error`. Rejected: a status computed in each view on
  the server (three views, and a page left open would still go stale without a clock).
- **Stale is 10 minutes**: the round reads an idle subscription every 5, so an older reading
  means the round is backing off or cannot read. It is the age the page already read again at.
- **A second per card** for the clock: a few date comparisons, drawn only on change, stopped
  with the last card. Rejected: a timer per reset (three kinds of deadlines per card), and
  redrawing only on the next list read (not "the moment its window resets").
- **Another model's used-up window is a line, not "Limit reached"**: the default model still
  runs there (SPEC-464 UCL-005), and the Status panel's counts follow the same rule.
- **Not done:** asking about a model's allowance in the background (ADR-0307); a stale mark in
  the folded row's summary (SPEC-172's row has no room; unfolded, its body starts with the
  age); an audit entry per refresh (the page's own readings would fill the log).
