# 0427. Task authorization covers independent re-review

## Status

Accepted for [SPEC-588](../../.ai/specs/SPEC-588-it-shouldn-t-ask-me-for-this-agents-must.md). Amends the escalation rule in [ADR-0141](0141-review-findings-in-the-change-s-own-code-are-fixed-before-merge.md).

## Context

Task #570 asked its owner twice to authorize independent review of corrections that its delivery task already covered. Its briefing and review skills treated the third round as a reason to seek another decision.

## Decision

The author owns in-scope corrections and independent re-review until the reviewer approves. If findings persist, the author can split the work within scope and submit each slice for review. A request to the owner must identify an action that actually requires the owner's authority, such as granting an external permission. The existing independent approval and CI rules govern merge.

## Alternatives

A higher round cap would move the same interruption to a later round. Automatically approving at the cap would let unresolved findings through. Filtering the text of `needs_you` requests would leave the author with conflicting instructions and could suppress legitimate requests.

## Consequences

A fourth or later independent review proceeds under the original delivery authorization. Authors still fix their own defects test-first, and may file eligible cosmetic nits after three rounds. Previously issued prompts remain part of their session history.
