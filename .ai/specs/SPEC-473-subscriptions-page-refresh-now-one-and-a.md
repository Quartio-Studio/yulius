---
yulius-task: ce6a43f9-05c3-42d5-b364-6fd9d09e4ab9
status: in-progress
---

# SPEC-473: The Subscriptions page reads usage and limit state again on request

Status: in progress
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
- `usage.observed_at` is stamped by every writer of the stored usage: a provider's reading, a
  session's reading also when it carries one window only, and a limit Claude reports. The
  background round skips a subscription stamped in the last 90 s.

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
  a model's allowance, and a session's reading may hold one window only. By reading, that is
  how evidence (c) comes about: such a reading stamped the usage as seen, and the round then
  never asked. Production's records were not looked at (they are outside the sandbox).

Related work: SPEC-007 (usage shown all the time, the header probe), SPEC-172 (the folded card's
glance), SPEC-447 ("Run #N on…", which already judges by windows when the menu opens), SPEC-463
(the card's editable lines and their focus rules). No other task works on these files
(`scopes_list`, 2026-11-12).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| RFR-001 | The probe asks Claude about one model: one request of one output token with that model, read as a **model's answer**. A response that ran (2xx) says the model runs, whatever its headers say. HTTP 429 with a claim or `credits_required` is a refusal: what it binds and its window come from the claim by the rule the harness uses (one function in the domain, `ReportedLimit::of_refusal`), its reset from `unified-reset`; a model's own limit that names no reset takes the allowance's (`7d_oi-reset`), and a credits refusal that claims another window takes only that. The 5-hour and weekly windows the headers carry are read in either case. HTTP 401 is a refused sign-in; a 429 without a claim, any other status, a timeout and an unreadable answer are a failure that says nothing about the limit. | to be filled |
| RFR-002 | A model's answer changes the stored usage of that model's family (`AccountUsage::record_model_answer`). **Ran:** a limit of the whole subscription ends (`limit_reached`), and so does the family's own window; nothing is stored in its place. **Refused:** recorded like a limit a session reports (`record_limit`), so a window stored without a reset gets the answer's; an answer that names no reset keeps the stored one while that is ahead. The subscription's status and the pool follow at once (`record_account_usage`): a task that waits for that model's reset resumes. | to be filled |
| RFR-003 | `POST /api/admin/accounts/{id}/usage` reads the usage and then asks about each limit stored for the subscription and in effect: with a model of each family whose own weekly window is used up, and with the default model for a limit of the whole subscription (the default model for its own family, else the first offered model of the family). On this path the reading keeps a model's window stored without a reset (`keeping_asked`), which any other provider's reading ends (SPEC-464 UCL-004): here the answer decides. Nothing is asked when no limit is stored. At most five provider requests per call, one after another, under one deadline of 25 s, which is shorter than the web client waits. | to be filled |
| RFR-004 | A refresh that could not do its work answers 502 with one plain sentence (no HTTP code, no credential), and what was stored about the limit stays. A refused sign-in or a usage request that failed ends it before any question. A reading Claude will not give for now (the sign-in may not read usage, or the request is throttled) does not stop the questions; a throttle is then still reported, an unshared reading is not a failure. A question Claude does not answer says "Claude did not answer when asked whether the limit still holds; the limit stays as it was."; a refused sign-in says "Claude did not accept this subscription's sign-in. If its tasks fail too, sign in again."; the deadline says "Claude took too long to answer; what it had answered by then is stored.". Each of those three is also stored as the usage note (`last_error`, `usage: …`); a throttle stores none, as before. | to be filled |
| RFR-005 | An answer is applied under the usage lock, and only while the sign-in it was asked with and the limit it was asked about are still the ones stored: a limit Claude reported to a session meanwhile is newer than the answer, and a new sign-in's limits are not the previous one's. A call that is cut off (the deadline, or the caller gone) leaves what its finished steps stored. | to be filled |
| RFR-006 | `usage.observed_at` says when the subscription's own windows were last read: by the provider, by a session whose reading carried both, or with an answer about a model that carried both. A session's reading of one window and a limit Claude reports change the stored usage without stamping it. So the background round, which skips a subscription read in the last 90 s, asks the provider for a subscription whose sessions report one window, at its own pace (5 minutes after a reading). | to be filled |
| RFR-007 | The background round does not ask about a model's allowance ahead of its limit; the cost and the reasons are ADR-0307. | ADR-0307; to be filled |
| RFR-008 | One function says what a subscription's stored usage means now (`limitsNow`, `web/assets/usage.js`): whether the default model can run (a limit of the subscription in effect, or the 5-hour, weekly or default model's window at 100 % and not past its reset), which windows are used up with their resets, and the reading's age. The card's state, the Status panel's counts and health line and "Run #N on…" use it. A stored `limited` whose window has reset reads ready, and a stored `ok` with a binding window at 100 % reads "Limit reached". | to be filled |
| RFR-009 | The card (`accountUsage`: Subscriptions page, Status panel, Dashboard) starts its body with the reading's age: "Usage read 3 min ago", "Usage read just now", "Usage not read yet"; one older than 10 minutes is marked with an icon and words, "Usage read 25 min ago · may be out of date". The foot no longer carries the age. A folded row whose reading is out of date or missing carries the same mark in its summary, as an icon with those words for a screen reader and on hover. | to be filled |
| RFR-010 | Under the age, one line per used-up window says which and when it resets: "5-hour window used up · resets at 5:50 PM", "This week used up · resets Friday 10:00 AM", "Fable this week used up · resets …" (also Opus, Sonnet); without a reset "… used up · Claude did not say when it resets". A limit of the subscription with no window at 100 % reads "Claude reported a usage limit · resets …" or "Claude reported a usage limit without saying when it resets". A window of a model other than the default one gets its line and does not make the card read "Limit reached". | to be filled |
| RFR-011 | A window past its reset is shown as not read since ("Reset since this reading", "—", a hatched bar), in the card's rows and in the folded glance, never as 100 % used. | to be filled |
| RFR-012 | A shown card follows the clock without a reading or a redraw of the list: its state, lines, rows and age are worked out again every second while it is on the page, and drawn again only when they changed. A name or email being edited, the keyboard focus and a running refresh are kept; a card that left the page is dropped. Nothing of the card is a live region, so the clock announces nothing. | to be filled |
| RFR-013 | Each signed-in subscription's card on the Subscriptions page has **Refresh** ("Refresh usage of {name}" to a screen reader). While it reads, the button reads "Reading…" and is off without losing the keyboard (`aria-busy`, `aria-disabled`); then the card is drawn again with the new reading and its age, and a toast says "Usage of {name} read again." (toasts are announced). When Claude gave no usage reading, the card says so for the visit: "Claude gave no new usage reading: it shares this subscription's usage only with its running tasks.". A failure is a toast "Couldn’t refresh {name}: {sentence}", the sentence stays on the card for the visit, and the card is drawn from the list read again. A subscription that needs a sign-in has no Refresh: its card offers "Sign in again". | to be filled |
| RFR-014 | **Refresh all** in the page's head reads every signed-in subscription. It ends with one toast: "Usage read again for 1 subscription." / "… for {n} subscriptions.", or "{k} of {n} subscriptions could not be read: each card says why.". It is off while it runs and when no subscription is signed in. | to be filled |
| RFR-015 | Every reading the page asks for goes through one queue: three subscriptions at a time, one request per subscription at a time (a second ask joins the first), kept across redraws of the list and dropped when the page is left. The page's own reading of a card older than 10 minutes happens once per subscription per visit, says nothing when it fails, and is not repeated at a redraw. After a reading the capacity panel is read again. | to be filled |
| RFR-016 | Only instance administrators get the buttons (the page and the route are theirs already); the Status panel and the Dashboard show the age and the used-up windows to everyone who sees usage there. No WCAG violation on the page, with a card reading and with a stale one. | to be filled |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The probe reads a model's answer: its allowance, or the refusal's claim and reset
- [x] A model's answer ends or dates the stored limit of its family
- [x] The refresh asks Claude about each stored limit and says why when it cannot
- [ ] The background round reads a window the sessions do not report, and ADR-0307 says why it does not read an allowance ahead
- [ ] One function says what a subscription's usage means now, for the card's state, the Status panel and "Run #N on…"
- [ ] The card says its reading's age and which window is used up, and follows the clock
- [ ] Refresh on each card and Refresh all, through one queue of the page
- [ ] The API, architecture, product and changes documents say what the refresh does
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- A subscription stored as out of Fable's allowance until a reset, whose default model now
  answers: after `POST …/usage` the view has no `weekly_fable`, the status is `ok`, and exactly
  one request went out with the default model. With real processes: a task that waits in its
  `limit` pause for that reset, 40 hours ahead, runs again on the same subscription right after
  the refresh.
- The same subscription stored without a reset, refused with the claim
  `seven_day_overage_included` and a reset: the view's `weekly_fable.resets_at` is that reset,
  the status stays `limited`, and the round's reading afterwards keeps the window and sends no
  request with the default model.
- The default model's request fails (a server error, a refused sign-in, a throttle): 502 with
  one sentence without "HTTP", the stored window unchanged with and without a reset, the
  sentence kept as the usage note. A question that is never answered ends at the deadline.
- A subscription with no stored limit: no request with the default model.
- A limit a session records while Claude is being asked is still stored after an answer that
  the model runs, and an answer for the previous sign-in is not recorded for a new one.
- A subscription whose sessions report only the weekly window gets its 5-hour window from the
  round, and its `observed_at` does not move at those sessions' readings.
- On the page: a card read 2 minutes ago says so at the top of its body; one read 3 days ago is
  marked "may be out of date", also in its folded row; the foot holds no age.
- A card whose 5-hour window is at 100 % with a reset two seconds ahead reads "Limit reached"
  and "5-hour window used up · resets at …", and stops doing so when the reset passes, with no
  request made. A stored `ok` with the 5-hour window at 100 % reads "Limit reached", and the
  Status panel counts it "at the limit".
- Refresh on a card: the button reads "Reading…" while the request is held and keeps the
  keyboard, then the card reads "Usage read just now". A refused refresh shows the server's
  sentence in a toast and on the card.
- Refresh all: every signed-in subscription is read once, the one that needs a sign-in is not,
  at most three requests run at a time, and one toast says how it went.
- axe finds no violation on the Subscriptions page in these states, nor on the Dashboard and
  the Status panel.

## Test plan

- Unit, `wiring::usage`: the answer of a model from status, headers and body (ran, ran on
  credits with `rejected` headers; each claim; `credits_required` with another claim; a 429
  without a claim; 401; 403; 500; odd header values).
- Unit, `yulius-domain`: the claim's scope and window (moved from the harness, whose tests keep
  passing); `record_model_answer` (ran, refused with and without a reset, a limit of the
  subscription, a family without a window, what stamps the reading's time); the reading that
  keeps a window without a reset; which families are asked.
- Integration, `crates/yulius/tests/usage_refresh.rs`: the real `ProviderUsage` pointed at a
  local stand-in for Claude's API that answers by path and model, refuses a request that is
  not the probe's (bearer, one token) and records what it was asked, behind the HTTP API: the
  acceptance cases above, an Opus window asked with an Opus model, a throttled reading, a
  reading that fails. Fake probes for the races (a limit recorded, a sign-in stored during the
  question) and the deadline.
- Integration with real processes, `usage_credits.rs`: the waiting task resumes after the
  refresh; an unanswered refresh leaves it waiting. The tests there that meant "a provider's
  reading" take `App::read_account_usage`, the reading the round makes.
- Integration, `subscription_pool.rs`: a session's one-window reading through the status line
  hook, then the round.
- e2e, `subscriptions.e2e.ts` (mock mode): the age and the stale mark, also folded; the used-up
  lines for each window, a limit without a reset and another model's window (a unit test of
  `limitsNow` in the page, and the cards); a reset that passes while the page is shown, with the
  mock's request count unchanged; Refresh with the mock's answer held, a refusal, no new
  reading, and a limit the refresh ends; Refresh all with one failing and the concurrency
  counted; a name being edited survives a clock redraw; axe in each state. The mock's
  `POST …/usage` can be held, refused and counted.
- Existing suites around the change: `account_usage`, `accounts`, `account_email`,
  `usage_credits`, `subscription_pool`, `subscription_moves`, `pool_supervision`, `ui_contract`;
  e2e `subscription-names`, `subscription-touch`, `consistency`, `loading-shapes`.
- Gates: `scripts/dev ci`, `cd e2e && npx e2e run`, `scripts/dev security`.

## Decisions

- **One change.** The buttons, the limit state and the card's wording are one capability: the
  refresh's result is only visible on the card, and the card's truth depends on what the
  refresh stores.
- **Refresh all is the page's loop over the one route, not a second route.** Each card shows
  its own progress and failure, one slow subscription holds nobody else up, and no new contract
  is needed; an API client loops the same way. The page's queue bounds it (RFR-015). Rejected:
  `POST /api/admin/accounts/usage` (a long request whose result is all or nothing to the page),
  and sharing the background round (its back-off would refuse the reading the person asked
  for).
- **The limit is asked about with the model it binds** (ADR-0307): a full Opus window with an
  Opus model, a limit of the whole subscription with the default model. Only the model's own
  response says whether it runs.
- **What says the model runs is the HTTP status, not `unified-status`.** A request that ran on
  bought credits carries `rejected` with `overage-status: allowed`. Assumed (reversible): the
  header semantics are read from Claude Code 2.1.322's code, not from a live response. If they
  are wrong the refresh fails with its sentence, or ends a limit that the next refusal records
  again at the cost of one refused request.
- **A model that ran stores nothing about its allowance** (spec review). The first version
  stored an allowance read below 100 %. Nothing reads it again (RFR-007), so a reading of 98 to
  99.9 % would have kept the pool off the subscription for that model until the reset, with a
  card that reads ready and a Refresh that asks nothing.
- **The refresh's reading does not end a limit without a reset; the answer does** (spec
  review). "Reads the usage as before" would have ended such a limit before Claude was asked,
  and a question that then failed could not leave it. The round's reading still ends it
  (SPEC-464 UCL-004, UCL-010).
- **A failed question fails the refresh** (502), also after the usage reading itself succeeded
  and was stored: the person asked for the limit state, and a 200 would read as "still
  limited". The contract of the route stays: a view, or 502 with one sentence. Rejected: 200
  with an outcome field beside the view (a new contract; the page keeps the sentence itself).
- **One deadline of 25 s** (spec review): five requests of 15 s each would outlast the 30 s
  the web client waits, and the request would be cut off with nothing said. Rejected: asking
  the families at once (still 45 s at worst), and a task that outlives its caller (each step
  stores whole or not at all, so a cut-off call leaves nothing half-written).
- **The reading's time is the time both windows were read** (spec review, RFR-006). The card's
  age would otherwise be the time of the last write of any kind: "Usage read just now" over a
  5-hour row nobody read, which is evidence (a) again. It also answers evidence (c) with one
  rule in place of a second check in the round. A limit Claude reports no longer stamps the
  usage either; the round may then read that subscription up to 90 s sooner, which for a limit
  without a reset brings SPEC-464's one retry forward by as much. Rejected: a time per window
  (`UsageWindow` is built in about forty places), and a check for missing windows in the round
  (the first version: it re-reads for good a window the provider does not have).
- **No server-side debounce of the manual refresh.** The route is the administrators', each
  call is bounded, a button is off while its request runs, and the page's own reading is now
  once per visit (RFR-015). Rejected: returning the stored state for a reading younger than
  some seconds (the person asked to read now).
- **The card's state is the page's computation, the stored `status` stays the server's.** The
  API keeps `status` as written at the last reading (documented as such); `limitsNow` uses it
  only for `relogin`, `pending_login` and `error`. Rejected: a status computed in each view on
  the server (three views, and a page left open would still go stale without a clock).
  Known limit: the page compares Claude's reset times with the browser's clock, so a clock
  that is wrong shifts the moment a card changes by its error.
- **"No new reading" is told by the reading's time**, which only a reading moves: the page
  compares it before and after. Rejected: a field in the answer for it (a new contract).
- **Stale is 10 minutes**: the round reads an idle subscription every 5, so an older reading
  means the round is backing off or cannot read. It is the age the page already read again at.
- **A second per card** for the clock: a few date comparisons, drawn only on change, stopped
  with the last card. Rejected: a timer per reset (three kinds of deadlines per card), and
  redrawing only on the next list read (not "the moment its window resets").
- **Another model's used-up window is a line, not "Limit reached"**: the default model still
  runs there (SPEC-464 UCL-005), and the Status panel's counts follow the same rule.
- **Known limit: an identical limit reported again during a question** is not told from the one
  asked about (the guard compares what is stored): an answer "the model ran" then ends it, and
  the next refusal records it again.
- **Not done:** asking about a model's allowance in the background (ADR-0307); an audit entry
  per refresh (the page's own readings would fill the log).
