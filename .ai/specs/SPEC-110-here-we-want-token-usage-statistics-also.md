---
yulius-task: 1e0c5639-069d-465d-b8fc-0a06d556f040
status: done
---

# SPEC-110: token usage, API-price cost and a chart for every figure

Status: done
Date: 2026-10-14
Owner: task #95
Scope authorization: the person's request (task #95), "here we want token usage statistics, also we
want a chart for every metric it makes sense as we have for tasks", with two screenshots: the
Subscriptions page's Capacity card (Available now, Left this week, Burn rate, Runs out, Paused) and
the Dashboard's key figures (only Tasks has a chart). Added while the task ran: "also for
subscriptions we want estimated cost in case it would be priced by API (cache, input, output etc)
and also similar chart".

## Goal

The subscription pool's Capacity card says how many tokens the pool used and what they would have
cost at Claude API prices, each with a 7-day chart; every Capacity and Dashboard figure whose
history means something carries a small chart like Tasks has.

## Context

- **Capacity card** (`capacityPanel`, `web/assets/layout.js`), shown on the Subscriptions page
  (`pages/admin.js`, region "Capacity") and in the Dashboard's Subscriptions card. Its data is
  `GET /api/capacity` → `CapacityView` (`yulius-contracts` `api/admin.rs`), built by
  `App::capacity_view` (`yulius-app/src/capacity.rs`) from `yulius_domain::capacity::forecast`
  (SPEC-002 ADR-17 §2.4). It has no token figure today; the burn rate is in percentage points of
  the subscriptions' weekly windows, read from provider usage samples (7 days kept).
- **Dashboard key figures** (`pages/dashboard.js` `renderSummary`): `GET /api/analytics/summary` →
  `AnalyticsSummary`, a cohort of tasks created in range (`yulius-app/src/analytics.rs`). Only
  `tasks_per_day` is a series; Tasks draws it as a sparkline (`sparkline()`, decorative).
- **Token data**: one `metrics` row per run (attempt): model, input, output, cache read, cache
  write, active ms (SPEC-087), sub-agents included, kept current while the run lives (#31). Runs
  have `started_at`/`ended_at` and the subscription (`account_id`) they ran on.
- **Prices**: Claude API list prices per million tokens, from
  `platform.claude.com/docs/en/about-claude/pricing` (read 2026-10-14). Cache writes have a 5-minute
  (1.25× input) and a 1-hour (2× input) rate; the metrics row stores one cache-write total.
  Cache reads are 0.1× input, except Fable 5.1 / Mythos 5.1 (0.025×) and Opus 5.5 / Sonnet 5.5
  (0.05×). Haiku 5.5 doubles its rates above 100 000 prompt tokens.
- No earlier spec covers token cost; SPEC-087 (agent time) and SPEC-091 (recounts) define the
  metrics this reads.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| TUS-001 | `yulius_domain::pricing` holds the Claude API list price (input, 5-minute cache write, cache read, output; USD per million tokens) of every Claude model on the pricing page, matched on the model id without a date suffix (`-YYYYMMDD`) or a bracketed variant (`[1m]`). An unknown model has no price, never a zero one. | unit tests `pricing::` |
| TUS-002 | A run's estimated cost is input × input price + cache writes × 5-minute write price + cache reads × read price + output × output price; unknown token figures add nothing. | unit tests `pricing::` |
| TUS-003 | `CapacityView.tokens` gives the pool's token use over the last 7 days in seven rolling 24-hour buckets ending now (oldest first) and in total and for the last 24 hours: input, output, cache read, cache write, estimated cost, runs without usage and runs of a model without a price. A run counts in a bucket in proportion to the part of its lifetime (start to end, or now) inside it. | domain unit tests `capacity::`, `capacity_view` test |
| TUS-004 | Each subscription's existing token totals (`GET /api/admin/accounts` `tokens_24h`, `tokens_7d`: runs started in the window, ROT-009) carry `cost_usd`, the estimated cost of its runs whose model has a price; `null` when none has one. | `subscription_pool.rs` capacity test |
| TUS-005 | `CapacityView` carries the burn rate of each of the seven buckets (`burn_per_day`, points per hour, `null` without enough readings) and the task-hours paused in each (`paused_hours_per_day`). | domain unit tests, `capacity_view` test |
| TUS-006 | The Capacity card adds two tiles, "Tokens (7 days)" (input + output, with in, out and cache and the last 24 hours below) and "API cost (7 days)" ("≈ $1,234", "at Claude API list prices", the unpriced runs said), each with a 7-day chart; Burn rate and Paused get a 7-day chart too. A cost no run could be priced for shows "—". | `subscriptions.e2e.ts` |
| TUS-007 | The Subscriptions page's "Observed by Yulius" rows of each subscription end with "≈ $X at API prices" when known. | `subscriptions.e2e.ts` |
| TUS-008 | Each `AnalyticsSummary.tasks_per_day` day also carries the agent working time (`active_ms`), the median agent work of the tasks created that day (`median_active_ms`), input and output tokens and the switches made; a run counts on the UTC day it started, a switch on the day it was made. | `analytics::` unit tests |
| TUS-009 | The Dashboard's Agent work per task, Agent working time, Tokens and Subscription switches figures each show a chart of the range's days like Tasks. Feedback gets none: a day's share of a handful of ratings is noise. | `times.e2e.ts` |
| TUS-010 | Every chart has a text alternative naming the figure, the period and its first, highest and last values; it never stands alone for a number the tile does not also show. | `subscriptions.e2e.ts`, `times.e2e.ts` (axe clean) |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Claude API prices and the cost of a run, in the domain
- [x] The pool's tokens and cost per 24 hours and per subscription in `GET /api/capacity`
- [x] Burn rate and paused hours per 24 hours in `GET /api/capacity`
- [x] Tokens, cost and their charts in the Capacity card and the subscriptions
- [x] Daily agent time, tokens and switches in the analytics summary
- [x] Charts beside the Dashboard's key figures
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- `GET /api/capacity` answers `tokens.per_day` with 7 buckets whose sums equal `tokens.last_7d`,
  and `tokens.last_24h` equals the last bucket.
- A run of `claude-opus-5-5` with 1 M input, 1 M output, 1 M cache read and 1 M cache write costs
  $4 + $20 + $0.20 + $5 = $29.20.
- In the mock, the Subscriptions page's Capacity region shows "Tokens (7 days)" and "API cost (7
  days)" with charts, and Burn rate and Paused have charts; the Dashboard's Agent work per task,
  Agent working time, Tokens and Subscription switches figures have charts; no WCAG violations.

## Test plan

- Domain unit tests: price lookup (exact ids, date suffix, `[1m]`, unknown), cost arithmetic,
  bucketing (a run spanning two buckets, an open run, a run before the window, zero-length run,
  unknown usage, unpriced model), per-bucket burn and paused hours.
- `yulius-app` analytics unit test: per-day active time, median, tokens and switches.
- Integration test through `App::capacity_view` with a seeded store (runs, metrics, accounts).
- The new runs-with-metrics query through the `capacity_view` and accounts-list integration test.
- e2e (mock): Capacity tiles and charts, account foot, Dashboard charts, axe.

## Decisions

- **Rolling 24-hour buckets for the pool, UTC days for the Dashboard.** The Capacity card speaks
  of "the last 24 h" and "this week" from now; rolling buckets make its last bar exactly the last
  24 hours and no bar a partial day. The Dashboard's range is UTC days and its cohort is tasks
  created in it, so its series stay on UTC days.
- **A run's tokens spread over its lifetime for the pool, on its start day for the Dashboard.**
  A run stores totals, not when they were spent; the pool's window cuts through runs, so only the
  share inside it may count. The Dashboard's figures are per cohort, and its days must add up to
  its totals, so a run counts once, on its start day.
- **Cache writes at the 5-minute rate.** The stored total does not say which cache duration was
  written; the 5-minute rate is the API default, and the tile says "list prices", an estimate.
  Haiku 5.5 is priced at its rates up to 100 000 prompt tokens for the same reason.
- **Per-subscription cost extends the existing token rows.** The Subscriptions page already lists
  each subscription's tokens of the last 24 hours and 7 days (runs started in the window); its cost
  joins those rows instead of a second, differently counted figure in `GET /api/capacity`.
- **Prices as a table in the domain** (ADR-0089), not fetched: Yulius must not call out for a
  figure, and prices change rarely; the table records its source and date.
- **Pool-wide figures for everyone signed in.** `GET /api/capacity` is for every signed-in person,
  and its tokens and cost cover the whole pool, including sandboxes they cannot see; only totals
  are shown (no task, sandbox or person), as the burn rate already does.
- **The Dashboard's days stay inside its range.** A run or switch after the range's last day (a
  task created late that day ran on) counts in the range's totals but adds no day to the series.
- **Size.** About 450 changed lines outside tests, a little over the 400 guide, kept in one pull
  request because the person asked for both parts in this task and the chart helper is shared.

## Evidence

- `cargo test -p yulius-domain --lib pricing` (5 tests: list prices, ids with date suffix and
  `[1m]`, unknown models, cost arithmetic: Opus 5.5 1 M of each = $29.20) and `capacity`
  (bucketing over rolling days, unknown usage and unpriced models, zero-length runs, per-day burn
  and paused hours): pass.
- `cargo test -p yulius-app --lib analytics` (per-day agent time, median, tokens, switches; nothing
  outside the range) and `accounts::tests::a_subscriptions_cost_counts_only_priced_runs_with_usage`:
  pass.
- `cargo test -p yulius --test main -- subscription_pool::` (10): `GET /api/capacity` gives 7
  periods, 600 000 / 400 000 input split of a 40-hour run, $31 for 7 days and $18.60 for the last
  24 h, `burn_per_day` and `paused_hours_per_day`; `GET /api/admin/accounts` gives the
  subscription's `cost_usd` $31 and `null` where nothing ran: pass.
- `scripts/dev ci` on the reviewed head `b064b81c`: check, lint (fmt, clippy `-D warnings`), deps,
  repository contracts, nextest `1589 tests run: 1589 passed, 16 skipped`; exit 0.
- `cd e2e && npx e2e run` (whole suite, before the last `dev` merge): `45 passed | 1 skipped`
  files, `330 passed | 1 skipped` tests; after merging `dev` (#429) `subscriptions`, `times` and
  `sessions`: 28 passed. Covers the Capacity tiles and their labelled charts, the per-subscription
  "≈ $X at API prices", an unknown 24-hour cost shown as "—", the Dashboard figures' charts and
  axe.
- Screenshots in the mock (dark theme) of the Capacity card, a subscription card and the Dashboard
  key figures looked right: charts sit between the value and the detail lines, nothing overflows.
- Independent review (fresh session, `om-code-review`): round 1 asked for changes (one major:
  days outside the range; minors: unknown 24-hour cost shown as $0.00, no-usage runs costing $0,
  a duplicate attempts scan, ADR gaps, test gaps), all fixed test-first; round 2 approved
  `b064b81c`.
- Not tested here: real Claude Code runs against live subscriptions (the figures come from the
  stored metrics, exercised through the store in the integration tests).
