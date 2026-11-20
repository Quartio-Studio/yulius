# 0416. Parked MCP rows use the verified plan and PR

## Status

Accepted for [SPEC-558](../../.ai/specs/SPEC-558-parked-codex-tasks-no-codex-exit-screen.md).

## Context

A Git overview can predate a changed PR head or a new owner decision. A task can also reopen a step while a slower row holds up a list. MCP callers need to know why a parked task appears in their Needs you page.

## Decision

Carry the plan and the attention facts that classified each parked row into its MCP response. A directly confirmed check result or the durable ordinary CI result for that PR and SHA supplies the wait reason. An owner decision or held prompt displayed in the response takes precedence. Apply the Needs you filter to the group that the response actually shows.

Walk the ordered candidates in windows bounded by eight and by the remaining page capacity. Give the parked rows in each window a share of the remaining PR lookup time proportional to their share of the parked candidates. Deduct time spent verifying parked rows. Stop once the page fills.

## Alternatives

Reading another plan when rendering can pair Done with a step that opened after verification. Checking every candidate before applying a filtered limit spends remote reads on rows beyond a full page. Reading each PR with an independent deadline makes request latency grow with the number of slow checks.

## Consequences

The classification, progress and detailed steps in a response describe one plan observation. A subsequent read picks up a reopened plan. Slow or unreadable checks keep their row under Needs you. Listings retain activity order and their existing page limits.
