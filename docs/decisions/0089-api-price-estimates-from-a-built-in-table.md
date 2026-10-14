# 0089. Token cost is estimated from a built-in table of Claude API list prices

## Status

Accepted. Specified in [SPEC-110](../../.ai/specs/SPEC-110-here-we-want-token-usage-statistics-also.md).

## Context

People asked what the subscription pool's work "would cost if it were priced by the API (cache,
input, output etc)", with a chart. Subscriptions are not billed per token, so nothing reports a
price: Yulius has only each run's model and its input, output, cache-read and cache-write tokens
(one cache-write total, without the 5-minute or 1-hour split the API bills apart). Yulius runs
self-hosted and must not depend on an outside service to show a figure.

## Decision

`yulius_domain::pricing` holds the Claude API list price (USD per million tokens) of every Claude
model on the pricing page (read 2026-10-14, source linked in the module): input, 5-minute cache
write, cache read and output. A model id matches without its date suffix (`-YYYYMMDD`) or a
bracketed variant (`[1m]`); an unknown model has no price, so its tokens are counted and its cost
is not (`unpriced_runs`), never priced at zero. Cache writes use the 5-minute rate and Haiku 5.5 its
rate for prompts up to 100 000 tokens. The UI says "≈" and "at Claude API list prices": an estimate.
Prices are updated by editing the table when Anthropic changes them.

## Alternatives

- Fetch prices at run time (the pricing page or an API): an outside dependency and a failure mode
  for one figure, on instances that may have no egress.
- An administrator-editable price list: settings and storage for something that changes a few
  times a year and is the same for everyone.
- Price cache writes at the 1-hour rate: Claude Code's choice of duration is not recorded, and the
  5-minute rate is the API default; the higher rate would overstate as much as this may understate.

## Consequences

- A new model shows its tokens with "runs of unknown models not counted" until the table learns it.
- Batch, priority, regional (`inference_geo`) and long-context premiums are not modelled; the
  figure is the standard list price.
- A run's metrics name one model (the last it saw) while its tokens include its sub-agents', so a
  run whose sub-agents ran on Haiku, or that switched model, is priced at the named model's rate.
- Haiku 5.5 is priced at its rate for prompts up to 100 000 tokens; Claude Code prompts are often
  longer (cache reads count), where the real rate is 5× higher, so Haiku 5.5 work is understated.
- Model ids of cloud platforms (`anthropic.…-v1:0` on Bedrock, `…@YYYYMMDD` on Vertex) are not
  recognised and count as unpriced; Yulius runs first-party Claude subscriptions.
