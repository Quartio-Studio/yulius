# 0440. Estimate Codex work at Standard and Ultrafast API prices

## Status

Accepted for [SPEC-602](../../.ai/specs/SPEC-602-it-doesnt-seem-real-verify-i-think-it-s.md).

## Context

The pool's [list-price estimate](0089-api-price-estimates-from-a-built-in-table.md) predates Codex. The stored run totals identify the model and token classes. Only native Codex rollouts contain thread settings and individual response usage.

## Decision

Price recorded GPT-6 family runs in the base estimate at OpenAI's Standard short-context rates, including 30-minute cache writes. When there is Ultrafast-eligible work, also calculate the weekly total with all eligible usage at the current Ultrafast API price. Both figures use the same recorded runs and rolling window. Retained per-response settings can be used separately to verify historical usage. [OpenAI API pricing](https://developers.openai.com/api/docs/pricing)

## Alternatives

Applying the launch default to historical totals would assign a tier across changes of subscription and speed. Reading private rollouts on every capacity request would add filesystem access and latency to an instance-wide aggregate.

## Consequences

Existing metrics can be repriced without a migration. The figures cover recorded run metrics. Separate Codex child rollouts, long-context premiums, other processing options and the subscription's credit consumption require additional evidence. A model outside the table continues to count toward the list of unpriced runs.
