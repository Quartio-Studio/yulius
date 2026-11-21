# 0441. GitHub PR openings trigger exact-head CI

## Status

Accepted for SPEC-606.

## Context

Task delivery triggers CI as soon as it pushes. People opening PRs through GitHub have to wait for the CI sweep. A faster global sweep spends installation quota on idle repositories.

## Decision

Accept GitHub App pull request opening events authenticated with HMAC-SHA256. Verify the installation, repository, base and head before scheduling with the existing CI gate. Keep periodic reconciliation for missed deliveries. Serialize queue decisions per sandbox and let a webhook pass an outstanding REST inventory. Publish commit statuses after releasing the scheduling lock. Track up to 30 open PRs per sandbox across both paths. Each keeps its place until a fresh inventory observes its closure or ineligibility. An inventory that predates a signed opening preserves that head until the next round. Bound incoming deliveries before buffering bodies or reading credentials. Register new Apps for these events and support enabling them on existing Apps.

## Alternatives

Conditional REST polls still spend quota when a response changes. Git pull refs omit the source repository and base branch. Both approaches keep a discovery interval between opening and queueing.

## Consequences

The hook uses GitHub's authenticated event metadata and the server's clone. The usual run status writes still reach GitHub. An existing App's owner enables its subscription. Delivery outages and occupied CI lanes remain visible as separate causes of delay.
