---
yulius-task: 16e0fc4e-6cb9-45bf-b5f9-6c9b011204c8
status: in-progress
---

# SPEC-602. Verify the seven-day API estimate for Astra Ultrafast

## Goal

Explain the missing Codex work in the reported seven-day estimate. Include GPT-6 Codex models in the existing estimate and show the effect of Ultrafast on the pool's recorded runs.

## Context

[SPEC-110](SPEC-110-here-we-want-token-usage-statistics-also.md) prices selected Claude models and counts unknown models separately. The captured card reports 151 unpriced runs. [SPEC-523](SPEC-523-make-ultrafast-default-speed-in-codex-if.md) sets a launch default on eligible subscriptions. The database keeps normalized token totals and one model per attempt. Native response records and applied thread settings provide finer evidence for a historical verification.

## Requirements

- **CST-1** Reconcile retained responses from the requesting project over a named seven-day window. Distinguish the instance-wide card, project evidence, API equivalents and subscription spending. Report the source of the tier and the limits of the comparison.
- **CST-2** Estimate recorded GPT-6 family runs at published short-context Standard prices using the four disjoint token classes. Apply the same base prices to existing pool history and per-account totals. Preserve existing window and unknown-model behavior.
- **CST-3** Show a second seven-day total when recorded work has an eligible Ultrafast model. Price only eligible usage at the Ultrafast rate in force at the time of the estimate. Identify the figure as a scenario, name the base rate, and disclose omitted runs.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Reconcile the screenshot window with recorded Codex and Claude responses
- [x] Price recorded Codex usage in the pool and subscription estimates
- [x] Show the Ultrafast scenario and the coverage of the estimate
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- **AC-1** Evidence uses distinct native response IDs, applied tiers and the public price schedule. State the assumed end time and whether a pool-wide invoice can be established.
- **AC-2** One million each of ordinary input, cache reads, writes and output on GPT-6.2 Astra adds $58.40 to the base total. An unlabeled model remains unpriced.
- **AC-3** During the GPT-6.2 promotion that same Astra usage adds $175.20 to the Ultrafast scenario. Other priced providers and ineligible Codex models contribute their base costs once. A run crossing the beginning of the week contributes only the overlapping fraction. The UI omits the scenario when the server supplies none.

## Test plan

Compare aggregates from saved rollout metadata and Claude message usage, keeping private source data in the sandbox. Verify model prices, tier eligibility, the promotion boundary, and mixed-window aggregation in Rust. Exercise the subscription page, the shared capacity panel and an older payload in the browser.

## Decisions

Retain the built-in list-price method in [ADR-0089](../../docs/decisions/0089-api-price-estimates-from-a-built-in-table.md). The historical audit can use the thread's recorded tier. The live card reads attempt totals, which have no tier or individual request lengths. Report a Standard estimate and an explicitly conditional Ultrafast figure for those totals. [Decision 0440](../../docs/decisions/0440-estimate-codex-work-at-standard-and-ultrafast-api-prices.md)

## Evidence

- The task's private verification record uses a seven-day window ending at submission on November 21, 2026 at 17:24:14 UTC. The Codex audit used unique response IDs, model by turn and the applied thread settings. Cached tokens were removed from ordinary input. Each response fell within short-context pricing. It included separate child rollouts and avoided summing repeated cumulative usage events. The Claude check used the greatest reported token counts per distinct message and the card's price table. Project totals and the instance-wide screenshot cover different sets of runs. Provider invoices were unavailable.
- The Standard-price regression failed for GPT-6.2 Astra before the new table. The browser scenario and subagent disclosure each failed before their corresponding UI changes.
- The app's mixed-pool and per-subscription tests passed. The mixed pool covers a partial week, mixed providers, an unknown model, an ineligible model, and the instant the GPT-6.2 Ultrafast promotion expires.
- The domain pricing suite passed six tests and the capacity suite passed eight. The Chromium coverage regression passed with both omitted-run counts and the Codex subagent disclosure. `scripts/dev quick` passed repository contracts and formatting.
- Separately saved Codex subagents contributed to the historical audit. The existing parent-only attempt metrics are tracked in [issue 1628](https://github.com/Quartio-Studio/yulius/issues/1628).
