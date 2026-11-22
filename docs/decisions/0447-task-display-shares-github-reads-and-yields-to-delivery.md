# 0447. Task display shares GitHub reads and yields to delivery

## Status

Accepted for SPEC-609.

## Context

Repeated task reads can spend the installation's hourly quota while a merge, CI status or staging request waits for it. The failure log identifies the caller GitHub refused. Attributing the hour requires counting outgoing attempts before the shortage.

## Decision

Keep terminal PR observations and independently observed open follow-up identities with task delivery data. Readers verify open checks even when they recover the PR number from disk. Backfill recorded merges, even where the landing SHA was unavailable. A three-minute background reconciliation visits up to 24 terminal task branches per project per tick, rotating through the project. Positive evidence of reopening or replacement updates the branch observation. A numbered operational read can recover a missing merge SHA. Share successful open display responses for 30 seconds per credential and request URL. Revalidate expired responses with the server's ETag. Reserve the final 10% of an installation's reported core quota for operational work. Report hourly attempts by caller purpose alongside the remaining capacity and reset reported by GitHub.

## Alternatives

Slowing one polling client leaves every other client able to consume the same quota. A cache of whole task cards would delay local plan, question and CI changes. Querying GitHub for budget information every time Status opens introduces more traffic during a shortage.

## Consequences

Readers can see an open PR observation up to 30 seconds old. Actions check current state. Authenticated conditional responses can return 304 at zero primary quota cost. Successful and refused HTTP attempts are both visible. [GitHub REST guidance](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api#use-conditional-requests)
