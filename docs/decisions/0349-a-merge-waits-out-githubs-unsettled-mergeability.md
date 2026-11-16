# 0349. A merge waits out GitHub's unsettled mergeability

## Status

Accepted (2026-11-16). Specified in
[SPEC-495](../../.ai/specs/SPEC-495-a-production-deploy-never-fails-on-a-pro.md) (MRG-001 to
MRG-005); task #477.

## Context

On 16 Nov 2026 a person's production deploy opened promotion pull request #1353 (dev → main) and
merged it at once. GitHub answered `405 Base branch was modified. Review and try the merge
again.`; moments later it reported the pull request mergeable and clean, with `main` unchanged
since the previous promotion. GitHub computes a pull request's mergeability in the background
after it is opened or its base moves (`mergeable: null` meanwhile), and a merge sent in that
window can get this 405. The GitHub adapter sent a merge once and turned every 405 into
`NotMergeable`, so the deploy ended with "deploy production again once it can", and the message
did not say whether anything was really wrong.

Every merge Yulius makes goes through `GitHubClient::merge_pull`: the promotion of a production
deploy, `delivery_merge`, the merge train and remote file edits.

## Decision

- **The adapter's `merge_pull` does not take a 405 at its word at once.** After a 405 it reads the
  pull request until GitHub has computed its mergeability (`mergeable` not `null`, at most 6
  reads with doubling backoff). A 405 is transient when GitHub's message says "Base branch was
  modified" or GitHub was still computing at that read; a transient 405 on a pull request GitHub
  then reports open, not a draft and mergeable is sent again with doubling backoff. A
  `mergeable_state` of `blocked` or `behind` does not stop the retry: GitHub reads it for a caller
  who may bypass the rule too (a repository admin's promotion past Yulius's production ruleset,
  ADR-0114), and a merge that is really blocked ends at its next, non-transient 405.
- **Bounded**: at most `MERGE_ATTEMPTS` (4) merge requests and 6 reads per wait (with the default
  500 ms first delay 5 reads fit in the 12 s, the sixth only with a shorter delay); all the sleeps
  of one call end within the client's `RetryPolicy::max_total`, capped at 12 s (`MERGE_WAIT`),
  counted from the first request. Each request has its own timeout (10 s) and each read its GET
  retries on top, so against a GitHub that answers at once the call ends within about 12 s, well
  before the CD page gives up on a deploy request (30 s). The future is cancelled by dropping it,
  as every adapter call is.
- **A real refusal ends the merge and says which**: `NotMergeable`'s message starts with the reason
  read from the pull request (it conflicts with the base branch, branch protection blocks it, it
  is behind the base branch it must contain, it is a draft, closed or already merged, GitHub had
  not finished checking, or GitHub still refused it after N attempts), then GitHub's words. Those
  reach the deploy's message and `delivery_merge`'s answers unchanged.
- The fast path is unchanged: a merge that succeeds at once is one request; a 409 (the head moved)
  and other failures map as before.

## Alternatives

- **Retry in the application (promotion code) only.** Rejected: `delivery_merge` and the merge
  train merge pull requests they have just updated too, and the app sees only
  `AppError::Conflict(text)`, so it would have to match GitHub's wording through a mapped string.
- **Read the pull request until `mergeable` is set before every merge.** Rejected: one or more
  extra requests on every merge, while GitHub answers most merges at once; reading after a 405
  reaches the same outcome only when it is needed.
- **Retry every 405.** Rejected: branch protection, required reviews and conflicts are real
  refusals; retrying them delays the answer and adds requests to GitHub for nothing.

## Consequences

- A production deploy whose promotion pull request GitHub has not finished checking merges and
  deploys instead of asking the person to press Deploy again; a merge of `delivery_merge` or the
  merge train in the same window goes through too.
- A merge GitHub keeps refusing as transient sleeps at most 12 s before it ends, inside the
  caller's request; a slow GitHub adds its request timeouts.
- The fake GitHub models a pull request whose mergeability is still computing
  (`FakeGitHub::set_pull_computing`) and reports `mergeable_state`.
